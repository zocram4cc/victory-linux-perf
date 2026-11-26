use super::*;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize};
use windows_sys::Win32::System::Memory::{CreateFileMappingW, FILE_MAP_WRITE, MapViewOfFile, PAGE_READWRITE};

struct Random(u64);

impl Random {
    fn next(&mut self) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 8) as usize
    }
}

/// View memory as the runtime makes the title's: a view of a section, noted
/// as it notes it. Kept: the journal of what was written (`crate::watch`)
/// knows its addresses, which must not become another test's fresh memory.
struct Title {
    at: usize,
    len: usize,
}

impl Title {
    fn new(len: usize) -> Title {
        // SAFETY: a pagefile section of our own and a view of it.
        unsafe {
            let section =
                CreateFileMappingW(-1isize as _, std::ptr::null(), PAGE_READWRITE, 0, len as u32, std::ptr::null());
            assert!(!section.is_null());
            let at = MapViewOfFile(section, FILE_MAP_WRITE, 0, 0, len).Value as usize;
            assert_ne!(at, 0);
            crate::inplace::title_memory_section(section as usize, len as u64, true);
            crate::title_memory_remapped(at, len);
            crate::inplace::title_memory_view(at, len, section as usize, 0);
            Title { at, len }
        }
    }

    fn bytes(&self) -> &[u8] {
        // SAFETY: the view.
        unsafe { std::slice::from_raw_parts(self.at as *const u8, self.len) }
    }
}

/// Every number once, whatever the count and the threads, batch after batch
/// and from several callers.
#[test]
fn the_pool_calls_every_number_once() {
    for threads in [1usize, 2, 4, 7] {
        let pool = Pool::new("victory-land", threads);
        std::thread::scope(|s| {
            for _ in 0..3 {
                s.spawn(|| {
                    for round in 0..300usize {
                        let count = round * 7 % 50;
                        let calls: Vec<AtomicUsize> = (0..count).map(|_| AtomicUsize::new(0)).collect();
                        pool.each(count, &|i| {
                            calls[i].fetch_add(1, Ordering::Relaxed);
                        });
                        assert!(calls.iter().all(|c| c.load(Ordering::Relaxed) == 1));
                    }
                });
            }
        });
    }
}

/// A job that panics is told to the caller, and the pool goes on.
#[test]
fn a_panicking_job_reaches_the_caller() {
    let pool = Pool::new("victory-land", 3);
    for bad in 0..8usize {
        let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            pool.each(8, &|i| assert_ne!(i, bad, "(expected)"));
        }));
        assert!(ran.is_err());
        let n = AtomicUsize::new(0);
        pool.each(20, &|i| {
            n.fetch_add(i, Ordering::Relaxed);
        });
        assert_eq!(n.into_inner(), 190);
    }
}

