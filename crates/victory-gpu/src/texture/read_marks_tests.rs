//! What a draw's read of a texture asks of the texture itself instead of
//! the shared tables (each behind a lock every recording thread took):
//! whether the host wrote it, the first level its staging buffer holds,
//! the placed texture a view names, and whether a target lies at its
//! memory. Each answer follows the table it stands for, and is given while
//! another thread holds that table's lock.

use std::sync::mpsc;
use std::time::Duration;

use windows::Win32::Graphics::Direct3D12::ID3D12Resource;
use windows::core::Interface;

use crate::object::{S_OK, This, guid, slot};

const RGBA8_64: [u32; 14] = [3, 0, 0, 0, 64, 0, 64, 1 | 1 << 16, 28, 1, 0, 0, 0, 0];
const RENDER_TARGET: u32 = 0x1;

/// The texture [`crate::resource::alias_of`] finds for `this`, as a pointer
/// (the handle it hands out is not comparable).
fn alias_of(this: This) -> Option<This> {
    crate::resource::alias_of(this).map(|held| held.get())
}

/// A 64x64 RGBA8 title texture at `address` with resource flags `flags`.
fn texture_at(address: u64, flags: u32) -> This {
    let mut words = RGBA8_64;
    words[12] = flags;
    let i = ID3D12Resource::IID;
    let iid = guid(i.data1, i.data2, i.data3, i.data4);
    let mut out = std::ptr::null();
    let made = crate::resource::create_at_address(
        std::ptr::null(),
        address,
        words.as_ptr().cast(),
        0,
        std::ptr::null(),
        &iid,
        &mut out,
    );
    assert_eq!(made, S_OK);
    out
}

/// Title memory of `bytes`, 64 KiB aligned; leaked, as title memory
/// outlives a test.
fn title_memory(bytes: usize) -> u64 {
    let layout = std::alloc::Layout::from_size_align(bytes, 1 << 16).unwrap();
    // SAFETY: a non-zero size.
    unsafe { std::alloc::alloc_zeroed(layout) as u64 }
}

fn release(t: This) {
    let _ = slot::<extern "system" fn(This) -> u32>(t, 0x10)(t);
}

/// Runs `prepare` on a thread of its own, then `read` there while this
/// thread holds what `hold` locks: whether `read` finished within five
/// seconds. (The lock is let go after, so a blocked `read` ends too.)
fn reads_while_held<G, S: 'static>(
    hold: impl FnOnce() -> G,
    prepare: impl FnOnce() -> S + Send + 'static,
    read: impl FnOnce(&S) + Send + 'static,
) -> bool {
    let (ready, is_ready) = mpsc::channel();
    let (go, goes) = mpsc::channel::<()>();
    let (done, is_done) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let state = prepare();
        ready.send(()).unwrap();
        goes.recv().unwrap();
        read(&state);
        done.send(()).unwrap();
    });
    is_ready.recv().unwrap();
    let held = hold();
    go.send(()).unwrap();
    let finished = is_done.recv_timeout(Duration::from_secs(5)).is_ok();
    drop(held);
    worker.join().unwrap();
    finished
}

/// The first level a texture's staging buffer holds, as a read finds it
/// without the staging table's lock, follows the table: made by the first
/// upload, gone when the buffer is let go.
#[test]
fn a_textures_staged_level_follows_its_staging_buffer() {
    let Some(host) = crate::host::host() else {
        return; // no host GPU here
    };
    let t = texture_at(title_memory(1 << 16), 0);
    assert_eq!(crate::resource::staged_first(t), u32::MAX, "nothing staged yet");
    let list = host.new_list().unwrap();
    super::upload_from(&list, t, 0);
    assert_eq!(crate::resource::staged_first(t), super::staged_from(t));
    assert_eq!(crate::resource::staged_first(t), 0, "the upload staged the whole texture");
    super::unstage(t);
    assert_eq!(crate::resource::staged_first(t), u32::MAX, "let go");
    assert_eq!(super::staged_from(t), u32::MAX);
    list.close();
    release(t);
}

