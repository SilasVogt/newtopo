//! Panic capture and previous-crash detection.
//!
//! * The panic hook records message, source location and a backtrace
//!   (scrubbed) for the panicking thread, logs it, and on the UI thread also
//!   writes this session's crash file, since a UI-thread panic that nobody
//!   catches ends the app.
//! * [`crate::docstate::guard`] and the job threads catch engine panics and
//!   hand them to [`caught`]: the record is queued for the problem-report
//!   dialog and the crash file is removed again (the app survived).
//! * Every session owns its files in `<data>/sessions/`, named after its
//!   process (id and start time): `<id>.lock` exists while it runs and is
//!   removed on a clean exit, `<id>.crash.json` after a UI-thread panic.
//!   At the next start, the crash file of a session that has ended (its
//!   process is gone) with its marker left behind means that session ended
//!   in a crash. Sessions still running (another window) are left alone.

use super::scrub::scrub;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, TryLockError};
use std::time::{Duration, Instant};

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

/// The running session's crash file (set by [`activate`]).
static ACTIVE: Mutex<Option<CrashFile>> = Mutex::new(None);

const SESSIONS: &str = "sessions";
const MARKER_EXT: &str = ".lock";
const CRASH_EXT: &str = ".crash.json";
/// A panic that repeats more often than this rewrites the crash file only
/// when it changes (see [`CrashFile`]).
const REPEATS: u32 = 3;
/// How long a repeating caught panic must stay away before its crash file
/// is withdrawn.
const WITHDRAW_AFTER: Duration = Duration::from_secs(1);
/// Seconds a process's start time may move between two looks at it (see
/// [`Owner::same_start`]).
const START_SLACK: u64 = 5;

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

/// Make `files` the running session: the hook writes its crash file and
/// [`end_session`] removes its files on a clean exit.
pub fn activate(files: SessionFiles) {
    *ACTIVE.lock().unwrap_or_else(|e| e.into_inner()) = Some(CrashFile::new(files));
}

/// The running session's crash file, unless the hook is already using it
/// on this thread.
fn with_active(f: impl FnOnce(&mut CrashFile)) {
    let mut g = match ACTIVE.try_lock() {
        Ok(g) => g,
        Err(TryLockError::Poisoned(p)) => p.into_inner(),
        Err(TryLockError::WouldBlock) => return,
    };
    if let Some(c) = g.as_mut() {
        f(c);
    }
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
    if ui_thread {
        with_active(|c| c.record(&f, seen));
    }
    let _ = LAST.try_with(|l| *l.borrow_mut() = Some(f));
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
    if std::thread::current().name() == Some("main") {
        with_active(|c| c.survived(Instant::now()));
    }
}

/// Once a frame (UI thread): withdraw the crash file of a repeating caught
/// panic once it has stopped.
pub fn tick() {
    with_active(|c| c.tick(Instant::now()));
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

/// The process that runs a session: its id and start time (Unix seconds,
/// 0 when unknown), so a reused process id is not taken for the same
/// session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Owner {
    pub pid: u32,
    pub start: u64,
}

impl Owner {
    /// This process.
    pub fn current() -> Owner {
        let pid = std::process::id();
        Owner {
            pid,
            start: process(pid).map(|(start, _)| start).unwrap_or(0),
        }
    }

    /// Whether the session's process is still running.
    pub fn alive(&self) -> bool {
        process(self.pid).is_some_and(|(start, zombie)| !zombie && self.same_start(start))
    }

    /// Whether a process that started at `start` is this one. Start times
    /// are derived from the boot time, which moves when the wall clock is
    /// stepped, so they may differ by a few seconds.
    fn same_start(&self, start: u64) -> bool {
        self.start == 0 || start == 0 || self.start.abs_diff(start) <= START_SLACK
    }

    fn id(&self) -> String {
        format!("{}-{}", self.pid, self.start)
    }

    fn parse(id: &str) -> Option<Owner> {
        let (pid, start) = id.split_once('-')?;
        Some(Owner {
            pid: pid.parse().ok()?,
            start: start.parse().ok()?,
        })
    }
}

/// Start time and zombie state of a running process.
fn process(pid: u32) -> Option<(u64, bool)> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, System};
    let pid = Pid::from_u32(pid);
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing(),
    );
    sys.process(pid)
        .map(|p| (p.start_time(), p.status() == ProcessStatus::Zombie))
}

/// One session's files in `<data>/sessions/`.
#[derive(Debug)]
pub struct SessionFiles {
    dir: PathBuf,
    owner: Owner,
}

impl SessionFiles {
    pub fn marker(&self) -> PathBuf {
        self.dir.join(format!("{}{MARKER_EXT}", self.owner.id()))
    }

