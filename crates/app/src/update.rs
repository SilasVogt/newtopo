//! Update check (SPEC §9.4): asks GitHub for the latest published release of
//! `SilasVogt/newtopo` and, when it is newer than this build, offers a link
//! to that release's page. It never downloads, installs or runs anything.
//!
//! Hardening, because the answer comes from the network:
//! * one fixed URL, HTTPS only, no redirects followed, a global timeout and
//!   a response size cap;
//! * only `tag_name`, `draft` and `prerelease` are read from the reply; the
//!   tag must be a plain `vMAJOR.MINOR.PATCH`;
//! * the link shown to the user is built from the parsed version numbers
//!   and the fixed repository, never taken from the reply, so a tampered
//!   response can't point anywhere else;
//! * no release text (name, notes) is displayed;
//! * the request carries only a `User-Agent` with the app version, as the
//!   GitHub API requires; there is no account, token or telemetry.
//!
//! Automatic checks are opt-in, run at most once a day, and are disabled by
//! the `NEWTOPO_NO_UPDATE_CHECK` environment variable (for packagers and
//! sandboxes). "Help > Check for updates" always works on demand.

use std::fmt;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Duration;

/// The only repository whose releases are considered.
macro_rules! repo {
    () => {
        "SilasVogt/newtopo"
    };
}
const API_URL: &str = concat!("https://api.github.com/repos/", repo!(), "/releases/latest");
const RELEASE_PAGE: &str = concat!("https://github.com/", repo!(), "/releases/tag/");
/// A release object is a few kB; anything far bigger is not what we asked for.
const MAX_BODY: u64 = 256 * 1024;
const TIMEOUT: Duration = Duration::from_secs(10);
/// Automatic checks run at most this often.
pub const INTERVAL_SECS: u64 = 24 * 60 * 60;
/// Set to any value to turn automatic checks off entirely.
pub const DISABLE_ENV: &str = "NEWTOPO_NO_UPDATE_CHECK";

/// A plain `MAJOR.MINOR.PATCH` version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u32, pub u32, pub u32);

impl Version {
    /// Parses `1.2.3` or `v1.2.3`. Anything else (pre-release suffixes,
    /// build metadata, extra parts, signs, whitespace) is rejected.
    pub fn parse(s: &str) -> Option<Version> {
        let s = s.strip_prefix('v').unwrap_or(s);
        let mut parts = s.split('.');
        let mut next = || -> Option<u32> {
            let p = parts.next()?;
            if p.is_empty() || p.len() > 9 || !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            p.parse().ok()
        };
        let v = Version(next()?, next()?, next()?);
        if parts.next().is_some() {
            return None;
        }
        Some(v)
    }

    /// This build's version.
    pub fn current() -> Version {
        Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Version(0, 0, 0))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// A newer release and the page to get it from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Available {
    pub version: Version,
    pub url: String,
}

