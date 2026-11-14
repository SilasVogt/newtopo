//! Studio panels (SPEC §9): batch rename (F1), bridge (F2), quick UV (F3).

use crate::app::AppState;
use crate::strings as S;
use crate::toasts::ToastKind;
use egui::{RichText, Ui, vec2};
use newtopo_core::ops::Op;
use newtopo_kernel::BodyId;
use std::collections::HashMap;

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
    pub sep: String,
    pub start: u32,
    pub step: u32,
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
            sep: "_".into(),
            start: 1,
            step: 1,
            pad: 2,
            strip: false,
        }
    }
}

/// Strip a trailing number (and the separator before it).
pub fn strip_number(name: &str, sep: &str) -> String {
    let trimmed = name.trim_end_matches(|c: char| c.is_ascii_digit());
    if trimmed.len() == name.len() {
        return name.to_string();
    }
    trimmed
        .strip_suffix(sep)
        .filter(|_| !sep.is_empty())
        .unwrap_or(trimmed)
        .to_string()
}

/// New names for `names` under the rename settings (SPEC §9.1).
pub fn rename_preview(r: &RenameState, names: &[String]) -> Vec<String> {
    names
        .iter()
        .enumerate()
        .map(|(i, n)| match r.kind {
            RenameKind::Set => {
                if r.set_name.is_empty() {
                    n.clone()
                } else {
                    r.set_name.clone()
                }
            }
            RenameKind::Add => {
                if r.add_suffix {
                    format!("{n}{}", r.add_text)
                } else {
                    format!("{}{n}", r.add_text)
                }
            }
            RenameKind::Remove => {
                let chars: Vec<char> = n.chars().collect();
                let mut lo = 0;
                let mut hi = chars.len();
                if r.remove_start {
                    lo = r.remove_n.min(hi);
                }
                if r.remove_end {
                    hi = hi.saturating_sub(r.remove_n).max(lo);
                }
                chars[lo..hi].iter().collect()
            }
            RenameKind::Replace => {
                if r.find.is_empty() {
                    n.clone()
                } else {
                    n.replace(&r.find, &r.with)
                }
            }
            RenameKind::Number => {
                let base = strip_number(n, &r.sep);
                if r.strip {
                    base
                } else {
                    let num = r.start as u64 + i as u64 * r.step as u64;
                    format!("{base}{}{:0width$}", r.sep, num, width = r.pad)
                }
            }
        })
        .collect()
}

/// For each new name: does it collide with another new name or an
/// existing name outside the selection (or end up empty)?
pub fn collisions(new: &[String], others: &[String]) -> Vec<bool> {
    let mut count: HashMap<&str, usize> = HashMap::new();
    for n in new {
        *count.entry(n.as_str()).or_default() += 1;
    }
    new.iter()
        .map(|n| n.trim().is_empty() || count[n.as_str()] > 1 || others.iter().any(|o| o == n))
        .collect()
}

// ---------------------------------------------------------------- bridge adapter

/// Thin adapter so the lead can wire `core::bridge` once it exists
/// (SPEC §9.3). Everything here reports "unavailable" until then.
pub mod bridge_adapter {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Host {
        Blender,
        Maya,
        Max,
    }