/// The order pieces are landed in holds every byte of every piece, under its
/// copy, and no other; blocks come in order, pieces of several copies in one
/// block stay in the order given, and jobs never cut a block.
#[test]
fn ordered_pieces_hold_the_same_bytes() {
    let mut random = Random(99);
    for _ in 0..400 {
        let count = random.next() % 40;
        let pieces: Vec<Piece> = (0..count)
            .map(|_| {
                let block = random.next() % 12 * BLOCK;
                let from = random.next() % BLOCK / 4 * 4;
                let to = (from + 4 + random.next() % BLOCK / 4 * 4).min(BLOCK);
                (random.next() as u64 % 3, block + from, to - from)
            })
            .collect();
        let out = ordered(&pieces);
        let bytes = |copy: u64, at: usize, all: &[Piece]| all.iter().any(|p| p.0 == copy && p.1 <= at && at < p.1 + p.2);
        let landed: Vec<Piece> = out.iter().map(|&(i, at, len)| (pieces[i].0, at, len)).collect();
        for copy in 0..3 {
            for at in (0..12 * BLOCK).step_by(1024 * 4) {
                assert_eq!(bytes(copy, at, &pieces), bytes(copy, at, &landed), "{pieces:?}");
            }
        }
        assert!(out.iter().all(|&(_, at, len)| len > 0 && at / BLOCK == (at + len - 1) / BLOCK));
        assert!(out.windows(2).all(|w| w[0].1 / BLOCK <= w[1].1 / BLOCK));
        for block in 0..12 {
            let given: Vec<Piece> = pieces.iter().copied().filter(|p| p.1 / BLOCK == block).collect();
            let here: Vec<Piece> = landed.iter().copied().filter(|p| p.1 / BLOCK == block).collect();
            if given.iter().any(|p| p.0 != given[0].0) {
                assert_eq!(here, given);
            } else {
                assert!(here.windows(2).all(|w| w[0].1 + w[0].2 < w[1].1), "apart, in order");
            }
        }
        let cut: Vec<(usize, usize)> = out.iter().map(|&(_, at, len)| (at, len)).collect();
        let jobs = jobs(&cut);
        assert_eq!(jobs.iter().cloned().flatten().collect::<Vec<_>>(), (0..cut.len()).collect::<Vec<_>>());
        for w in jobs.windows(2) {
            assert_ne!(cut[w[0].end - 1].0 / BLOCK, cut[w[1].start].0 / BLOCK);
        }
    }
}

/// A piece is left out only if a fetch behind its own flight, or behind a
/// later one, brought all of it, and is forgotten when that flight is done.
#[test]
fn only_what_a_fetch_behind_the_flight_brought_is_left_out() {
    let mut had = Had::default();
    had.fetched(7, [(1, BLOCK, BLOCK), (2, 0x100, 0x200)].into_iter());
    assert!(had.holds(6, (1, BLOCK, BLOCK)) && had.holds(7, (1, BLOCK + 64, 128)));
    assert!(!had.holds(8, (1, BLOCK, BLOCK)), "the fetch ran ahead of flight 8");
    assert!(!had.holds(7, (3, BLOCK, BLOCK)) && !had.holds(7, (1, 0, 64)));
    assert!(had.holds(7, (2, 0x100, 0x200)) && !had.holds(7, (2, 0x100, 0x204)) && !had.holds(7, (2, 0xfc, 8)));
    had.landing(7);
    assert!(had.holds(7, (1, BLOCK, BLOCK)));
    had.landing(8);
    assert!(had.0.is_empty());
}

