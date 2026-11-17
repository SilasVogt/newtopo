//! File work: import (helper process on a worker thread, progress, cancel,
//! import list, report), open/save/recent, autosave + recovery, export,
//! and the modal dialogs around them (SPEC §8).

use crate::app::{AppState, PendingAfterPrompt};
use crate::docstate::guard;
use crate::prefs::Paths;
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

pub fn remove_autosaves(st: &mut AppState) {
    if let Some(a) = autosave_path(&st.paths, st.doc.path.as_deref()) {
        let _ = std::fs::remove_file(a);
    }
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
}

struct ExportJob {
    path: PathBuf,
    rx: Receiver<Result<ExportFiles, String>>,
}

#[derive(Default)]
pub struct Jobs {
    import: Option<ImportJob>,
    export: Option<ExportJob>,
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

pub fn unit_mm(u: &str) -> f64 {
    match u {
        "cm" => 10.0,
        "m" => 1000.0,
        "in" => 25.4,
        _ => 1.0,
    }
}

impl AppState {
    /// An import or export is running.
    pub fn busy(&self) -> bool {
        self.jobs.import.is_some() || self.jobs.export.is_some()
    }

    // ------------------------------------------------------------ import

    pub fn import_dialog(&mut self) {
        if self.jobs.import.is_some() {
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
        if !p.is_file() {
            self.toast(
                ToastKind::Error,
                format!("Can't open {}: file not found", p.display()),
            );
        } else if is_scene(&p) {
            self.open_scene(Some(p));
        } else if is_cad(&p) {
            self.start_import(p);
        } else {
            self.toast(
                ToastKind::Warn,
                format!("Can't open {}: use STEP, IGES, BREP or .ntopo", p.display()),
            );
        }
    }

    pub fn start_import(&mut self, path: PathBuf) {
        if !newtopo_kernel::HAS_CAD_IMPORT {
            self.toast(ToastKind::Error, S::TOAST_NO_CAD_IMPORT);
            return;
        }
        if self.jobs.import.is_some() {
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
            units: self.prefs.import.units.clone(),
            helper: crate::launch::helper_path(),
            ..Default::default()
        };
        let (p2, c2, path2) = (progress.clone(), cancel.clone(), path.clone());
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
                    Err(_) => Err("import helper runner panicked".to_string()),
                };
                let _ = tx.send(r);
            })
            .ok();
        self.jobs.import = Some(ImportJob {
            path,
            progress,
            cancel,
            rx,
        });
    }

    pub fn poll_jobs(&mut self, ctx: &egui::Context) {
        if let Some(job) = &self.jobs.import {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            match job.rx.try_recv() {
                Ok(r) => {
                    let job = self.jobs.import.take().expect("job");
                    let cancelled = job.cancel.load(Ordering::Relaxed);
                    match r {
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
                    self.jobs.import = None;
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
                    match r.and_then(|files| write_export(&job.path, files)) {
                        Ok(()) => self.toast(
                            ToastKind::Info,
                            format!("{} {}", S::TOAST_EXPORTED, short_path(&job.path)),
                        ),
                        Err(e) => self.toast(ToastKind::Error, e),
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    self.jobs.export = None;
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
        if let Err(e) = self.doc.ensure_scene() {
            report.errors.push(e);
            self.ui.import_report = Some(report);
            return;
        }
        self.demo = None;
        let scene = self.doc.scene.as_mut().expect("scene");
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
                self.toast(
                    ToastKind::Warn,
                    format!("Can't open {}: use STEP, IGES, BREP or .ntopo", p.display()),
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
        match self.doc.open(&path) {
            Ok(()) => {
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
                    self.view.needs_frame = false;
                }
            }
            Err(e) => self.toast(
                ToastKind::Error,
                format!("Couldn't open {}: {e}", path.display()),
            ),
        }
    }

    pub fn save(&mut self, save_as: bool) {
        if !self.doc.has_scene() {
            self.toast(ToastKind::Info, S::TOAST_NO_SCENE);
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
        match self.doc.save(&path) {
            Ok(()) => {
                if let Some(a) = old_autosave {
                    let _ = std::fs::remove_file(a);
                }
                self.prefs.push_recent(&path);
                self.dirty_settings = true;
                self.toast(
                    ToastKind::Info,
                    format!("{} {}", S::TOAST_SAVED, short_path(&path)),
                );
            }
            Err(e) => self.toast(ToastKind::Error, format!("Couldn't save: {e}")),
        }
    }

    pub fn autosave_tick(&mut self) {
        if self.screenshot_mode || self.prefs.autosave_minutes == 0 {
            return;
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
        if !self.doc.is_dirty() {
            return;
        }
        let Some(p) = autosave_path(&self.paths, self.doc.path.as_deref()) else {
            return;
        };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        self.doc.set_camera(self.view.camera.to_doc());
        match self.doc.save_copy(&p) {
            Ok(()) => log::info!("autosaved to {}", p.display()),
            Err(e) => log::warn!("autosave failed: {e}"),
        }
    }

    fn recover(&mut self, autosave: &Path) {
        let original = autosave
            .to_str()
            .and_then(|s| s.strip_suffix(".autosave"))
            .map(PathBuf::from)
            .filter(|p| is_scene(p) && self.paths.untitled_autosave().as_deref() != Some(autosave));
        match self.doc.open(autosave) {
            Ok(()) => {
                self.demo = None;
                self.doc.path = original;
                self.doc.bump();
                self.view.needs_frame = true;
                self.toast(ToastKind::Info, "Recovered. Save to keep it.");
            }
            Err(e) => self.toast(ToastKind::Error, format!("Couldn't recover: {e}")),
        }
    }

    // ------------------------------------------------------------ export

    pub fn open_export_dialog(&mut self) {
        if !self.doc.has_scene() {
            self.toast(ToastKind::Info, S::TOAST_NO_SCENE);
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
    pub fn export_items(&self) -> Option<Vec<ExportItem>> {
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
        opts.scale *= unit_mm(&self.prefs.units) / unit_mm(&d.units);
        let format = d.format;
        self.prefs.export_format = d.format;
        self.prefs.export = d.opts;
        self.dirty_settings = true;
        let (tx, rx) = channel();
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
                    Err(_) => Err(format!("Export failed: {}", S::TOAST_ENGINE_MISSING)),
                };
                let _ = tx.send(r);
            })
            .ok();
        self.jobs.export = Some(ExportJob { path, rx });
    }

    // ------------------------------------------------------------ dialogs

    pub fn dialogs_ui(&mut self, ctx: &egui::Context) {
        let pal = self.pal;
        // Confirmation for ops that need it (SPEC §6.8–6.10).
        if let Some(c) = self.ui.confirm.clone() {
            let mut close = false;
            let mut ok = false;
            egui::Modal::new(egui::Id::new("confirm")).show(ctx, |ui| {
                ui.set_width(380.0);
                ui.label(
                    RichText::new(&c.title)
                        .font(crate::theme::medium(16.0))
                        .color(pal.text_strong),
                );
                ui.add_space(6.0);
                ui.label(&c.body);
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(S::DLG_CONTINUE).color(pal.accent_text),
                            )
                            .fill(pal.accent),
                        )
                        .clicked()
                    {
                        ok = true;
                    }
                    if ui.button(S::DLG_CANCEL).clicked() {
                        close = true;
                    }
                });
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
        if let Some(pending) = self.ui.unsaved_prompt.clone() {
            let quitting = matches!(pending, PendingAfterPrompt::Quit);
            let mut choice = None;
            egui::Modal::new(egui::Id::new("unsaved")).show(ctx, |ui| {
                ui.set_width(360.0);
                ui.label(
                    RichText::new(S::DLG_UNSAVED_TITLE)
                        .font(crate::theme::medium(16.0))
                        .color(pal.text_strong),
                );
                ui.label(S::DLG_UNSAVED_BODY);
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(RichText::new(S::DLG_SAVE).color(pal.accent_text))
                                .fill(pal.accent),
                        )
                        .clicked()
                    {
                        choice = Some(0);
                    }
                    if ui.button(S::DLG_DISCARD).clicked() {
                        choice = Some(1);
                    }
                    if ui.button(S::DLG_CANCEL).clicked() {
                        choice = Some(2);
                    }
                });
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
        if !self.ui.recovery.is_empty() {
            let list = self.ui.recovery.clone();
            let mut recover: Option<PathBuf> = None;
            let mut discard_all = false;
            egui::Modal::new(egui::Id::new("recovery")).show(ctx, |ui| {
                ui.set_width(440.0);
                ui.label(
                    RichText::new(S::DLG_RECOVER_TITLE)
                        .font(crate::theme::medium(16.0))
                        .color(pal.text_strong),
                );
                ui.label(RichText::new(S::DLG_RECOVER_BODY).color(pal.text_dim));
                ui.add_space(8.0);
                for p in &list {
                    ui.horizontal(|ui| {
                        ui.label(
                            p.file_name()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                        );
                        if ui.button(S::DLG_RECOVER).clicked() {
                            recover = Some(p.clone());
                        }
                    });
                }
                ui.add_space(8.0);
                if ui.button(S::DLG_DISCARD).clicked() {
                    discard_all = true;
                }
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
        if self.ui.about_open {
            let mut close = false;
            egui::Modal::new(egui::Id::new("about")).show(ctx, |ui| {
                ui.set_width(360.0);
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(36.0, 36.0), egui::Sense::hover());
                    crate::app::paint_logo(ui.painter(), r, pal.accent);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(S::APP_NAME).font(crate::theme::medium(18.0)).color(pal.text_strong));
                        ui.label(RichText::new(format!("version {}", env!("CARGO_PKG_VERSION"))).small().color(pal.text_dim));
                    });
                });
                ui.add_space(8.0);
                ui.label(S::DLG_ABOUT_BODY);
                ui.add_space(4.0);
                ui.label(RichText::new("CAD kernel: Open CASCADE Technology (LGPL-2.1 with exception), loaded in a separate helper process. UI font: Noto Sans (SIL OFL 1.1).").small().color(pal.text_dim));
                ui.add_space(10.0);
                if ui.button(S::DLG_CLOSE).clicked() {
                    close = true;
                }
            });
            if close {
                self.ui.about_open = false;
            }
        }

        self.import_list_ui(ctx);
        self.import_report_ui(ctx);
        self.export_dialog_ui(ctx);
    }

    fn import_list_ui(&mut self, ctx: &egui::Context) {
        let Some(list) = &mut self.ui.import_list else {
            return;
        };
        let pal = self.pal;
        let mut done: Option<bool> = None;
        egui::Modal::new(egui::Id::new("import-list")).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.label(
                RichText::new(S::IMPORT_LIST_TITLE)
                    .font(crate::theme::medium(16.0))
                    .color(pal.text_strong),
            );
            ui.label(
                RichText::new(list.path.display().to_string())
                    .small()
                    .color(pal.text_dim),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut list.search)
                        .hint_text(S::IMPORT_SEARCH)
                        .desired_width(200.0),
                );
                let q = list.search.to_lowercase();
                let visible = |r: &ImportRow| q.is_empty() || r.name.to_lowercase().contains(&q);
                if ui.button(S::IMPORT_ALL).clicked() {
                    list.rows
                        .iter_mut()
                        .filter(|r| visible(r))
                        .for_each(|r| r.selected = true);
                }
                if ui.button(S::IMPORT_NONE).clicked() {
                    list.rows.iter_mut().for_each(|r| r.selected = false);
                }
                ui.separator();
                for (c, l) in [
                    (ImportChoice::Keep, S::IMPORT_KEEP),
                    (ImportChoice::Exclude, S::IMPORT_EXCLUDE),
                    (ImportChoice::Replace, S::IMPORT_REPLACE),
                ] {
                    if ui
                        .button(l)
                        .on_hover_text("Set for the selected rows")
                        .clicked()
                    {
                        list.rows
                            .iter_mut()
                            .filter(|r| r.selected)
                            .for_each(|r| r.choice = c);
                    }
                }
            });
            ui.add_space(6.0);
            let q = list.search.to_lowercase();
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .show(ui, |ui| {
                    egui::Grid::new("import-rows")
                        .num_columns(4)
                        .striped(true)
                        .spacing(vec2(10.0, 4.0))
                        .show(ui, |ui| {
                            for r in list
                                .rows
                                .iter_mut()
                                .filter(|r| q.is_empty() || r.name.to_lowercase().contains(&q))
                            {
                                ui.checkbox(&mut r.selected, "");
                                ui.label(&r.name);
                                ui.label(
                                    RichText::new(r.layer.clone().unwrap_or_default())
                                        .small()
                                        .color(pal.text_dim),
                                );
                                ui.horizontal(|ui| {
                                    ui.selectable_value(
                                        &mut r.choice,
                                        ImportChoice::Keep,
                                        S::IMPORT_KEEP,
                                    );
                                    ui.selectable_value(
                                        &mut r.choice,
                                        ImportChoice::Exclude,
                                        S::IMPORT_EXCLUDE,
                                    );
                                    ui.selectable_value(
                                        &mut r.choice,
                                        ImportChoice::Replace,
                                        S::IMPORT_REPLACE,
                                    );
                                });
                                ui.end_row();
                            }
                        });
                });
            ui.add_space(10.0);
            let keep = list
                .rows
                .iter()
                .filter(|r| r.choice != ImportChoice::Exclude)
                .count();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        keep > 0,
                        egui::Button::new(
                            RichText::new(format!("{} ({keep})", S::IMPORT_RUN))
                                .color(pal.accent_text),
                        )
                        .fill(pal.accent),
                    )
                    .clicked()
                {
                    done = Some(true);
                }
                if ui.button(S::DLG_CANCEL).clicked() {
                    done = Some(false);
                }
            });
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
        egui::Modal::new(egui::Id::new("import-report")).show(ctx, |ui| {
            ui.set_width(440.0);
            let failed = !r.errors.is_empty() && r.bodies == 0;
            let title = if failed {
                S::IMPORT_FAILED
            } else {
                S::IMPORT_REPORT_TITLE
            };
            ui.label(
                RichText::new(title)
                    .font(crate::theme::medium(16.0))
                    .color(if failed { pal.error } else { pal.text_strong }),
            );
            ui.label(RichText::new(&r.file).small().color(pal.text_dim));
            ui.add_space(8.0);
            egui::Grid::new("report")
                .num_columns(2)
                .spacing(vec2(16.0, 4.0))
                .show(ui, |ui| {
                    ui.label("Objects");
                    ui.label(r.bodies.to_string());
                    ui.end_row();
                    ui.label("Faces");
                    ui.label(r.faces.to_string());
                    ui.end_row();
                    ui.label("Fixes applied");
                    ui.label(r.fixes.len().to_string());
                    ui.end_row();
                    ui.label("Faces that failed");
                    ui.label(RichText::new(r.failed_faces.to_string()).color(
                        if r.failed_faces > 0 {
                            pal.warn
                        } else {
                            pal.text
                        },
                    ));
                    ui.end_row();
                });
            if !r.fixes.is_empty() {
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
            ui.add_space(10.0);
            if ui.button(S::DLG_CLOSE).clicked() {
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
        egui::Modal::new(egui::Id::new("export")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.label(
                RichText::new(S::EXPORT_TITLE)
                    .font(crate::theme::medium(16.0))
                    .color(pal.text_strong),
            );
            ui.label(
                RichText::new(if self.sel.objects.is_empty() {
                    S::EXPORT_SCOPE_ALL
                } else {
                    S::EXPORT_SCOPE_SEL
                })
                .small()
                .color(pal.text_dim),
            );
            ui.add_space(8.0);
            egui::Grid::new("export-opts")
                .num_columns(2)
                .spacing(vec2(16.0, 8.0))
                .show(ui, |ui| {
                    ui.label(S::EXPORT_FORMAT);
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut d.format, Format::Fbx, "FBX");
                        ui.selectable_value(&mut d.format, Format::Obj, "OBJ");
                        ui.selectable_value(&mut d.format, Format::Glb, "glTF (.glb)");
                    });
                    ui.end_row();
                    ui.label(S::EXPORT_TRIANGULATE);
                    if d.format == Format::Glb {
                        ui.label(RichText::new(S::EXPORT_GLB_TRI).small().color(pal.text_dim));
                    } else {
                        ui.checkbox(&mut d.opts.triangulate, "");
                    }
                    ui.end_row();
                    ui.label(S::EXPORT_UNITS);
                    ui.horizontal(|ui| {
                        for u in ["mm", "cm", "m", "in"] {
                            ui.selectable_value(&mut d.units, u.to_string(), u);
                        }
                    });
                    ui.end_row();
                    ui.label(S::EXPORT_SCALE);
                    ui.add(
                        egui::DragValue::new(&mut d.opts.scale)
                            .speed(0.01)
                            .range(0.0001..=10000.0),
                    );
                    ui.end_row();
                    ui.label(S::EXPORT_UP);
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut d.opts.up, ExportUp::Y, "Y");
                        ui.selectable_value(&mut d.opts.up, ExportUp::Z, "Z");
                    });
                    ui.end_row();
                    ui.label(S::EXPORT_NORMALS);
                    ui.horizontal(|ui| {
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
                    ui.label("");
                    ui.checkbox(&mut d.opts.layers_as_hierarchy, S::EXPORT_LAYERS);
                    ui.end_row();
                    ui.label("");
                    ui.checkbox(&mut d.opts.materials, S::EXPORT_MATERIALS);
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
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Button::new(RichText::new(S::EXPORT_RUN).color(pal.accent_text))
                            .fill(pal.accent),
                    )
                    .clicked()
                {
                    go = true;
                }
                if ui.button(S::DLG_CANCEL).clicked() {
                    close = true;
                }
            });
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
