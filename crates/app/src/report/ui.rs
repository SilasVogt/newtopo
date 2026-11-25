//! The "Report a problem" dialog (SPEC §5.11): what went wrong, toggles for
//! what to include, a live list of the report's files, Save / Open GitHub
//! issue, and a prompt for an AI coding agent.

use super::crash::{self, Failure};
use super::{Choices, Contents, Data, ModelFile, Row};
use crate::app::AppState;
use crate::strings as S;
use crate::theme::{self, Palette};
use crate::toasts::ToastKind;
use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{
    Align, Align2, Color32, CornerRadius, FontId, Layout, Margin, Rect, RichText, Sense, Shape,
    Stroke, Ui, pos2, vec2,
};
use std::path::{Path, PathBuf};

/// Why the dialog is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// An engine panic the app caught.
    Caught,
    /// The previous session ended in a crash.
    PreviousCrash,
    /// Help > Report a problem (no error attached).
    Manual,
}

/// Whether the open scene has been copied for "The model".
#[derive(Clone, Debug, PartialEq)]
enum SceneCopy {
    NotYet,
    Done,
    Failed,
}

pub struct ReportDialog {
    pub origin: Origin,
    pub choices: Choices,
    pub contents: Contents,
    /// Where the zip was last saved, and with which choices.
    pub saved: Option<(PathBuf, Choices)>,
    /// Save here instead of asking (tests and screenshots).
    pub save_to: Option<PathBuf>,
    pub zip_name: String,
    /// The open scene's title, for `model/<title>.ntopo`.
    scene_title: Option<String>,
    scene: SceneCopy,
    copied_at: Option<f64>,
}

impl ReportDialog {
    pub fn failure(&self) -> Option<&Failure> {
        self.contents.failure.as_ref()
    }

    /// The files the zip will contain with the current choices.
    pub fn parts(&self) -> Vec<super::Part> {
        super::parts(&self.contents, &self.choices)
    }

    pub fn rows(&self) -> Vec<Row> {
        super::rows(&self.contents, &self.choices)
    }

    /// Whether "The model" can be switched on at all.
    pub(crate) fn has_model(&self) -> bool {
        !self.contents.model.is_empty() || self.scene_title.is_some()
    }

    fn saved_current(&self) -> Option<&Path> {
        self.saved
            .as_ref()
            .filter(|(_, c)| *c == self.choices)
            .map(|(p, _)| p.as_path())
    }
}

/// `model/<base>` for a file named `base` (scrubbed), safe to extract on
/// any OS (see [`super::safe_file_name`]) and unique among `taken`
/// ignoring case (Windows and macOS do not tell `A.step` from `a.step`).
pub(crate) fn model_entry(taken: &[ModelFile], base: &str) -> String {
    let base = super::safe_file_name(base);
    let free = |n: &str| {
        !taken
            .iter()
            .any(|m| m.name.to_lowercase() == n.to_lowercase())
    };
    let mut name = format!("model/{base}");
    let mut i = 2;
    while !free(&name) {
        name = format!("model/{i}-{base}");
        i += 1;
    }
    name
}

/// Model files of this session: imported CAD files that still exist.
pub(crate) fn source_files(sources: &[PathBuf]) -> Vec<ModelFile> {
    let mut out: Vec<ModelFile> = Vec::new();
    for p in sources {
        let Ok(meta) = std::fs::metadata(p) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let name = model_entry(&out, &super::file_name(p));
        out.push(ModelFile {
            name,
            data: Data::File(p.clone()),
            size: meta.len(),
        });
    }
    out
}