/// A piece landed through either address holds what the merge of one thread
/// and one pass ([`crate::safecopy::merge`]) is to leave, with parts left
/// out, wherever it begins and ends.
#[test]
fn a_piece_lands_what_the_card_wrote_in_its_parts() {
    assert!(crate::safecopy::on());
    let title = Title::new(2 * BLOCK);
    let mut random = Random(5);
    for round in 0..300usize {
        let from = (random.next() % 5000) & !3;
        let len = (random.next() % BLOCK).min(BLOCK - from).max(1);
        let base: Vec<u8> = (0..len).map(|_| random.next() as u8).collect();
        let (mut now, mut held) = (base.clone(), base.clone());
        for _ in 0..random.next() % 60 {
            let at = random.next() % len;
            let n = (random.next() % 300).min(len - at);
            for b in &mut now[at..at + n] {
                *b = random.next() as u8;
            }
        }
        let mut there = base.clone();
        for _ in 0..random.next() % 60 {
            there[random.next() % len] ^= 0x11;
        }
        // Left out: some ranges, anywhere.
        let mut parts = vec![(0, len)];
        for _ in 0..round % 4 {
            let a = random.next() % len;
            let b = (a + random.next() % 200).min(len);
            parts = parts
                .into_iter()
                .flat_map(|(x, y)| [(x, y.min(a)), (x.max(b), y)])
                .filter(|(x, y)| x < y)
                .collect();
        }
        // SAFETY: inside the view; `kernel_will_write`'s rule for our own
        // writes: the guard goes first.
        unsafe {
            crate::watch::writing(title.at, 2 * BLOCK);
            std::ptr::copy_nonoverlapping(there.as_ptr(), (title.at + from) as *mut u8, len);
        }
        // Guarded, as after a refresh, in two rounds of three.
        if round % 3 != 0 {
            assert!(crate::watch::writes(title.at, title.at + 2 * BLOCK, true).is_some());
            assert_eq!(crate::watch::writes(title.at, title.at + 2 * BLOCK, true).map(|w| w.1.len() > 2), Some(false));
        }
        let asked = crate::watch::writes(title.at, title.at + BLOCK, true).expect("a view").0;
        let mut notes = Notes::default();
        // SAFETY: vectors of `len` bytes.
        let put = unsafe { piece(title.at + from, now.as_ptr(), held.as_mut_ptr(), &parts, true, &mut notes, std::ptr::null()) };
        notes.flush();
        let mut expect_held = base.clone();
        let mut expect = there.clone();
        for &(a, b) in &parts {
            for i in a..b {
                if now[i] != base[i] {
                    expect[i] = now[i];
                    expect_held[i] = now[i];
                }
            }
        }
        assert!(held == expect_held, "round {round}");
        assert!(title.bytes()[from..from + len] == expect[..], "round {round}");
        // Told: a refresh from now on takes the block again.
        let (_, written) = crate::watch::writes(title.at, title.at + BLOCK, true).expect("a view");
        let told = written.iter().any(|&(page, at)| page == title.at / BLOCK && at > asked);
        assert!(told || put == 0, "round {round}: {put} bytes, {written:?} after {asked}");
    }
}

/// A piece whose title memory is away only while the runtime changes a
/// mapping lands when that is done; where it stays away the copy's CPU side
/// takes the card's bytes (`take`) or not.
#[test]
fn a_piece_waits_for_a_change_of_mapping() {
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_NOACCESS, VirtualAlloc, VirtualFree};
    assert!(crate::safecopy::on());
    // SAFETY: a reservation of ours, nothing of it committed.
    let at = unsafe { VirtualAlloc(std::ptr::null(), BLOCK, MEM_RESERVE, PAGE_NOACCESS) } as usize;
    assert_ne!(at, 0);
    let now = [9u8; 100];
    for take in [true, false] {
        let mut held = [0u8; 100];
        // SAFETY: arrays of 100 bytes.
        let put = unsafe { piece(at, now.as_ptr(), held.as_mut_ptr(), &[(0, 100)], take, &mut Notes::default(), std::ptr::null()) };
        assert_eq!((put, held), (0, [if take { 9 } else { 0 }; 100]));
    }
    let (began, wait) = std::sync::mpsc::channel();
    let change = std::thread::spawn(move || {
        let _changing = crate::watch::title_memory_remapping();
        began.send(()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        // SAFETY: the reservation's first page.
        assert!(!unsafe { VirtualAlloc(at as _, PAGE, MEM_COMMIT, PAGE_READWRITE) }.is_null());
    });
    wait.recv().unwrap();
    let mut held = [0u8; 100];
    // SAFETY: as above.
    let put = unsafe { piece(at, now.as_ptr(), held.as_mut_ptr(), &[(0, 100)], true, &mut Notes::default(), std::ptr::null()) };
    change.join().unwrap();
    assert_eq!((put, held), (100, now));
    // SAFETY: committed by now.
    assert_eq!(unsafe { std::slice::from_raw_parts(at as *const u8, 100) }, now);
    // SAFETY: our reservation.
    unsafe { VirtualFree(at as _, 0, MEM_RELEASE) };
}

