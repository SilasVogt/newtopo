//! File work: import (helper process on a worker thread, progress, cancel,
//! import list, report), open/save/recent, autosave + recovery, export,
//! and the modal dialogs around them (SPEC §8).

use crate::app::{AppState, PendingAfterPrompt};
use crate::docstate::guard;
use crate::kit;
use crate::prefs::Paths;
use crate::report::crash::Failure;
use crate::strings as S;
use crate::toasts::ToastKind;
use egui::{RichText, Ui, vec2};
use newtopo_core::export::{ExportFiles, ExportObject, ExportOptions, ExportUp, Format};
use newtopo_core::mesh::OutputMesh;
use newtopo_core::ops::Op;
use newtopo_kernel::helper::{ImportOptions, ImportedFile, run_import};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};

/// Owned data for one export object (moved to the export thread).
pub type ExportItem = (String, String, Option<[f32; 3]>, glam::DMat4, OutputMesh);

pub const CAD_EXTS: [&str; 5] = ["step", "stp", "iges", "igs", "brep"];

/// `name.ext` in `folder` — short enough for a toast.
pub fn short_path(p: &Path) -> String {
    let name = p
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    match p.parent().and_then(|d| d.file_name()) {
        Some(d) => format!("{name} in {}", d.to_string_lossy()),
        None => name,
    }
}

pub fn is_cad(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| CAD_EXTS.contains(&e.to_ascii_lowercase().as_str()))
}

pub fn is_scene(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("ntopo"))
}

pub fn autosave_path(paths: &Paths, scene: Option<&Path>) -> Option<PathBuf> {
    match scene {
        Some(p) => Some(PathBuf::from(format!("{}.autosave", p.display()))),
        None => paths.untitled_autosave(),
    }
}

/// Autosaves newer than their scene (offered for recovery at start).
pub fn find_recoverable(paths: &Paths, prefs: &crate::prefs::Prefs) -> Vec<PathBuf> {
    let newer = |a: &Path, b: &Path| -> bool {
        let ma = std::fs::metadata(a).and_then(|m| m.modified()).ok();
        let mb = std::fs::metadata(b).and_then(|m| m.modified()).ok();
        match (ma, mb) {
            (Some(x), Some(y)) => x > y,
            (Some(_), None) => true,
            _ => false,
        }
    };
    let mut out = Vec::new();
    for p in &prefs.recent {
        if let Some(a) = autosave_path(paths, Some(p))
            && newer(&a, p)
        {
            out.push(a);
        }
    }
    if let Some(u) = paths.untitled_autosave()
        && u.exists()
    {
        out.push(u);
    }
    out
}

/// Delete the autosaves this session wrote (or recovered), never one still
/// offered in the recovery dialog: those belong to the user's decision.
pub fn remove_autosaves(st: &mut AppState) {
    for p in removable_autosaves(&st.owned_autosaves, &st.ui.recovery) {
        let _ = std::fs::remove_file(&p);
        st.owned_autosaves.remove(&p);
    }
}

/// Owned autosaves minus those pending in the recovery dialog.
pub fn removable_autosaves(
    owned: &std::collections::BTreeSet<PathBuf>,
    pending: &[PathBuf],
) -> Vec<PathBuf> {
    owned
        .iter()
        .filter(|p| !pending.contains(p))
        .cloned()
        .collect()
}

/// Seconds until the next autosave should run, when one is due at all
/// (dirty document, autosave on). The frame loop asks for a repaint then,
/// so an idle window still autosaves.
pub fn autosave_wait(dirty: bool, minutes: u32, now: f64, next: f64) -> Option<f64> {
    (dirty && minutes > 0 && next > 0.0).then(|| (next - now).max(0.0))
}

pub fn with_confirmed(op: Op) -> Op {
    match op {
        Op::RemoveCadEdge { body, edge, .. } => Op::RemoveCadEdge {
            body,
            edge,
            confirmed: true,
        },
        Op::Collapse { body, edge, .. } => Op::Collapse {
            body,
            edge,
            confirmed: true,
        },
        Op::Weld { body, from, to, .. } => Op::Weld {
            body,
            from,
            to,
            confirmed: true,
        },
        other => other,
    }
}

struct ImportJob {
    path: PathBuf,
    progress: Arc<Mutex<(f32, String)>>,
    cancel: Arc<AtomicBool>,
    rx: Receiver<Result<ImportedFile, String>>,
    /// A panic on the job thread, for the problem report.
    panic: Arc<Mutex<Option<Failure>>>,
}

struct ExportJob {
    path: PathBuf,
    rx: Receiver<Result<ExportFiles, String>>,
    panic: Arc<Mutex<Option<Failure>>>,
}

/// On a job thread after a caught panic: keep the hook's record (or a
/// plain one) for the UI thread.
fn keep_panic(slot: &Mutex<Option<Failure>>, context: &str, fallback: &str) {
    let mut f =
        crate::report::crash::take_last().unwrap_or_else(|| Failure::from_message(fallback));
    f.context = Some(context.to_string());
    if let Ok(mut g) = slot.lock() {
        *g = Some(f);
    }
}

/// On the UI thread: queue a job's panic for the report dialog.
fn queue_panic(slot: &Mutex<Option<Failure>>) {
    if let Some(f) = slot.lock().ok().and_then(|mut g| g.take()) {
        crate::report::crash::queue(f);
    }
}

#[derive(Default)]
pub struct Jobs {
    import: Option<ImportJob>,
    export: Option<ExportJob>,
    /// A finished import that waits for the open preview to be applied or
    /// discarded: added to a previewed scene, Discard would remove it.
    held_import: Option<(PathBuf, ImportedFile, Vec<ImportChoice>)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportChoice {
    Keep,
    Exclude,
    Replace,
}

pub struct ImportRow {
    pub name: String,
    pub layer: Option<String>,
    pub choice: ImportChoice,
    pub selected: bool,
}

pub struct ImportList {
    pub path: PathBuf,
    pub file: ImportedFile,
    pub rows: Vec<ImportRow>,
    pub search: String,
}

pub struct ImportReport {
    pub file: String,
    pub bodies: usize,
    pub faces: usize,
    pub fixes: Vec<String>,
    pub failed_faces: usize,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ExportDialog {
    pub format: Format,
    pub opts: ExportOptions,
    pub units: String,
}

/// Millimetres per unit for "mm", "cm", "m", "in" (anything else: mm).
pub fn unit_mm(u: &str) -> f64 {
    newtopo_core::export::unit_metres(u).map_or(1.0, |m| m * 1000.0)
}

/// The app's modal dialogs, in the order [`AppState::active_modal`] picks them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modal {
    Confirm,
    Unsaved,
    Recovery,
    ImportList,
    ImportReport,
    Report,
    Export,
    About,
}

impl AppState {
    /// The open scene's length unit (what its geometry is stored in), or the
    /// preferred unit for new scenes when nothing is open.
    pub fn scene_units(&self) -> String {
        self.doc
            .scene
            .as_ref()
            .and_then(|s| guard(|| s.doc().settings.units.clone()).ok())
            .unwrap_or_else(|| self.prefs.units.clone())
    }

