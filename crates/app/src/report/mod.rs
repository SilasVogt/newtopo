//! Crash logging and problem reports (SPEC §5.11).
//!
//! * [`log`]: the app log (file + in-memory ring), scrubbed.
//! * [`crash`]: panic hook, caught failures, previous-crash detection.
//! * [`system`]: "PC details".
//! * this module: the session's steps, the report's files, the zip, the
//!   GitHub issue link and the AI agent prompt.
//! * [`ui`]: the "Report a problem" dialog.
//!
//! Nothing here sends anything anywhere: the user saves the zip and opens
//! the issue page themselves.

pub mod crash;
pub mod log;
pub mod scrub;
pub mod system;
pub mod ui;

use crash::Failure;
use newtopo_core::ops::Op;
use scrub::scrub;

/// Text that goes into a log line or a report: known folders, home and
/// user name removed and folders stripped from file paths (see
/// [`scrub::Scrubber::clean`]).
pub fn clean(text: &str) -> String {
    scrub::Scrubber::current().clean(text)
}
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// The repository reports go to.
pub const REPO: &str = "SilasVogt/newtopo";
pub const ISSUE_URL: &str = "https://github.com/SilasVogt/newtopo/issues/new";
/// Browsers and GitHub handle URLs up to ~8 KB; stay well below.
pub const MAX_ISSUE_URL: usize = 6000;
/// Steps JSON up to this size is pasted into the agent prompt.
pub const PROMPT_STEPS_MAX: usize = 6 * 1024;
/// Lines of `error.txt` (mostly backtrace) pasted into the agent prompt.
const PROMPT_ERROR_LINES: usize = 60;
/// The session record stops growing here (it is for reproducing, and a
/// report should stay small).
const MAX_OPS: usize = 5000;
const MAX_EVENTS: usize = 10000;

// ------------------------------------------------------------ start-up

/// What [`start`] found.
#[derive(Debug, Default)]
pub struct Startup {
    /// Where this run logs and keeps its crash file; `None` for capture runs.
    pub data: Option<PathBuf>,
    /// This session's marker and crash file (already marked as running),
    /// for [`crash::activate`]; `None` for capture runs.
    pub session: Option<crash::SessionFiles>,
    /// An earlier session's crash, to offer once the app is up (it stays
    /// on disk until offered).
    pub previous_crash: Option<crash::PreviousCrash>,
}

/// Start the crash-report session, before the window and graphics start
/// (a panic there must find the session marker). Capture runs (`--shot`,
/// `--window-shot`) get no data folder: no log file, no marker, and a crash
/// report waiting from an earlier session stays for the next real start.
pub fn start(launch: &crate::launch::LaunchArgs, data: Option<&Path>) -> Startup {
    let data = data
        .filter(|_| launch.shot.is_none() && launch.window_shot.is_none())
        .map(Path::to_path_buf);
    let (session, previous_crash) = match data.as_deref() {
        Some(d) => {
            let (s, prev) = crash::begin_session(d);
            (Some(s), prev)
        }
        None => (None, None),
    };
    Startup {
        data,
        session,
        previous_crash,
    }
}

// ------------------------------------------------------------ build facts

/// The git commit this build was made from, when the build knew it.
pub fn commit() -> Option<&'static str> {
    option_env!("NEWTOPO_COMMIT").filter(|c| !c.is_empty())
}

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// "newtopo 0.1.0 (3f2a9c1d0e)".
pub fn version_line() -> String {
    match commit() {
        Some(c) => format!("newtopo {} ({})", version(), short_commit(c)),
        None => format!("newtopo {}", version()),
    }
}

fn short_commit(c: &str) -> &str {
    c.get(..10).unwrap_or(c)
}

// ------------------------------------------------------------ time

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Civil UTC date and time of a Unix timestamp.
fn civil(secs: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (
        y,
        m,
        d,
        (rem / 3600) as u32,
        (rem / 60 % 60) as u32,
        (rem % 60) as u32,
    )
}

