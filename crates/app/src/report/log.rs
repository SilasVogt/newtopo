//! The app log: stderr (as before, `RUST_LOG`, default `warn`), plus a
//! bounded file in the data directory and an in-memory ring of recent
//! lines for problem reports. Every line is cleaned before it is stored
//! anywhere: home directory and user name replaced, folders removed from
//! paths (see [`super::clean`]).
//!
//! File: `<data dir>/logs/newtopo.log`, moved to `newtopo.log.1` once it
//! passes [`MAX_FILE`], so at most about twice that is kept on disk.

use super::scrub::scrub;
use std::collections::VecDeque;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Size at which the log file is rotated.
pub const MAX_FILE: u64 = 1024 * 1024;
/// Lines kept in memory.
pub const RING_LINES: usize = 2000;
/// Most of the log that goes into a report.
pub const REPORT_LOG_BYTES: usize = 256 * 1024;

const FILE_NAME: &str = "newtopo.log";

/// `<data dir>/logs`.
pub fn log_dir(data: &Path) -> PathBuf {
    data.join("logs")
}

pub fn log_file(data: &Path) -> PathBuf {
    log_dir(data).join(FILE_NAME)
}

fn ring() -> &'static Mutex<VecDeque<String>> {
    static R: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(VecDeque::with_capacity(RING_LINES)))
}

/// The most recent log lines of this session (oldest first).
pub fn recent_lines() -> Vec<String> {
    ring()
        .lock()
        .map(|r| r.iter().cloned().collect())
        .unwrap_or_default()
}

/// Store one already-formatted line in the ring (and the file, when the
/// app logger is installed).
fn push_line(line: &str) {
    if let Ok(mut r) = ring().lock() {
        if r.len() >= RING_LINES {
            r.pop_front();
        }
        r.push_back(line.to_string());
    }
    if let Some(l) = LOGGER.get()
        && let Ok(mut f) = l.file.lock()
        && let Some(f) = f.as_mut()
    {
        f.write_line(line);
    }
}

struct LogFile {
    path: PathBuf,
    file: std::fs::File,
    size: u64,
    /// The file could be neither rotated nor emptied: nothing more is
    /// written, so it stays bounded. The in-memory ring carries on.
    stopped: bool,
    /// A line of this session may be missing from the file (it was
    /// emptied, a write failed, or it stopped). A report then adds the
    /// lines in memory (see [`report_log`]).
    gap: bool,
}

impl LogFile {
    fn open(path: PathBuf) -> Option<LogFile> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok()?;
        }
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_FILE) && !rotate(&path) {
            return None;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok()?;
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        Some(LogFile {
            path,
            file,
            size,
            stopped: false,
            gap: false,
        })
    }

    fn write_line(&mut self, line: &str) {
        if self.stopped {
            return;
        }
        if self.size > MAX_FILE {
            let rotated = rotate(&self.path);
            // A full file that cannot be moved aside is emptied instead:
            // the older lines are lost, the file stays bounded and keeps
            // this session's lines from here on.
            let mut open = std::fs::OpenOptions::new();
            if rotated {
                open.create(true).append(true);
            } else {
                self.gap = true;
                open.write(true).truncate(true);
            }
            match open.open(&self.path) {
                Ok(f) => {
                    self.file = f;
                    self.size = 0;
                }
                Err(_) => {
                    self.stopped = true;
                    self.gap = true;
                    return;
                }
            }
        }
        if writeln!(self.file, "{line}").is_ok() {
            self.size += line.len() as u64 + 1;
        } else {
            self.gap = true;
        }
    }
}

/// Moves the log aside. `false` when the full file is still in place
/// (the folder refuses the rename, or something else sits at the old
/// log's name): writing on would grow it without limit.
fn rotate(path: &Path) -> bool {
    let old = path.with_extension("log.1");
    let _ = std::fs::remove_file(&old);
    std::fs::rename(path, &old).is_ok()
}

/// One log line: `2026-11-25 10:04:05Z INFO  newtopo::jobs: message`,
/// cleaned (see [`super::clean`]).
pub fn format_line(secs: u64, level: log::Level, target: &str, msg: &str) -> String {
    let line = format!(
        "{} {:<5} {target}: {msg}",
        super::utc_stamp(secs),
        level.as_str()
    );
    super::clean(&line)
}

/// Which records go to the file and the ring: warnings and errors from
/// anywhere, info from the app's own crates (wgpu and friends are chatty).
fn wanted(level: log::Level, target: &str) -> bool {
    level <= log::Level::Warn || (level <= log::Level::Info && target.starts_with("newtopo"))
}

struct AppLogger {
    stderr: env_logger::Logger,
    file: Mutex<Option<LogFile>>,
    /// A log file was wanted but could not be opened.
    no_file: bool,
}

