//! Panic capture and previous-crash detection.
//!
//! * The panic hook records message, source location and a backtrace
//!   (scrubbed) for the panicking thread, logs it, and on the UI thread also
//!   writes `<data>/crash.json`, since a UI-thread panic that nobody catches
//!   ends the app.
//! * [`crate::docstate::guard`] and the job threads catch engine panics and
//!   hand them to [`caught`]: the record is queued for the problem-report
//!   dialog and the crash file is removed again (the app survived).
//! * A session marker (`<data>/session.lock`) exists while the app runs and
//!   is removed on a clean exit. At the next start, a crash file next to a
//!   leftover marker means the last session ended in a crash.

use super::scrub::scrub;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

/// One engine failure: what the report's `error.txt` describes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Failure {
    pub message: String,
    /// `file:line:column` of the panic.
    pub location: Option<String>,
    pub backtrace: String,
    pub thread: String,
    /// What the user was doing ("Connect", "Import", …).
    pub context: Option<String>,
    pub version: String,
    pub commit: Option<String>,
    /// Unix seconds.
    pub time: u64,
}

impl Failure {
    /// A failure with only a message (no hook record available).
    pub fn from_message(message: &str) -> Failure {
        Failure {
            message: scrub(message),
            location: None,
            backtrace: String::new(),
            thread: std::thread::current()
                .name()
                .unwrap_or("unnamed")
                .to_string(),
            context: None,
            version: env!("CARGO_PKG_VERSION").to_string(),
            commit: super::commit().map(str::to_string),
            time: super::now_secs(),
        }
    }

    /// Identifies "the same failure" so the dialog opens once per failure.
    pub fn key(&self) -> (String, Option<String>) {
        (self.message.clone(), self.location.clone())
    }
}

thread_local! {
    /// The last panic on this thread, recorded by the hook.
    static LAST: RefCell<Option<Failure>> = const { RefCell::new(None) };
    /// Caught failures waiting for the UI (this is the UI thread's queue).
    static CAUGHT: RefCell<Vec<Failure>> = const { RefCell::new(Vec::new()) };
    /// What the user is doing right now, for the failure's context.
    static ACTION: RefCell<Option<String>> = const { RefCell::new(None) };
}

static CRASH_DIR: OnceLock<PathBuf> = OnceLock::new();
/// A UI-thread panic was recorded and not caught (yet).
static PENDING: AtomicBool = AtomicBool::new(false);

const CRASH_FILE: &str = "crash.json";
const MARKER: &str = "session.lock";

/// Install the panic hook (once). It runs before the previous hook, which
/// still prints the panic to stderr.
pub fn install_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            record_panic(info);
            prev(info);
        }));
    });
}

/// Where the hook writes the crash file (the data directory).
pub fn set_crash_dir(dir: &Path) {
    let _ = CRASH_DIR.set(dir.to_path_buf());
}

fn record_panic(info: &std::panic::PanicHookInfo) {
    let payload = info.payload();
    let message = payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_else(|| "panic with a non-text payload".into());
    let mut f = Failure::from_message(&message);
    f.location = info
        .location()
        .map(|l| scrub(&format!("{}:{}:{}", l.file(), l.line(), l.column())));
    // A broken scene can panic at the same place every frame: resolve the
    // backtrace and log the panic only the first few times.
    let seen = {
        static SEEN: OnceLock<std::sync::Mutex<std::collections::HashMap<String, u32>>> =
            OnceLock::new();
        let key = format!("{:?}|{}", f.location, f.message);
        SEEN.get_or_init(Default::default)
            .lock()
            .map(|mut m| {
                let n = m.entry(key).or_insert(0);
                *n += 1;
                *n
            })
            .unwrap_or(1)
    };
    f.backtrace = if seen <= 3 {
        scrub(&std::backtrace::Backtrace::force_capture().to_string())
    } else {
        format!("(same as the first time; this panic happened {seen} times)")
    };
    f.context = current_action();
    if seen <= 3 || seen.is_power_of_two() {
        log::error!(
            "panic in thread '{}' at {}: {}{}",
            f.thread,
            f.location.as_deref().unwrap_or("unknown location"),
            f.message,
            if seen > 1 {
                format!(" (×{seen})")
            } else {
                String::new()
            }
        );
    }
    let ui_thread = std::thread::current().name() == Some("main");
    if ui_thread && let Some(dir) = CRASH_DIR.get() {
        PENDING.store(true, Ordering::SeqCst);
        write_crash_file(dir, &f);
    }
    let _ = LAST.try_with(|l| *l.borrow_mut() = Some(f));
}

pub fn write_crash_file(dir: &Path, f: &Failure) {
    if let Ok(s) = serde_json::to_string_pretty(f) {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(dir.join(CRASH_FILE), s);
    }
}

/// The hook's record of the last panic on this thread (taken).
pub fn take_last() -> Option<Failure> {
    LAST.try_with(|l| l.borrow_mut().take()).ok().flatten()
}