/// `2026-11-25 10:04:05Z`.
pub fn utc_stamp(secs: u64) -> String {
    let (y, mo, d, h, mi, s) = civil(secs);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}Z")
}

/// `2026-11-25`.
pub fn utc_date(secs: u64) -> String {
    let (y, mo, d, ..) = civil(secs);
    format!("{y:04}-{mo:02}-{d:02}")
}

/// `newtopo-report-2026-11-25.zip`.
pub fn zip_name(secs: u64) -> String {
    format!("newtopo-report-{}.zip", utc_date(secs))
}

// ------------------------------------------------------------ steps

/// The op's script name (`connect`, `remove_cad_edge`, …).
pub fn op_name(op: &Op) -> String {
    serde_json::to_value(op)
        .ok()
        .and_then(|v| v.get("op").and_then(|n| n.as_str()).map(str::to_string))
        .unwrap_or_else(|| "op".into())
}

/// A readable name for an op script name: "Connect", "Remove CAD edge".
pub fn op_label(name: &str) -> String {
    match name {
        "grid" => return "Surface Grid".into(),
        "weld" => return "Target Weld".into(),
        "set_autofill" => return "Auto-fill".into(),
        "set_link" => return "Link".into(),
        "set_symmetry" => return "Symmetry".into(),
        _ => {}
    }
    let words: Vec<String> = name
        .split('_')
        .filter(|w| !w.is_empty())
        .map(|w| match w {
            "cad" => "CAD".to_string(),
            w => w.to_string(),
        })
        .collect();
    let mut s = words.join(" ");
    if let Some(c) = s.get(..1) {
        let up = c.to_uppercase();
        s.replace_range(..1, &up);
    }
    s
}

/// What happened to one op.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Applied,
    Refused,
    /// The engine failed (a caught panic).
    Failed,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Outcome::Applied => "applied",
            Outcome::Refused => "refused",
            Outcome::Failed => "failed",
        }
    }
}