    /// Unit to convert an import into: the scene's own unit once it holds
    /// objects, otherwise the preferred unit for new scenes.
    fn import_units(&self) -> String {
        let has_bodies = self
            .doc
            .scene
            .as_ref()
            .and_then(|s| guard(|| !s.doc().bodies.is_empty()).ok())
            .unwrap_or(false);
        if has_bodies {
            self.scene_units()
        } else {
            self.prefs.units.clone()
        }
    }

    /// An import or export is running.
    pub fn busy(&self) -> bool {
        self.jobs.import.is_some() || self.jobs.export.is_some()
    }

    // ------------------------------------------------------------ import

    /// An import is running, waits in the object chooser, or is held for
    /// an open preview. One at a time, so none overwrites another.
    fn import_busy(&self) -> bool {
        self.jobs.import.is_some()
            || self.ui.import_list.is_some()
            || self.jobs.held_import.is_some()
    }

    pub fn import_dialog(&mut self) {
        if self.import_busy() {
            self.toast(ToastKind::Info, "An import is already running.");
            return;
        }
        let picked = rfd::FileDialog::new()
            .set_title(S::IMPORT_TITLE)
            .add_filter(S::IMPORT_FILTER, &CAD_EXTS)
            .pick_file();
        if let Some(p) = picked {
            self.start_import(p);
        }
    }

    /// A document passed on the command line (file association).
    pub fn open_from_launch(&mut self, p: PathBuf) {
        crate::report::scrub::know_file(&p);
        let name = crate::report::file_name(&p);
        if !p.is_file() {
            self.toast_log(
                ToastKind::Error,
                format!("Can't open {}: file not found", p.display()),
                &format!("open failed: {name}: file not found"),
            );
        } else if is_scene(&p) {
            self.open_scene(Some(p));
        } else if is_cad(&p) {
            self.start_import(p);
        } else {
            self.toast_log(
                ToastKind::Warn,
                format!("Can't open {}: use STEP, IGES, BREP or .ntopo", p.display()),
                &format!("can't open {name}: not STEP, IGES, BREP or .ntopo"),
            );
        }
    }

