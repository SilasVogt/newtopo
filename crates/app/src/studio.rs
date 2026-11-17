//! Studio panels (SPEC §9): batch rename (F1) on `core::rename`, bridge (F2)
//! on `core::bridge`, quick UV (F3, options only for now).

use crate::app::AppState;
use crate::strings as S;
use crate::toasts::ToastKind;
use egui::{RichText, Ui, vec2};
use newtopo_core::bridge::{self, BridgeStatus, Host};
use newtopo_core::ops::Op;
use newtopo_core::rename::{self, Placement, RenameOp, Side};
use newtopo_kernel::BodyId;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------- rename

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RenameKind {
    #[default]
    Set,
    Add,
    Remove,
    Replace,
    Number,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenameState {
    pub kind: RenameKind,
    pub set_name: String,
    pub add_text: String,
    pub add_suffix: bool,
    pub remove_n: usize,
    pub remove_start: bool,
    pub remove_end: bool,
    pub find: String,
    pub with: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub sep: String,
    pub start: i64,
    pub step: i64,
    pub pad: usize,
    pub strip: bool,
}

impl Default for RenameState {
    fn default() -> Self {
        RenameState {
            kind: RenameKind::Set,
            set_name: String::new(),
            add_text: String::new(),
            add_suffix: true,
            remove_n: 1,
            remove_start: false,
            remove_end: true,
            find: String::new(),
            with: String::new(),
            regex: false,
            case_sensitive: true,
            sep: "_".into(),
            start: 1,
            step: 1,
            pad: 2,
            strip: false,
        }
    }
}

impl RenameState {
    /// The core operation for the current settings (`None` = no change).
    pub fn op(&self) -> Option<RenameOp> {
        Some(match self.kind {
            RenameKind::Set if self.set_name.is_empty() => return None,
            RenameKind::Set => RenameOp::Set {
                name: self.set_name.clone(),
            },
            RenameKind::Add => RenameOp::Add {
                text: self.add_text.clone(),
                at: if self.add_suffix {
                    Placement::Suffix
                } else {
                    Placement::Prefix
                },
            },
            RenameKind::Remove => RenameOp::Remove {
                count: self.remove_n,
                from: match (self.remove_start, self.remove_end) {
                    (true, true) => Side::Both,
                    (true, false) => Side::Start,
                    (false, true) => Side::End,
                    (false, false) => return None,
                },
            },
            RenameKind::Replace if self.find.is_empty() => return None,
            RenameKind::Replace => RenameOp::Replace {
                find: self.find.clone(),
                with: self.with.clone(),
                regex: self.regex,
                case_sensitive: self.case_sensitive,
            },
            RenameKind::Number if self.strip => RenameOp::RemoveNumbering {
                separator: self.sep.clone(),
            },
            RenameKind::Number => RenameOp::Number {
                separator: self.sep.clone(),
                start: self.start,
                step: self.step,
                pad: self.pad,
            },
        })
    }
}

// ---------------------------------------------------------------- bridge

/// Bridge panel state: statuses come from a background probe so the UI
/// never waits on sockets.
#[derive(Default)]
pub struct BridgeUi {
    status: Arc<Mutex<[Option<BridgeStatus>; 3]>>,
    probing: Arc<std::sync::atomic::AtomicBool>,
    last_probe: f64,
    send: Option<(Host, Receiver<Result<bridge::Ack, String>>)>,
}

fn host_index(h: Host) -> usize {
    match h {
        Host::Blender => 0,
        Host::Maya => 1,
        Host::Max => 2,
    }
}

impl AppState {
    pub fn rename_ui(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        let rows = self.body_rows();
        let selected: Vec<(BodyId, String)> = rows
            .iter()
            .filter(|b| self.sel.objects.contains(&b.id))
            .map(|b| (b.id, b.name.clone()))
            .collect();
        let others: Vec<String> = rows
            .iter()
            .filter(|b| !self.sel.objects.contains(&b.id))
            .map(|b| b.name.clone())
            .collect();
        let r = &mut self.ui.rename;
        ui.horizontal_wrapped(|ui| {
            for (k, l) in [
                (RenameKind::Set, S::RENAME_SET),
                (RenameKind::Add, S::RENAME_ADD),
                (RenameKind::Remove, S::RENAME_REMOVE),
                (RenameKind::Replace, S::RENAME_REPLACE),
                (RenameKind::Number, S::RENAME_NUMBER),
            ] {
                ui.selectable_value(&mut r.kind, k, l);
            }
        });
        ui.add_space(6.0);
        let active_name = selected.first().map(|s| s.1.clone());
        egui::Grid::new("rename-opts")
            .num_columns(2)
            .spacing(vec2(10.0, 6.0))
            .show(ui, |ui| match r.kind {
                RenameKind::Set => {
                    ui.label(S::RENAME_SET);
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut r.set_name).desired_width(140.0));
                        if ui
                            .add_enabled(
                                active_name.is_some(),
                                egui::Button::new(S::RENAME_COPY_ACTIVE),
                            )
                            .clicked()
                            && let Some(RenameOp::Set { name }) = active_name
                                .as_ref()
                                .and_then(|n| RenameOp::copy_active(std::slice::from_ref(n), 0))
                        {
                            r.set_name = name;
                        }
                    });
                    ui.end_row();
                }
                RenameKind::Add => {
                    ui.label(S::RENAME_ADD);
                    ui.add(egui::TextEdit::singleline(&mut r.add_text).desired_width(160.0));
                    ui.end_row();
                    ui.label("");
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut r.add_suffix, false, S::RENAME_PREFIX);
                        ui.selectable_value(&mut r.add_suffix, true, S::RENAME_SUFFIX);
                    });
                    ui.end_row();
                }
                RenameKind::Remove => {
                    ui.label(S::RENAME_REMOVE);
                    ui.add(egui::DragValue::new(&mut r.remove_n).range(0..=64));
                    ui.end_row();
                    ui.label("");
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut r.remove_start, S::RENAME_FROM_START);
                        ui.checkbox(&mut r.remove_end, S::RENAME_FROM_END);
                    });
                    ui.end_row();
                }
                RenameKind::Replace => {
                    ui.label(S::RENAME_FIND);
                    ui.add(egui::TextEdit::singleline(&mut r.find).desired_width(160.0));
                    ui.end_row();
                    ui.label(S::RENAME_WITH);
                    ui.add(egui::TextEdit::singleline(&mut r.with).desired_width(160.0));
                    ui.end_row();
                    ui.label("");
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut r.regex, S::RENAME_REGEX)
                            .on_hover_text(S::RENAME_REGEX_HINT);
                        ui.checkbox(&mut r.case_sensitive, S::RENAME_CASE);
                    });
                    ui.end_row();
                }
                RenameKind::Number => {
                    ui.label(S::RENAME_SEPARATOR);
                    ui.add(egui::TextEdit::singleline(&mut r.sep).desired_width(40.0));
                    ui.end_row();
                    ui.label(S::RENAME_START);
                    ui.add_enabled(!r.strip, egui::DragValue::new(&mut r.start));
                    ui.end_row();
                    ui.label(S::RENAME_STEP);
                    ui.add_enabled(!r.strip, egui::DragValue::new(&mut r.step).range(1..=1000));
                    ui.end_row();
                    ui.label(S::RENAME_PADDING);
                    ui.add_enabled(!r.strip, egui::DragValue::new(&mut r.pad).range(0..=8));
                    ui.end_row();
                    ui.label("");
                    ui.checkbox(&mut r.strip, S::RENAME_STRIP_NUMBER);
                    ui.end_row();
                }
            });
        ui.add_space(8.0);
        ui.label(
            RichText::new(S::RENAME_PREVIEW.to_uppercase())
                .small()
                .color(pal.text_dim),
        );
        if selected.is_empty() {
            ui.label(RichText::new(S::RENAME_NOTHING).color(pal.text_dim));
            return;
        }
        let old: Vec<String> = selected.iter().map(|s| s.1.clone()).collect();
        let entries = match self.ui.rename.op() {
            None => rename::preview(
                &old,
                &RenameOp::Set {
                    name: String::new(),
                },
                &others,
            )
            .map(|v| {
                v.into_iter()
                    .zip(&old)
                    .map(|(mut e, o)| {
                        e.new = o.clone();
                        e.collision = false;
                        e
                    })
                    .collect()
            }),
            Some(op) => rename::preview(&old, &op, &others),
        };
        let entries = match entries {
            Ok(e) => e,
            Err(e) => {
                ui.label(RichText::new(e.to_string()).color(pal.error));
                return;
            }
        };
        egui::ScrollArea::vertical()
            .max_height(220.0)
            .show(ui, |ui| {
                egui::Grid::new("rename-preview")
                    .num_columns(3)
                    .spacing(vec2(8.0, 3.0))
                    .show(ui, |ui| {
                        for e in &entries {
                            ui.label(RichText::new(&e.old).color(pal.text_dim));
                            ui.label(RichText::new("›").color(pal.text_dim));
                            let col = if e.collision {
                                pal.error
                            } else if e.changed() {
                                pal.text_strong
                            } else {
                                pal.text
                            };
                            let t = ui.label(RichText::new(&e.new).color(col));
                            if e.collision {
                                t.on_hover_text(S::RENAME_COLLISION);
                            }
                            ui.end_row();
                        }
                    });
            });
        ui.add_space(6.0);
        let changed = entries.iter().filter(|e| e.changed()).count();
        if ui
            .add_enabled(
                changed > 0,
                egui::Button::new(format!("{}  ({changed})", S::DLG_APPLY)),
            )
            .clicked()
        {
            if entries.iter().any(|e| e.collision) {
                self.toast(ToastKind::Warn, S::RENAME_COLLISION);
            }
            if self.doc.has_scene() {
                // One undo step for the whole batch (SPEC §9.1).
                let ops: Vec<Op> = selected
                    .iter()
                    .zip(&entries)
                    .filter(|(_, e)| e.changed())
                    .map(|((id, _), e)| Op::RenameBody {
                        body: *id,
                        name: e.new.clone(),
                    })
                    .collect();
                self.apply_group(ops);
            } else if let Some(demo) = &mut self.demo {
                for ((id, _), e) in selected.iter().zip(&entries) {
                    if let Some(b) = demo.bodies.iter_mut().find(|b| b.id == id.0) {
                        b.name = e.new.clone();
                    }
                }
            }
        }
    }

    fn bridge_poll(&mut self, ctx: &egui::Context) {
        use std::sync::atomic::Ordering;
        // Probe every few seconds while the panel is visible.
        if self.time - self.ui.bridge.last_probe > 3.0
            && !self.ui.bridge.probing.load(Ordering::Relaxed)
        {
            self.ui.bridge.last_probe = self.time;
            self.ui.bridge.probing.store(true, Ordering::Relaxed);
            let status = self.ui.bridge.status.clone();
            let probing = self.ui.bridge.probing.clone();
            let ports = self.prefs.bridge.ports;
            let token = self.prefs.bridge.token.clone();
            let ctx = ctx.clone();
            let _ = std::thread::Builder::new()
                .name("bridge-probe".into())
                .spawn(move || {
                    let r: Vec<BridgeStatus> = Host::ALL
                        .iter()
                        .map(|h| bridge::probe(ports[host_index(*h)], &token))
                        .collect();
                    if let Ok(mut s) = status.lock() {
                        for (i, st) in r.into_iter().enumerate() {
                            s[i] = Some(st);
                        }
                    }
                    probing.store(false, Ordering::Relaxed);
                    ctx.request_repaint();
                });
        }
        ctx.request_repaint_after(std::time::Duration::from_secs(3));
        if let Some((host, rx)) = &self.ui.bridge.send {
            let host = *host;
            match rx.try_recv() {
                Ok(Ok(ack)) => {
                    self.ui.bridge.send = None;
                    let renamed: Vec<String> = ack
                        .renamed()
                        .map(|o| format!("{} → {}", o.name, o.actual))
                        .collect();
                    let msg = if renamed.is_empty() {
                        format!("Sent {} object(s) to {}", ack.objects.len(), host.label())
                    } else {
                        format!(
                            "Sent to {}; renamed by the host: {}",
                            host.label(),
                            renamed.join(", ")
                        )
                    };
                    self.toast(ToastKind::Info, msg);
                }
                Ok(Err(e)) => {
                    self.ui.bridge.send = None;
                    self.toast(ToastKind::Error, e);
                }
                Err(TryRecvError::Disconnected) => self.ui.bridge.send = None,
                Err(TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100))
                }
            }
        }
    }

    fn bridge_send(&mut self, host: Host) {
        let Some(items) = self.export_items() else {
            self.toast(ToastKind::Info, S::TOAST_NO_SCENE);
            return;
        };
        if items.is_empty() {
            self.toast(
                ToastKind::Warn,
                "Nothing to send: the output mesh is empty.",
            );
            return;
        }
        let port = self.prefs.bridge.ports[host_index(host)];
        let token = self.prefs.bridge.token.clone();
        let opts = bridge::SendOptions {
            // Scene units → metres in the host.
            scale: crate::jobs::unit_mm(&self.prefs.units) / 1000.0,
            keep_both: self.prefs.bridge.keep_both,
            ..Default::default()
        };
        let (tx, rx) = channel();
        let _ = std::thread::Builder::new()
            .name("bridge-send".into())
            .spawn(move || {
                let objs: Vec<bridge::BridgeObject> = items
                    .iter()
                    .map(|(n, l, c, t, m)| {
                        newtopo_core::export::ExportObject {
                            name: n,
                            layer: l,
                            color: *c,
                            transform: *t,
                            mesh: m,
                        }
                        .into()
                    })
                    .collect();
                let r = bridge::send_with(port, &token, &objs, &opts).map_err(|e| e.to_string());
                let _ = tx.send(r);
            });
        self.ui.bridge.send = Some((host, rx));
    }

    pub fn bridge_ui(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        let ctx = ui.ctx().clone();
        self.bridge_poll(&ctx);
        let statuses = self
            .ui
            .bridge
            .status
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default();
        let sending = self.ui.bridge.send.as_ref().map(|(h, _)| *h);
        let mut send: Option<Host> = None;
        egui::Grid::new("bridge-hosts")
            .num_columns(4)
            .spacing(vec2(10.0, 8.0))
            .show(ui, |ui| {
                for h in Host::ALL {
                    let i = host_index(h);
                    let (dot, text) = match &statuses[i] {
                        Some(BridgeStatus::Ready {
                            host,
                            addon_version,
                        }) => (
                            pal.ok,
                            format!(
                                "Ready · {} {addon_version}",
                                if host.is_empty() { h.label() } else { host }
                            ),
                        ),
                        Some(BridgeStatus::Offline) => {
                            (pal.text_dim, S::BRIDGE_DISCONNECTED.to_string())
                        }
                        Some(BridgeStatus::BadToken) => (pal.warn, S::BRIDGE_BAD_TOKEN.to_string()),
                        Some(BridgeStatus::Incompatible { version }) => {
                            (pal.warn, format!("Add-on protocol v{version}: update it"))
                        }
                        Some(BridgeStatus::Error(e)) => (pal.error, e.clone()),
                        None => (pal.text_dim.gamma_multiply(0.6), "Checking…".to_string()),
                    };
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), egui::Sense::hover());
                        ui.painter().circle_filled(r.center(), 4.0, dot);
                        ui.label(RichText::new(h.label()).font(crate::theme::medium(13.0)));
                    });
                    ui.label(RichText::new(text).small().color(pal.text_dim));
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(S::BRIDGE_PORT).small().color(pal.text_dim));
                        if ui
                            .add(
                                egui::DragValue::new(&mut self.prefs.bridge.ports[i])
                                    .range(1024..=65535),
                            )
                            .changed()
                        {
                            self.dirty_settings = true;
                            self.ui.bridge.last_probe = 0.0;
                        }
                    });
                    let ready = matches!(statuses[i], Some(BridgeStatus::Ready { .. }));
                    if sending == Some(h) {
                        ui.spinner();
                    } else if ui
                        .add_enabled(
                            ready && sending.is_none() && self.doc.has_scene(),
                            egui::Button::new(S::BRIDGE_SEND),
                        )
                        .clicked()
                    {
                        send = Some(h);
                    }
                    ui.end_row();
                }
            });
        if let Some(h) = send {
            self.bridge_send(h);
        }
        ui.add_space(6.0);
        if ui
            .checkbox(&mut self.prefs.bridge.keep_both, S::BRIDGE_KEEP_BOTH)
            .changed()
        {
            self.dirty_settings = true;
        }
        ui.label(RichText::new(S::BRIDGE_SCOPE).small().color(pal.text_dim));
        ui.add_space(10.0);
        ui.label(
            RichText::new(S::BRIDGE_TOKEN.to_uppercase())
                .small()
                .color(pal.text_dim),
        );
        ui.horizontal(|ui| {
            let mut t = self.prefs.bridge.token.clone();
            ui.add(
                egui::TextEdit::singleline(&mut t)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(260.0)
                    .interactive(false),
            );
            if ui.button("Copy").clicked() {
                ui.ctx().copy_text(self.prefs.bridge.token.clone());
                self.toast(ToastKind::Info, S::TOAST_COPIED);
            }
            if ui
                .button("New")
                .on_hover_text("Generate a new token (add-ons must be paired again)")
                .clicked()
            {
                self.prefs.bridge.token = bridge::generate_token();
                self.dirty_settings = true;
                self.ui.bridge.last_probe = 0.0;
            }
        });
        ui.label(
            RichText::new(S::BRIDGE_TOKEN_HINT)
                .small()
                .color(pal.text_dim),
        );
    }
}