/// One entry of the session timeline.
#[derive(Clone, Debug)]
enum Event {
    Op(Op, Outcome),
    /// "undo", "redo", "apply_preview", …
    Step(&'static str),
    /// A step about a file: (step, file name, already scrubbed).
    File(&'static str, String),
}

/// This session's steps, for `steps.json`: every op the user ran with its
/// outcome, plus undo/redo, preview apply/discard and imports, in order.
#[derive(Clone, Debug, Default)]
pub struct Session {
    /// Applied and failed ops of the open scene, in order
    /// (script-compatible). File > New and Open start a new list.
    ops: Vec<Op>,
    /// The file the open scene started from (the first CAD import into
    /// it, or the scene file opened), scrubbed, no folders.
    input: Option<String>,
    events: Vec<Event>,
    /// File names of every CAD file imported this session (no folders).
    inputs: Vec<String>,
    /// Full paths of imported CAD files, for "The model".
    pub sources: Vec<PathBuf>,
    truncated: bool,
}

/// `op` with the text the user typed cleaned (home, user name and the
/// folders of anything path-like). Only free-text fields change: field
/// names, op names and enum values stay as they are, so the script still
/// reads back whatever the user name is.
fn scrub_op(op: &Op, s: &scrub::Scrubber) -> Op {
    let mut op = op.clone();
    match &mut op {
        Op::RenameBody { name, .. } | Op::CreateLayer { name } | Op::RenameLayer { name, .. } => {
            *name = s.clean(name)
        }
        _ => {}
    }
    op
}

impl Session {
    /// Ops applied or failed so far.
    pub fn op_count(&self) -> usize {
        self.ops.len()
    }

    fn push_event(&mut self, e: Event) {
        if self.events.len() >= MAX_EVENTS {
            self.truncated = true;
            return;
        }
        self.events.push(e);
    }

    pub fn op(&mut self, op: &Op, outcome: Outcome) {
        let name = op_name(op);
        ::log::info!("op {name}: {}", outcome.as_str());
        if outcome != Outcome::Refused {
            // A full replay list stops growing; the timeline goes on.
            if self.ops.len() >= MAX_OPS {
                self.truncated = true;
            } else {
                self.ops.push(op.clone());
            }
        }
        self.push_event(Event::Op(op.clone(), outcome));
    }

    /// File > New: the replay script starts over (the timeline keeps the
    /// earlier scene).
    pub fn new_scene(&mut self) {
        self.event("new_scene");
        self.ops.clear();
        self.input = None;
    }

    /// A scene file was opened: it is the replay script's input from now on.
    pub fn open(&mut self, path: &Path) {
        self.file_event("open", path);
        self.ops.clear();
        self.input = Some(file_name(path));
    }

    /// A step without an op: "undo", "redo", "apply_preview", …
    pub fn event(&mut self, step: &'static str) {
        ::log::info!("{step}");
        self.push_event(Event::Step(step));
    }

    /// A step about a file (import, open): only its name is recorded.
    pub fn file_event(&mut self, step: &'static str, path: &Path) {
        scrub::know_file(path);
        let name = file_name(path);
        ::log::info!("{step}: {name}");
        self.push_event(Event::File(step, name));
    }

    /// A CAD import finished.
    pub fn import(&mut self, path: &Path) {
        self.file_event("import", path);
        let name = file_name(path);
        if self.input.is_none() {
            self.input = Some(name.clone());
        }
        if !self.inputs.contains(&name) {
            self.inputs.push(name);
        }
        if !self.sources.iter().any(|p| p == path) {
            self.sources.push(path.to_path_buf());
        }
    }

    /// `steps.json`: an op script for the open scene (`newtopo-cli run`
    /// reads `version`, `input` and `ops`) plus the full `session` timeline,
    /// earlier scenes included.
    pub fn steps_json(&self) -> String {
        self.steps_json_with(scrub::Scrubber::current())
    }

    /// [`Session::steps_json`] with a given scrubber. The user's text is
    /// scrubbed field by field before serializing (never the JSON as a
    /// whole, which would also rewrite field and op names).
    fn steps_json_with(&self, s: &scrub::Scrubber) -> String {
        let ops: Vec<Op> = self.ops.iter().map(|o| scrub_op(o, s)).collect();
        let events: Vec<serde_json::Value> = self
            .events
            .iter()
            .map(|e| match e {
                Event::Op(op, outcome) => serde_json::json!({
                    "step": "op",
                    "op": scrub_op(op, s),
                    "outcome": outcome.as_str(),
                }),
                Event::Step(step) => serde_json::json!({ "step": step }),
                Event::File(step, name) => serde_json::json!({ "step": step, "file": name }),
            })
            .collect();
        let v = serde_json::json!({
            "version": 1,
            "input": self.input,
            "ops": ops,
            "session": events,
            "inputs": self.inputs,
            "truncated": self.truncated,
            "note": "`input` and `ops` describe the scene that was open: every \
                     op applied to it (or failed) in order, ready for \
                     `newtopo-cli run steps.json --input <model>`, including ops \
                     that were later undone or discarded with a preview: `run` \
                     replays them all. `session` is the full timeline, including \
                     earlier scenes (before `new_scene` or `open`), refused ops, \
                     undo, redo and discarded previews, and shows which ops \
                     really stayed. `inputs` lists every CAD file imported.",
        });
        serde_json::to_string_pretty(&v).unwrap_or_default()
    }
}

/// The file name of `path` (scrubbed), never its folder.
pub fn file_name(path: &Path) -> String {
    scrub(
        &path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
    )
}

/// `name` (already scrubbed) as a file name Windows, macOS and Linux all
/// accept, for files inside the zip, which the recipient extracts. The
/// scrubber's `<user>` reads `[user]`; the other characters Windows
/// forbids, and control characters, become `_`; trailing dots and spaces
/// go; a Windows device name (`CON`, `nul.step`) gets a leading `_`.
/// Ordinary names stay as they are.
pub fn safe_file_name(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| match c {
            '<' => '[',
            '>' => ']',
            ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    s.truncate(s.trim_end_matches(['.', ' ']).len());
    if s.is_empty() {
        return "file".into();
    }
    let stem = s
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_ascii_uppercase();
    let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit());
    if device {
        s.insert(0, '_');
    }
    s
}

// ------------------------------------------------------------ report files

/// What the user switched on.
#[derive(Clone, Debug, PartialEq)]
pub struct Choices {
    pub steps: bool,
    pub model: bool,
    pub pc: bool,
    pub comment: String,
}

impl Default for Choices {
    fn default() -> Self {
        Choices {
            steps: true,
            model: false,
            pc: true,
            comment: String::new(),
        }
    }
}

/// Where a part's bytes come from.
#[derive(Clone, Debug, PartialEq)]
pub enum Data {
    /// Shared, so listing or collecting the report's files never copies a
    /// (possibly large) scene archive.
    Bytes(std::sync::Arc<[u8]>),
    /// A file on disk, read when the zip is written.
    File(PathBuf),
}

/// One file in the report.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    /// Name inside the zip.
    pub name: String,
    pub size: u64,
    pub data: Data,
}

impl Part {
    fn text(name: &str, s: String) -> Part {
        Part {
            name: name.into(),
            size: s.len() as u64,
            data: Data::Bytes(s.into_bytes().into()),
        }
    }
}

/// A row of the "What the report contains" list.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub name: String,
    pub size: Option<u64>,
    pub included: bool,
}

