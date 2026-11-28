//! One tracing setup for every binary.
//!
//! Filter with `VICTORY_LOG` (falls back to `RUST_LOG`), e.g.
//! `VICTORY_LOG=victory_loader=trace,info`. Debug and trace output exist
//! only in builds with the `diagnostics` feature; a plain release build
//! logs info and above whatever the filter asks for.
//!
//! `VICTORY_LOG_FILE=<path>` also writes every line, without colour codes,
//! to that file (appended). Under Wine/Proton the process often gets its own
//! console window, so redirecting stderr from the shell catches nothing.

use std::io::Write;
use std::sync::{Arc, Mutex, mpsc};

use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// The most verbose level this build can log.
const CAP: LevelFilter = if cfg!(feature = "diagnostics") {
    LevelFilter::TRACE
} else {
    LevelFilter::INFO
};

enum Sink {
    Stderr,
    File(Arc<Mutex<std::fs::File>>),
}

impl Sink {
    fn clone_sink(&self) -> Self {
        match self {
            Sink::Stderr => Sink::Stderr,
            Sink::File(f) => Sink::File(Arc::clone(f)),
        }
    }

    fn write_all(&self, bytes: &[u8]) {
        match self {
            Sink::Stderr => {
                let _ = std::io::stderr().lock().write_all(bytes);
            }
            Sink::File(f) => {
                let _ = f.lock().unwrap_or_else(|e| e.into_inner()).write_all(bytes);
            }
        }
    }
}

/// Asynchronous batched writer for info/debug/trace lines (`VICTORY_LOG_SYNC=1`
/// writes synchronously as before; ported from dolb 658d5615 and extended to
/// cover `VICTORY_LOG_FILE` too so Win32/Wine file writes never stall the
/// submitting thread). Warnings and errors still write immediately so the last
/// lines before a crash reach disk.
struct AsyncLines {
    tx: Option<mpsc::Sender<Vec<u8>>>,
    sink: Sink,
}

impl AsyncLines {
    fn new(name: &'static str, sink: Sink) -> Self {
        if std::env::var_os("VICTORY_LOG_SYNC").is_some_and(|v| v == "1") {
            return Self { tx: None, sink };
        }
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let bg = sink.clone_sink();
        let spawned = std::thread::Builder::new().name(name.into()).spawn(move || {
            let mut batch = Vec::with_capacity(64 << 10);
            while let Ok(line) = rx.recv() {
                batch.extend_from_slice(&line);
                while batch.len() < 256 << 10 {
                    match rx.try_recv() {
                        Ok(more) => batch.extend_from_slice(&more),
                        Err(_) => break,
                    }
                }
                bg.write_all(&batch);
                batch.clear();
            }
        });
        Self {
            tx: spawned.ok().map(|_| tx),
            sink,
        }
    }
}

enum LineWriter {
    Queued(Vec<u8>, mpsc::Sender<Vec<u8>>, Sink),
    Now(Sink),
}

impl Write for LineWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            LineWriter::Queued(line, ..) => {
                line.extend_from_slice(buf);
                Ok(buf.len())
            }
            LineWriter::Now(sink) => {
                sink.write_all(buf);
                Ok(buf.len())
            }
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for LineWriter {
    fn drop(&mut self) {
        if let LineWriter::Queued(line, tx, sink) = self
            && !line.is_empty()
            && let Err(lost) = tx.send(std::mem::take(line))
        {
            sink.write_all(&lost.0);
        }
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for AsyncLines {
    type Writer = LineWriter;

    fn make_writer(&'a self) -> LineWriter {
        match &self.tx {
            Some(tx) => LineWriter::Queued(Vec::with_capacity(256), tx.clone(), self.sink.clone_sink()),
            None => LineWriter::Now(self.sink.clone_sink()),
        }
    }

    fn make_writer_for(&'a self, meta: &tracing::Metadata<'_>) -> LineWriter {
        if *meta.level() <= tracing::Level::WARN {
            LineWriter::Now(self.sink.clone_sink())
        } else {
            self.make_writer()
        }
    }
}

pub fn init(default_level: &str) {
    let filter = std::env::var("VICTORY_LOG")
        .or_else(|_| std::env::var("RUST_LOG"))
        .ok()
        .and_then(|v| EnvFilter::try_new(v).ok())
        .unwrap_or_else(|| EnvFilter::new(default_level));
    let file = std::env::var_os("VICTORY_LOG_FILE").and_then(|path| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| eprintln!("VICTORY_LOG_FILE {}: {e}", path.to_string_lossy()))
            .ok()
    });
    let file_layer = file.map(|f| {
        let writer = AsyncLines::new("victory-log-file", Sink::File(Arc::new(Mutex::new(f))));
        tracing_subscriber::fmt::layer()
            .with_target(true)
            .with_ansi(false)
            .with_writer(writer)
    });
    let _ = tracing_subscriber::registry()
        .with(CAP)
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(true)
                .with_writer(AsyncLines::new("victory-log", Sink::Stderr)),
        )
        .with(file_layer)
        .try_init();
}