/// Whether a line of this session may be missing from the log file.
fn file_gap() -> bool {
    LOGGER.get().is_some_and(|l| {
        l.no_file
            || l.file
                .lock()
                .map(|f| f.as_ref().is_some_and(|f| f.gap))
                .unwrap_or(true)
    })
}

/// Stands between the log file's text and the lines kept in memory when
/// the file may lack lines of this session.
pub const GAP_MARKER: &str = "--- the log file was not being written; the lines below were kept in memory (some may repeat) ---";

static LOGGER: OnceLock<AppLogger> = OnceLock::new();

impl log::Log for AppLogger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        self.stderr.enabled(m) || wanted(m.level(), m.target())
    }

    fn log(&self, record: &log::Record) {
        if self.stderr.matches(record) {
            self.stderr.log(record);
        }
        if wanted(record.level(), record.target()) {
            let line = format_line(
                super::now_secs(),
                record.level(),
                record.target(),
                &record.args().to_string(),
            );
            push_line(&line);
        }
    }

    fn flush(&self) {
        self.stderr.flush();
        if let Ok(mut f) = self.file.lock()
            && let Some(f) = f.as_mut()
        {
            let _ = f.file.flush();
        }
    }
}

/// Install the app logger. `data` is the data directory (`None` keeps the
/// log in memory only, e.g. for screenshot runs).
pub fn init(data: Option<&Path>) {
    let stderr =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).build();
    let max = stderr.filter().max(log::LevelFilter::Info);
    let file = data.and_then(|d| LogFile::open(log_file(d)));
    let no_file = data.is_some() && file.is_none();
    if LOGGER
        .set(AppLogger {
            stderr,
            file: Mutex::new(file),
            no_file,
        })
        .is_err()
    {
        return;
    }
    if log::set_logger(LOGGER.get().expect("set above")).is_ok() {
        log::set_max_level(max);
    }
}

/// The log for a report: the end of the log file (this and earlier
/// sessions) when there is one, else the in-memory lines. When the file may
/// lack lines of this session (it could not be opened, rotated or written),
/// the in-memory lines follow it after [`GAP_MARKER`]. At most
/// [`REPORT_LOG_BYTES`], cut at a line start.
pub fn report_log(data: Option<&Path>) -> String {
    report_log_from(data, file_gap(), &recent_lines())
}

fn report_log_from(data: Option<&Path>, gap: bool, ring: &[String]) -> String {
    let from_file = data.and_then(|d| {
        let path = log_file(d);
        let cur = std::fs::read(&path).ok()?;
        let mut all = if (cur.len() as u64) < REPORT_LOG_BYTES as u64 {
            std::fs::read(path.with_extension("log.1")).unwrap_or_default()
        } else {
            Vec::new()
        };
        all.extend_from_slice(&cur);
        Some(String::from_utf8_lossy(&all).into_owned())
    });
    // Lines are scrubbed when written; scrub again in case the file came
    // from an older build or the home directory changed. The parts are
    // scrubbed before they are measured, so the cuts below are final.
    let mut memory = scrub(&ring.join("\n"));
    memory.push('\n');
    match from_file {
        Some(file) if gap => {
            let mut file = scrub(&file);
            if !file.ends_with('\n') && !file.is_empty() {
                file.push('\n');
            }
            // The marker always stays, then the newest lines in memory,
            // then as much of the file as still fits.
            let room = REPORT_LOG_BYTES - (GAP_MARKER.len() + 1);
            let memory = tail(&memory, room);
            let file = tail(&file, room - memory.len());
            format!("{file}{GAP_MARKER}\n{memory}")
        }
        Some(file) => tail(&scrub(&file), REPORT_LOG_BYTES),
        None => tail(&memory, REPORT_LOG_BYTES),
    }
}