/// A model file that could go into the report.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelFile {
    /// Name inside the zip (`model/…`).
    pub name: String,
    pub data: Data,
    pub size: u64,
}

/// Everything a report can contain, already scrubbed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Contents {
    pub failure: Option<Failure>,
    /// Cleaned once when collected (see [`clean`]): the dialog lists the
    /// report's files every frame, and cleaning 256 KiB of log is not free.
    pub log: String,
    pub steps_json: String,
    pub op_count: usize,
    pub system: String,
    /// One line for the dialog ("Windows 11 · RTX 4070 · 32 GB").
    pub system_summary: String,
    pub model: Vec<ModelFile>,
}

/// `error.txt`.
pub fn error_text(f: Option<&Failure>) -> String {
    let mut s = String::new();
    match f {
        Some(f) => {
            s.push_str(&format!("Error: {}\n", f.message));
            if let Some(c) = &f.context {
                s.push_str(&format!("While: {c}\n"));
            }
            s.push_str(&format!(
                "Location: {}\n",
                f.location.as_deref().unwrap_or("unknown")
            ));
            s.push_str(&format!("Thread: {}\n", f.thread));
            s.push_str(&format!("Time: {}\n", utc_stamp(f.time)));
            s.push_str(&format!("Version: newtopo {}\n", f.version));
            s.push_str(&format!(
                "Commit: {}\n",
                f.commit.as_deref().unwrap_or("unknown")
            ));
            s.push_str("\nBacktrace:\n");
            if f.backtrace.trim().is_empty() {
                s.push_str("(not captured)\n");
            } else {
                s.push_str(f.backtrace.trim_end());
                s.push('\n');
            }
        }
        None => {
            s.push_str("No error attached: the report was opened from Help.\n");
            s.push_str(&format!("Version: newtopo {}\n", version()));
            s.push_str(&format!("Commit: {}\n", commit().unwrap_or("unknown")));
            s.push_str(&format!("Time: {}\n", utc_stamp(now_secs())));
        }
    }
    clean(&s)
}

