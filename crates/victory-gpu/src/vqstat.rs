//! Counts of Windows memory calls by call site (`VirtualQuery`,
//! `VirtualProtect`, `GetWriteWatch`), logged every five seconds as calls per
//! second: under Wine each takes the process-wide `virtual_mutex`, and the
//! threads that call them queue there. Diagnostic counters only (one relaxed
//! atomic addition a call).

use std::sync::atomic::{AtomicU64, Ordering};

const NAMES: [&str; 28] = [
    "watch.rs:262:GetWriteWatch",
    "watch.rs:1768:VirtualQuery",
    "watch.rs:1866:VirtualProtect",
    "watch.rs:1881:VirtualProtect",
    "watch.rs:1896:VirtualProtect",
    "watch.rs:1907:VirtualProtect",
    "watch.rs:1957:VirtualQuery",
    "watch.rs:2020:VirtualProtect",
    "watch.rs:2025:VirtualProtect",
    "watch.rs:2368:VirtualQuery",
    "watch.rs:2514:VirtualQuery",
    "watch.rs:2662:VirtualQuery",
    "watch.rs:2853:VirtualQuery",
    "watch.rs:4352:VirtualProtect",
    "texture.rs:1667:VirtualQuery",
    "texture.rs:1734:VirtualQuery",
    "host.rs:8150:VirtualQuery",
    "inplace.rs:605:VirtualQuery",
    "inplace.rs:896:GetWriteWatch",
    "inplace.rs:911:VirtualProtect",
    "inplace.rs:971:VirtualProtect",
    "inplace.rs:1102:VirtualQuery",
    "safecopy.rs:542:VirtualProtect",
    "safecopy.rs:546:VirtualProtect",
    "safecopy.rs:693:VirtualProtect",
    "safecopy.rs:698:VirtualProtect",
    "safecopy.rs:701:VirtualProtect",
    "safecopy.rs:705:VirtualProtect"
];

static HITS: [AtomicU64; 28] = [const { AtomicU64::new(0) }; 28];
static ALL: AtomicU64 = AtomicU64::new(0);

pub(crate) fn hit(site: usize) {
    HITS[site].fetch_add(1, Ordering::Relaxed);
    if ALL.fetch_add(1, Ordering::Relaxed) % 4096 == 4095 {
        report();
    }
}

fn report() {
    static LAST: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    let Ok(mut last) = LAST.try_lock() else { return };
    let now = std::time::Instant::now();
    let Some(then) = *last else {
        *last = Some(now);
        return;
    };
    let secs = now.duration_since(then).as_secs_f64();
    if secs < 5.0 {
        return;
    }
    *last = Some(now);
    let mut rows: Vec<(u64, &str)> = HITS.iter().zip(NAMES).map(|(h, n)| (h.swap(0, Ordering::Relaxed), n)).filter(|r| r.0 > 0).collect();
    rows.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    let text: Vec<String> = rows.iter().take(12).map(|(n, name)| format!("{name} {:.0}/s", *n as f64 / secs)).collect();
    tracing::info!("memory calls by site: {}", text.join(", "));
}