    pub fn start_import(&mut self, path: PathBuf) {
        crate::report::scrub::know_file(&path);
        if !newtopo_kernel::HAS_CAD_IMPORT {
            self.toast(ToastKind::Error, S::TOAST_NO_CAD_IMPORT);
            return;
        }
        if self.import_busy() {
            self.toast(
                ToastKind::Info,
                format!(
                    "An import is already running; {} was not imported.",
                    path.display()
                ),
            );
            return;
        }
        let progress = Arc::new(Mutex::new((0.0f32, String::from("Starting…"))));
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let opts = ImportOptions {
            heal: self.prefs.import.heal,
            sewing_tolerance: self.prefs.import.sewing_tolerance,
            // Geometry is stored in the scene's units: convert into the open
            // scene's units, or into the preferred units for a new scene.
            units: self.import_units(),
            helper: crate::launch::helper_path(),
            ..Default::default()
        };
        let (p2, c2, path2) = (progress.clone(), cancel.clone(), path.clone());
        let panic = Arc::new(Mutex::new(None));
        let panic2 = panic.clone();
        log::info!("import started: {}", crate::report::file_name(&path));
        std::thread::Builder::new()
            .name("import".into())
            .spawn(move || {
                let mut cb = |f: f32, msg: &str| {
                    if let Ok(mut g) = p2.lock() {
                        *g = (f, msg.to_string());
                    }
                    !c2.load(Ordering::Relaxed)
                };
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_import(&path2, &opts, &mut cb)
                }));
                let r = match r {
                    Ok(Ok(f)) => Ok(f),
                    Ok(Err(e)) => Err(e.to_string()),
                    Err(_) => {
                        keep_panic(&panic2, "Import", "import helper runner panicked");
                        Err("import helper runner panicked".to_string())
                    }
                };
                let _ = tx.send(r);
            })
            .ok();
        self.jobs.import = Some(ImportJob {
            path,
            progress,
            cancel,
            rx,
            panic,
        });
    }

    /// The document was replaced: an import held for its preview doesn't
    /// belong to the new one.
    pub fn drop_held_import(&mut self) {
        self.jobs.held_import = None;
    }

    pub fn poll_jobs(&mut self, ctx: &egui::Context) {
        if self.jobs.held_import.is_some() && !self.doc.in_preview() {
            let (path, file, choices) = self.jobs.held_import.take().expect("held");
            self.finish_import(&path, file, choices);
        }
        if let Some(job) = &self.jobs.import {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            match job.rx.try_recv() {
                Ok(r) => {
                    let job = self.jobs.import.take().expect("job");
                    let cancelled = job.cancel.load(Ordering::Relaxed);
                    queue_panic(&job.panic);
                    let name = crate::report::file_name(&job.path);
                    match &r {
                        _ if cancelled => log::info!("import cancelled: {name}"),
                        Ok(f) => log::info!(
                            "import read: {name}, {} object(s), unit {}",
                            f.bodies.len(),
                            f.units
                        ),
                        Err(e) => log::warn!("import failed: {name}: {e}"),
                    }
                    match r {
                        // Cancel wins even if the helper finished meanwhile.
                        Ok(_) if cancelled => self.toast(ToastKind::Info, S::IMPORT_CANCELLED),
                        Ok(file) => {
                            if file.bodies.len() > 1 {
                                let existing_names: Vec<String> = self
                                    .doc
                                    .scene
                                    .as_ref()
                                    .and_then(|s| {
                                        guard(|| {
                                            s.doc().bodies.iter().map(|b| b.name.clone()).collect()
                                        })
                                        .ok()
                                    })
                                    .unwrap_or_default();
                                let rows = file
                                    .bodies
                                    .iter()
                                    .map(|b| ImportRow {
                                        name: b.attrs.name.clone(),
                                        layer: b.attrs.layer.clone(),
                                        choice: if existing_names.contains(&b.attrs.name) {
                                            ImportChoice::Replace
                                        } else {
                                            ImportChoice::Keep
                                        },
                                        selected: false,
                                    })
                                    .collect();
                                self.ui.import_list = Some(ImportList {
                                    path: job.path,
                                    file,
                                    rows,
                                    search: String::new(),
                                });
                            } else {
                                let n = file.bodies.len();
                                self.finish_import(&job.path, file, vec![ImportChoice::Keep; n]);
                            }
                        }
                        Err(e) if cancelled => {
                            self.toast(ToastKind::Info, format!("{}: {e}", S::IMPORT_CANCELLED))
                        }
                        Err(e) => {
                            self.ui.import_report = Some(ImportReport {
                                file: job
                                    .path
                                    .file_name()
                                    .map(|s| s.to_string_lossy().into_owned())
                                    .unwrap_or_default(),
                                bodies: 0,
                                faces: 0,
                                fixes: vec![],
                                failed_faces: 0,
                                warnings: vec![],
                                errors: vec![e],
                            });
                        }
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    if let Some(job) = self.jobs.import.take() {
                        queue_panic(&job.panic);
                    }
                    self.toast(ToastKind::Error, S::IMPORT_FAILED);
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(job) = &self.jobs.export {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            match job.rx.try_recv() {
                Ok(r) => {
                    let job = self.jobs.export.take().expect("job");
                    queue_panic(&job.panic);
                    match r.and_then(|files| write_export(&job.path, files)) {
                        Ok(()) => self.toast_log(
                            ToastKind::Info,
                            format!("{} {}", S::TOAST_EXPORTED, short_path(&job.path)),
                            &format!("export finished: {}", crate::report::file_name(&job.path)),
                        ),
                        Err(e) => self.toast_log(
                            ToastKind::Error,
                            &e,
                            &format!(
                                "export failed: {}: {e}",
                                crate::report::file_name(&job.path)
                            ),
                        ),
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    if let Some(job) = self.jobs.export.take() {
                        queue_panic(&job.panic);
                    }
                }
                Err(TryRecvError::Empty) => {}
            }
        }
    }

    /// Import the objects marked Keep/Replace in the open import list.
    pub fn accept_import_list(&mut self) {
        if let Some(list) = self.ui.import_list.take() {
            let choices = list.rows.iter().map(|r| r.choice).collect();
            self.finish_import(&list.path, list.file, choices);
        }
    }

    fn finish_import(&mut self, path: &Path, file: ImportedFile, choices: Vec<ImportChoice>) {
        if self.doc.in_preview() {
            self.jobs.held_import = Some((path.to_path_buf(), file, choices));
            self.toast(ToastKind::Info, S::IMPORT_WAITS_FOR_PREVIEW);
            return;
        }
        let mut report = ImportReport {
            file: path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            bodies: 0,
            faces: 0,
            fixes: vec![],
            failed_faces: 0,
            warnings: file.warnings.clone(),
            errors: vec![],
        };
        if choices.iter().all(|c| *c == ImportChoice::Exclude) {
            return;
        }
        let _action = crate::report::crash::action("Import");
        self.session.import(path);
        if let Err(e) = self.doc.ensure_scene() {
            report.errors.push(e);
            self.ui.import_report = Some(report);
            return;
        }
        self.demo = None;
        let scene = self.doc.scene.as_mut().expect("scene");
        // Record the unit the file was converted to (an empty scene takes it;
        // a scene with objects only accepts geometry in its own unit).
        let scene_units = guard(|| scene.doc().settings.units.clone()).unwrap_or_default();
        let mut set_units = || {
            guard(|| {
                scene.apply(Op::SetUnits {
                    units: file.units.clone(),
                })
            })
            .map_err(|e| e.to_string())
            .and_then(|r| r.map_err(|r| r.reason))
        };
        if file.units != scene_units
            && let Err(e) = set_units()
        {
            report.errors.push(format!(
                "the file was converted to {} but the scene is in {scene_units}: {e}",
                file.units
            ));
            self.ui.import_report = Some(report);
            return;
        }
        let existing: Vec<(String, newtopo_kernel::BodyId)> = guard(|| {
            scene
                .doc()
                .bodies
                .iter()
                .map(|b| (b.name.clone(), b.id))
                .collect()
        })
        .unwrap_or_default();
        for (b, c) in file.bodies.into_iter().zip(choices) {
            if c == ImportChoice::Exclude {
                continue;
            }
            report.faces += b.heal.faces_after;
            report.failed_faces += b.heal.failed_faces.len();
            report.fixes.extend(b.heal.fixes.iter().cloned());
            let name = b.attrs.name.clone();
            // Replace: swap the same-named object, carrying rules where faces
            // still match (SPEC §8.1).
            let old = (c == ImportChoice::Replace)
                .then(|| existing.iter().find(|(n, _)| *n == name).map(|x| x.1))
                .flatten();
            if c == ImportChoice::Replace && old.is_none() {
                report.warnings.push(format!(
                    "{name}: nothing with that name to replace; imported as a new object"
                ));
            }
            match old {
                Some(old) => {
                    match guard(|| scene.replace_body_with_report(old, &b.brep, b.attrs, b.heal)) {
                        Ok(Ok(r)) => {
                            report.bodies += 1;
                            report.warnings.push(format!(
                            "{name}: replaced; {} faces matched, {} unmatched, {} rules kept, {} dropped, {} edits dropped",
                            r.matched,
                            r.unmatched.len(),
                            r.rules_kept,
                            r.rules_dropped,
                            r.edits_dropped
                        ));
                        }
                        Ok(Err(e)) => report.errors.push(format!("{name}: {e:#}")),
                        Err(e) => report.errors.push(format!("{name}: {e}")),
                    }
                }
                None => match guard(|| scene.add_body(&b.brep, b.attrs, b.heal)) {
                    Ok(Ok(_)) => report.bodies += 1,
                    Ok(Err(e)) => report.errors.push(format!("{name}: {e:#}")),
                    Err(e) => report.errors.push(format!("{name}: {e}")),
                },
            }
        }
        self.doc.bump();
        self.view.needs_frame = true;
        self.ui.import_report = Some(report);
    }

    pub fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        for p in dropped {
            if is_cad(&p) {
                self.start_import(p);
            } else if is_scene(&p) {
                self.request(PendingAfterPrompt::Open(Some(p)));
            } else {
                crate::report::scrub::know_file(&p);
                self.toast_log(
                    ToastKind::Warn,
                    format!("Can't open {}: use STEP, IGES, BREP or .ntopo", p.display()),
                    &format!(
                        "can't open {}: not STEP, IGES, BREP or .ntopo",
                        crate::report::file_name(&p)
                    ),
                );
            }
        }
    }

    pub fn jobs_status(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        let mut cancel = false;
        if let Some(job) = &self.jobs.import {
            let (f, msg) = job.progress.lock().map(|g| g.clone()).unwrap_or_default();
            if ui.small_button("Cancel").clicked() {
                cancel = true;
            }
            ui.add(
                egui::ProgressBar::new(f)
                    .desired_width(140.0)
                    .desired_height(10.0)
                    .fill(pal.accent),
            );
            ui.label(
                RichText::new(format!(
                    "Importing {} · {msg}",
                    job.path
                        .file_name()
                        .map(|s| s.to_string_lossy())
                        .unwrap_or_default()
                ))
                .small()
                .color(pal.text),
            );
        }
        if cancel && let Some(job) = &self.jobs.import {
            job.cancel.store(true, Ordering::Relaxed);
        }
        if self.jobs.export.is_some() {
            ui.spinner();
            ui.label(RichText::new("Exporting…").small().color(pal.text));
        }
    }

    // ------------------------------------------------------------ open / save / autosave

    pub fn open_scene(&mut self, path: Option<PathBuf>) {
        let path = match path {
            Some(p) => p,
            None => match rfd::FileDialog::new()
                .add_filter("newtopo scene", &["ntopo"])
                .pick_file()
            {
                Some(p) => p,
                None => return,
            },
        };
        let _action = crate::report::crash::action("Open");
        crate::report::scrub::know_file(&path);
        match self.doc.open(&path) {
            Ok(()) => {
                self.session.open(&path);
                self.end_compare();
                self.drop_held_import();
                self.demo = None;
                self.prefs.push_recent(&path);
                self.dirty_settings = true;
                self.sel = Default::default();
                self.view.needs_frame = true;
                if let Some(cam) = self
                    .doc
                    .scene
                    .as_ref()
                    .and_then(|s| guard(|| s.doc().camera).ok())
                    // The default camera means none was saved: frame instead.
                    .filter(|c| *c != newtopo_core::doc::Camera::default())
                {
                    self.view.camera = newtopo_render::OrbitCamera::from_doc(&cam, self.prefs.up);
                    self.view.auto_ortho = false;
                    self.view.needs_frame = false;
                }
            }
            Err(e) => self.toast_log(
                ToastKind::Error,
                format!("Couldn't open {}: {e}", path.display()),
                &format!("open failed: {}: {e}", crate::report::file_name(&path)),
            ),
        }
    }

    pub fn save(&mut self, save_as: bool) {
        if !self.doc.has_scene() {
            self.toast(ToastKind::Info, S::TOAST_NO_SCENE);
            return;
        }
        if self.blocked_by_preview() {
            return;
        }
        let path = match (&self.doc.path, save_as) {
            (Some(p), false) => p.clone(),
            _ => {
                let name = format!("{}.ntopo", self.doc.title());
                match rfd::FileDialog::new()
                    .add_filter("newtopo scene", &["ntopo"])
                    .set_file_name(name)
                    .save_file()
                {
                    Some(p) if p.extension().is_none() => p.with_extension("ntopo"),
                    Some(p) => p,
                    None => return,
                }
            }
        };
        let old_autosave = autosave_path(&self.paths, self.doc.path.as_deref());
        self.doc.set_camera(self.view.camera.to_doc());
        crate::report::scrub::know_file(&path);
        let name = crate::report::file_name(&path);
        match self.doc.save(&path) {
            Ok(()) => {
                if let Some(a) = old_autosave
                    && self.owned_autosaves.contains(&a)
                    && !self.ui.recovery.contains(&a)
                {
                    let _ = std::fs::remove_file(&a);
                    self.owned_autosaves.remove(&a);
                }
                self.prefs.push_recent(&path);
                self.dirty_settings = true;
                self.toast_log(
                    ToastKind::Info,
                    format!("{} {}", S::TOAST_SAVED, short_path(&path)),
                    &format!("saved: {name}"),
                );
            }
            Err(e) => self.toast_log(
                ToastKind::Error,
                format!("Couldn't save: {e}"),
                &format!("save failed: {name}: {e}"),
            ),
        }
    }

    pub fn autosave_tick(&mut self, ctx: &egui::Context) {
        if self.screenshot_mode || self.prefs.autosave_minutes == 0 {
            return;
        }
        // Wake up for the deadline even when the user is idle.
        if let Some(wait) = autosave_wait(
            self.doc.is_dirty(),
            self.prefs.autosave_minutes,
            self.time,
            self.next_autosave,
        ) {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait.max(0.05)));
        }
        let interval = self.prefs.autosave_minutes as f64 * 60.0;
        if self.next_autosave == 0.0 || self.next_autosave > self.time + interval {
            self.next_autosave = self.time + interval;
            return;
        }
        if self.time < self.next_autosave {
            return;
        }
        self.next_autosave = self.time + interval;
        // An open preview isn't written anywhere, autosave included.
        if !self.doc.is_dirty() || self.doc.in_preview() {
            return;
        }
        let Some(p) = autosave_path(&self.paths, self.doc.path.as_deref()) else {
            return;
        };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        self.doc.set_camera(self.view.camera.to_doc());
        crate::report::scrub::know_file(&p);
        let name = crate::report::file_name(&p);
        match self.doc.save_copy(&p) {
            Ok(()) => {
                log::info!("autosaved: {name}");
                self.owned_autosaves.insert(p);
            }
            Err(e) => log::warn!("autosave failed: {name}: {e}"),
        }
    }

    fn recover(&mut self, autosave: &Path) {
        let original = autosave
            .to_str()
            .and_then(|s| s.strip_suffix(".autosave"))
            .map(PathBuf::from)
            .filter(|p| is_scene(p) && self.paths.untitled_autosave().as_deref() != Some(autosave));
        crate::report::scrub::know_file(autosave);
        match self.doc.open(autosave) {
            Ok(()) => {
                // The user chose this autosave: it is ours to clean up now.
                self.owned_autosaves.insert(autosave.to_path_buf());
                self.end_compare();
                self.drop_held_import();
                self.demo = None;
                self.doc.path = original;
                self.doc.bump();
                self.view.needs_frame = true;
                self.toast(ToastKind::Info, "Recovered. Save to keep it.");
            }
            Err(e) => self.toast_log(
                ToastKind::Error,
                format!("Couldn't recover: {e}"),
                &format!(
                    "recover failed: {}: {e}",
                    crate::report::file_name(autosave)
                ),
            ),
        }
    }

    // ------------------------------------------------------------ export

    pub fn open_export_dialog(&mut self) {
        if !self.doc.has_scene() {
            self.toast(ToastKind::Info, S::TOAST_NO_SCENE);
            return;
        }
        if self.blocked_by_preview() {
            return;
        }
        self.ui.export = Some(ExportDialog {
            format: self.prefs.export_format,
            opts: self.prefs.export.clone(),
            units: self.prefs.units.clone(),
        });
    }

    /// Owned export data for the selected objects, or every object
    /// (hidden ones included) when nothing is selected (SPEC §8.3, §9.3).
    pub fn export_items(&mut self) -> Option<Vec<ExportItem>> {
        // The output cache is otherwise refreshed only when the viewport is
        // drawn; a hidden viewport must not leave discarded geometry in it.
        self.ensure_view_data();
        let scene = self.doc.scene.as_ref()?;
        let (bodies, layers) =
            guard(|| (scene.doc().bodies.clone(), scene.doc().layers.clone())).ok()?;
        let selected = &self.sel.objects;
        Some(
            bodies
                .iter()
                .filter(|b| selected.is_empty() || selected.contains(&b.id))
                .filter_map(|b| {
                    let mesh = self.vd.outputs.iter().find(|o| o.body == b.id)?.clone();
                    let layer = layers
                        .iter()
                        .find(|l| l.id == b.layer)
                        .map(|l| l.name.clone())
                        .unwrap_or_default();
                    Some((b.name.clone(), layer, b.color, b.transform, mesh))
                })
                .collect(),
        )
    }

    fn run_export(&mut self, d: ExportDialog) {
        let (ext, filter) = match d.format {
            Format::Obj => ("obj", "OBJ"),
            Format::Glb => ("glb", "glTF binary"),
            Format::Fbx => ("fbx", "FBX"),
        };
        let name = format!("{}.{ext}", self.doc.title());
        let Some(path) = rfd::FileDialog::new()
            .add_filter(filter, &[ext])
            .set_file_name(name)
            .save_file()
        else {
            return;
        };
        let path = if path.extension().is_none() {
            path.with_extension(ext)
        } else {
            path
        };
        self.export_to(path, d);
    }

    /// Export to `path` on a worker thread (no file dialog).
    pub fn export_to(&mut self, path: PathBuf, d: ExportDialog) {
        if self.blocked_by_preview() {
            return;
        }
        crate::report::scrub::know_file(&path);
        let Some(objects) = self.export_items() else {
            self.toast(ToastKind::Error, S::TOAST_ENGINE_MISSING);
            return;
        };
        if objects.is_empty() {
            self.toast(
                ToastKind::Warn,
                "Nothing to export: the output mesh is empty.",
            );
            return;
        }
        let mut opts = d.opts.clone();
        // Scene unit -> chosen export unit, and declare that unit (FBX) or
        // convert it to metres (glTF); OBJ keeps the export unit.
        opts.scale *= unit_mm(&self.scene_units()) / unit_mm(&d.units);
        opts.unit_metres = unit_mm(&d.units) / 1000.0;
        let format = d.format;
        self.prefs.export_format = d.format;
        self.prefs.export = d.opts;
        self.dirty_settings = true;
        let (tx, rx) = channel();
        let panic = Arc::new(Mutex::new(None));
        let panic2 = panic.clone();
        log::info!("export started: {:?}, {} object(s)", format, objects.len());
        std::thread::Builder::new()
            .name("export".into())
            .spawn(move || {
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let objs: Vec<ExportObject> = objects
                        .iter()
                        .map(|(n, l, c, t, m)| ExportObject {
                            name: n,
                            layer: l,
                            color: *c,
                            transform: *t,
                            mesh: m,
                        })
                        .collect();
                    newtopo_core::export::export(format, &objs, &opts)
                }));
                let r = match r {
                    Ok(Ok(f)) => Ok(f),
                    Ok(Err(e)) => Err(format!("Export failed: {e:#}")),
                    Err(_) => {
                        keep_panic(&panic2, "Export", "export panicked");
                        Err(format!("Export failed: {}", S::TOAST_ENGINE_MISSING))
                    }
                };
                let _ = tx.send(r);
            })
            .ok();
        self.jobs.export = Some(ExportJob { path, rx, panic });
    }

    // ------------------------------------------------------------ dialogs

    /// The app modal to show this frame. Only one is shown at a time, in
    /// this priority order; the others wait. `AppState::escape` closes the
    /// one that was last drawn (`UiState::shown_modal`), so Esc always acts
    /// on the visible dialog, even if a higher-priority one arrived in the
    /// same frame and hasn't been drawn yet.
    pub fn active_modal(&self) -> Option<Modal> {
        if self.ui.confirm.is_some() {
            Some(Modal::Confirm)
        } else if self.ui.unsaved_prompt.is_some() {
            Some(Modal::Unsaved)
        } else if !self.ui.recovery.is_empty() {
            Some(Modal::Recovery)
        } else if self.ui.import_list.is_some() {
            Some(Modal::ImportList)
        } else if self.ui.import_report.is_some() {
            Some(Modal::ImportReport)
        } else if self.ui.report.is_some() {
            Some(Modal::Report)
        } else if self.ui.export.is_some() {
            Some(Modal::Export)
        } else if self.ui.about_open {
            Some(Modal::About)
        } else {
            None
        }
    }

    /// Whether modal `m` still has something to show.
    pub fn modal_open(&self, m: Modal) -> bool {
        match m {
            Modal::Confirm => self.ui.confirm.is_some(),
            Modal::Unsaved => self.ui.unsaved_prompt.is_some(),
            Modal::Recovery => !self.ui.recovery.is_empty(),
            Modal::ImportList => self.ui.import_list.is_some(),
            Modal::ImportReport => self.ui.import_report.is_some(),
            Modal::Report => self.ui.report.is_some(),
            Modal::Export => self.ui.export.is_some(),
            Modal::About => self.ui.about_open,
        }
    }

    /// Esc on `m`: closes it like its Cancel/Close button. The recovery
    /// offer has no cancel, so Esc leaves it open.
    pub fn dismiss_modal(&mut self, m: Modal) {
        match m {
            Modal::Confirm => self.ui.confirm = None,
            Modal::Unsaved => self.ui.unsaved_prompt = None,
            Modal::Recovery => {}
            Modal::ImportList => self.ui.import_list = None,
            Modal::ImportReport => self.ui.import_report = None,
            Modal::Report => self.ui.report = None,
            Modal::Export => self.ui.export = None,
            Modal::About => self.ui.about_open = false,
        }
    }

    pub fn dialogs_ui(&mut self, ctx: &egui::Context) {
        let pal = self.pal;
        let active = self.active_modal();
        // Esc acts on what the user saw: the modal drawn this frame.
        self.ui.shown_modal = active;
        // Confirmation for ops that need it (SPEC §6.8–6.10).
        if active == Some(Modal::Confirm)
            && let Some(c) = self.ui.confirm.clone()
        {
            let mut close = false;
            let mut ok = false;
            kit::modal("confirm", &pal).show(ctx, |ui| {
                ui.set_width(400.0);
                close |= kit::dialog_header(ui, &pal, &c.title, None);
                ui.label(&c.body);
                match kit::dialog_footer(ui, &pal, S::DLG_CONTINUE, true, &[S::DLG_CANCEL]) {
                    Some(0) => ok = true,
                    Some(_) => close = true,
                    None => {}
                }
            });
            if ok {
                self.ui.confirm = None;
                let ops: Vec<Op> = c.ops.into_iter().map(with_confirmed).collect();
                if ops.len() == 1 {
                    self.apply(ops.into_iter().next().expect("op"));
                } else {
                    self.apply_group(ops);
                }
            } else if close {
                self.ui.confirm = None;
            }
        }

        // Unsaved changes.
        if active == Some(Modal::Unsaved)
            && let Some(pending) = self.ui.unsaved_prompt.clone()
        {
            let quitting = matches!(pending, PendingAfterPrompt::Quit);
            let mut choice = None;
            kit::modal("unsaved", &pal).show(ctx, |ui| {
                ui.set_width(400.0);
                if kit::dialog_header(ui, &pal, S::DLG_UNSAVED_TITLE, None) {
                    choice = Some(2);
                }
                ui.label(S::DLG_UNSAVED_BODY);
                match kit::dialog_footer(
                    ui,
                    &pal,
                    S::DLG_SAVE,
                    true,
                    &[S::DLG_CANCEL, S::DLG_DISCARD],
                ) {
                    Some(0) => choice = Some(0),
                    Some(1) => choice = Some(2),
                    Some(_) => choice = Some(1),
                    None => {}
                }
            });
            match choice {
                Some(0) => {
                    self.ui.unsaved_prompt = None;
                    self.save(false);
                    if !self.doc.is_dirty() {
                        self.do_pending(pending);
                    }
                }
                Some(1) => {
                    self.ui.unsaved_prompt = None;
                    self.do_pending(pending);
                }
                Some(_) => self.ui.unsaved_prompt = None,
                None => {}
            }
            if self.ui.unsaved_prompt.is_none() && quitting && self.quit {
                remove_autosaves(self);
            }
        }

        // Crash recovery offer.
        if active == Some(Modal::Recovery) {
            let list = self.ui.recovery.clone();
            let mut recover: Option<PathBuf> = None;
            let mut discard_all = false;
            kit::modal("recovery", &pal).show(ctx, |ui| {
                ui.set_width(460.0);
                ui.label(
                    RichText::new(S::DLG_RECOVER_TITLE)
                        .font(crate::theme::medium(16.0))
                        .color(pal.text_strong),
                );
                ui.label(RichText::new(S::DLG_RECOVER_BODY).color(pal.text_dim));
                ui.add_space(10.0);
                for p in &list {
                    ui.horizontal(|ui| {
                        ui.label(
                            p.file_name()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(S::DLG_RECOVER).clicked() {
                                recover = Some(p.clone());
                            }
                        });
                    });
                }
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(S::DLG_DISCARD).clicked() {
                        discard_all = true;
                    }
                });
            });
            if let Some(p) = recover {
                self.ui.recovery.clear();
                self.recover(&p);
            } else if discard_all {
                for p in &list {
                    let _ = std::fs::remove_file(p);
                }
                self.ui.recovery.clear();
            }
        }

        // About.
        if active == Some(Modal::About) {
            let mut close = false;
            kit::modal("about", &pal).show(ctx, |ui| {
                ui.set_width(380.0);
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(40.0, 40.0), egui::Sense::hover());
                    crate::app::paint_logo(ui.painter(), r, pal.accent);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(S::APP_NAME).font(crate::theme::medium(18.0)).color(pal.text_strong));
                        ui.label(RichText::new(format!("version {}", env!("CARGO_PKG_VERSION"))).small().color(pal.text_dim));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        close |= kit::close_button(ui, &pal).clicked();
                    });
                });
                ui.add_space(12.0);
                ui.label(S::DLG_ABOUT_BODY);
                ui.add_space(6.0);
                ui.label(RichText::new("CAD kernel: Open CASCADE Technology (LGPL-2.1 with exception), loaded in a separate helper process. UI font: Noto Sans (SIL OFL 1.1).").small().color(pal.text_dim));
                if kit::dialog_footer(ui, &pal, S::DLG_CLOSE, true, &[]).is_some() {
                    close = true;
                }
            });
            if close {
                self.ui.about_open = false;
            }
        }

        match active {
            Some(Modal::ImportList) => self.import_list_ui(ctx),
            Some(Modal::ImportReport) => self.import_report_ui(ctx),
            Some(Modal::Report) => self.report_dialog_ui(ctx),
            Some(Modal::Export) => self.export_dialog_ui(ctx),
            _ => {}
        }
    }

    fn import_list_ui(&mut self, ctx: &egui::Context) {
        let Some(list) = &mut self.ui.import_list else {
            return;
        };
        let pal = self.pal;
        let mut done: Option<bool> = None;
        kit::modal("import-list", &pal).show(ctx, |ui| {
            ui.set_width(600.0);
            let path = list.path.display().to_string();
            if kit::dialog_header(ui, &pal, S::IMPORT_LIST_TITLE, Some(&path)) {
                done = Some(false);
            }
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut list.search)
                        .hint_text(S::IMPORT_SEARCH)
                        .desired_width(180.0),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Bulk actions appear once rows are ticked, and say how many.
                    let n = list.rows.iter().filter(|r| r.selected).count();
                    if n == 0 {
                        ui.label(
                            RichText::new(S::IMPORT_BULK_HINT)
                                .small()
                                .color(pal.text_dim),
                        );
                        return;
                    }
                    for (c, l) in [
                        (ImportChoice::Replace, S::IMPORT_REPLACE),
                        (ImportChoice::Exclude, S::IMPORT_EXCLUDE),
                        (ImportChoice::Keep, S::IMPORT_KEEP),
                    ] {
                        if ui
                            .add(egui::Button::new(l).small())
                            .on_hover_text("Set for the ticked rows")
                            .clicked()
                        {
                            list.rows
                                .iter_mut()
                                .filter(|r| r.selected)
                                .for_each(|r| r.choice = c);
                        }
                    }
                    ui.label(
                        RichText::new(format!("{n} {}", S::IMPORT_SET_TICKED))
                            .small()
                            .color(pal.text),
                    );
                });
            });
            ui.add_space(10.0);
            let q = list.search.to_lowercase();
            egui::Frame::new()
                .fill(pal.inset)
                .stroke(egui::Stroke::new(1.0, pal.border))
                .corner_radius(pal.rounding.round() as u8)
                .inner_margin(egui::Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    egui::ScrollArea::vertical()
                        .max_height(320.0)
                        .show(ui, |ui| {
                            egui::Grid::new("import-rows")
                                .num_columns(4)
                                .min_col_width(0.0)
                                .spacing(vec2(14.0, 6.0))
                                .show(ui, |ui| {
                                    // Header tick: select every visible row, or none.
                                    let visible = |r: &ImportRow| {
                                        q.is_empty() || r.name.to_lowercase().contains(&q)
                                    };
                                    let shown = list.rows.iter().filter(|r| visible(r)).count();
                                    let ticked = list
                                        .rows
                                        .iter()
                                        .filter(|r| visible(r) && r.selected)
                                        .count();
                                    let mut all = shown > 0 && ticked == shown;
                                    if kit::checkbox(ui, &mut all, "")
                                        .on_hover_text(if all {
                                            S::IMPORT_NONE
                                        } else {
                                            S::IMPORT_ALL
                                        })
                                        .changed()
                                    {
                                        list.rows
                                            .iter_mut()
                                            .filter(|r| visible(r))
                                            .for_each(|r| r.selected = all);
                                    }
                                    for h in [
                                        S::IMPORT_COL_OBJECT,
                                        S::IMPORT_COL_LAYER,
                                        S::IMPORT_COL_ACTION,
                                    ] {
                                        ui.label(RichText::new(h).small().color(pal.text_dim));
                                    }
                                    ui.end_row();
                                    for r in list.rows.iter_mut().filter(|r| {
                                        q.is_empty() || r.name.to_lowercase().contains(&q)
                                    }) {
                                        ui.allocate_ui_with_layout(
                                            vec2(18.0, 28.0),
                                            egui::Layout::left_to_right(egui::Align::Center),
                                            |ui| kit::checkbox(ui, &mut r.selected, ""),
                                        );
                                        let excluded = r.choice == ImportChoice::Exclude;
                                        let mut name = RichText::new(&r.name).color(if excluded {
                                            pal.text_dim
                                        } else {
                                            pal.text_strong
                                        });
                                        if excluded {
                                            name = name.strikethrough();
                                        }
                                        ui.allocate_ui_with_layout(
                                            vec2(230.0, 24.0),
                                            egui::Layout::left_to_right(egui::Align::Center),
                                            |ui| {
                                                ui.set_min_width(230.0);
                                                ui.add(egui::Label::new(name).truncate());
                                            },
                                        );
                                        ui.label(
                                            RichText::new(r.layer.clone().unwrap_or_default())
                                                .small()
                                                .color(pal.text_dim),
                                        );
                                        kit::segmented(ui, &pal, |ui| {
                                            for (c, l) in [
                                                (ImportChoice::Keep, S::IMPORT_KEEP),
                                                (ImportChoice::Exclude, S::IMPORT_EXCLUDE),
                                                (ImportChoice::Replace, S::IMPORT_REPLACE),
                                            ] {
                                                ui.selectable_value(&mut r.choice, c, l);
                                            }
                                        });
                                        ui.end_row();
                                    }
                                });
                        });
                });
            let keep = list
                .rows
                .iter()
                .filter(|r| r.choice != ImportChoice::Exclude)
                .count();
            let run = format!("{} ({keep})", S::IMPORT_RUN);
            match kit::dialog_footer(ui, &pal, &run, keep > 0, &[S::DLG_CANCEL]) {
                Some(0) => done = Some(true),
                Some(_) => done = Some(false),
                None => {}
            }
        });
        match done {
            Some(true) => self.accept_import_list(),
            Some(false) => self.ui.import_list = None,
            None => {}
        }
    }

    fn import_report_ui(&mut self, ctx: &egui::Context) {
        let Some(r) = &self.ui.import_report else {
            return;
        };
        let pal = self.pal;
        let mut close = false;
        kit::modal("import-report", &pal).show(ctx, |ui| {
            ui.set_width(460.0);
            let failed = !r.errors.is_empty() && r.bodies == 0;
            let title = if failed {
                S::IMPORT_FAILED
            } else {
                S::IMPORT_REPORT_TITLE
            };
            close |= kit::dialog_header(ui, &pal, title, Some(&r.file));
            egui::Grid::new("report")
                .num_columns(2)
                .spacing(vec2(24.0, 6.0))
                .show(ui, |ui| {
                    let row = |ui: &mut Ui, k: &str, v: String, c: egui::Color32| {
                        ui.label(RichText::new(k).color(pal.text));
                        kit::right_cell(ui, 60.0, RichText::new(v).monospace().color(c));
                        ui.end_row();
                    };
                    row(ui, "Objects", r.bodies.to_string(), pal.text_strong);
                    row(ui, "Faces", r.faces.to_string(), pal.text_strong);
                    row(
                        ui,
                        "Fixes applied",
                        r.fixes.len().to_string(),
                        pal.text_strong,
                    );
                    row(
                        ui,
                        "Faces that failed",
                        r.failed_faces.to_string(),
                        if r.failed_faces > 0 {
                            pal.warn
                        } else {
                            pal.text_strong
                        },
                    );
                });
            if !r.fixes.is_empty() {
                ui.add_space(4.0);
                ui.collapsing("Fixes", |ui| {
                    for f in &r.fixes {
                        ui.label(RichText::new(f).small());
                    }
                });
            }
            if r.failed_faces > 0 {
                ui.add_space(6.0);
                ui.label(RichText::new(S::IMPORT_DAMAGED).color(pal.warn));
            }
            for w in &r.warnings {
                ui.label(RichText::new(w).small().color(pal.text_dim));
            }
            for e in &r.errors {
                ui.label(RichText::new(e).color(pal.error));
            }
            if kit::dialog_footer(ui, &pal, S::DLG_CLOSE, true, &[]).is_some() {
                close = true;
            }
        });
        if close {
            self.ui.import_report = None;
        }
    }

    fn export_dialog_ui(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.ui.export.take() else {
            return;
        };
        let pal = self.pal;
        let autofill = self.scene_toggles().map(|s| s.autofill).unwrap_or(true);
        let fans: Vec<(String, u32)> = self
            .vd
            .outputs
            .iter()
            .flat_map(|o| {
                let name = self
                    .vd
                    .body(o.body)
                    .map(|b| b.name.clone())
                    .unwrap_or_else(|| format!("Body {}", o.body));
                o.polygons
                    .iter()
                    .filter(|p| p.fan_fallback)
                    .map(move |p| (name.clone(), p.face.0))
            })
            .collect();
        let mut close = false;
        let mut go = false;
        kit::modal("export", &pal).show(ctx, |ui| {
            ui.set_width(460.0);
            let scope = if self.sel.objects.is_empty() {
                S::EXPORT_SCOPE_ALL
            } else {
                S::EXPORT_SCOPE_SEL
            };
            close |= kit::dialog_header(ui, &pal, S::EXPORT_TITLE, Some(scope));
            egui::Grid::new("export-opts")
                .num_columns(2)
                .spacing(vec2(20.0, 10.0))
                .min_col_width(96.0)
                .show(ui, |ui| {
                    ui.label(RichText::new(S::EXPORT_FORMAT).color(pal.text_dim));
                    kit::segmented(ui, &pal, |ui| {
                        ui.selectable_value(&mut d.format, Format::Fbx, "FBX");
                        ui.selectable_value(&mut d.format, Format::Obj, "OBJ");
                        ui.selectable_value(&mut d.format, Format::Glb, "glTF (.glb)");
                    });
                    ui.end_row();
                    ui.label(RichText::new(S::EXPORT_TRIANGULATE).color(pal.text_dim));
                    if d.format == Format::Glb {
                        ui.label(RichText::new(S::EXPORT_GLB_TRI).small().color(pal.text_dim));
                    } else {
                        kit::checkbox(ui, &mut d.opts.triangulate, "");
                    }
                    ui.end_row();
                    ui.label(RichText::new(S::EXPORT_UNITS).color(pal.text_dim));
                    kit::segmented(ui, &pal, |ui| {
                        for u in ["mm", "cm", "m", "in"] {
                            ui.selectable_value(&mut d.units, u.to_string(), u);
                        }
                    });
                    ui.end_row();
                    ui.label(RichText::new(S::EXPORT_SCALE).color(pal.text_dim));
                    ui.add(
                        egui::DragValue::new(&mut d.opts.scale)
                            .speed(0.01)
                            .range(0.0001..=10000.0),
                    );
                    ui.end_row();
                    ui.label(RichText::new(S::EXPORT_UP).color(pal.text_dim));
                    kit::segmented(ui, &pal, |ui| {
                        ui.selectable_value(&mut d.opts.up, ExportUp::Y, "Y");
                        ui.selectable_value(&mut d.opts.up, ExportUp::Z, "Z");
                    });
                    ui.end_row();
                    ui.label(RichText::new(S::EXPORT_NORMALS).color(pal.text_dim));
                    kit::segmented(ui, &pal, |ui| {
                        ui.selectable_value(
                            &mut d.opts.face_weighted_normals,
                            false,
                            S::EXPORT_NORMALS_CAD,
                        );
                        ui.selectable_value(
                            &mut d.opts.face_weighted_normals,
                            true,
                            S::EXPORT_NORMALS_WEIGHTED,
                        );
                    });
                    ui.end_row();
                    ui.label(RichText::new(S::EXPORT_OPTIONS).color(pal.text_dim));
                    ui.vertical(|ui| {
                        kit::checkbox(ui, &mut d.opts.layers_as_hierarchy, S::EXPORT_LAYERS);
                        kit::checkbox(ui, &mut d.opts.materials, S::EXPORT_MATERIALS);
                    });
                    ui.end_row();
                });
            if !autofill {
                ui.add_space(6.0);
                ui.label(RichText::new(S::EXPORT_NGON_NOTE).small().color(pal.warn));
            }
            if !fans.is_empty() {
                ui.add_space(6.0);
                ui.collapsing(format!("{} ({})", S::EXPORT_FAN_TITLE, fans.len()), |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(120.0)
                        .show(ui, |ui| {
                            for (name, face) in &fans {
                                ui.label(RichText::new(format!("{name} · face {face}")).small());
                            }
                        });
                });
            }
            match kit::dialog_footer(ui, &pal, S::EXPORT_RUN, true, &[S::DLG_CANCEL]) {
                Some(0) => go = true,
                Some(_) => close = true,
                None => {}
            }
        });
        if go {
            self.run_export(d);
        } else if !close {
            self.ui.export = Some(d);
        }
    }
}