/// The report's files for these choices, in zip order.
pub fn parts(c: &Contents, ch: &Choices) -> Vec<Part> {
    let mut out = vec![
        Part::text("error.txt", error_text(c.failure.as_ref())),
        Part::text("log.txt", c.log.clone()),
    ];
    if ch.steps {
        out.push(Part::text("steps.json", c.steps_json.clone()));
    }
    if ch.pc {
        out.push(Part::text("system.txt", c.system.clone()));
    }
    let comment = ch.comment.trim();
    if !comment.is_empty() {
        out.push(Part::text("comment.txt", format!("{}\n", clean(comment))));
    }
    if ch.model {
        for m in &c.model {
            out.push(Part {
                name: m.name.clone(),
                size: m.size,
                data: m.data.clone(),
            });
        }
    }
    out
}

/// The dialog's list: included files with sizes, then the optional ones
/// that are switched off.
pub fn rows(c: &Contents, ch: &Choices) -> Vec<Row> {
    let mut rows: Vec<Row> = parts(c, ch)
        .into_iter()
        .map(|p| Row {
            name: p.name,
            size: Some(p.size),
            included: true,
        })
        .collect();
    let off = |name: &str| Row {
        name: name.into(),
        size: None,
        included: false,
    };
    if !ch.steps {
        rows.push(off("steps.json"));
    }
    if !ch.pc {
        rows.push(off("system.txt"));
    }
    if !ch.model {
        rows.extend(c.model.iter().map(|m| off(&m.name)));
    }
    rows
}

/// Write the report zip. Files that can no longer be read are noted in
/// `missing.txt` instead, and their names returned (the rest is saved
/// anyway, so the user can still send it). The zip is written to a fresh temporary file
/// created exclusively next to `path` (a planted file or link there is
/// never opened), then renamed over `path`.
pub fn write_zip(path: &Path, parts: &[Part]) -> Result<Vec<String>, String> {
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    let tmp = tempfile::Builder::new()
        .prefix(".newtopo-report-")
        .suffix(".zip.part")
        .tempfile_in(dir)
        .map_err(|e| e.to_string())?;
    let file = tmp.reopen().map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut missing = Vec::new();
    let mut not_saved = Vec::new();
    for p in parts {
        let read;
        let bytes: &[u8] = match &p.data {
            Data::Bytes(b) => b,
            Data::File(f) => match std::fs::read(f) {
                Ok(b) => {
                    read = b;
                    &read
                }
                Err(e) => {
                    missing.push(format!("{}: {e}", p.name));
                    not_saved.push(p.name.clone());
                    continue;
                }
            },
        };
        zip.start_file(p.name.as_str(), opts)
            .map_err(|e| e.to_string())?;
        zip.write_all(bytes).map_err(|e| e.to_string())?;
    }
    if !missing.is_empty() {
        zip.start_file("missing.txt", opts)
            .map_err(|e| e.to_string())?;
        let text = scrub(&format!(
            "These files could not be read when the report was saved:\n{}\n",
            missing.join("\n")
        ));
        zip.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    }
    zip.finish().map_err(|e| e.to_string())?;
    // Dropping `tmp` on an error above removes the temporary file.
    tmp.persist(path).map_err(|e| e.error.to_string())?;
    Ok(not_saved)
}

/// A fresh temporary folder only this user can read (0700 on Unix), for
/// copies of the user's files that must not sit in the shared temp folder.
/// Removed when dropped.
pub fn private_temp_dir() -> std::io::Result<tempfile::TempDir> {
    let mut b = tempfile::Builder::new();
    b.prefix("newtopo-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        b.permissions(std::fs::Permissions::from_mode(0o700));
    }
    b.tempdir()
}

/// "1.2 MB", "31 KB", "512 B".
pub fn human_size(bytes: u64) -> String {
    const K: f64 = 1024.0;
    let b = bytes as f64;
    if b < K {
        format!("{bytes} B")
    } else if b < K * K {
        format!("{} KB", (b / K).ceil() as u64)
    } else if b < K * K * K {
        format!("{:.1} MB", b / (K * K))
    } else {
        format!("{:.1} GB", b / (K * K * K))
    }
}

