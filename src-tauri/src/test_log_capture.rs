#![allow(dead_code)] // Shared test API is also consumed by downstream diagnostic tests.

use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone)]
pub(crate) struct CapturedLog {
    pub level: log::Level,
    pub target: String,
    pub message: String,
}

static LOGGER: TestLogger = TestLogger;
static INSTALL: OnceLock<Result<(), log::SetLoggerError>> = OnceLock::new();
static PROCESS: Mutex<Vec<CapturedLog>> = Mutex::new(Vec::new());
thread_local! {
    static THREAD: RefCell<Option<Vec<CapturedLog>>> = const { RefCell::new(None) };
}

struct TestLogger;
impl log::Log for TestLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.target().starts_with(env!("CARGO_CRATE_NAME"))
            || metadata.level() <= log::Level::Warn
    }
    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let captured = CapturedLog {
            level: record.level(),
            target: record.target().to_owned(),
            message: record.args().to_string(),
        };
        let _ = THREAD.try_with(|buffer| {
            if let Some(buffer) = buffer.borrow_mut().as_mut() {
                buffer.push(captured.clone());
            }
        });
        PROCESS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(captured);
    }
    fn flush(&self) {}
}

/// Install the single process-wide test logger, refusing a foreign logger.
pub(crate) fn install() {
    INSTALL
        .get_or_init(|| log::set_logger(&LOGGER))
        .as_ref()
        .expect("cannot install shared test logger: a different logger already owns the process");
    log::set_max_level(log::LevelFilter::Trace);
}

/// Captures records emitted on the current thread until finished or dropped.
pub(crate) struct ThreadLogCapture {
    _not_send: PhantomData<Rc<()>>,
}
impl ThreadLogCapture {
    pub(crate) fn start() -> Self {
        install();
        THREAD.with_borrow_mut(|buffer| {
            assert!(
                buffer.is_none(),
                "a log capture is already active on this thread"
            );
            *buffer = Some(Vec::new());
        });
        Self {
            _not_send: PhantomData,
        }
    }
    pub(crate) fn records(&self) -> Vec<CapturedLog> {
        THREAD.with_borrow(|buffer| buffer.as_ref().expect("active log capture").clone())
    }
    pub(crate) fn finish(self) -> Vec<CapturedLog> {
        THREAD.with_borrow_mut(|buffer| buffer.take().expect("active log capture"))
    }
}
impl Drop for ThreadLogCapture {
    fn drop(&mut self) {
        let _ = THREAD.try_with(|buffer| *buffer.borrow_mut() = None);
    }
}

pub(crate) fn process_records() -> Vec<CapturedLog> {
    PROCESS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}
pub(crate) fn assert_process_logs_exclude(needles: &[&str]) {
    for needle in needles {
        assert!(!needle.is_empty(), "empty log exclusion needle");
    }
    for record in process_records() {
        let line = render(&[record]);
        for needle in needles {
            assert!(!line.contains(needle), "log contained {needle:?}: {line}");
        }
    }
}
pub(crate) fn process_logs_contain(label: &str) -> bool {
    process_records()
        .iter()
        .any(|record| render(std::slice::from_ref(record)).contains(label))
}
pub(crate) fn render(records: &[CapturedLog]) -> String {
    records
        .iter()
        .map(|record| format!("{} {}: {}", record.level, record.target, record.message))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_logs_during_thread_local_destruction() {
        struct LogOnDrop;
        impl Drop for LogOnDrop {
            fn drop(&mut self) {
                log::warn!(target: "audio_graph::test", "tls-drop-marker-83c7");
            }
        }
        thread_local! { static LOG_ON_DROP: LogOnDrop = const { LogOnDrop }; }
        install();
        std::thread::spawn(|| {
            LOG_ON_DROP.with(|_| {});
            drop(ThreadLogCapture::start());
        })
        .join()
        .expect("thread-local destructor must not panic");
        assert!(process_logs_contain("tls-drop-marker-83c7"));
    }

    #[test]
    fn captures_threads_and_filters_dependencies() {
        let capture = ThreadLogCapture::start();
        log::info!(target: "audio_graph::test", "capture-own-unique-54a");
        std::thread::spawn(|| {
            log::info!(target: "audio_graph::test", "capture-worker-unique-54a");
            log::debug!(target: "dependency", "capture-dropped-unique-54a");
            log::warn!(target: "dependency", "capture-warn-unique-54a");
        })
        .join()
        .unwrap();
        let own = render(&capture.finish());
        assert!(own.contains("capture-own-unique-54a"));
        assert!(!own.contains("capture-worker-unique-54a"));
        let all = render(&process_records());
        assert!(all.contains("capture-own-unique-54a"));
        assert!(all.contains("capture-worker-unique-54a"));
        assert!(all.contains("capture-warn-unique-54a"));
        assert!(!all.contains("capture-dropped-unique-54a"));
    }
    #[test]
    #[should_panic(expected = "already active")]
    fn nested_capture_panics() {
        let _capture = ThreadLogCapture::start();
        let _nested = ThreadLogCapture::start();
    }
    #[test]
    #[should_panic(expected = "empty log exclusion needle")]
    fn empty_exclusion_panics() {
        assert_process_logs_exclude(&[""]);
    }
}