/// The last `max` bytes of `s`, starting at a line.
pub fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    let cut = &s[start..];
    match cut.find('\n') {
        Some(i) => cut[i + 1..].to_string(),
        None => cut.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_formatted_and_filtered() {
        let l = format_line(0, log::Level::Info, "newtopo::jobs", "import started");
        assert_eq!(
            l,
            "1970-01-01 00:00:00Z INFO  newtopo::jobs: import started"
        );
        assert!(wanted(log::Level::Info, "newtopo::app"));
        assert!(!wanted(log::Level::Info, "wgpu_core::instance"));
        assert!(wanted(log::Level::Warn, "wgpu_core::instance"));
        assert!(!wanted(log::Level::Debug, "newtopo::app"));
    }

    #[test]
    fn a_failed_rotation_keeps_the_file_bounded_and_the_report_complete() {
        let dir = tempfile::tempdir().unwrap();
        let path = log_file(dir.path());
        // Something that cannot be replaced sits where the old log goes.
        let old = path.with_extension("log.1");
        std::fs::create_dir_all(old.join("keep")).unwrap();
        let mut f = LogFile::open(path.clone()).unwrap();
        let line = "x".repeat(1000);
        for _ in 0..5000 {
            f.write_line(&line);
        }
        let cur = std::fs::metadata(&path).unwrap().len();
        assert!(cur <= MAX_FILE + 2000, "current file {cur}");
        assert!(f.gap, "lines were dropped from the file");
        assert!(!f.stopped, "the emptied file is written again");
        f.write_line("after the failed rotation");
        drop(f);
        // The file has the line; the report also adds what memory kept.
        let ring = vec!["kept in memory".to_string()];
        let log = report_log_from(Some(dir.path()), true, &ring);
        assert!(log.contains("after the failed rotation"));
        assert!(log.contains(GAP_MARKER));
        assert!(log.ends_with("kept in memory\n"));
        assert!(log.len() <= REPORT_LOG_BYTES);
    }

    #[test]
    fn a_log_file_that_cannot_be_opened_does_not_hide_this_session() {
        let dir = tempfile::tempdir().unwrap();
        let path = log_file(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // An oversized file from earlier sessions that cannot be rotated.
        std::fs::create_dir_all(path.with_extension("log.1").join("keep")).unwrap();
        let mut old = "earlier session\n".repeat((MAX_FILE as usize / 16) + 10);
        old.push_str("last line of the earlier session\n");
        std::fs::write(&path, &old).unwrap();
        assert!(LogFile::open(path.clone()).is_none());
        assert_eq!(std::fs::metadata(&path).unwrap().len(), old.len() as u64);
        let ring = vec!["logged this session".to_string()];
        let log = report_log_from(Some(dir.path()), true, &ring);
        let marker = log.find(GAP_MARKER).expect("marker");
        assert!(log[..marker].contains("last line of the earlier session"));
        assert!(log[marker..].contains("logged this session"));
        assert!(log.len() <= REPORT_LOG_BYTES);
        // Without a gap the file alone is the log, as before.
        let log = report_log_from(Some(dir.path()), false, &ring);
        assert!(!log.contains(GAP_MARKER) && !log.contains("logged this session"));
    }

    #[test]
    fn the_marker_survives_a_memory_log_larger_than_the_report() {
        let dir = tempfile::tempdir().unwrap();
        let path = log_file(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "from the file\n".repeat(50)).unwrap();
        let ring: Vec<String> = (0..2000)
            .map(|i| format!("line {i:04} {}", "m".repeat(130)))
            .collect();
        assert!(ring.iter().map(|l| l.len() + 1).sum::<usize>() > REPORT_LOG_BYTES);
        let log = report_log_from(Some(dir.path()), true, &ring);
        assert!(log.len() <= REPORT_LOG_BYTES);
        let marker = log.find(GAP_MARKER).expect("the marker is kept");
        assert!(
            log.ends_with(&format!("{}\n", ring[1999])),
            "the newest line"
        );
        // Whole lines only: file lines before the marker, memory lines after.
        assert!(log[..marker].lines().all(|l| l == "from the file"));
        let after: Vec<&str> = log[marker..].lines().skip(1).collect();
        let known: std::collections::HashSet<&str> = ring.iter().map(|l| l.as_str()).collect();
        assert!(after.len() > 1000 && after.iter().all(|l| known.contains(l)));
    }

    #[test]
    fn a_failed_write_is_remembered() {
        let dir = tempfile::tempdir().unwrap();
        let path = log_file(dir.path());
        let mut f = LogFile::open(path.clone()).unwrap();
        // A handle that cannot be written to.
        f.file = std::fs::File::open(&path).unwrap();
        f.write_line("lost");
        assert!(f.gap);
    }

    #[test]
    fn file_rotates_and_stays_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let path = log_file(dir.path());
        let mut f = LogFile::open(path.clone()).unwrap();
        let line = "x".repeat(1000);
        for _ in 0..2500 {
            f.write_line(&line);
        }
        let cur = std::fs::metadata(&path).unwrap().len();
        let old = std::fs::metadata(path.with_extension("log.1"))
            .unwrap()
            .len();
        assert!(cur <= MAX_FILE + 2000, "current file {cur}");
        assert!(old <= MAX_FILE + 2000, "rotated file {old}");
        let log = report_log(Some(dir.path()));
        assert!(log.len() <= REPORT_LOG_BYTES);
        assert!(log.starts_with('x'), "cut at a line start");
    }

    #[test]
    fn tail_cuts_at_lines() {
        assert_eq!(tail("a\nb\nc\n", 100), "a\nb\nc\n");
        assert_eq!(tail("aaaa\nbb\ncc\n", 6), "cc\n");
        assert_eq!(tail("ääää\nö\n", 5), "ö\n");
    }
}
