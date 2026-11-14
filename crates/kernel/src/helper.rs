//! Runs the `newtopo-import` helper process (SPEC §3.4, §8.1) and reads its
//! output. A crash or hang in the CAD readers kills the helper, never the app.
//! OWNER: occt. Signatures are a contract used by the app and CLI.
//!
//! Protocol (own design):
//! * `newtopo-import <input> --out-dir <dir> [--no-heal] [--sewing-tol x] [--units mm]`
//! * stdout: lines `progress <0..1> <message>`.
//! * stderr: free text; lines starting with `warning: ` are surfaced as
//!   [`ImportedFile::warnings`], everything else is shown on failure.
//! * On success `<dir>` holds `body-<n>.bbrep` and [`SIDECAR_FILE`] (a
//!   [`Sidecar`]); exit status 0. Any other exit means failure.

use crate::{BodyAttributes, HealReport, KernelError};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImportOptions {
    pub heal: bool,
    /// Sewing tolerance in model units.
    pub sewing_tolerance: f64,
    /// Target units ("mm", "cm", "m", "in"); the helper converts.
    pub units: String,
    pub timeout: Duration,
    /// Explicit helper path; default: next to the current executable.
    pub helper: Option<PathBuf>,
}

impl Default for ImportOptions {
    fn default() -> Self {
        ImportOptions {
            heal: true,
            sewing_tolerance: 1e-3,
            units: "mm".into(),
            timeout: Duration::from_secs(300),
            helper: None,
        }
    }
}

/// One body read from a CAD file.
#[derive(Clone, Debug)]
pub struct ImportedBody {
    /// OCCT binary B-rep bytes, loadable with `CadKernel::load_brep`.
    pub brep: Vec<u8>,
    pub attrs: BodyAttributes,
    pub heal: HealReport,
}

#[derive(Clone, Debug)]
pub struct ImportedFile {
    pub bodies: Vec<ImportedBody>,
    /// SHA-256 of the source file, hex.
    pub source_sha256: String,
    /// Helper's warnings (stderr lines worth showing).
    pub warnings: Vec<String>,
}

/// Progress callback: fraction 0..1 and a message. Return false to cancel.
pub type Progress<'a> = &'a mut dyn FnMut(f32, &str) -> bool;

/// File name of the sidecar the helper writes next to the bodies.
pub const SIDECAR_FILE: &str = "sidecar.json";
/// Current [`Sidecar::version`].
pub const SIDECAR_VERSION: u32 = 1;
/// Environment variable overriding the helper path.
pub const HELPER_ENV: &str = "NEWTOPO_IMPORT_HELPER";

/// The helper's JSON sidecar (own format).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sidecar {
    pub version: u32,
    /// Source file name (no directories).
    pub source: String,
    pub source_sha256: String,
    pub units: String,
    pub kernel: String,
    pub bodies: Vec<SidecarBody>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SidecarBody {
    /// `body-<n>.bbrep`, relative to the sidecar.
    pub file: String,
    pub attrs: BodyAttributes,
    pub heal: HealReport,
}

/// Parses a `progress <f> <message>` line.
pub fn parse_progress(line: &str) -> Option<(f32, String)> {
    let rest = line.strip_prefix("progress ")?;
    let (num, msg) = rest.split_once(' ').unwrap_or((rest, ""));
    let f: f32 = num.parse().ok()?;
    Some((f.clamp(0.0, 1.0), msg.trim().to_string()))
}

fn helper_name() -> String {
    format!("newtopo-import{}", std::env::consts::EXE_SUFFIX)
}

/// Finds the helper: explicit path, `$NEWTOPO_IMPORT_HELPER`, next to the
/// current executable, then up to three parent directories of it and their
/// `debug/` and `release/` sub-directories (covers `target/debug/deps` test
/// binaries and mixed debug/release builds).
pub fn find_helper(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return p.is_file().then(|| p.to_path_buf());
    }
    if let Some(p) = std::env::var_os(HELPER_ENV).map(PathBuf::from)
        && p.is_file()
    {
        return Some(p);
    }
    let exe = std::env::current_exe().ok()?;
    let name = helper_name();
    let mut dir = exe.parent();
    for _ in 0..4 {
        let d = dir?;
        for cand in [
            d.join(&name),
            d.join("debug").join(&name),
            d.join("release").join(&name),
        ] {
            if cand.is_file() {
                return Some(cand);
            }
        }
        dir = d.parent();
    }
    None
}