    pub fn crash_file(&self) -> PathBuf {
        self.dir.join(format!("{}{CRASH_EXT}", self.owner.id()))
    }

    pub fn write_crash(&self, f: &Failure) {
        if let Ok(s) = serde_json::to_string_pretty(f) {
            let _ = std::fs::create_dir_all(&self.dir);
            let _ = std::fs::write(self.crash_file(), s);
        }
    }

    fn remove_crash(&self) {
        let _ = std::fs::remove_file(self.crash_file());
    }

    /// A clean exit: this session's marker and crash file go; other
    /// sessions' files stay.
    pub fn end(&self) {
        let _ = std::fs::remove_file(self.marker());
        self.remove_crash();
    }
}

/// The running session's crash file as the panic hook and the app see it.
/// A caught panic withdraws the file; one that repeats (every frame, say)
/// leaves it on disk until it stops, instead of writing and deleting it
/// every time.
#[derive(Debug)]
struct CrashFile {
    files: SessionFiles,
    /// The failure the file on disk describes.
    on_disk: Option<(String, Option<String>)>,
    /// How often the last recorded panic has happened.
    seen: u32,
    /// A UI-thread panic was recorded and not caught (yet).
    pending: bool,
    /// Withdraw the file then, unless the panic comes back.
    withdraw_at: Option<Instant>,
    /// Crash files written (for tests).
    writes: u32,
}

impl CrashFile {
    fn new(files: SessionFiles) -> CrashFile {
        CrashFile {
            files,
            on_disk: None,
            seen: 0,
            pending: false,
            withdraw_at: None,
            writes: 0,
        }
    }

    /// The hook saw a UI-thread panic (the `seen`th of its kind).
    fn record(&mut self, f: &Failure, seen: u32) {
        self.pending = true;
        self.seen = seen;
        self.withdraw_at = None;
        if seen > REPEATS && self.on_disk.as_ref() == Some(&f.key()) {
            return;
        }
        self.files.write_crash(f);
        self.on_disk = Some(f.key());
        self.writes += 1;
    }

    /// The panic was caught and the app goes on.
    fn survived(&mut self, now: Instant) {
        if !std::mem::take(&mut self.pending) {
            return;
        }
        if self.seen > REPEATS {
            self.withdraw_at = Some(now + WITHDRAW_AFTER);
        } else {
            self.withdraw();
        }
    }

    fn tick(&mut self, now: Instant) {
        if !self.pending && self.withdraw_at.is_some_and(|t| now >= t) {
            self.withdraw();
        }
    }

    fn withdraw(&mut self) {
        self.files.remove_crash();
        self.on_disk = None;
        self.withdraw_at = None;
    }

    /// A clean exit, unless a UI-thread panic is still unhandled.
    fn end(&self) {
        if !self.pending {
            self.files.end();
        }
    }
}

/// An earlier session's crash, found at start-up. Its files stay on disk
/// until [`PreviousCrash::offered`], so a start-up that fails before the
/// app can offer it leaves it for the next start.
#[derive(Debug)]
pub struct PreviousCrash {
    pub failure: Failure,
    /// The crashed session's files; `None` when they are already gone.
    files: Option<SessionFiles>,
}

impl PreviousCrash {
    /// The report for this crash was offered: its files go.
    pub fn offered(&self) {
        if let Some(f) = &self.files {
            f.end();
        }
    }
}

/// Start of an interactive session in the data folder `data`: returns this
/// session's files (already marked as running) and the crash of an earlier
/// session that ended in one.
pub fn begin_session(data: &Path) -> (SessionFiles, Option<PreviousCrash>) {
    begin_session_as(data, Owner::current(), &|o: &Owner| o.alive())
}

/// [`begin_session`] for a given process, with a given test of whether a
/// process still runs.
pub fn begin_session_as(
    data: &Path,
    me: Owner,
    alive: &dyn Fn(&Owner) -> bool,
) -> (SessionFiles, Option<PreviousCrash>) {
    let dir = data.join(SESSIONS);
    let prev = previous_crash(&dir, me, alive);
    let files = SessionFiles { dir, owner: me };
    let _ = std::fs::create_dir_all(&files.dir);
    let _ = std::fs::write(files.marker(), me.id());
    (files, prev)
}

/// A clean exit of the running session (see [`activate`]).
pub fn end_session() {
    if let Some(c) = ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).take() {
        c.end();
    }
}