/// A panic was caught on this thread and the app goes on: queue it for the
/// report dialog and withdraw the crash file. `message` is used when the
/// hook left no record. Returns the queued failure.
pub fn caught(message: &str) -> Failure {
    let mut f = take_last().unwrap_or_else(|| Failure::from_message(message));
    if f.context.is_none() {
        f.context = current_action();
    }
    survived();
    queue(f.clone());
    f
}

/// Queue a failure caught on another thread (a job), on the UI thread.
pub fn queue(f: Failure) {
    let _ = CAUGHT.try_with(|c| c.borrow_mut().push(f));
}

/// Failures caught since the last call (UI thread).
pub fn take_caught() -> Vec<Failure> {
    CAUGHT
        .try_with(|c| std::mem::take(&mut *c.borrow_mut()))
        .unwrap_or_default()
}

fn survived() {
    if std::thread::current().name() == Some("main")
        && PENDING.swap(false, Ordering::SeqCst)
        && let Some(dir) = CRASH_DIR.get()
    {
        let _ = std::fs::remove_file(dir.join(CRASH_FILE));
    }
}

fn current_action() -> Option<String> {
    ACTION.try_with(|a| a.borrow().clone()).ok().flatten()
}

/// Names what the user is doing until the returned guard drops, so a
/// panic in between carries that context.
pub fn action(name: impl Into<String>) -> ActionGuard {
    let prev = ACTION
        .try_with(|a| a.borrow_mut().replace(name.into()))
        .ok()
        .flatten();
    ActionGuard { prev }
}

pub struct ActionGuard {
    prev: Option<String>,
}

impl Drop for ActionGuard {
    fn drop(&mut self) {
        let prev = self.prev.take();
        let _ = ACTION.try_with(|a| *a.borrow_mut() = prev);
    }
}

// ------------------------------------------------------------ sessions

/// Start of an interactive session: returns the crash of the previous
/// session if it ended in one, and marks this session as running.
pub fn begin_session(dir: &Path) -> Option<Failure> {
    let prev = previous_crash(dir);
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(dir.join(MARKER), std::process::id().to_string());
    prev
}

/// A clean exit: remove the marker (and a stale crash file), unless a
/// UI-thread panic is still unhandled, which is not a clean exit.
pub fn end_session(dir: &Path) {
    if PENDING.load(Ordering::SeqCst) {
        return;
    }
    let _ = std::fs::remove_file(dir.join(MARKER));
    let _ = std::fs::remove_file(dir.join(CRASH_FILE));
}

/// The previous session's crash: a crash file plus a marker that was never
/// removed. Both files are consumed.
pub fn previous_crash(dir: &Path) -> Option<Failure> {
    let marker = dir.join(MARKER);
    let crash = dir.join(CRASH_FILE);
    let unclean = marker.exists();
    let failure = std::fs::read_to_string(&crash)
        .ok()
        .and_then(|s| serde_json::from_str::<Failure>(&s).ok());
    let _ = std::fs::remove_file(&crash);
    let _ = std::fs::remove_file(&marker);
    failure.filter(|_| unclean)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previous_crash_needs_marker_and_file() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        // First start: nothing.
        assert!(begin_session(d).is_none());
        // Clean exit: nothing next time.
        end_session(d);
        assert!(begin_session(d).is_none());
        // Crash: marker left behind plus a crash file.
        let f = Failure::from_message("index out of bounds");
        write_crash_file(d, &f);
        let got = begin_session(d).expect("crash offered");
        assert_eq!(got.message, "index out of bounds");
        // Offered once.
        assert!(begin_session(d).is_none());
        // A crash file without a leftover marker (clean exit after a
        // caught panic) is not offered, and is removed.
        end_session(d);
        write_crash_file(d, &f);
        assert!(begin_session(d).is_none());
        assert!(!d.join(CRASH_FILE).exists());
    }

    #[test]
    fn hook_records_location_and_context() {
        install_hook();
        let _a = action("Connect");
        let r = std::panic::catch_unwind(|| {
            let v: Vec<u32> = Vec::new();
            v[3]
        });
        assert!(r.is_err());
        let f = caught("fallback");
        assert!(f.message.contains("index out of bounds"), "{}", f.message);
        assert!(
            f.location
                .as_deref()
                .unwrap_or_default()
                .contains("crash.rs"),
            "{:?}",
            f.location
        );
        assert_eq!(f.context.as_deref(), Some("Connect"));
        assert!(!f.backtrace.is_empty());
        assert_eq!(take_caught(), vec![f]);
        assert!(take_caught().is_empty());
    }

    #[test]
    fn action_guards_nest() {
        let _a = action("Import");
        {
            let _b = action("Undo");
            assert_eq!(current_action().as_deref(), Some("Undo"));
        }
        assert_eq!(current_action().as_deref(), Some("Import"));
    }
}