/// First line of the failure, short enough for a title.
pub fn error_line(f: &Failure) -> String {
    clean(f.message.lines().next().unwrap_or_default().trim())
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

/// The first `n` lines of `s` (trimmed), with a note when cut.
fn head_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.trim_end().lines().collect();
    if lines.len() <= n {
        return lines.join("\n");
    }
    format!(
        "{}\n… ({} more lines in error.txt)",
        lines[..n].join("\n"),
        lines.len() - n
    )
}

// ------------------------------------------------------------ GitHub issue

/// Percent-encode for a URL query value.
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The issue's title.
pub fn issue_title(c: &Contents, ch: &Choices) -> String {
    let t = match &c.failure {
        Some(f) => format!(
            "{} failed: {}",
            f.context.as_deref().unwrap_or("newtopo"),
            error_line(f)
        ),
        None => match ch.comment.lines().map(str::trim).find(|l| !l.is_empty()) {
            Some(l) => format!("Problem report: {l}"),
            None => "Problem report".into(),
        },
    };
    truncate_chars(&clean(&t), 90)
}

fn issue_body(
    c: &Contents,
    ch: &Choices,
    zip: &str,
    comment_max: usize,
    error_max: usize,
) -> String {
    let mut s = String::new();
    s.push_str("**What happened**\n");
    match &c.failure {
        Some(f) => {
            s.push_str(&format!(
                "`{}`",
                truncate_chars(&error_line(f), error_max).replace('`', "'")
            ));
            if let Some(l) = &f.location {
                s.push_str(&format!(" at `{l}`"));
            }
            if let Some(ctx) = &f.context {
                s.push_str(&format!(" during {ctx}"));
            }
            s.push('\n');
        }
        None => s.push_str("(no error attached)\n"),
    }
    s.push_str(&format!("\n**Version:** {}\n", version_line()));
    if ch.pc {
        s.push_str(&format!("**Platform:** {}\n", c.system_summary));
    } else {
        s.push_str("**Platform:** not shared\n");
    }
    let comment = ch.comment.trim();
    if !comment.is_empty() {
        s.push_str("\n**What I was doing**\n");
        s.push_str(&truncate_chars(&clean(comment), comment_max));
        s.push('\n');
    }
    let all: Vec<String> = parts(c, ch).into_iter().map(|p| p.name).collect();
    let mut names: Vec<String> = all.iter().take(8).map(|n| truncate_chars(n, 60)).collect();
    if all.len() > 8 {
        names.push(format!("{} more", all.len() - 8));
    }
    s.push_str(&format!(
        "\n**Report file**\nPlease attach `{zip}` (drag it into this box). \
         The app saved it on this computer and does not upload anything. \
         It contains: {}.\n",
        names.join(", ")
    ));
    // Everything above that came from the session is scrubbed already; the
    // fixed text (the repository name) is left alone.
    s
}

/// The "new issue" link with title and body filled in, at most
/// [`MAX_ISSUE_URL`] long (the comment, then the error line, are shortened
/// to fit; the log is never in it).
pub fn issue_url(c: &Contents, ch: &Choices, zip: &str) -> String {
    let title = issue_title(c, ch);
    let mut comment_max = 2000;
    let mut error_max = 300;
    loop {
        let body = issue_body(c, ch, zip, comment_max, error_max);
        let url = format!(
            "{ISSUE_URL}?title={}&body={}",
            encode(&title),
            encode(&body)
        );
        if url.len() <= MAX_ISSUE_URL || (comment_max == 0 && error_max <= 40) {
            return url;
        }
        if comment_max > 0 {
            comment_max = if comment_max > 50 { comment_max / 2 } else { 0 };
        } else {
            error_max = (error_max / 2).max(40);
        }
    }
}

// ------------------------------------------------------------ agent prompt