// ---------------------------------------------------------------- uv

#[derive(Clone, Debug, PartialEq)]
pub struct UvOptions {
    pub padding: u32,
    pub tex_size: u32,
    pub rotation: bool,
    pub uniform_texel: bool,
    pub checker: bool,
    pub tiles: u32,
}

impl Default for UvOptions {
    fn default() -> Self {
        UvOptions {
            padding: 4,
            tex_size: 2048,
            rotation: true,
            uniform_texel: true,
            checker: false,
            tiles: 16,
        }
    }
}

impl AppState {
    pub fn uv_ui(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        ui.label(
            RichText::new(S::UV_TITLE)
                .font(crate::theme::medium(14.0))
                .color(pal.text_strong),
        );
        egui::Frame::new()
            .fill(pal.accent.gamma_multiply(0.10))
            .corner_radius(6)
            .inner_margin(8)
            .show(ui, |ui| {
                ui.label(RichText::new(S::UV_COMING_SOON).color(pal.accent));
            });
        ui.add_space(6.0);
        let o = &mut self.ui.uv;
        egui::Grid::new("uv-opts")
            .num_columns(2)
            .spacing(vec2(10.0, 6.0))
            .show(ui, |ui| {
                ui.label(S::UV_PADDING);
                ui.add(egui::DragValue::new(&mut o.padding).range(0..=64));
                ui.end_row();
                ui.label(S::UV_TEX_SIZE);
                egui::ComboBox::from_id_salt("uv-size")
                    .selected_text(format!("{}", o.tex_size))
                    .show_ui(ui, |ui| {
                        for s in [512, 1024, 2048, 4096, 8192] {
                            ui.selectable_value(&mut o.tex_size, s, s.to_string());
                        }
                    });
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut o.rotation, S::UV_ROTATION);
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut o.uniform_texel, S::UV_TEXEL);
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut o.checker, S::UV_CHECKER);
                ui.end_row();
                ui.label(S::UV_TILES);
                ui.add_enabled(o.checker, egui::DragValue::new(&mut o.tiles).range(1..=64));
                ui.end_row();
            });
        ui.add_space(8.0);
        ui.add_enabled(
            false,
            egui::Button::new(S::UV_RUN).min_size(vec2(120.0, 28.0)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn run(r: &RenameState, n: &[String]) -> Vec<String> {
        rename::apply(n, &r.op().expect("op")).unwrap()
    }

    #[test]
    fn panel_settings_map_to_core_ops() {
        let n = names(&["Body", "Bolt_03", "Nut"]);
        let mut r = RenameState {
            kind: RenameKind::Add,
            add_text: "_LP".into(),
            ..Default::default()
        };
        assert_eq!(run(&r, &n), names(&["Body_LP", "Bolt_03_LP", "Nut_LP"]));
        r = RenameState {
            kind: RenameKind::Remove,
            remove_n: 2,
            remove_start: true,
            remove_end: false,
            ..Default::default()
        };
        assert_eq!(run(&r, &n), names(&["dy", "lt_03", "t"]));
        r = RenameState {
            kind: RenameKind::Replace,
            find: r"_(\d+)".into(),
            with: "-$1".into(),
            regex: true,
            ..Default::default()
        };
        assert_eq!(run(&r, &n), names(&["Body", "Bolt-03", "Nut"]));
        r = RenameState {
            kind: RenameKind::Number,
            ..Default::default()
        };
        assert_eq!(run(&r, &n), names(&["Body_01", "Bolt_03_02", "Nut_03"]));
        r.strip = true;
        assert_eq!(run(&r, &n), names(&["Body", "Bolt", "Nut"]));
        // Nothing to do → no op.
        assert!(
            RenameState {
                kind: RenameKind::Set,
                ..Default::default()
            }
            .op()
            .is_none()
        );
        assert!(
            RenameState {
                kind: RenameKind::Remove,
                remove_start: false,
                remove_end: false,
                ..Default::default()
            }
            .op()
            .is_none()
        );
    }
}