fn write_export(path: &Path, files: ExportFiles) -> Result<(), String> {
    std::fs::write(path, &files.main)
        .map_err(|e| format!("Couldn't write {}: {e}", path.display()))?;
    for (suffix, bytes) in files.side {
        let stem = path.with_extension("");
        let p = PathBuf::from(format!("{}{suffix}", stem.display()));
        std::fs::write(&p, bytes).map_err(|e| format!("Couldn't write {}: {e}", p.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::app::App;

    fn app() -> (egui::Context, App) {
        let ctx = egui::Context::default();
        let app = App::new(&ctx, Paths::default(), false, true);
        (ctx, app)
    }

    #[test]
    fn pending_recovery_autosave_survives_exit_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let (_ctx, mut app) = app();
        let pending = dir.path().join("untitled.ntopo.autosave");
        let mine = dir.path().join("mine.ntopo.autosave");
        std::fs::write(&pending, b"x").unwrap();
        std::fs::write(&mine, b"y").unwrap();
        app.st.ui.recovery = vec![pending.clone()];
        app.st.owned_autosaves.insert(mine.clone());
        // Even if this session had written to the pending path, it stays
        // while the user hasn't answered the recovery dialog.
        app.st.owned_autosaves.insert(pending.clone());
        remove_autosaves(&mut app.st);
        assert!(pending.exists(), "pending recovery file must survive");
        assert!(!mine.exists(), "our own autosave is cleaned up");
    }

    #[test]
    fn new_and_open_drop_the_old_scene() {
        let (_ctx, mut app) = app();
        app.st.doc.ensure_scene().unwrap();
        app.st.ensure_view_data();
        let weak = Arc::downgrade(&app.st.doc.scene.as_ref().unwrap().kernel);
        app.st.do_pending(crate::app::PendingAfterPrompt::New);
        assert!(
            weak.upgrade().is_none(),
            "New must drop the old scene (and its kernel)"
        );
        // Open replaces the scene the same way.
        app.st.doc.ensure_scene().unwrap();
        let weak = Arc::downgrade(&app.st.doc.scene.as_ref().unwrap().kernel);
        let next = newtopo_core::scene::Scene::new(newtopo_kernel::default_kernel());
        app.st.doc.set_scene(next, None);
        assert!(
            weak.upgrade().is_none(),
            "Open must drop the previous scene"
        );
    }

    #[test]
    fn idle_window_wakes_for_autosave() {
        assert_eq!(autosave_wait(true, 5, 100.0, 400.0), Some(300.0));
        assert_eq!(autosave_wait(true, 5, 500.0, 400.0), Some(0.0));
        assert_eq!(autosave_wait(false, 5, 100.0, 400.0), None);
        assert_eq!(autosave_wait(true, 0, 100.0, 400.0), None);
    }

    fn fake_import(cancel: bool) -> (egui::Context, App) {
        fake_import_in(cancel, "mm")
    }

    fn fake_import_in(cancel: bool, units: &str) -> (egui::Context, App) {
        let (ctx, mut app) = app();
        let body = |n: &str| newtopo_kernel::helper::ImportedBody {
            brep: vec![],
            attrs: newtopo_kernel::BodyAttributes {
                name: n.into(),
                ..Default::default()
            },
            heal: Default::default(),
        };
        let (tx, rx) = channel();
        tx.send(Ok(ImportedFile {
            bodies: vec![body("a"), body("b")],
            source_sha256: String::new(),
            warnings: vec![],
            units: units.into(),
        }))
        .unwrap();
        app.st.jobs.import = Some(ImportJob {
            path: PathBuf::from("/x/a.step"),
            progress: Arc::new(Mutex::new((1.0, String::new()))),
            cancel: Arc::new(AtomicBool::new(cancel)),
            rx,
            panic: Default::default(),
        });
        app.st.poll_jobs(&ctx);
        (ctx, app)
    }

    /// The unit an import was converted to becomes the scene's unit, and
    /// export/bridge scale from it (not from the preference).
    #[test]
    fn import_records_the_scene_unit() {
        let (_c, mut app) = fake_import_in(false, "m");
        app.st.prefs.units = "mm".into();
        app.st.accept_import_list();
        assert!(app.st.doc.has_scene());
        assert_eq!(app.st.scene_units(), "m");
        // Changing the preference later doesn't rescale the open scene.
        app.st.prefs.units = "cm".into();
        assert_eq!(app.st.scene_units(), "m");
    }

    #[test]
    fn cancelled_import_is_discarded_even_when_it_succeeded() {
        let (_c, app) = fake_import(true);
        assert!(app.st.ui.import_list.is_none() && app.st.ui.import_report.is_none());
        assert!(!app.st.doc.has_scene() && !app.st.busy());
        let (_c, app) = fake_import(false);
        assert!(
            app.st.ui.import_list.is_some(),
            "control: an uncancelled result shows the list"
        );
    }

    use super::*;

    #[test]
    fn file_kinds() {
        assert!(is_cad(Path::new("a/b.STEP")));
        assert!(is_cad(Path::new("x.igs")));
        assert!(!is_cad(Path::new("x.obj")));
        assert!(is_scene(Path::new("x.ntopo")));
    }

    #[test]
    fn autosave_paths() {
        let paths = Paths {
            config: None,
            data: Some(PathBuf::from("/d")),
        };
        assert_eq!(
            autosave_path(&paths, Some(Path::new("/s/a.ntopo"))),
            Some(PathBuf::from("/s/a.ntopo.autosave"))
        );
        assert_eq!(
            autosave_path(&paths, None),
            Some(PathBuf::from("/d/untitled.ntopo.autosave"))
        );
    }

    #[test]
    fn confirmed() {
        let op = Op::RemoveCadEdge {
            body: None,
            edge: newtopo_kernel::EdgeId(3),
            confirmed: false,
        };
        assert_eq!(
            with_confirmed(op),
            Op::RemoveCadEdge {
                body: None,
                edge: newtopo_kernel::EdgeId(3),
                confirmed: true
            }
        );
    }

    #[test]
    fn export_writes_side_files() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("part.obj");
        write_export(
            &p,
            ExportFiles {
                main: b"o".to_vec(),
                side: vec![(".mtl".into(), b"m".to_vec())],
            },
        )
        .unwrap();
        assert!(dir.path().join("part.mtl").exists());
    }
}