impl AppState {
    /// Open the report dialog (no-op when it is already open: dialogs never
    /// stack).
    pub fn open_report(&mut self, failure: Option<Failure>, origin: Origin) {
        if self.ui.report.is_some() {
            return;
        }
        let m = super::system::Machine::get();
        let gpu = self.gpu_info.as_ref();
        let contents = Contents {
            log: super::clean(&super::log::report_log(self.paths.data.as_deref())),
            steps_json: self.session.steps_json(),
            op_count: self.session.op_count(),
            system: super::system::system_text(m, gpu, self.display),
            system_summary: super::system::summary(m, gpu),
            model: source_files(&self.session.sources),
            failure,
        };
        let scene_title = self
            .doc
            .has_scene()
            .then(|| super::scrub::scrub(&self.doc.title()));
        log::info!(
            "report dialog opened ({})",
            match origin {
                Origin::Caught => "after an error",
                Origin::PreviousCrash => "after a crash in the last session",
                Origin::Manual => "from Help",
            }
        );
        self.ui.report = Some(ReportDialog {
            origin,
            choices: Choices::default(),
            contents,
            saved: None,
            save_to: None,
            zip_name: super::zip_name(super::now_secs()),
            scene_title,
            scene: SceneCopy::NotYet,
            copied_at: None,
        });
    }

    /// Failures caught since the last frame: log them, and open the dialog
    /// for the first one not reported yet (once per failure). The previous
    /// session's crash is offered once the autosave recovery is done, so the
    /// report sees the recovered scene and the graphics adapter, and only
    /// when no dialog that goes first is open (its files go once offered,
    /// so it must be shown right away: Discard in a Quit prompt would
    /// otherwise lose it). Returns when to draw a frame again for a crash
    /// file waiting to be withdrawn (see [`crash::tick`]).
    pub fn poll_failures(&mut self) -> Option<std::time::Duration> {
        let wake = crash::tick();
        let first = matches!(
            self.active_modal(),
            None | Some(crate::jobs::Modal::Export | crate::jobs::Modal::About)
        );
        if first && let Some(p) = self.previous_crash.take() {
            self.open_report(Some(p.failure.clone()), Origin::PreviousCrash);
            // Only now: a start-up that fails before this leaves it on disk.
            p.offered();
        }
        for f in crash::take_caught() {
            if !self.reported.insert(f.key()) {
                continue;
            }
            if self.ui.report.is_some() {
                log::warn!(
                    "another error while the report dialog is open: {}",
                    f.message
                );
                continue;
            }
            self.open_report(Some(f), Origin::Caught);
        }
        wake
    }

    /// Copy the open scene into the report the first time "The model" is on.
    fn ensure_scene_copy(&mut self) {
        let Some(d) = self.ui.report.as_mut() else {
            return;
        };
        if !d.choices.model || d.scene != SceneCopy::NotYet {
            return;
        }
        let Some(title) = d.scene_title.clone() else {
            d.scene = SceneCopy::Failed;
            return;
        };
        // A private folder: the scene must not be readable by other users
        // of the machine while it is copied.
        let r = super::private_temp_dir()
            .map_err(|e| e.to_string())
            .and_then(|dir| {
                let tmp = dir.path().join("scene.ntopo");
                self.doc.save_copy(&tmp)?;
                std::fs::read(&tmp).map_err(|e| e.to_string())
            });
        let Some(d) = self.ui.report.as_mut() else {
            return;
        };
        match r {
            Ok(bytes) => {
                d.contents.model.push(ModelFile {
                    name: model_entry(&d.contents.model, &format!("{title}.ntopo")),
                    size: bytes.len() as u64,
                    data: Data::Bytes(bytes.into()),
                });
                d.scene = SceneCopy::Done;
            }
            Err(e) => {
                d.scene = SceneCopy::Failed;
                log::warn!("couldn't copy the scene into the report: {e}");
                self.toast(ToastKind::Warn, S::REPORT_SCENE_FAILED);
            }
        }
    }