impl Available {
    pub fn for_version(version: Version) -> Available {
        Available {
            version,
            url: format!("{RELEASE_PAGE}v{version}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The latest release is not newer than this build.
    UpToDate(Version),
    Newer(Available),
}

/// Decides what a `releases/latest` reply means for `current`.
pub fn evaluate(json: &str, current: Version) -> Result<Outcome, String> {
    #[derive(serde::Deserialize)]
    struct Release {
        tag_name: String,
        #[serde(default)]
        draft: bool,
        #[serde(default)]
        prerelease: bool,
    }
    let r: Release =
        serde_json::from_str(json).map_err(|_| "GitHub sent an unexpected reply".to_string())?;
    if r.draft || r.prerelease {
        return Err("GitHub's latest release isn't a published stable release".into());
    }
    let v = Version::parse(&r.tag_name)
        .ok_or_else(|| "the latest release has an unexpected tag".to_string())?;
    Ok(if v > current {
        Outcome::Newer(Available::for_version(v))
    } else {
        Outcome::UpToDate(v)
    })
}

/// True when `url` is a release page of [`REPO`] (the only kind of link the
/// update UI may open).
pub fn is_release_page(url: &str) -> bool {
    url.strip_prefix(RELEASE_PAGE)
        .is_some_and(|tag| Version::parse(tag).is_some() && tag.starts_with('v'))
}

fn fetch() -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .https_only(true)
        .max_redirects(0)
        .timeout_global(Some(TIMEOUT))
        .http_status_as_error(true)
        .user_agent(format!(
            "newtopo/{} (update check)",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .into();
    let mut resp = agent
        .get(API_URL)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .map_err(|e| match e {
            ureq::Error::StatusCode(404) => "no release has been published yet".to_string(),
            ureq::Error::StatusCode(403 | 429) => {
                "GitHub is rate-limiting requests; try later".into()
            }
            ureq::Error::StatusCode(code) => format!("GitHub answered with HTTP {code}"),
            ureq::Error::Timeout(_) => "the request timed out".into(),
            other => format!("couldn't reach GitHub ({other})"),
        })?;
    resp.body_mut()
        .with_config()
        .limit(MAX_BODY)
        .read_to_string()
        .map_err(|_| "GitHub's reply couldn't be read".to_string())
}

/// A check running on a background thread.
pub struct Check {
    rx: Receiver<Result<Outcome, String>>,
    /// Started from the menu (report every result) rather than at startup.
    pub manual: bool,
}

impl Check {
    pub fn spawn(manual: bool) -> Check {
        let (tx, rx) = channel();
        let spawned = std::thread::Builder::new()
            .name("update-check".into())
            .spawn(move || {
                let r = fetch().and_then(|json| evaluate(&json, Version::current()));
                let _ = tx.send(r);
            });
        if spawned.is_err() {
            let (tx, rx2) = channel();
            let _ = tx.send(Err("couldn't start the check".to_string()));
            return Check { rx: rx2, manual };
        }
        Check { rx, manual }
    }

    /// A check that has already finished (tests).
    #[cfg(test)]
    pub fn finished(result: Result<Outcome, String>, manual: bool) -> Check {
        let (tx, rx) = channel();
        let _ = tx.send(result);
        Check { rx, manual }
    }

    /// The result once the check has finished.
    pub fn poll(&self) -> Option<Result<Outcome, String>> {
        match self.rx.try_recv() {
            Ok(r) => Some(r),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("the check stopped unexpectedly".into())),
        }
    }
}

/// Update-check UI state.
#[derive(Default)]
pub struct UpdateUi {
    pub check: Option<Check>,
    pub available: Option<Available>,
    /// The "a new version is available" dialog.
    pub dialog_open: bool,
    /// The one-time "check automatically?" question.
    pub ask_open: bool,
}

/// Seconds since the Unix epoch (0 if the clock is before it).
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Whether an automatic check is due.
pub fn due(last_check: u64, now: u64) -> bool {
    // A clock that went backwards counts as due rather than never.
    now < last_check || now - last_check >= INTERVAL_SECS
}

// ------------------------------------------------------------------ app side

use crate::app::AppState;
use crate::kit;
use crate::strings as S;
use crate::toasts::ToastKind;
use egui::RichText;

impl AppState {
    /// Settings are remembered and the user hasn't turned checks off by env.
    fn updates_allowed(&self) -> bool {
        !self.screenshot_mode
            && self.paths.config.is_some()
            && std::env::var_os(DISABLE_ENV).is_none()
    }

    /// At launch: ask once, or run the daily check if it is on and due.
    pub fn startup_update_check(&mut self) {
        if !self.updates_allowed() {
            return;
        }
        match self.prefs.update_check {
            None => self.updates.ask_open = true,
            Some(true) if due(self.prefs.update_last_check, now_secs()) => {
                self.start_update_check(false)
            }
            Some(_) => {}
        }
    }

    /// Starts a check unless one is already running. `manual` reports
    /// every outcome; automatic checks stay quiet unless there's news.
    pub fn start_update_check(&mut self, manual: bool) {
        if let Some(c) = &mut self.updates.check {
            c.manual |= manual;
            return;
        }
        if manual {
            self.toast(ToastKind::Info, S::UPDATE_CHECKING);
        } else {
            self.prefs.update_last_check = now_secs();
            self.dirty_settings = true;
        }
        self.updates.check = Some(Check::spawn(manual));
    }

    pub fn poll_update(&mut self, ctx: &egui::Context) {
        let Some(check) = &self.updates.check else {
            return;
        };
        let Some(result) = check.poll() else {
            ctx.request_repaint_after(Duration::from_millis(250));
            return;
        };
        let manual = check.manual;
        self.updates.check = None;
        match result {
            Ok(Outcome::Newer(a)) => {
                let skipped = self.prefs.update_skipped.as_deref() == Some(&a.version.to_string());
                if manual || !skipped {
                    if manual {
                        self.updates.dialog_open = true;
                    } else {
                        self.toast(
                            ToastKind::Info,
                            format!("newtopo {} is available", a.version),
                        );
                    }
                    self.updates.available = Some(a);
                }
            }
            Ok(Outcome::UpToDate(_)) => {
                self.updates.available = None;
                if manual {
                    self.toast(
                        ToastKind::Info,
                        format!("{} ({})", S::UPDATE_LATEST, Version::current()),
                    );
                }
            }
            Err(e) => {
                if manual {
                    self.toast(ToastKind::Warn, format!("{}: {e}", S::UPDATE_FAILED));
                } else {
                    log::info!("update check: {e}");
                }
            }
        }
    }

    pub fn answer_update_question(&mut self, yes: bool) {
        self.updates.ask_open = false;
        self.prefs.update_check = Some(yes);
        self.dirty_settings = true;
        if yes {
            self.start_update_check(false);
        }
    }

    /// Any dialog or overlay that Esc would close before the update dialogs.
    fn other_modal_open(&self) -> bool {
        self.ui.palette.open || self.ui.radial.open || self.active_modal().is_some()
    }

    /// Top-bar chip while a newer version is known.
    pub fn update_chip(&mut self, ui: &mut egui::Ui) {
        let Some(a) = &self.updates.available else {
            return;
        };
        let pal = self.pal;
        let text = format!("{} {}", S::UPDATE_CHIP, a.version);
        if ui
            .add(
                egui::Button::new(RichText::new(text).small().color(pal.accent_text))
                    .fill(pal.accent)
                    .corner_radius(8),
            )
            .on_hover_text(S::UPDATE_CHIP_HINT)
            .clicked()
        {
            self.updates.dialog_open = true;
        }
        ui.add_space(8.0);
    }

    pub fn update_dialogs(&mut self, ctx: &egui::Context) {
        let pal = self.pal;
        // Update dialogs never stack on another modal or overlay: they wait
        // until it closes, so the dialog Esc closes is always the visible one
        // (see `AppState::escape`).
        if self.other_modal_open() {
            return;
        }
        if self.updates.ask_open {
            let mut answer = None;
            kit::modal("update-ask", &pal).show(ctx, |ui| {
                ui.set_width(440.0);
                if kit::dialog_header(ui, &pal, S::UPDATE_ASK_TITLE, None) {
                    answer = Some(false);
                }
                ui.label(S::UPDATE_ASK_BODY);
                ui.add_space(6.0);
                ui.label(
                    RichText::new(S::UPDATE_ASK_PRIVACY)
                        .small()
                        .color(pal.text_dim),
                );
                match kit::dialog_footer(ui, &pal, S::UPDATE_ASK_YES, true, &[S::UPDATE_ASK_NO]) {
                    Some(0) => answer = Some(true),
                    Some(_) => answer = Some(false),
                    None => {}
                }
            });
            if let Some(yes) = answer {
                self.answer_update_question(yes);
            }
        }

        if !self.updates.dialog_open {
            return;
        }
        let Some(a) = self.updates.available.clone() else {
            self.updates.dialog_open = false;
            return;
        };
        let mut choice = None;
        kit::modal("update", &pal).show(ctx, |ui| {
            ui.set_width(440.0);
            let title = format!("newtopo {} {}", a.version, S::UPDATE_TITLE_SUFFIX);
            if kit::dialog_header(ui, &pal, &title, None) {
                choice = Some(2);
            }
            ui.label(format!(
                "{} {}. {}",
                S::UPDATE_BODY_HAVE,
                Version::current(),
                S::UPDATE_BODY
            ));
            ui.add_space(6.0);
            ui.label(
                RichText::new(&a.url)
                    .small()
                    .monospace()
                    .color(pal.text_dim),
            );
            match kit::dialog_footer(
                ui,
                &pal,
                S::UPDATE_OPEN,
                true,
                &[S::UPDATE_LATER, S::UPDATE_SKIP],
            ) {
                Some(0) => choice = Some(0),
                Some(1) => choice = Some(2),
                Some(_) => choice = Some(1),
                None => {}
            }
        });
        match choice {
            Some(0) => {
                self.updates.dialog_open = false;
                // Only ever open a release page of this repository.
                if is_release_page(&a.url)
                    && let Err(e) = open::that_detached(&a.url)
                {
                    self.toast(ToastKind::Warn, format!("{}: {e}", S::UPDATE_OPEN_FAILED));
                }
            }
            Some(1) => {
                self.updates.dialog_open = false;
                self.prefs.update_skipped = Some(a.version.to_string());
                self.updates.available = None;
                self.dirty_settings = true;
            }
            Some(_) => self.updates.dialog_open = false,
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_versions_only() {
        assert_eq!(Version::parse("v0.2.0"), Some(Version(0, 2, 0)));
        assert_eq!(Version::parse("1.10.3"), Some(Version(1, 10, 3)));
        for bad in [
            "",
            "v",
            "v1",
            "v1.2",
            "v1.2.3.4",
            "v1.2.3-rc1",
            "v1.2.3+build",
            "V1.2.3",
            "vv1.2.3",
            " v1.2.3",
            "v1.2.3 ",
            "v1..3",
            "v+1.2.3",
            "v1.2.-3",
            "v1.2.3/../../evil",
            "v99999999999.0.0",
        ] {
            assert_eq!(Version::parse(bad), None, "{bad:?} must be rejected");
        }
        assert!(Version(0, 10, 0) > Version(0, 9, 9));
        assert!(Version(1, 0, 0) > Version(0, 99, 99));
    }

    #[test]
    fn current_version_parses() {
        assert_eq!(
            Version::current().to_string(),
            env!("CARGO_PKG_VERSION"),
            "the crate version must be plain MAJOR.MINOR.PATCH"
        );
    }

    #[test]
    fn newer_release_links_to_its_own_page() {
        let json = r#"{"tag_name":"v0.3.1","draft":false,"prerelease":false,
            "html_url":"https://evil.example/download","name":"<b>x</b>"}"#;
        let r = evaluate(json, Version(0, 1, 0)).unwrap();
        let Outcome::Newer(a) = r else {
            panic!("expected newer, got {r:?}")
        };
        assert_eq!(a.version, Version(0, 3, 1));
        // The reply's html_url is ignored; the link is built locally.
        assert_eq!(
            a.url,
            "https://github.com/SilasVogt/newtopo/releases/tag/v0.3.1"
        );
        assert!(is_release_page(&a.url));
    }

    #[test]
    fn same_or_older_is_up_to_date() {
        let same = r#"{"tag_name":"v0.1.0","draft":false,"prerelease":false}"#;
        assert_eq!(
            evaluate(same, Version(0, 1, 0)).unwrap(),
            Outcome::UpToDate(Version(0, 1, 0))
        );
        let older = r#"{"tag_name":"v0.0.9"}"#;
        assert_eq!(
            evaluate(older, Version(0, 1, 0)).unwrap(),
            Outcome::UpToDate(Version(0, 0, 9))
        );
    }

    #[test]
    fn rejects_drafts_prereleases_and_odd_replies() {
        for json in [
            r#"{"tag_name":"v9.0.0","draft":true}"#,
            r#"{"tag_name":"v9.0.0","prerelease":true}"#,
            r#"{"tag_name":"v9.0.0-beta"}"#,
            r#"{"tag_name":"nightly"}"#,
            r#"{"tag_name":12}"#,
            r#"{"message":"Not Found"}"#,
            r#"[]"#,
            "not json",
        ] {
            assert!(
                evaluate(json, Version(0, 1, 0)).is_err(),
                "{json} must be an error"
            );
        }
    }

    #[test]
    fn only_this_repos_release_pages_are_openable() {
        assert!(is_release_page(
            "https://github.com/SilasVogt/newtopo/releases/tag/v1.2.3"
        ));
        for bad in [
            "https://github.com/SilasVogt/newtopo/releases/tag/1.2.3",
            "https://github.com/SilasVogt/newtopo/releases/tag/v1.2.3/x",
            "https://github.com/SilasVogt/newtopo/releases/tag/v1.2.3?x=1",
            "https://github.com/Other/newtopo/releases/tag/v1.2.3",
            "http://github.com/SilasVogt/newtopo/releases/tag/v1.2.3",
            "https://github.com.evil/SilasVogt/newtopo/releases/tag/v1.2.3",
            "file:///etc/passwd",
        ] {
            assert!(!is_release_page(bad), "{bad} must not be openable");
        }
    }

    #[test]
    fn fixed_endpoints() {
        assert_eq!(
            API_URL,
            "https://api.github.com/repos/SilasVogt/newtopo/releases/latest"
        );
        assert_eq!(
            RELEASE_PAGE,
            "https://github.com/SilasVogt/newtopo/releases/tag/"
        );
    }

    #[test]
    fn daily_interval() {
        assert!(due(0, INTERVAL_SECS));
        assert!(!due(1000, 1000 + INTERVAL_SECS - 1));
        assert!(due(1000, 1000 + INTERVAL_SECS));
        assert!(due(5000, 4000), "clock went backwards");
    }

    /// Talks to the real GitHub API. Opt-in: `NEWTOPO_NET_TESTS=1`.
    #[test]
    #[ignore]
    fn live_check() {
        if std::env::var_os("NEWTOPO_NET_TESTS").is_none() {
            return;
        }
        match fetch().and_then(|j| evaluate(&j, Version::current())) {
            Ok(o) => eprintln!("live check: {o:?}"),
            Err(e) => eprintln!("live check error (acceptable before a release exists): {e}"),
        }
    }
}
