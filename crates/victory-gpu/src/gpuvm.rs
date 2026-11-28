use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, Ordering};
use windows::core::{GUID, HRESULT, Interface, IUnknown};
use windows::Win32::Graphics::Direct3D12::ID3D12Resource;

pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_GPUVM_LINKS").is_some_and(|v| v == "1"))
}

struct Interop(IUnknown, ResourceInfo);
unsafe impl Send for Interop {}
unsafe impl Sync for Interop {}
type ResourceInfo = unsafe extern "system" fn(*mut c_void, *mut c_void, *mut u64, *mut u64) -> HRESULT;

fn native(resource: &ID3D12Resource) -> (u64, u64) {
    static API: OnceLock<Option<Interop>> = OnceLock::new();
    let Some(host) = crate::host::made_host() else { return (0, 0) };
    let Some(api) = API.get_or_init(|| unsafe {
        let mut p = std::ptr::null_mut();
        if host.device.query(&GUID::from_u128(0x39da4e09_bd1c_4198_9fae_86bbe3be41fd), &mut p).is_err() || p.is_null() { return None; }
        let get = std::mem::transmute::<usize, ResourceInfo>(*(*(p as *const *const usize)).add(10));
        Some(Interop(IUnknown::from_raw(p), get))
    }) else { return (0, 0) };
    let (mut object, mut offset) = (0, 0);
    if unsafe { (api.1)(api.0.as_raw(), resource.as_raw(), &mut object, &mut offset) }.is_err() { return (0,0); }
    (object, offset)
}

#[derive(Clone, Copy, Default)]
struct Binding { object: u64, offset: u64, frame: u64, first: u64, last: u64 }
#[derive(Default)]
struct Shard { now: HashMap<(usize,u32), Binding>, history: VecDeque<((usize,u32), Binding, Binding)> }
static SLOTS: std::sync::LazyLock<[Mutex<Shard>; 32]> = std::sync::LazyLock::new(|| std::array::from_fn(|_| Mutex::default()));

pub(crate) fn publish(heap: usize, slot: u32, resource: Option<&ID3D12Resource>) {
    let (object, offset) = resource.map_or((0,0), native);
    let (first, last) = crate::host::submissions();
    let b = Binding {object, offset, frame: crate::frame::current(), first, last};
    let key=(heap,slot);
    let mut s = SLOTS[(slot as usize)%SLOTS.len()].lock().unwrap_or_else(|e|e.into_inner());
    let prev = if object != 0 { s.now.insert(key,b) } else { s.now.remove(&key) };
    if s.history.len()==8192 { s.history.pop_front(); }
    s.history.push_back((key, prev.unwrap_or_default(), b));
}

#[derive(Default)]
pub(crate) struct Uses { pso: HashMap<usize,String>, slots: HashMap<(usize, u32, usize, u64), (Binding, u64, u64)> }
impl Uses {
    pub(crate) fn bind(&mut self, pso: usize, name: &str) {
        self.pso.entry(pso).or_insert_with(||name.to_owned());
    }
    pub(crate) fn read(&mut self, heap: usize, slot: u32, pso: usize, title: u64, token: u64) {
        if self.slots.len()>=65536 { return; }
        let b=SLOTS[(slot as usize)%SLOTS.len()].lock().unwrap_or_else(|e|e.into_inner()).now.get(&(heap,slot)).copied().unwrap_or_default();
        self.slots.insert((heap,slot,pso,token),(b,title,crate::frame::current()));
        static WITNESS: AtomicBool = AtomicBool::new(false);
        if b.object != 0 && token != 0 && !WITNESS.swap(true,Ordering::Relaxed) {
            let name=self.pso.get(&pso).map_or("?",String::as_str);
            tracing::info!(slot, title=format_args!("{title:#x}"), object=format_args!("{:#x}",b.object), offset=b.offset, name, "GPUVM witness: a declared table slot and its Vulkan resource");
        }
    }
    pub(crate) fn clear(&mut self) { self.slots.clear(); self.pso.clear(); }
}

pub(crate) fn lost(lists: &[&crate::host::HostList]) {
    use std::io::Write as _;
    static SAVED: AtomicBool = AtomicBool::new(false);
    if !on() || lists.is_empty() || SAVED.swap(true,Ordering::Relaxed) {return;}
    let Some(dir)=std::env::var_os("VICTORY_GPUVM_DIR") else {return};
    let path=std::path::Path::new(&dir).join(format!("lost-{}.tsv",std::process::id()));
    let Ok(file)=std::fs::File::create(&path) else {return};
    let mut out=std::io::BufWriter::new(file);
    let _=writeln!(out,"submissions\t{:?}\tlists\t{}",crate::host::submissions(),lists.len());
    for (i,list) in lists.iter().enumerate() {
        let Ok(u)=list.gpuvm.try_lock() else {let _=writeln!(out,"busy-list\t{i}");continue;};
        for (&(heap,slot,pso,token),&(b,title,frame)) in &u.slots {
            let name=u.pso.get(&pso).map_or("?",String::as_str);
            let _=writeln!(out,"read\t{i}\t{frame}\t{heap:x}\t{slot}\t{title:x}\t{token:x}\t{:x}\t{:x}\t{}\t{}\t{name}",b.object,b.offset,b.first,b.last);
        }
    }
    for (part,shard) in SLOTS.iter().enumerate() {
        let Ok(s)=shard.try_lock() else {let _=writeln!(out,"busy-shard\t{part}");continue;};
        for (&(heap,slot),b) in &s.now {
            let _=writeln!(out,"current\t{heap:x}\t{slot}\t{:x}\t{:x}\t{}\t{}\t{}",b.object,b.offset,b.frame,b.first,b.last);
        }
        for &((heap,slot),old,b) in &s.history {
            let _=writeln!(out,"slot\t{heap:x}\t{slot}\t{:x}\t{:x}\t{:x}\t{:x}\t{}\t{}\t{}",old.object,old.offset,b.object,b.offset,b.frame,b.first,b.last);
        }
    }
    let flushed=out.flush().is_ok();
    tracing::warn!(file=%path.display(),flushed,"GPUVM: recorded declarations and slot transitions of in-flight lists");
}