    /// Save the zip (asking where, unless `save_to` is set). Returns the
    /// path when saved.
    pub(crate) fn save_report(&mut self) -> Option<PathBuf> {
        self.ensure_scene_copy();
        let d = self.ui.report.as_mut()?;
        let path = match d.save_to.clone() {
            Some(p) => p,
            None => {
                let p = rfd::FileDialog::new()
                    .set_title(S::REPORT_SAVE_TITLE)
                    .add_filter("Zip archive", &["zip"])
                    .set_file_name(d.zip_name.as_str())
                    .save_file()?;
                if p.extension().is_none() {
                    p.with_extension("zip")
                } else {
                    p
                }
            }
        };
        super::scrub::know_file(&path);
        let name = super::file_name(&path);
        let parts = d.parts();
        match super::write_zip(&path, &parts) {
            Ok(not_saved) => {
                d.saved = Some((path.clone(), d.choices.clone()));
                self.toast_log(
                    ToastKind::Info,
                    format!("{} {}", S::REPORT_SAVED, crate::jobs::short_path(&path)),
                    &format!(
                        "report saved: {name} ({} files)",
                        parts.len() - not_saved.len()
                    ),
                );
                if !not_saved.is_empty() {
                    let list = not_saved.join(", ");
                    self.toast_log(
                        ToastKind::Warn,
                        format!("{} {list}", S::REPORT_NOT_SAVED),
                        &format!("report saved without unreadable files: {list}"),
                    );
                }
                if !self.screenshot_mode
                    && let Err(e) = reveal(&path)
                {
                    log::warn!("couldn't show the report in the file manager: {e}");
                }
                Some(path)
            }
            Err(e) => {
                self.toast_log(
                    ToastKind::Error,
                    format!("{}: {e}", S::REPORT_SAVE_FAILED),
                    &format!("couldn't save the report {name}: {e}"),
                );
                None
            }
        }
    }

    /// Open the pre-filled GitHub issue, saving the zip first if needed.
    pub(crate) fn open_issue(&mut self) {
        let unsaved = self
            .ui
            .report
            .as_ref()
            .is_some_and(|d| d.saved_current().is_none());
        // Cancelled or failed: the issue text would ask for a file that
        // doesn't exist.
        if unsaved && self.save_report().is_none() {
            return;
        }
        let Some(d) = self.ui.report.as_ref() else {
            return;
        };
        let zip = d
            .saved_current()
            .map(super::file_name)
            .unwrap_or_else(|| d.zip_name.clone());
        let url = super::issue_url(&d.contents, &d.choices, &zip);
        self.open_url(&url);
    }

    /// Open a report link in the browser: only GitHub's new-issue page of
    /// this repository. Screenshot and test runs only record it.
    pub fn open_url(&mut self, url: &str) {
        if !url.starts_with(&format!("{}?", super::ISSUE_URL)) {
            return;
        }
        self.opened_urls.push(url.to_string());
        if self.screenshot_mode {
            return;
        }
        log::info!("opening the GitHub issue page");
        match open::that_detached(url) {
            Ok(()) => self.toast(ToastKind::Info, S::REPORT_ISSUE_OPENED),
            Err(e) => self.toast(ToastKind::Warn, format!("{}: {e}", S::REPORT_ISSUE_FAILED)),
        }
    }

    fn copy_prompt(&mut self, ctx: &egui::Context) {
        let Some(d) = self.ui.report.as_mut() else {
            return;
        };
        let zip = d
            .saved_current()
            .map(super::file_name)
            .unwrap_or_else(|| d.zip_name.clone());
        let prompt = super::agent_prompt(&d.contents, &d.choices, &zip);
        ctx.copy_text(prompt);
        d.copied_at = Some(self.time);
        self.toast(ToastKind::Info, S::REPORT_PROMPT_COPIED);
    }

