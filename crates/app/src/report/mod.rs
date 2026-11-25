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

/// Text that goes into a report: home and user name removed (see
/// [`scrub::Scrubber`]) and folders stripped from file paths.
pub fn clean(text: &str) -> String {
    scrub::redact_dirs(&scrub(text))
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

/// This session's steps, for `steps.json`: every op the user ran with its
/// outcome, plus undo/redo, preview apply/discard and imports, in order.
#[derive(Clone, Debug, Default)]
pub struct Session {
    /// Applied and failed ops, in order (script-compatible).
    ops: Vec<Op>,
    events: Vec<serde_json::Value>,
    /// File names of imported CAD files (no folders).
    inputs: Vec<String>,
    /// Full paths of imported CAD files, for "The model".
    pub sources: Vec<PathBuf>,
    truncated: bool,
}

impl Session {
    /// Ops applied or failed so far.
    pub fn op_count(&self) -> usize {
        self.ops.len()
    }

    fn push_event(&mut self, v: serde_json::Value) {
        if self.events.len() >= MAX_EVENTS {
            self.truncated = true;
            return;
        }
        self.events.push(v);
    }

    pub fn op(&mut self, op: &Op, outcome: Outcome) {
        let name = op_name(op);
        ::log::info!("op {name}: {}", outcome.as_str());
        if outcome != Outcome::Refused {
            if self.ops.len() >= MAX_OPS {
                self.truncated = true;
                return;
            }
            self.ops.push(op.clone());
        }
        let op_json = serde_json::to_value(op).unwrap_or(serde_json::Value::Null);
        self.push_event(serde_json::json!({
            "step": "op",
            "op": op_json,
            "outcome": outcome.as_str(),
        }));
    }

    /// A step without an op: "undo", "redo", "apply_preview", …
    pub fn event(&mut self, step: &str) {
        ::log::info!("{step}");
        self.push_event(serde_json::json!({ "step": step }));
    }

    /// A step about a file (import, open): only its name is recorded.
    pub fn file_event(&mut self, step: &str, path: &Path) {
        let name = file_name(path);
        ::log::info!("{step}: {name}");
        self.push_event(serde_json::json!({ "step": step, "file": name }));
    }

    /// A CAD import finished.
    pub fn import(&mut self, path: &Path) {
        self.file_event("import", path);
        let name = file_name(path);
        if !self.inputs.contains(&name) {
            self.inputs.push(name);
        }
        if !self.sources.iter().any(|p| p == path) {
            self.sources.push(path.to_path_buf());
        }
    }

    /// `steps.json`: an op script (`newtopo-cli run` reads `version`,
    /// `input` and `ops`) plus the full `session` timeline.
    pub fn steps_json(&self) -> String {
        let v = serde_json::json!({
            "version": 1,
            "input": self.inputs.first(),
            "ops": self.ops,
            "session": self.events,
            "inputs": self.inputs,
            "truncated": self.truncated,
            "note": "`ops` lists every op that was applied (or failed) in order, \
                     ready for `newtopo-cli run steps.json --input <model>`. \
                     `session` is the full timeline, including refused ops, undo, \
                     redo and discarded previews, which `run` does not replay.",
        });
        scrub(&serde_json::to_string_pretty(&v).unwrap_or_default())
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
    Bytes(Vec<u8>),
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
            data: Data::Bytes(s.into_bytes()),
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
        Part::text("log.txt", clean(&c.log)),
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
/// `missing.txt` instead.
pub fn write_zip(path: &Path, parts: &[Part]) -> Result<(), String> {
    let tmp = path.with_extension("zip.part");
    let r = (|| -> Result<(), String> {
        let file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let mut missing = Vec::new();
        for p in parts {
            let bytes = match &p.data {
                Data::Bytes(b) => b.clone(),
                Data::File(f) => match std::fs::read(f) {
                    Ok(b) => b,
                    Err(e) => {
                        missing.push(format!("{}: {e}", p.name));
                        continue;
                    }
                },
            };
            zip.start_file(p.name.as_str(), opts)
                .map_err(|e| e.to_string())?;
            zip.write_all(&bytes).map_err(|e| e.to_string())?;
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
        Ok(())
    })();
    match r {
        Ok(()) => std::fs::rename(&tmp, path).map_err(|e| e.to_string()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
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
    let checkout = match c
        .failure
        .as_ref()
        .and_then(|f| f.commit.clone())
        .or(commit().map(str::to_string))
    {
        Some(sha) => format!("commit `{sha}` (newtopo {})", version()),
        None => format!(
            "the tag `v{v}` (newtopo {v}; the build did not record its commit, so use `main` if the tag does not exist)",
            v = version()
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
             replays its `ops`, and its `session` list has the full timeline",
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
