//! Diagnostics: how long the card itself works on our submissions, by its own
//! timestamps (`VICTORY_CARD_SPANS=1`). The clock of the frame line
//! (`card_clock_line`) counts from the call that sends a submission to the
//! moment a thread of ours wakes behind its last signal: the time the system
//! takes to hand a submission to the card and to report it done is in it, 40
//! times a frame. Here two lists made once, of nothing but a timestamp, stand
//! around each submission.

use std::sync::Mutex;

use windows::Win32::Graphics::Direct3D12::*;
use windows::core::Interface;

use super::Host;

/// Submissions stamped before the first stamps are read and used again.
const SPANS: usize = 512;

/// The stamps of one queue's submissions.
pub(super) struct Spans {
    _allocator: ID3D12CommandAllocator,
    _heap: ID3D12QueryHeap,
    _back: ID3D12Resource,
    /// The mapped results, two a submission.
    data: usize,
    lists: Vec<ID3D12CommandList>,
    ticks_per_ms: f64,
    next: usize,
    sums: Sums,
    feed: Option<Feed>,
}

// SAFETY: free-threaded objects and a mapping only read behind a lock.
unsafe impl Send for Spans {}

/// What the stamps read so far add up to.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct Sums {
    /// Submissions, ticks the card worked on them, ticks between them.
    count: u64,
    busy: u64,
    between: u64,
    /// Ticks of the longest one, and the last one's end.
    longest: u64,
    last: u64,
}

impl Sums {
    /// A submission that ran from `start` to `end` (card ticks), after those
    /// before it. The start's stamp may be of before the last one's end: the
    /// part before that end is the last one's.
    fn add(&mut self, start: u64, end: u64) {
        if start == 0 || end < start {
            return;
        }
        let start = start.max(self.last);
        let took = end.saturating_sub(start);
        if self.last != 0 {
            self.between += start - self.last;
        }
        self.count += 1;
        self.busy += took;
        self.longest = self.longest.max(took);
        self.last = end.max(self.last);
    }
}

fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_CARD_SPANS").is_some_and(|v| v == "1"))
}

pub(super) fn feeding() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| on() && crate::switch::on("VICTORY_FEED_TIMES"))
}

pub(super) fn milestone() -> u64 {
    if feeding() { crate::timeline::ticks() } else { 0 }
}

struct Feed {
    stamps: Vec<[u64; 5]>,
    gpu: u64,
    cpu: u64,
    rate: f64,
    ticks: [u64; 6],
    measured: u64,
    rejected: u64,
    trace: Option<(std::fs::File, Vec<u8>)>,
}

impl Feed {
    fn measure(&mut self, previous: u64, start: u64, marks: [u64; 5]) {
        if previous == 0 || start < previous || marks[0] == 0 || marks[4] == 0 { return; }
        let at = |t: u64| (self.cpu as f64 + (i128::from(t) - i128::from(self.gpu)) as f64 * self.rate).round() as u64;
        let (mut from, to) = (at(previous), at(start));
        if to < marks[3] || marks.windows(2).any(|w| w[1] < w[0]) {
            self.rejected += 1;
            return;
        }
        if let Some((_, rows)) = self.trace.as_mut() {
            for tick in [from, to].into_iter().chain(marks) {
                rows.extend_from_slice(&tick.to_le_bytes());
            }
        }
        for (i, wall) in marks.into_iter().chain([to]).enumerate() {
            let stop = wall.clamp(from, to);
            self.ticks[i] += stop - from;
            from = stop;
        }
        self.measured += 1;
    }
}

pub(super) fn sent() {
    if !feeding() { return; }
    let at = crate::timeline::ticks();
    let mut made = SPANS_MADE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(Some(spans)) = made.as_mut() && let Some(feed) = spans.feed.as_mut() {
        feed.stamps[(SPANS + spans.next - 1) % SPANS][4] = at;
    }
}

static SPANS_MADE: Mutex<Option<Option<Spans>>> = Mutex::new(None);