/// A temporary directory removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> std::io::Result<TempDir> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let p =
            std::env::temp_dir().join(format!("newtopo-import-{}-{nanos}-{n}", std::process::id()));
        std::fs::create_dir_all(&p)?;
        Ok(TempDir(p))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

enum Line {
    Out(String),
    Err(String),
}

fn fail(msg: impl Into<String>) -> anyhow_like::Error {
    anyhow_like::Error(format!("import failed: {}", msg.into()))
}

fn kill(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Error text from the helper's stderr (non-warning lines first).
fn stderr_text(lines: &[String]) -> String {
    let errs: Vec<&str> = lines
        .iter()
        .map(String::as_str)
        .filter(|l| !l.starts_with("warning: "))
        .collect();
    let pick = if errs.is_empty() {
        lines.iter().map(String::as_str).collect()
    } else {
        errs
    };
    let tail = pick.len().saturating_sub(20);
    pick[tail..].join("\n")
}

pub fn run_import(
    path: &Path,
    opts: &ImportOptions,
    progress: Progress,
) -> anyhow_like::Result<ImportedFile> {
    let helper = find_helper(opts.helper.as_deref()).ok_or_else(|| {
        fail(match &opts.helper {
            Some(p) => format!("import helper not found at {}", p.display()),
            None => format!(
                "import helper `{}` not found next to the executable (set {HELPER_ENV})",
                helper_name()
            ),
        })
    })?;
    if !path.is_file() {
        return Err(fail(format!("{} is not a file", path.display())));
    }
    let dir = TempDir::new().map_err(|e| fail(format!("temp dir: {e}")))?;
    let mut cmd = Command::new(&helper);
    cmd.arg(path)
        .arg("--out-dir")
        .arg(&dir.0)
        .arg("--sewing-tol")
        .arg(opts.sewing_tolerance.to_string())
        .arg("--units")
        .arg(&opts.units)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if !opts.heal {
        cmd.arg("--no-heal");
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| fail(format!("cannot start {}: {e}", helper.display())))?;

    let (tx, rx) = mpsc::channel::<Line>();
    let out = child.stdout.take().expect("piped stdout");
    let err = child.stderr.take().expect("piped stderr");
    let tx2 = tx.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if tx.send(Line::Out(line)).is_err() {
                break;
            }
        }
    });
    std::thread::spawn(move || {
        for line in BufReader::new(err).lines().map_while(Result::ok) {
            if tx2.send(Line::Err(line)).is_err() {
                break;
            }
        }
    });

    let start = Instant::now();
    let mut stderr = Vec::new();
    let mut streams_open = true;
    let mut last = (0.0f32, String::new());
    let mut last_poll = Instant::now();
    let status = loop {
        if streams_open {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(Line::Out(l)) => {
                    if let Some((f, m)) = parse_progress(&l) {
                        last_poll = Instant::now();
                        let go = progress(f, &m);
                        last = (f, m);
                        if !go {
                            kill(&mut child);
                            return Err(anyhow_like::Error("import cancelled".into()));
                        }
                    }
                }
                Ok(Line::Err(l)) => stderr.push(l),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => streams_open = false,
            }
        } else {
            std::thread::sleep(Duration::from_millis(10));
        }
        if !streams_open && let Some(st) = child.try_wait().map_err(|e| fail(e.to_string()))? {
            break st;
        }
        if start.elapsed() > opts.timeout {
            kill(&mut child);
            while let Ok(Line::Err(l)) | Ok(Line::Out(l)) = rx.try_recv() {
                stderr.push(l);
            }
            let text = stderr_text(&stderr);
            let sep = if text.is_empty() { "" } else { "\n" };
            return Err(fail(format!(
                "timed out after {} s{sep}{text}",
                opts.timeout.as_secs_f32()
            )));
        }
        if last_poll.elapsed() > Duration::from_millis(100) {
            // Re-poll with the last state so cancel works while OCCT is busy.
            last_poll = Instant::now();
            if !progress(last.0, &last.1) {
                kill(&mut child);
                return Err(anyhow_like::Error("import cancelled".into()));
            }
        }
    };
    let warnings: Vec<String> = stderr
        .iter()
        .filter_map(|l| l.strip_prefix("warning: ").map(str::to_string))
        .collect();
    if !status.success() {
        let text = stderr_text(&stderr);
        return Err(fail(if text.is_empty() {
            format!("helper exited with {status}")
        } else {
            text
        }));
    }

    let side_path = dir.0.join(SIDECAR_FILE);
    let side_bytes = std::fs::read(&side_path).map_err(|e| fail(format!("no sidecar: {e}")))?;
    let side: Sidecar =
        serde_json::from_slice(&side_bytes).map_err(|e| fail(format!("bad sidecar: {e}")))?;
    if side.version != SIDECAR_VERSION {
        return Err(fail(format!(
            "sidecar version {} (expected {SIDECAR_VERSION})",
            side.version
        )));
    }
    let mut bodies = Vec::with_capacity(side.bodies.len());
    for b in side.bodies {
        if b.file.contains(['/', '\\']) || b.file.contains("..") {
            return Err(fail(format!("bad body file name {:?}", b.file)));
        }
        let brep =
            std::fs::read(dir.0.join(&b.file)).map_err(|e| fail(format!("{}: {e}", b.file)))?;
        bodies.push(ImportedBody {
            brep,
            attrs: b.attrs,
            heal: b.heal,
        });
    }
    let mut all_warnings = side.warnings;
    all_warnings.extend(warnings);
    Ok(ImportedFile {
        bodies,
        source_sha256: side.source_sha256,
        warnings: all_warnings,
    })
}