    pub fn report_dialog_ui(&mut self, ctx: &egui::Context) {
        self.ensure_scene_copy();
        let pal = self.pal;
        let time = self.time;
        let Some(d) = self.ui.report.as_mut() else {
            return;
        };
        let mut action = None;
        let frame = egui::Frame::new()
            .fill(pal.panel_raised)
            .stroke(Stroke::new(1.0, pal.border))
            .corner_radius(CORNER)
            .inner_margin(Margin::ZERO)
            .shadow(egui::Shadow {
                offset: [0, 10],
                blur: 32,
                spread: 0,
                color: Color32::from_black_alpha(if pal.dark { 120 } else { 45 }),
            });
        egui::Modal::new(egui::Id::new("report"))
            .frame(frame)
            .show(ctx, |ui| {
                // Scroll instead of running off a short window.
                let max_h = (ctx.content_rect().height() - 48.0).max(200.0);
                egui::ScrollArea::vertical()
                    .max_height(max_h)
                    .min_scrolled_height(max_h)
                    .show(ui, |ui| action = dialog(ui, &pal, d, time));
            });
        match action {
            Some(Action::Close) => self.ui.report = None,
            Some(Action::Save) => {
                self.save_report();
            }
            Some(Action::Issue) => self.open_issue(),
            Some(Action::Copy) => self.copy_prompt(ctx),
            None => {}
        }
    }
}

/// Show `path` in the file manager.
fn reveal(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(windows)]
    {
        let mut arg = std::ffi::OsString::from("/select,");
        arg.push(path.as_os_str());
        std::process::Command::new("explorer")
            .arg(arg)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let dir = path.parent().unwrap_or(Path::new("."));
        open::that_detached(dir).map_err(|e| e.to_string())
    }
}

// ------------------------------------------------------------ layout

#[derive(Clone, Copy, Debug, PartialEq)]
enum Action {
    Close,
    Save,
    Issue,
    Copy,
}

const WIDTH: f32 = 760.0;
const CORNER: u8 = 12;
const PAD_X: i8 = 18;
const SPACING: egui::Vec2 = egui::Vec2::new(8.0, 6.0);

fn dialog(ui: &mut Ui, pal: &Palette, d: &mut ReportDialog, time: f64) -> Option<Action> {
    let mut action = None;
    ui.set_width(WIDTH);
    ui.spacing_mut().item_spacing = egui::Vec2::ZERO;

    section(ui, Margin::symmetric(PAD_X, 16), None, |ui| {
        header(ui, pal, d)
    });
    hline(ui, pal.border);
    columns(ui, pal, d);
    hline(ui, pal.border);
    section(ui, Margin::symmetric(PAD_X, 12), None, |ui| {
        if let Some(a) = footer(ui, pal, d) {
            action = Some(a);
        }
    });
    hline(ui, pal.border);
    section(
        ui,
        Margin::symmetric(PAD_X, 12),
        Some((
            pal.panel,
            CornerRadius {
                nw: 0,
                ne: 0,
                sw: CORNER,
                se: CORNER,
            },
        )),
        |ui| {
            if ai_strip(ui, pal, copied_recently(d, time)) {
                action = Some(Action::Copy);
            }
        },
    );
    if copied_recently(d, time) {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(250));
    }
    action
}

fn copied_recently(d: &ReportDialog, time: f64) -> bool {
    d.copied_at.is_some_and(|t| time - t < 2.5)
}

/// A full-width block with its own padding and optional fill.
fn section(
    ui: &mut Ui,
    margin: Margin,
    fill: Option<(Color32, CornerRadius)>,
    add: impl FnOnce(&mut Ui),
) {
    let mut f = egui::Frame::new().inner_margin(margin);
    if let Some((c, r)) = fill {
        f = f.fill(c).corner_radius(r);
    }
    f.show(ui, |ui| {
        ui.set_width(WIDTH - margin.sum().x);
        ui.spacing_mut().item_spacing = SPACING;
        add(ui);
    });
}

fn hline(ui: &mut Ui, c: Color32) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, c);
}

fn title_and_subtitle(d: &ReportDialog) -> (String, &'static str) {
    match d.origin {
        Origin::Manual => (S::REPORT_TITLE_MANUAL.into(), S::REPORT_SUB_MANUAL),
        Origin::PreviousCrash => (S::REPORT_TITLE_CRASH.into(), S::REPORT_SUB_CRASH),
        Origin::Caught => (
            match d.failure().and_then(|f| f.context.clone()) {
                Some(c) => format!("{c} {}", S::REPORT_TITLE_COULD_NOT),
                None => S::REPORT_TITLE_ERROR.into(),
            },
            S::REPORT_SUB_ERROR,
        ),
    }
}