impl Host {
    /// The lists that go in front of and behind a submission about to be
    /// sent to the drawing queue, if stamps are asked for.
    pub(super) fn span_lists(&self, phases: [u64; 3]) -> Option<[ID3D12CommandList; 2]> {
        if !on() {
            return None;
        }
        let mut made = SPANS_MADE.lock().unwrap_or_else(|e| e.into_inner());
        let spans = made
            .get_or_insert_with(|| {
                self.make_spans()
                    .inspect_err(|e| tracing::warn!(error = %e, "no stamps around submissions"))
                    .ok()
            })
            .as_mut()?;
        let i = spans.next;
        spans.next = (i + 1) % SPANS;
        // The stamps of the submission that had these lists, that many
        // submissions ago: long done.
        // SAFETY: inside the mapping; written by the card at whole words.
        let (start, end) = unsafe {
            let at = (spans.data as *mut u64).add(2 * i);
            let read = (at.read_volatile(), at.add(1).read_volatile());
            at.write_volatile(0);
            at.add(1).write_volatile(0);
            read
        };
        if let Some(feed) = spans.feed.as_mut() {
            if end >= start { feed.measure(spans.sums.last, start, feed.stamps[i]); }
            feed.stamps[i] = [phases[0], phases[1], phases[2], crate::timeline::ticks(), 0];
        }
        spans.sums.add(start, end);
        Some([spans.lists[2 * i].clone(), spans.lists[2 * i + 1].clone()])
    }

    fn make_spans(&self) -> windows::core::Result<Spans> {
        let device = &self.device;
        // SAFETY: plain object creation; lists of one allocator recorded one
        // after the other.
        unsafe {
            let mut heap: Option<ID3D12QueryHeap> = None;
            device.CreateQueryHeap(
                &D3D12_QUERY_HEAP_DESC {
                    Type: D3D12_QUERY_HEAP_TYPE_TIMESTAMP,
                    Count: 2 * SPANS as u32,
                    NodeMask: 0,
                },
                &mut heap,
            )?;
            let heap = heap.ok_or_else(windows::core::Error::empty)?;
            let back = self.read_back_buffer(16 * SPANS as u64).ok_or_else(windows::core::Error::empty)?;
            let mut data = std::ptr::null_mut();
            back.Map(0, None, Some(&mut data))?;
            std::ptr::write_bytes(data.cast::<u8>(), 0, 16 * SPANS);
            let allocator: ID3D12CommandAllocator = device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)?;
            let mut lists = Vec::with_capacity(2 * SPANS);
            let group = if crate::switch::on("VICTORY_STAMP_GROUPS") { 128 } else { 1 };
            for i in 0..2 * SPANS as u32 {
                let list: ID3D12GraphicsCommandList =
                    device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None)?;
                list.EndQuery(&heap, D3D12_QUERY_TYPE_TIMESTAMP, i);
                if (i + 1) % group == 0 {
                    let first = i + 1 - group;
                    list.ResolveQueryData(&heap, D3D12_QUERY_TYPE_TIMESTAMP, first, group, &back, u64::from(first) * 8);
                }
                list.Close()?;
                lists.push(list.cast()?);
            }
            let freq = self.queue.GetTimestampFrequency()?;
            let feed = feeding().then(|| super::stamps::calibration(&self.queue).ok()).flatten().map(|(gpu, cpu)| Feed {
                stamps: vec![[0; 5]; SPANS], gpu, cpu, rate: crate::timeline::frequency() as f64 / freq as f64,
                ticks: [0; 6], measured: 0, rejected: 0,
                trace: std::env::var_os("VICTORY_FEED_FILE").and_then(|path| {
                    use std::io::Write;
                    let mut file = std::fs::File::create(path).ok()?;
                    let t = crate::timeline::ticks();
                    let wall = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos() as u64;
                    for value in [crate::timeline::frequency(), t, wall] { file.write_all(&value.to_le_bytes()).ok()?; }
                    Some((file, Vec::with_capacity(128 * 1024)))
                }),
            });
            Ok(Spans {
                feed,
                _allocator: allocator,
                _heap: heap,
                _back: back,
                data: data as usize,
                lists,
                ticks_per_ms: freq as f64 / 1000.0,
                next: 0,
                sums: Sums::default(),
            })
        }
    }
}

