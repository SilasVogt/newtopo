//! The app log: stderr (as before, `RUST_LOG`, default `warn`), plus a
//! bounded file in the data directory and an in-memory ring of recent
//! lines for problem reports. Every line is scrubbed of the home directory
//! and user name before it is stored anywhere.
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
}

impl LogFile {
    fn open(path: PathBuf) -> Option<LogFile> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok()?;
        }
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_FILE) {
            rotate(&path);
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok()?;
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        Some(LogFile { path, file, size })
    }

    fn write_line(&mut self, line: &str) {
        if self.size > MAX_FILE {
            rotate(&self.path);
            match std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
            {
                Ok(f) => {
                    self.file = f;
                    self.size = 0;
                }
                Err(_) => return,
            }
        }
        if writeln!(self.file, "{line}").is_ok() {
            self.size += line.len() as u64 + 1;
        }
    }
}

fn rotate(path: &Path) {
    let old = path.with_extension("log.1");
    let _ = std::fs::remove_file(&old);
    let _ = std::fs::rename(path, &old);
}

/// One log line: `2026-11-25 10:04:05Z INFO  newtopo::jobs: message`,
/// scrubbed.
pub fn format_line(secs: u64, level: log::Level, target: &str, msg: &str) -> String {
    let line = format!(
        "{} {:<5} {target}: {msg}",
        super::utc_stamp(secs),
        level.as_str()
    );
    scrub(&line)
}

/// Which records go to the file and the ring: warnings and errors from
/// anywhere, info from the app's own crates (wgpu and friends are chatty).
fn wanted(level: log::Level, target: &str) -> bool {
    level <= log::Level::Warn || (level <= log::Level::Info && target.starts_with("newtopo"))
}

struct AppLogger {
    stderr: env_logger::Logger,
    file: Mutex<Option<LogFile>>,
}

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
    if LOGGER
        .set(AppLogger {
            stderr,
            file: Mutex::new(file),
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
/// sessions) when there is one, else the in-memory lines. At most
/// [`REPORT_LOG_BYTES`], cut at a line start.
pub fn report_log(data: Option<&Path>) -> String {
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
    let text = from_file.unwrap_or_else(|| {
        let mut s = recent_lines().join("\n");
        s.push('\n');
        s
    });
    // Lines are scrubbed when written; scrub again in case the file came
    // from an older build or the home directory changed.
    tail(&scrub(&text), REPORT_LOG_BYTES)
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