fn header(ui: &mut Ui, pal: &Palette, d: &ReportDialog) {
    let (title, sub) = title_and_subtitle(d);
    let error = d.origin != Origin::Manual;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        let (r, _) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::hover());
        let c = if error { pal.error } else { pal.accent };
        ui.painter().rect_filled(r, 9.0, c.gamma_multiply(0.16));
        ui.painter().text(
            r.center(),
            Align2::CENTER_CENTER,
            if error { "!" } else { "?" },
            theme::medium(18.0),
            c,
        );
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            ui.label(
                RichText::new(title)
                    .font(theme::medium(16.0))
                    .color(pal.text_strong),
            );
            ui.label(RichText::new(sub).size(13.0).color(pal.text_dim));
        });
    });
}

fn caption(ui: &mut Ui, pal: &Palette, text: &str) {
    ui.label(
        RichText::new(text.to_uppercase())
            .font(theme::medium(11.0))
            .color(pal.text_dim),
    );
    ui.add_space(2.0);
}

fn columns(ui: &mut Ui, pal: &Palette, d: &mut ReportDialog) {
    let left_w = (WIDTH * 1.15 / 2.15).round();
    let right_w = WIDTH - left_w;
    // The right column's background spans the full row height, which is
    // known only after both columns are laid out.
    let bg = ui.painter().add(Shape::Noop);
    let row = ui
        .horizontal_top(|ui| {
            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
            ui.allocate_ui_with_layout(vec2(left_w, 0.0), Layout::top_down(Align::Min), |ui| {
                egui::Frame::new()
                    .inner_margin(Margin::symmetric(PAD_X, 16))
                    .show(ui, |ui| {
                        ui.set_width(left_w - 2.0 * PAD_X as f32);
                        ui.spacing_mut().item_spacing = SPACING;
                        left_column(ui, pal, d);
                    });
            });
            ui.allocate_ui_with_layout(vec2(right_w, 0.0), Layout::top_down(Align::Min), |ui| {
                egui::Frame::new()
                    .inner_margin(Margin::symmetric(PAD_X, 16))
                    .show(ui, |ui| {
                        ui.set_width(right_w - 2.0 * PAD_X as f32);
                        ui.spacing_mut().item_spacing = SPACING;
                        right_column(ui, pal, d);
                    });
            });
        })
        .response
        .rect;
    let r = Rect::from_min_max(pos2(row.left() + left_w, row.top()), row.max);
    ui.painter().set(
        bg,
        Shape::Vec(vec![
            Shape::rect_filled(r, 0.0, pal.panel),
            Shape::line_segment(
                [r.left_top(), r.left_bottom()],
                Stroke::new(1.0, pal.border),
            ),
        ]),
    );
}

fn left_column(ui: &mut Ui, pal: &Palette, d: &mut ReportDialog) {
    caption(ui, pal, S::REPORT_ADD);
    let steps_desc = match d.contents.op_count {
        0 => S::REPORT_STEPS_NONE.to_string(),
        1 => S::REPORT_STEPS_ONE.to_string(),
        n => format!("The {n} {}", S::REPORT_STEPS_MANY),
    };
    option_row(
        ui,
        pal,
        S::REPORT_STEPS,
        &steps_desc,
        &mut d.choices.steps,
        true,
    );
    let has_model = d.has_model();
    let model_desc = model_description(d);
    if !has_model {
        d.choices.model = false;
    }
    option_row(
        ui,
        pal,
        S::REPORT_MODEL,
        &model_desc,
        &mut d.choices.model,
        has_model,
    );
    option_row(
        ui,
        pal,
        S::REPORT_PC,
        S::REPORT_PC_DESC,
        &mut d.choices.pc,
        true,
    );
    ui.add_space(6.0);
    ui.add(
        egui::TextEdit::multiline(&mut d.choices.comment)
            .hint_text(RichText::new(S::REPORT_COMMENT_HINT).color(pal.text_dim))
            .desired_rows(3)
            .desired_width(f32::INFINITY)
            .char_limit(4000)
            .margin(Margin::symmetric(10, 8))
            .background_color(pal.panel),
    );
}