/// The crash of an earlier session: sessions whose process has ended are
/// consumed (marker and crash file), except the newest crash (a crash file
/// next to its leftover marker), which is returned and stays on disk until
/// it is offered. A crash file without its marker is removed and not
/// offered. Sessions still running are left alone. A session of this very
/// process (same id and start time) cannot be running in parallel, so it
/// counts as ended; its files are the new session's, so they are consumed
/// right away.
fn previous_crash(dir: &Path, me: Owner, alive: &dyn Fn(&Owner) -> bool) -> Option<PreviousCrash> {
    let mut ids: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.strip_suffix(MARKER_EXT)
                .or_else(|| name.strip_suffix(CRASH_EXT))
                .map(str::to_string)
        })
        .collect();
    ids.sort();
    ids.dedup();
    let mut found: Option<PreviousCrash> = None;
    for id in ids {
        let Some(owner) = Owner::parse(&id) else {
            continue;
        };
        if owner != me && alive(&owner) {
            continue;
        }
        let files = SessionFiles {
            dir: dir.to_path_buf(),
            owner,
        };
        let failure = files
            .marker()
            .exists()
            .then(|| std::fs::read_to_string(files.crash_file()).ok())
            .flatten()
            .and_then(|s| serde_json::from_str::<Failure>(&s).ok());
        let Some(failure) = failure else {
            files.end();
            continue;
        };
        if found
            .as_ref()
            .is_some_and(|g| failure.time < g.failure.time)
        {
            files.end();
            continue;
        }
        let files = if owner == me {
            files.end();
            None
        } else {
            Some(files)
        };
        // An older crash is superseded: consumed like the other ended
        // sessions.
        if let Some(older) = found.replace(PreviousCrash { failure, files }) {
            older.offered();
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Owner = Owner {
        pid: 1001,
        start: 50,
    };
    const B: Owner = Owner {
        pid: 1002,
        start: 60,
    };
    const C: Owner = Owner {
        pid: 1003,
        start: 70,
    };

    /// `begin_session_as` with the given processes still running; a crash
    /// found is offered right away, as the app does once it is up.
    fn begin(d: &Path, me: Owner, running: &[Owner]) -> (SessionFiles, Option<Failure>) {
        let (files, prev) = begin_unoffered(d, me, running);
        (
            files,
            prev.map(|p| {
                p.offered();
                p.failure
            }),
        )
    }

    /// `begin_session_as` without offering the crash found.
    fn begin_unoffered(
        d: &Path,
        me: Owner,
        running: &[Owner],
    ) -> (SessionFiles, Option<PreviousCrash>) {
        let running = running.to_vec();
        begin_session_as(d, me, &move |o: &Owner| running.contains(o))
    }

    #[test]
    fn previous_crash_needs_marker_and_file() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        // First start: nothing.
        let (a, prev) = begin(d, A, &[]);
        assert!(prev.is_none());
        assert!(a.marker().exists(), "marked as running");
        // Clean exit: nothing next time.
        a.end();
        let (b, prev) = begin(d, B, &[]);
        assert!(prev.is_none());
        // Crash: marker left behind plus a crash file.
        b.write_crash(&Failure::from_message("index out of bounds"));
        let (c, prev) = begin(d, C, &[]);
        assert_eq!(prev.expect("crash offered").message, "index out of bounds");
        assert!(!b.crash_file().exists() && !b.marker().exists(), "consumed");
        // Offered once.
        let (_a, prev) = begin(d, A, &[]);
        assert!(prev.is_none());
        // A crash file without its marker (an OS kill after the marker was
        // removed) is not offered, and is removed.
        c.end();
        c.write_crash(&Failure::from_message("stale"));
        let (_b, prev) = begin(d, B, &[A]);
        assert!(prev.is_none());
        assert!(!c.crash_file().exists());
        // An OS kill with no crash file: the marker alone offers nothing.
        let (_c, prev) = begin(d, C, &[]);
        assert!(prev.is_none());
    }

    #[test]
    fn a_clean_exit_of_another_window_keeps_this_ones_crash() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        // A and B run side by side.
        let (a, _) = begin(d, A, &[]);
        let (b, prev) = begin(d, B, &[A]);
        assert!(prev.is_none());
        assert!(a.marker().exists(), "B leaves running A alone");
        // B closes cleanly, then A panics and dies.
        b.end();
        assert!(a.marker().exists(), "B removed only its own marker");
        a.write_crash(&Failure::from_message("A crashed"));
        let (_c, prev) = begin(d, C, &[]);
        assert_eq!(prev.expect("A's crash").message, "A crashed");
    }

    #[test]
    fn a_crash_before_another_window_exits_survives_that_exit() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let (a, _) = begin(d, A, &[]);
        let (b, _) = begin(d, B, &[A]);
        // A crashes while B still runs; B exits cleanly afterwards.
        a.write_crash(&Failure::from_message("A crashed first"));
        b.end();
        assert!(a.crash_file().exists(), "B never touches A's crash file");
        // A session starting while B still ran would also have found it;
        // here the next start comes after both.
        let (_c, prev) = begin(d, C, &[]);
        assert_eq!(prev.expect("A's crash").message, "A crashed first");
    }

    #[test]
    fn a_running_window_is_not_a_previous_session() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let (a, _) = begin(d, A, &[]);
        // A has a crash file on disk (a panic being handled right now).
        a.write_crash(&Failure::from_message("still running"));
        let (_b, prev) = begin(d, B, &[A]);
        assert!(prev.is_none(), "A is still running");
        assert!(a.crash_file().exists() && a.marker().exists(), "left alone");
        // Once A is gone, the next start finds it.
        let (_c, prev) = begin(d, C, &[B]);
        assert_eq!(prev.unwrap().message, "still running");
    }

    #[test]
    fn a_crash_stays_on_disk_until_it_is_offered() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let (a, _) = begin(d, A, &[]);
        a.write_crash(&Failure::from_message("A crashed"));
        // B finds it, then its start-up fails before the app offers it
        // (no window: the marker stays, no crash file of its own).
        let (_b, prev) = begin_unoffered(d, B, &[]);
        assert_eq!(prev.expect("found").failure.message, "A crashed");
        assert!(a.crash_file().exists() && a.marker().exists(), "kept");
        // C still gets it, and offers it: then it is gone.
        let (_c, prev) = begin_unoffered(d, C, &[]);
        let prev = prev.expect("still there");
        assert_eq!(prev.failure.message, "A crashed");
        prev.offered();
        assert!(!a.crash_file().exists() && !a.marker().exists(), "consumed");
        let (_a, prev) = begin(d, A, &[]);
        assert!(prev.is_none(), "offered once");
    }

    #[test]
    fn only_the_newest_crash_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let (a, _) = begin(d, A, &[]);
        let (b, _) = begin(d, B, &[A]);
        let mut old = Failure::from_message("older");
        old.time = 100;
        let mut new = Failure::from_message("newer");
        new.time = 200;
        b.write_crash(&old);
        a.write_crash(&new);
        let (_c, prev) = begin_unoffered(d, C, &[]);
        assert_eq!(prev.expect("found").failure.message, "newer");
        assert!(a.crash_file().exists(), "the offered one waits");
        assert!(
            !b.crash_file().exists() && !b.marker().exists(),
            "superseded"
        );
    }

    #[test]
    fn this_process_is_running() {
        let me = Owner::current();
        assert!(me.alive());
        let gone = Owner {
            pid: u32::MAX - 7,
            start: 1,
        };
        assert!(!gone.alive());
        // A start time that moved a little (the wall clock was stepped) is
        // still this process; a different one is not.
        if me.start > START_SLACK {
            for start in [me.start - START_SLACK, me.start + START_SLACK] {
                assert!(Owner { start, ..me }.alive(), "{start} vs {}", me.start);
            }
            for start in [me.start - START_SLACK - 1, me.start + START_SLACK + 60] {
                assert!(!Owner { start, ..me }.alive(), "{start} vs {}", me.start);
            }
            assert!(me.same_start(me.start + 3) && !me.same_start(me.start + 6));
        }
    }

    #[test]
    fn a_panic_every_frame_writes_the_crash_file_rarely() {
        let dir = tempfile::tempdir().unwrap();
        let (files, _) = begin(dir.path(), A, &[]);
        let path = files.crash_file();
        let mut c = CrashFile::new(files);
        let f = Failure::from_message("every frame");
        let t0 = Instant::now();
        // First few times: written and withdrawn as before.
        for seen in 1..=REPEATS {
            c.record(&f, seen);
            assert!(path.exists());
            c.survived(t0);
            assert!(!path.exists(), "caught: withdrawn");
        }
        // Then it keeps repeating, caught each time (60 frames a second).
        for seen in REPEATS + 1..REPEATS + 300 {
            let now = t0 + Duration::from_millis(16 * u64::from(seen));
            c.tick(now);
            c.record(&f, seen);
            c.survived(now);
            assert!(path.exists(), "kept while it repeats");
        }
        assert_eq!(c.writes, REPEATS + 1, "not rewritten every frame");
        // It stops: withdrawn shortly after.
        let later = t0 + Duration::from_secs(10);
        c.tick(later);
        assert!(!path.exists());
        // A different failure is written right away.
        c.record(&Failure::from_message("another"), REPEATS + 1);
        assert!(path.exists());
        assert_eq!(c.writes, REPEATS + 2);
        // Not caught: the exit is not clean, the file stays for next time.
        c.end();
        assert!(path.exists());
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
