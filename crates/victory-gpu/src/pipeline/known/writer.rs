use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::Duration;

pub(super) struct Writer {
    requests: Sender<Sender<()>>,
}

impl Writer {
    pub(super) fn start(flush: impl Fn() + Send + 'static) -> std::io::Result<Self> {
        let (requests, pending) = mpsc::channel::<Sender<()>>();
        std::thread::Builder::new().name("victory-known".into()).spawn(move || loop {
            let done = match pending.recv_timeout(Duration::from_secs(2)) {
                Ok(done) => Some(done),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            flush();
            if let Some(done) = done {
                let _ = done.send(());
            }
        })?;
        Ok(Self { requests })
    }

    /// The failing thread may already hold a lock the writer needs. Only the
    /// writer touches the book and the disk; the caller can leave on timeout.
    pub(super) fn request_flush(&self, timeout: Duration) {
        let (done, finished) = mpsc::channel();
        if self.requests.send(done).is_ok() {
            let _ = finished.recv_timeout(timeout);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_returns_after_the_writer_finishes() {
        let (saved, observed) = mpsc::channel();
        let writer = Writer::start(move || {
            let _ = saved.send(());
        })
        .unwrap();
        writer.request_flush(Duration::from_secs(1));
        observed.try_recv().unwrap();
    }

    #[test]
    fn a_stalled_save_does_not_hold_the_failing_thread() {
        let (started, entered) = mpsc::channel();
        let (release, stalled) = mpsc::channel();
        let writer = Writer::start(move || {
            let _ = started.send(());
            let _ = stalled.recv();
        })
        .unwrap();
        let (returned, finished) = mpsc::channel();
        std::thread::spawn(move || {
            writer.request_flush(Duration::from_millis(25));
            let _ = returned.send(());
        });
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        finished.recv_timeout(Duration::from_secs(2)).unwrap();
        let _ = release.send(());
    }
}