fn model_description(d: &ReportDialog) -> String {
    let cad: Vec<&ModelFile> = d
        .contents
        .model
        .iter()
        .filter(|m| !m.name.ends_with(".ntopo"))
        .collect();
    match (cad.first(), &d.scene_title, d.scene == SceneCopy::Failed) {
        (Some(first), scene, _) => {
            let mut s = format!(
                "{} · {}",
                first.name.trim_start_matches("model/"),
                super::human_size(first.size)
            );
            if cad.len() > 1 {
                s.push_str(&format!(" + {} more", cad.len() - 1));
            }
            if scene.is_some() {
                s.push_str(" + scene");
            }
            s
        }
        (None, Some(t), false) => format!("{t}.ntopo · {}", S::REPORT_MODEL_SCENE),
        (None, Some(_), true) => S::REPORT_SCENE_FAILED.into(),
        (None, None, _) => S::REPORT_MODEL_NONE.into(),
    }
}

/// A toggle row: title, description and a switch; the whole row toggles.
fn option_row(ui: &mut Ui, pal: &Palette, title: &str, desc: &str, on: &mut bool, enabled: bool) {
    let id = ui.id().with(("report-option", title));
    let w = ui.available_width();
    let fill = if *on && enabled {
        pal.widget
    } else {
        Color32::TRANSPARENT
    };
    let inner = egui::Frame::new()
        .inner_margin(Margin::symmetric(12, 9))
        .corner_radius(9)
        .stroke(Stroke::new(1.0, pal.border))
        .fill(fill)
        .show(ui, |ui| {
            ui.set_width(w - 26.0);
            ui.horizontal(|ui| {
                let text_w = ui.available_width() - 32.0 - 12.0;
                ui.allocate_ui_with_layout(vec2(text_w, 0.0), Layout::top_down(Align::Min), |ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    let strong = if enabled {
                        pal.text_strong
                    } else {
                        pal.text_dim
                    };
                    ui.add(
                        egui::Label::new(
                            RichText::new(title).font(theme::medium(13.5)).color(strong),
                        )
                        .truncate(),
                    );
                    ui.add(
                        egui::Label::new(RichText::new(desc).size(12.0).color(pal.text_dim))
                            .truncate(),
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(32.0, 18.0), Sense::hover());
                    paint_switch(ui, pal, r, *on && enabled, enabled);
                });
            });
        });
    let rect = inner.response.rect;
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let resp = ui.interact(rect, id, sense);
    resp.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, *on, title)
    });
    if enabled && resp.hovered() {
        ui.painter().rect_stroke(
            rect,
            9.0,
            Stroke::new(1.0, pal.text_dim.gamma_multiply(0.6)),
            egui::StrokeKind::Inside,
        );
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if enabled && resp.clicked() {
        *on = !*on;
    }
}

fn paint_switch(ui: &Ui, pal: &Palette, r: Rect, on: bool, enabled: bool) {
    let dim = |c: Color32| if enabled { c } else { c.gamma_multiply(0.45) };
    let track = match (on, pal.dark) {
        (true, _) => pal.accent,
        (false, true) => pal.widget_active,
        (false, false) => pal.text_dim.gamma_multiply(0.55),
    };
    ui.painter().rect_filled(r, 9.0, dim(track));
    let knob_x = if on { r.right() - 9.0 } else { r.left() + 9.0 };
    let knob = match (on, pal.dark) {
        (true, _) => pal.accent_text,
        (false, true) => pal.text,
        (false, false) => Color32::WHITE,
    };
    ui.painter()
        .circle_filled(pos2(knob_x, r.center().y), 7.0, dim(knob));
}