/// Landings on several threads beside title threads that write the same
/// blocks (other dwords) and a thread that refreshes a copy, guarding what
/// it takes: no write of the title's is lost or undone, all the card wrote
/// arrives, and once everything stands still the next refresh takes the
/// last of it (no note is lost).
#[test]
fn landings_beside_the_title_and_its_refreshes_lose_nothing() {
    assert!(crate::safecopy::on());
    const BLOCKS: usize = 24;
    const LEN: usize = BLOCKS * BLOCK;
    let title = Title::new(LEN);
    let at = title.at;
    // SAFETY: committed, aligned, touched as atomics or by the landing's
    // stores of whole dwords.
    let dword = |i: usize| unsafe { &*((at + i * 4) as *const AtomicU32) };
    let stop = AtomicBool::new(false);
    // The copy: its bytes, and per block the asking that last took it.
    let copy: Mutex<(Vec<u8>, Vec<u64>)> = Mutex::new((vec![0; LEN], vec![0; BLOCKS]));
    let refresh = || {
        let mut copy = copy.lock().unwrap_or_else(|e| e.into_inner());
        let (now, written) = crate::watch::writes(at, at + LEN, true).expect("a view");
        let (bytes, taken) = &mut *copy;
        for block in 0..BLOCKS {
            let write = written.iter().find(|w| w.0 == at / BLOCK + block).map_or(0, |w| w.1);
            if taken[block] == 0 || write > taken[block] {
                taken[block] = now;
                let part = &mut bytes[block * BLOCK..(block + 1) * BLOCK];
                // SAFETY: inside the view; whoever writes it meanwhile is
                // noted, and the block taken again.
                assert!(unsafe { crate::safecopy::copy(part.as_mut_ptr(), (at + block * BLOCK) as *const u8, BLOCK) });
            }
        }
    };
    let mut held = vec![0u8; LEN];
    std::thread::scope(|s| {
        let writers: Vec<_> = (0..3usize)
            .map(|w| {
                let stop = &stop;
                s.spawn(move || {
                    let mut value = 1u32;
                    while !stop.load(Ordering::Relaxed) {
                        // Its own dwords, in every other block (in the rest
                        // only landings change anything).
                        for i in (w..LEN / 4).step_by(4 * 97).filter(|i| (i * 4 / BLOCK).is_multiple_of(2)) {
                            assert_eq!(dword(i).swap(value, Ordering::Relaxed), value - 1, "dword {i}: undone");
                        }
                        value += 1;
                    }
                    value - 1
                })
            })
            .collect();
        s.spawn(|| {
            while !stop.load(Ordering::Relaxed) {
                refresh();
            }
        });
        let mut random = Random(77);
        for round in 1..250u32 {
            // The card's dwords: every fourth, from the third on, in some
            // pages of some blocks.
            let mut now = held.clone();
            for _ in 0..60 {
                let page = random.next() % (LEN / PAGE);
                for i in (page * 1024 + 3..(page + 1) * 1024).step_by(4) {
                    now[i * 4..i * 4 + 4].copy_from_slice(&(round * 4096 + i as u32).to_le_bytes());
                }
            }
            let pieces: Vec<Piece> = (0..BLOCKS).map(|b| (1, at + b * BLOCK, BLOCK)).collect();
            let order = ordered(&pieces);
            let cut: Vec<(usize, usize)> = order.iter().map(|&(_, at, len)| (at, len)).collect();
            let jobs = jobs(&cut);
            let into = held.as_mut_ptr() as usize;
            each(jobs.len(), &|j| {
                let mut notes = Notes::default();
                for &(address, len) in &cut[jobs[j].clone()] {
                    let offset = address - at;
                    // SAFETY: each job its own pieces of the two vectors.
                    unsafe {
                        piece(address, now.as_ptr().add(offset), (into + offset) as *mut u8, &[(0, len)], true, &mut notes, std::ptr::null());
                    }
                }
                notes.flush();
            });
            assert!(held == now, "round {round}");
            for i in (3..LEN / 4).step_by(4 * 61) {
                let expect = u32::from_le_bytes(now[i * 4..i * 4 + 4].try_into().expect("four"));
                assert_eq!(dword(i).load(Ordering::Relaxed), expect, "round {round} dword {i}");
            }
        }
        stop.store(true, Ordering::Relaxed);
        for (w, writer) in writers.into_iter().enumerate() {
            let last = writer.join().expect("nothing undone");
            for i in (w..LEN / 4).step_by(4 * 97).filter(|i| (i * 4 / BLOCK).is_multiple_of(2)) {
                assert_eq!(dword(i).load(Ordering::Relaxed), last);
            }
        }
    });
    refresh();
    let copy = copy.into_inner().unwrap_or_else(|e| e.into_inner());
    assert!(copy.0 == title.bytes(), "a write was not told to the copy");
}