/// A texture the host wrote is known to be so without the set's lock, and
/// is not uploaded from title memory.
#[test]
fn a_texture_the_host_wrote_is_marked_and_not_uploaded() {
    let Some(host) = crate::host::host() else {
        return; // no host GPU here
    };
    let t = texture_at(title_memory(1 << 16), 0);
    assert!(!crate::resource::host_written_mark(t));
    super::host_written(t);
    assert!(crate::resource::host_written_mark(t));
    assert!(super::host_wrote(t as usize));
    let list = host.new_list().unwrap();
    super::upload_from(&list, t, 0);
    assert!(list.first_use(t as usize), "not uploaded");
    assert_eq!(crate::resource::staged_first(t), u32::MAX, "not staged");
    list.close();
    release(t);
}

/// A texture read again in a list, from another first level, asks whether
/// the host wrote it and which level its staging buffer holds while
/// another thread holds both tables' locks.
#[test]
fn a_texture_read_again_takes_neither_tables_lock() {
    if crate::host::host().is_none() {
        return; // no host GPU here
    }
    let prepare = || {
        let host = crate::host::host().unwrap();
        let t = texture_at(title_memory(1 << 16), 0);
        let list = host.new_list().unwrap();
        super::upload_from(&list, t, 0);
        assert_eq!(super::staged_from(t), 0);
        (t, list)
    };
    let read = |(t, list): &(This, crate::host::HostList)| {
        // Levels from 1 on: held by the staging buffer of the levels from 0.
        super::upload_from(list, *t, 1);
        list.close();
    };
    assert!(
        reads_while_held(super::hold_tables, prepare, read),
        "the read waited for the host-written set or the staging table"
    );
}

/// A placed texture's kept "no target at its memory" answer does not hide
/// a render target made there later, nor one gone after it; while kept, it
/// is given while another thread holds the targets' table.
#[test]
fn a_placed_texture_finds_a_target_made_at_its_memory_later() {
    if crate::host::host().is_none() {
        return; // no host GPU here
    }
    let at = title_memory(1 << 16);
    let placed = crate::resource::texture_at(at, &RGBA8_64).unwrap();
    assert_eq!(alias_of(placed), None);
    let p = placed as usize;
    assert!(
        reads_while_held(crate::resource::hold_targets, || (), move |_| {
            assert_eq!(alias_of(p as This), None, "the answer kept");
        }),
        "the read waited for the targets' table"
    );
    let target = texture_at(at, RENDER_TARGET);
    assert_eq!(alias_of(placed), Some(target), "the target made since");
    release(target);
    assert_eq!(alias_of(placed), None, "the target gone");
}

/// What a thread found of a placed texture is the table's (the filler
/// words are not part of the key, as before), and found again while
/// another thread holds the table's lock.
#[test]
fn placed_textures_found_again_are_the_tables() {
    if crate::host::host().is_none() {
        return; // no host GPU here
    }
    let at = title_memory(1 << 16);
    let first = crate::resource::texture_at(at, &RGBA8_64).unwrap();
    assert_eq!(crate::resource::texture_at(at, &RGBA8_64), Some(first));
    let mut filler = RGBA8_64;
    filler[1] = 7;
    assert_eq!(crate::resource::texture_at(at, &filler), Some(first), "the filler is not part of the key");
    let first = first as usize;
    // Each looked up once by that thread before the lock (what it found is
    // its own), then again while the table is held.
    let prepare = move || {
        assert_eq!(crate::resource::texture_at(at, &RGBA8_64), Some(first as This));
        assert_eq!(crate::resource::texture_at(at, &filler), Some(first as This));
    };
    let read = move |_: &()| {
        assert_eq!(crate::resource::texture_at(at, &RGBA8_64), Some(first as This));
        assert_eq!(crate::resource::texture_at(at, &filler), Some(first as This));
    };
    assert!(
        reads_while_held(crate::resource::hold_placed, prepare, read),
        "the read waited for the placed textures' table"
    );
}