/// Minimal error type so the kernel crate needs no anyhow.
pub mod anyhow_like {
    #[derive(Debug, Clone)]
    pub struct Error(pub String);
    impl std::fmt::Display for Error {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.0)
        }
    }
    impl std::error::Error for Error {}
    pub type Result<T> = std::result::Result<T, Error>;
}

impl From<KernelError> for anyhow_like::Error {
    fn from(e: KernelError) -> Self {
        anyhow_like::Error(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_lines() {
        assert_eq!(
            parse_progress("progress 0.5 healing body 1"),
            Some((0.5, "healing body 1".into()))
        );
        assert_eq!(parse_progress("progress 1"), Some((1.0, String::new())));
        assert_eq!(parse_progress("progress 7 x"), Some((1.0, "x".into())));
        assert_eq!(parse_progress("progress x y"), None);
        assert_eq!(parse_progress("hello"), None);
    }

    /// Fake helpers (shell scripts) exercise crash, hang and failure paths
    /// without OCCT.
    #[cfg(unix)]
    fn fake_helper(body: &str) -> (TempDir, PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let script = dir.0.join("newtopo-import");
        std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let input = dir.0.join("model.step");
        std::fs::write(&input, "ISO-10303-21;").unwrap();
        (dir, script, input)
    }

    #[cfg(unix)]
    #[test]
    fn crashing_hanging_and_failing_helpers() {
        let run = |body: &str, timeout_ms: u64| {
            let (_dir, script, input) = fake_helper(body);
            let opts = ImportOptions {
                helper: Some(script),
                timeout: Duration::from_millis(timeout_ms),
                ..Default::default()
            };
            run_import(&input, &opts, &mut |_, _| true).unwrap_err().0
        };
        let e = run("echo 'progress 0.1 reading'; echo 'reader exploded' >&2; kill -SEGV $$", 10_000);
        assert!(e.starts_with("import failed:") && e.contains("reader exploded"), "{e}");
        let e = run("kill -SEGV $$", 10_000);
        assert!(e.starts_with("import failed:") && e.contains("signal"), "{e}");
        let started = Instant::now();
        let e = run("echo 'stuck' >&2; sleep 30", 300);
        assert!(e.contains("timed out") && e.contains("stuck"), "{e}");
        assert!(started.elapsed() < Duration::from_secs(10));
        let e = run("echo 'warning: odd entity' >&2; echo 'bad file' >&2; exit 3", 10_000);
        assert!(e.contains("bad file") && !e.contains("odd entity"), "{e}");
        // Exit 0 without a sidecar is still a failure.
        let e = run("exit 0", 10_000);
        assert!(e.contains("no sidecar"), "{e}");
    }

    #[cfg(unix)]
    #[test]
    fn cancel_while_helper_is_silent() {
        let (_dir, script, input) = fake_helper("echo 'progress 0.2 reading'; sleep 30");
        let opts = ImportOptions { helper: Some(script), ..Default::default() };
        let started = Instant::now();
        let mut calls = 0;
        let e = run_import(&input, &opts, &mut |_, _| {
            calls += 1;
            calls < 3
        })
        .unwrap_err();
        assert_eq!(e.0, "import cancelled");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn missing_helper_is_an_error() {
        let opts = ImportOptions {
            helper: Some("/nonexistent/newtopo-import".into()),
            ..Default::default()
        };
        let e = run_import(Path::new("x.step"), &opts, &mut |_, _| true).unwrap_err();
        assert!(e.0.starts_with("import failed:"), "{e}");
    }
}