/// What a frame's landing costs (ignored: a measurement): 1,536 pieces of
/// 64 KiB, 96 MB, of a 174 MB heap of view memory in 15 landings, each block
/// guarded by a refresh before it; three quarters of the pages of a piece
/// with new bytes, every dword of them. As before, and here on one thread
/// and more, through the title's address and through a view of ours.
#[test]
#[ignore]
fn what_landing_costs() {
    assert!(crate::safecopy::on());
    const HEAP: usize = 2784 * BLOCK;
    const COUNT: usize = 1536;
    let title = Title::new(HEAP);
    let mut held = vec![0u8; HEAP];
    let mut card = vec![0u8; COUNT * BLOCK];
    let busy = AtomicBool::new(true);
    // The pieces: runs of eight blocks, scattered.
    let pieces: Vec<usize> = (0..COUNT).map(|i| (i / 8 * 1021 % (HEAP / BLOCK / 8)) * 8 + i % 8).collect();
    std::thread::scope(|s| {
        // Other threads of the process at work (what a protection change
        // has to reach).
        for _ in 0..6 {
            s.spawn(|| {
                let mut v = vec![0u64; 1 << 14];
                while busy.load(Ordering::Relaxed) {
                    for (i, x) in v.iter_mut().enumerate() {
                        *x = x.wrapping_mul(31).wrapping_add(i as u64);
                    }
                }
            });
        }
        let mut round = 0u32;
        // `guarded`: 2 as a refresh leaves them, 1 opened again before (that
        // is timed beside), 0 not at all, as they were after the last.
        let mut frame = |label: &str, dense: bool, guarded: u8, land: &dyn Fn(&[(usize, usize, usize)])| {
            let mut best = f64::MAX;
            let (mut guards, mut opening) = (f64::MAX, f64::MAX);
            for _ in 0..5 {
                round += 1;
                for (k, &block) in pieces.iter().enumerate() {
                    for page in 0..16 {
                        if page % 4 == 3 {
                            continue;
                        }
                        let from = k * BLOCK + page * PAGE;
                        let step = if dense { 4 } else { 64 };
                        // (The rest as held.)
                        let old = block * BLOCK + page * PAGE;
                        card[from..from + PAGE].copy_from_slice(&held[old..old + PAGE]);
                        for b in card[from..from + PAGE].chunks_exact_mut(step) {
                            b[..4].copy_from_slice(&round.wrapping_mul(0x0101_0101).to_le_bytes());
                        }
                    }
                    let (a, b) = (k * BLOCK + 12 * PAGE, block * BLOCK + 12 * PAGE);
                    for page in [3usize, 7, 11, 15] {
                        let (a, b) = (a - 12 * PAGE + page * PAGE, b - 12 * PAGE + page * PAGE);
                        card[a..a + PAGE].copy_from_slice(&held[b..b + PAGE]);
                    }
                }
                let t = std::time::Instant::now();
                if guarded > 0 {
                    assert!(crate::watch::writes(title.at, title.at + HEAP, true).is_some());
                }
                guards = guards.min(t.elapsed().as_secs_f64() * 1e3);
                let t = std::time::Instant::now();
                if guarded == 1 {
                    pieces.iter().for_each(|block| crate::watch::landing(title.at + block * BLOCK, BLOCK));
                }
                opening = opening.min(t.elapsed().as_secs_f64() * 1e3);
                let all: Vec<(usize, usize, usize)> = pieces
                    .iter()
                    .enumerate()
                    .map(|(k, &block)| {
                        (title.at + block * BLOCK, card.as_ptr() as usize + k * BLOCK, held.as_mut_ptr() as usize + block * BLOCK)
                    })
                    .collect();
                let t = std::time::Instant::now();
                for landing in all.chunks(COUNT / 15 + 1) {
                    land(landing);
                }
                best = best.min(t.elapsed().as_secs_f64() * 1e3);
            }
            println!("{label}: {best:.1} ms a frame to land; the guards before it {guards:.1} ms, opening them {opening:.1} ms");
        };
        for dense in [true, false] {
            println!("{}", if dense { "every dword of three pages in four new" } else { "every sixteenth dword of them new" });
            let before: &dyn Fn(&[(usize, usize, usize)]) = &|landing| {
                let mut scratch: Vec<u8> = Vec::new();
                for &(at, now, held) in landing {
                    crate::shadow::land_as_before(at, now, held, BLOCK, &mut scratch);
                }
            };
            frame("as before", dense, 2, before);
            frame("as before, guards opened ahead", dense, 1, before);
            frame("as before, nothing guarded", dense, 0, before);
            for threads in [1usize, 2, 3, 4, 5, 6, 8] {
                let pool = Pool::new("victory-land", threads);
                for beside in [false, true] {
                    frame(&format!("{threads} threads, beside {beside}"), dense, 2, &|landing| {
                        let cut: Vec<(usize, usize)> = landing.iter().map(|p| (p.0, BLOCK)).collect();
                        let jobs = jobs(&cut);
                        pool.each(jobs.len(), &|j| {
                            let mut notes = Notes::default();
                            for &(at, now, held) in &landing[jobs[j].clone()] {
                                // SAFETY: each its own bytes.
                                unsafe { piece_at(at, now as *const u8, held as *mut u8, &[(0, BLOCK)], true, &mut notes, beside, std::ptr::null()) };
                            }
                            notes.flush();
                        });
                    });
                }
            }
        }
        busy.store(false, Ordering::Relaxed);
    });
}