/// Instructions for an AI coding agent to fix the problem from the report.
/// Contains only what the user switched on, all scrubbed.
pub fn agent_prompt(c: &Contents, ch: &Choices, zip: &str) -> String {
    let names: Vec<String> = parts(c, ch).into_iter().map(|p| p.name).collect();
    // The build that failed: an attached failure may come from an older
    // build than this one (a crash in the last session, before an update).
    let (sha, v) = match &c.failure {
        Some(f) => (f.commit.clone(), f.version.clone()),
        None => (commit().map(str::to_string), version().to_string()),
    };
    let checkout = match sha {
        Some(sha) => format!("commit `{sha}` (newtopo {v})"),
        None => format!(
            "the tag `v{v}` (newtopo {v}; the build did not record its commit, so use `main` if the tag does not exist)"
        ),
    };
    let mut s = String::new();
    s.push_str(&format!(
        "You are fixing a bug in newtopo, an open-source (MIT) Rust desktop app that turns CAD \
         models (STEP/IGES) into quad meshes: https://github.com/{REPO}\n\n"
    ));
    s.push_str(&format!(
        "A user saved a problem report as `{zip}`. It contains: {}. \
         Ask the user for that file if you do not have it yet.\n\n",
        names.join(", ")
    ));
    s.push_str("Do this:\n");
    s.push_str(&format!(
        "1. Fork `{REPO}` on GitHub and clone your fork.\n"
    ));
    s.push_str(&format!(
        "2. Check out {checkout} to reproduce on the same code the user ran.\n"
    ));
    s.push_str(
        "3. Build it (see README.md / CONTRIBUTING.md for the OCCT setup) and reproduce the \
         problem from the report",
    );
    if ch.steps {
        s.push_str(
            ": `steps.json` is an op script; `newtopo-cli run steps.json --input <model>` \
             replays every op in its `ops` list in order, including ops the user later undid \
             and changes they discarded from a preview, so the replayed result can differ \
             from what the user saw; its `session` list is the full timeline (refused ops, \
             undo, redo, preview apply and discard) and shows which ops really stayed",
        );
    }
    if ch.model && !c.model.is_empty() {
        s.push_str("; the model files are in the zip's `model/` folder");
    }
    s.push_str(".\n");
    s.push_str(
        "4. Find the root cause and fix it on a new branch from `main` (not only the symptom).\n",
    );
    s.push_str("5. Add a regression test that fails without the fix.\n");
    s.push_str(
        "6. Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` \
         and `cargo test --workspace`; all must pass.\n",
    );
    s.push_str(&format!(
        "7. Open a pull request against `main` of `{REPO}` that explains the cause and the fix \
         and mentions this report.\n"
    ));
    s.push_str("\n## Error\n```\n");
    s.push_str(&head_lines(
        &error_text(c.failure.as_ref()),
        PROMPT_ERROR_LINES,
    ));
    s.push_str("\n```\n");
    if ch.steps {
        if c.steps_json.len() <= PROMPT_STEPS_MAX {
            s.push_str("\n## Steps (steps.json)\n```json\n");
            s.push_str(c.steps_json.trim_end());
            s.push_str("\n```\n");
        } else {
            s.push_str(&format!(
                "\n## Steps\n`steps.json` in the zip ({}, {} ops) is too large to paste here.\n",
                human_size(c.steps_json.len() as u64),
                c.op_count
            ));
        }
    }
    if ch.pc {
        s.push_str("\n## PC details (system.txt)\n```\n");
        s.push_str(c.system.trim_end());
        s.push_str("\n```\n");
    }
    let comment = ch.comment.trim();
    if !comment.is_empty() {
        s.push_str("\n## What the user was doing\n");
        s.push_str(&clean(comment));
        s.push('\n');
    }
    // Contents are scrubbed when collected; the fixed text (the repository
    // name) is left alone.
    s
}

#[cfg(test)]
mod tests;