/// The GPU timestamps the title was given since the last frame-rate line:
/// how many, how often a later one was 1 ms or more after the one before
/// (written with another submission), and where the first few lie.
static STAMPED: Mutex<(u64, u64, u64, Vec<usize>)> = Mutex::new((0, 0, 0, Vec::new()));

/// The title's GPU timestamp at `at` was given the value `now` (ticks of 10
/// ns).
pub(crate) fn stamped(at: usize, now: u64) {
    if !on() {
        return;
    }
    let mut s = STAMPED.lock().unwrap_or_else(|e| e.into_inner());
    s.0 += 1;
    s.1 += u64::from(now.saturating_sub(s.2) > 100_000);
    s.2 = now;
    if s.3.len() < 64 && !s.3.contains(&at) {
        s.3.push(at);
        tracing::info!(address = format_args!("{at:#x}"), now, "a GPU timestamp of the title");
    }
}

/// The viewports' sizes seen (each said once): whether the title draws its
/// scene at sizes of its choice.
pub(crate) fn viewport(width: u32, height: u32) {
    if !on() || width < 1024 || height < 576 {
        return;
    }
    static SEEN: Mutex<Vec<(u32, u32)>> = Mutex::new(Vec::new());
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if seen.len() < 200 && !seen.contains(&(width, height)) {
        seen.push((width, height));
        tracing::info!(width, height, "a viewport of the title");
    }
}

/// For the frame-rate line: what the stamps read since the last one say, per
/// frame (they are of some [`SPANS`] submissions ago).
pub(super) fn line(frames: f64) {
    {
        let mut s = STAMPED.lock().unwrap_or_else(|e| e.into_inner());
        if s.0 > 0 {
            tracing::info!(
                "GPU timestamps the title asked for per frame: {:.1}, in {:.1} groups written at one time",
                s.0 as f64 / frames,
                s.1 as f64 / frames
            );
        }
        (s.0, s.1) = (0, 0);
    }
    let mut made = SPANS_MADE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(Some(spans)) = made.as_mut() else {
        return;
    };
    if let Some(feed) = spans.feed.as_mut() {
        if let Some((file, rows)) = feed.trace.as_mut() {
            use std::io::Write;
            if let Err(error) = file.write_all(rows) { tracing::warn!(%error, "could not record queue timing"); }
            rows.clear();
        }
        let q = crate::timeline::frequency() as f64 * frames / 1000.0;
        let [before, prepare, steps, finish, sending, driver] = std::mem::take(&mut feed.ticks).map(|v| v as f64 / q);
        let measured = std::mem::take(&mut feed.measured);
        let rejected = std::mem::take(&mut feed.rejected);
        tracing::info!(measured, rejected, "GPU gaps per frame: before submit {before:.3}, preparation {prepare:.3}, list steps {steps:.3}, final preparation {finish:.3}, queue call {sending:.3}, after queue call {driver:.3} ms");
        if let Some(host) = super::host() && let Ok((gpu, cpu)) = super::stamps::calibration(&host.queue) {
            feed.gpu = gpu;
            feed.cpu = cpu;
        }
    }
    let last = spans.sums.last;
    let s = std::mem::replace(&mut spans.sums, Sums { last, ..Sums::default() });
    let ms = |ticks: u64| ticks as f64 / spans.ticks_per_ms / frames;
    tracing::info!(
        "the card's own stamps per frame: {:.1} submissions, working {:.1} ms, idle between them {:.1} ms, the longest {:.1} ms",
        s.count as f64 / frames,
        ms(s.busy),
        ms(s.between),
        s.longest as f64 / spans.ticks_per_ms
    );
}

#[cfg(test)]
mod tests {
    use super::Sums;

    #[test]
    fn submissions_add_up_without_counting_anything_twice() {
        let mut s = Sums::default();
        s.add(100, 150);
        s.add(140, 200); // began before the last one ended
        s.add(260, 270);
        assert_eq!((s.count, s.busy, s.between, s.longest, s.last), (3, 110, 60, 50, 270));
        // Never written, or nonsense: nothing.
        s.add(0, 0);
        s.add(300, 290);
        assert_eq!(s.count, 3);
        // Whole inside the last one.
        s.add(265, 268);
        assert_eq!((s.count, s.busy, s.between), (4, 110, 60));
    }
}