/// A landing through the title's own address, under a standing asking, is in the journal before the
/// refresh after it. While the asking stands the journal answers for the range, so a landing it does
/// not note leaves the copy with the bytes from before.
#[test]
fn a_landing_under_a_standing_asking_is_seen_by_the_refresh_after_it() {
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree};
    const MEM_WRITE_WATCH: u32 = 0x20_0000;
    assert!(crate::safecopy::on());
    // SAFETY: a fresh write-watched allocation, freed at the end.
    let at = unsafe { VirtualAlloc(std::ptr::null(), BLOCK, MEM_RESERVE | MEM_COMMIT | MEM_WRITE_WATCH, PAGE_READWRITE) } as usize;
    assert_ne!(at, 0);
    // The card wrote 7s into the block's second page; title memory holds zeros there.
    let now = [7u8; BLOCK];
    let mut held = [0u8; BLOCK];
    crate::watch::ask_once(&[(at, at + BLOCK)]);
    let (asked, _) = crate::watch::writes(at, at + BLOCK, false).expect("watched");
    let mut notes = Notes::default();
    // SAFETY: the part lies in the block, and `now` and `held` are a block each.
    let put = unsafe { piece_at(at, now.as_ptr(), held.as_mut_ptr(), &[(PAGE, 2 * PAGE)], true, &mut notes, false, std::ptr::null()) };
    notes.flush();
    let (_, after) = crate::watch::writes(at, at + BLOCK, false).expect("watched");
    crate::watch::ask_once_end();
    // SAFETY: our allocation.
    unsafe { VirtualFree(at as *mut _, 0, MEM_RELEASE) };
    assert_eq!(put, PAGE as u64);
    assert!(
        after.iter().any(|&(page, n)| page == at / crate::watch::PAGE && n > asked),
        "the landed page is not in the journal: {after:?} (asked {asked})"
    );
}