fn right_column(ui: &mut Ui, pal: &Palette, d: &ReportDialog) {
    caption(ui, pal, S::REPORT_CONTAINS);
    let w = ui.available_width();
    let mono = FontId::monospace(12.0);
    let rows = d.rows();
    let mut total = 0u64;
    for r in &rows {
        let (size, color) = match (r.included, r.size) {
            (true, Some(s)) => {
                total += s;
                (super::human_size(s), pal.text_dim)
            }
            (true, None) => (String::new(), pal.text_dim),
            (false, _) => (S::REPORT_NOT_INCLUDED.to_string(), pal.text_dim),
        };
        let name_color = if r.included { pal.text } else { pal.text_dim };
        two_cells(
            ui,
            pal,
            w,
            &r.name,
            name_color,
            &size,
            color,
            mono.clone(),
            !r.included,
            true,
        );
    }
    ui.add_space(2.0);
    let zip = d
        .saved_current()
        .map(super::file_name)
        .unwrap_or_else(|| d.zip_name.clone());
    two_cells(
        ui,
        pal,
        w,
        &zip,
        pal.text_dim,
        &super::human_size(total),
        pal.text_dim,
        FontId::proportional(12.0),
        false,
        false,
    );
    ui.add_space(10.0);
    peek(ui, pal, d);
}

/// A row with `left` (truncated to fit) and `right` text, optionally
/// struck through and underlined by a separator.
#[allow(clippy::too_many_arguments)]
fn two_cells(
    ui: &mut Ui,
    pal: &Palette,
    w: f32,
    left: &str,
    left_color: Color32,
    right: &str,
    right_color: Color32,
    font: FontId,
    strike: bool,
    separator: bool,
) {
    let (r, _) = ui.allocate_exact_size(vec2(w, 27.0), Sense::hover());
    let p = ui.painter();
    let right_g = p.layout_no_wrap(right.to_string(), FontId::proportional(12.0), right_color);
    let left_w = (w - right_g.size().x - 12.0).max(20.0);
    let mut job = LayoutJob::single_section(
        left.to_string(),
        TextFormat {
            font_id: font,
            color: left_color,
            strikethrough: if strike {
                Stroke::new(1.0, left_color)
            } else {
                Stroke::NONE
            },
            ..Default::default()
        },
    );
    job.wrap = TextWrapping::truncate_at_width(left_w);
    let left_g = ui.fonts_mut(|f| f.layout_job(job));
    let cy = r.center().y;
    p.galley(
        pos2(r.left(), cy - left_g.size().y / 2.0),
        left_g,
        left_color,
    );
    p.galley(
        pos2(r.right() - right_g.size().x, cy - right_g.size().y / 2.0),
        right_g,
        right_color,
    );
    if separator {
        p.hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, pal.border));
    }
}

/// The error at a glance, in the right column.
fn peek(ui: &mut Ui, pal: &Palette, d: &ReportDialog) {
    let mono = |size: f32| FontId::monospace(size);
    let bar = if d.failure().is_some() {
        pal.error
    } else {
        pal.text_dim.gamma_multiply(0.6)
    };
    let top = ui.cursor().top();
    let inner = ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            let w = ui.available_width();
            let (first, second) = match d.failure() {
                Some(f) => {
                    let loc = f
                        .location
                        .as_deref()
                        .map(short_location)
                        .unwrap_or_else(|| "unknown location".into());
                    (
                        super::error_line(f),
                        format!("{loc} · newtopo {}", f.version),
                    )
                }
                None => (S::REPORT_NO_ERROR.to_string(), super::version_line()),
            };
            let mut job = LayoutJob::single_section(
                first,
                TextFormat {
                    font_id: mono(11.5),
                    color: pal.text_strong,
                    ..Default::default()
                },
            );
            job.wrap = TextWrapping {
                max_width: w,
                max_rows: 3,
                break_anywhere: true,
                overflow_character: Some('…'),
            };
            ui.label(job);
            let mut lines = vec![second];
            if d.choices.pc {
                lines.push(d.contents.system_summary.clone());
            }
            for l in lines {
                let mut job = LayoutJob::single_section(
                    l,
                    TextFormat {
                        font_id: mono(11.0),
                        color: pal.text_dim,
                        ..Default::default()
                    },
                );
                job.wrap = TextWrapping {
                    max_width: w,
                    max_rows: 2,
                    break_anywhere: true,
                    overflow_character: Some('…'),
                };
                ui.label(job);
            }
        });
    });
    let bottom = inner.response.rect.bottom();
    let x = inner.response.rect.left() + 1.0;
    ui.painter().vline(x, top..=bottom, Stroke::new(2.0, bar));
}