    impl Host {
        pub const ALL: [Host; 3] = [Host::Blender, Host::Maya, Host::Max];
        pub fn label(self) -> &'static str {
            match self {
                Host::Blender => "Blender",
                Host::Maya => "Maya",
                Host::Max => "3ds Max",
            }
        }
        pub fn index(self) -> usize {
            self as usize
        }
    }

    /// Connection state of one host. `Disconnected`/`Connected` are produced
    /// once `core::bridge` is wired in.
    #[allow(dead_code)]
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Status {
        Unavailable,
        Disconnected,
        Connected { version: String },
    }

    pub fn available() -> bool {
        false
    }

    pub fn status(_host: Host, _port: u16) -> Status {
        Status::Unavailable
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
                        {
                            r.set_name = active_name.clone().unwrap_or_default();
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
                }
                RenameKind::Number => {
                    ui.label(S::RENAME_SEPARATOR);
                    ui.add(egui::TextEdit::singleline(&mut r.sep).desired_width(40.0));
                    ui.end_row();
                    ui.label(S::RENAME_START);
                    ui.add(egui::DragValue::new(&mut r.start));
                    ui.end_row();
                    ui.label(S::RENAME_STEP);
                    ui.add(egui::DragValue::new(&mut r.step).range(1..=1000));
                    ui.end_row();
                    ui.label(S::RENAME_PADDING);
                    ui.add(egui::DragValue::new(&mut r.pad).range(0..=8));
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
        let new = rename_preview(&self.ui.rename, &old);
        let bad = collisions(&new, &others);
        egui::ScrollArea::vertical()
            .max_height(220.0)
            .show(ui, |ui| {
                egui::Grid::new("rename-preview")
                    .num_columns(3)
                    .spacing(vec2(8.0, 3.0))
                    .show(ui, |ui| {
                        for ((o, n), b) in old.iter().zip(&new).zip(&bad) {
                            ui.label(RichText::new(o).color(pal.text_dim));
                            ui.label(RichText::new("›").color(pal.text_dim));
                            let col = if *b {
                                pal.error
                            } else if o != n {
                                pal.text_strong
                            } else {
                                pal.text
                            };
                            let t = ui.label(RichText::new(n).color(col));
                            if *b {
                                t.on_hover_text(S::RENAME_COLLISION);
                            }
                            ui.end_row();
                        }
                    });
            });
        ui.add_space(6.0);
        let changed = old.iter().zip(&new).filter(|(o, n)| o != n).count();
        if ui
            .add_enabled(
                changed > 0,
                egui::Button::new(format!("{}  ({changed})", S::DLG_APPLY)),
            )
            .clicked()
        {
            if bad.iter().any(|b| *b) {
                self.toast(ToastKind::Warn, S::RENAME_COLLISION);
            }
            // One RenameBody op per object (see docs/notes/app.md: batch op requested).
            for ((id, o), n) in selected.iter().zip(&new) {
                if o != n {
                    if self.doc.has_scene() {
                        self.apply(Op::RenameBody {
                            body: *id,
                            name: n.clone(),
                        });
                    } else if let Some(demo) = &mut self.demo
                        && let Some(b) = demo.bodies.iter_mut().find(|b| b.id == id.0)
                    {
                        b.name = n.clone();
                    }
                }
            }
        }
    }

    pub fn bridge_ui(&mut self, ui: &mut Ui) {
        use bridge_adapter::{Host, Status};
        let pal = self.pal;
        if !bridge_adapter::available() {
            egui::Frame::new()
                .fill(pal.warn.gamma_multiply(0.12))
                .corner_radius(6)
                .inner_margin(8)
                .show(ui, |ui| {
                    ui.label(RichText::new(S::BRIDGE_UNAVAILABLE).color(pal.warn));
                });
            ui.add_space(6.0);
        }
        egui::Grid::new("bridge-hosts")
            .num_columns(4)
            .spacing(vec2(10.0, 8.0))
            .show(ui, |ui| {
                for h in Host::ALL {
                    let port = &mut self.prefs.bridge.ports[h.index()];
                    let status = bridge_adapter::status(h, *port);
                    let (dot, text) = match &status {
                        Status::Connected { version } => (pal.ok, format!("Connected · {version}")),
                        Status::Disconnected => (pal.text_dim, S::BRIDGE_DISCONNECTED.to_string()),
                        Status::Unavailable => (
                            pal.text_dim.gamma_multiply(0.6),
                            S::BRIDGE_DISCONNECTED.to_string(),
                        ),
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
                            .add(egui::DragValue::new(port).range(1024..=65535))
                            .changed()
                        {
                            self.dirty_settings = true;
                        }
                    });
                    let connected = matches!(status, Status::Connected { .. });
                    if ui
                        .add_enabled(connected, egui::Button::new(S::BRIDGE_SEND))
                        .clicked()
                    {
                        self.toast(ToastKind::Warn, S::BRIDGE_UNAVAILABLE);
                    }
                    ui.end_row();
                }
            });
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
                .on_hover_text("Generate a new token (addons must be re-paired)")
                .clicked()
            {
                self.prefs.bridge.token = crate::prefs::random_token();
                self.dirty_settings = true;
            }
        });
        ui.label(
            RichText::new(S::BRIDGE_TOKEN_HINT)
                .small()
                .color(pal.text_dim),
        );
    }

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

    #[test]
    fn rename_ops() {
        let n = names(&["Body", "Bolt_03", "Nut"]);
        let mut r = RenameState {
            kind: RenameKind::Add,
            add_text: "_LP".into(),
            add_suffix: true,
            ..Default::default()
        };
        assert_eq!(
            rename_preview(&r, &n),
            names(&["Body_LP", "Bolt_03_LP", "Nut_LP"])
        );
        r.add_suffix = false;
        r.add_text = "SM_".into();
        assert_eq!(rename_preview(&r, &n)[0], "SM_Body");
        r = RenameState {
            kind: RenameKind::Remove,
            remove_n: 2,
            remove_start: true,
            remove_end: false,
            ..Default::default()
        };
        assert_eq!(rename_preview(&r, &n), names(&["dy", "lt_03", "t"]));
        r = RenameState {
            kind: RenameKind::Replace,
            find: "o".into(),
            with: "0".into(),
            ..Default::default()
        };
        assert_eq!(rename_preview(&r, &n), names(&["B0dy", "B0lt_03", "Nut"]));
        r = RenameState {
            kind: RenameKind::Number,
            sep: "_".into(),
            start: 1,
            step: 1,
            pad: 2,
            ..Default::default()
        };
        assert_eq!(
            rename_preview(&r, &n),
            names(&["Body_01", "Bolt_02", "Nut_03"])
        );
        r.strip = true;
        assert_eq!(rename_preview(&r, &n), names(&["Body", "Bolt", "Nut"]));
    }

    #[test]
    fn collisions_flagged() {
        let new = names(&["A", "A", "B", ""]);
        assert_eq!(
            collisions(&new, &names(&["B"])),
            vec![true, true, true, true]
        );
        assert_eq!(
            collisions(&names(&["X", "Y"]), &names(&["Z"])),
            vec![false, false]
        );
    }

    #[test]
    fn strip() {
        assert_eq!(strip_number("Part_012", "_"), "Part");
        assert_eq!(strip_number("Part12", "_"), "Part");
        assert_eq!(strip_number("Part", "_"), "Part");
    }
}