/// `crates/core/src/eval/assemble.rs:368:12` -> `eval/assemble.rs:368`.
pub fn short_location(loc: &str) -> String {
    let mut parts: Vec<&str> = loc.rsplitn(3, ':').collect();
    parts.reverse();
    let (file, line) = match parts.as_slice() {
        [file, line, _col] => (*file, Some(*line)),
        [file, line] => (*file, Some(*line)),
        _ => (loc, None),
    };
    let segs: Vec<&str> = file.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    let tail = if segs.len() >= 2 {
        let src = segs.iter().rposition(|s| *s == "src");
        match src {
            Some(i) if i + 1 < segs.len() => segs[i + 1..].join("/"),
            _ => segs[segs.len() - 2..].join("/"),
        }
    } else {
        file.to_string()
    };
    match line {
        Some(l) => format!("{tail}:{l}"),
        None => tail,
    }
}

fn button(ui: &mut Ui, pal: &Palette, text: &str, primary: bool) -> bool {
    let b = if primary {
        egui::Button::new(
            RichText::new(text)
                .font(theme::medium(13.0))
                .color(pal.accent_text),
        )
        .fill(pal.accent)
    } else {
        egui::Button::new(RichText::new(text).size(13.0))
    };
    ui.add(b.min_size(vec2(0.0, 30.0)).corner_radius(8))
        .clicked()
}

fn footer(ui: &mut Ui, pal: &Palette, d: &ReportDialog) -> Option<Action> {
    let mut action = None;
    let note = if d.choices.model {
        S::REPORT_PRIVACY_MODEL
    } else {
        S::REPORT_PRIVACY
    };
    egui::Sides::new().shrink_left().wrap().show(
        ui,
        |ui| {
            ui.label(RichText::new(note).size(12.0).color(pal.text_dim));
        },
        |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if button(ui, pal, S::REPORT_OPEN_ISSUE, true) {
                action = Some(Action::Issue);
            }
            if button(ui, pal, S::REPORT_SAVE, false) {
                action = Some(Action::Save);
            }
            if button(ui, pal, S::DLG_CLOSE, false) {
                action = Some(Action::Close);
            }
        },
    );
    action
}

fn ai_strip(ui: &mut Ui, pal: &Palette, copied: bool) -> bool {
    let mut clicked = false;
    egui::Sides::new().shrink_left().wrap().show(
        ui,
        |ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(
                    RichText::new(S::REPORT_AI_TITLE)
                        .font(theme::medium(13.5))
                        .color(pal.text_strong),
                );
                ui.label(
                    RichText::new(S::REPORT_AI_DESC)
                        .size(12.0)
                        .color(pal.text_dim),
                );
            });
        },
        |ui| {
            let label = if copied {
                S::REPORT_COPIED
            } else {
                S::REPORT_COPY_PROMPT
            };
            clicked = button(ui, pal, label, false);
        },
    );
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations_are_shortened() {
        assert_eq!(
            short_location("crates/core/src/eval/assemble.rs:368:12"),
            "eval/assemble.rs:368"
        );
        assert_eq!(short_location(r"C:\x\src\lib.rs:3:1"), "lib.rs:3");
        assert_eq!(
            short_location("~/.cargo/registry/foo/bar.rs:9:2"),
            "foo/bar.rs:9"
        );
        assert_eq!(short_location("weird"), "weird");
    }
}
