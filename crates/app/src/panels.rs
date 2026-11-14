//! Docked panels: outliner (SPEC §5.5), statistics (§5.6), history (§5.1)
//! and the tool panel (§6.1).

use crate::app::AppState;
use crate::commands::Cmd;
use crate::docstate::guard;
use crate::icons::{self, Icon};
use crate::prefs::StatsShow;
use crate::selection::{Combine, Mode, combine};
use crate::strings as S;
use crate::toasts::ToastKind;
use crate::tools::{IsoDirSel, Tool};
use egui::{Color32, RichText, Sense, Ui, vec2};
use newtopo_core::doc::{Edit, LayerId, Rule, SliceScope, Spacing};
use newtopo_core::mesh::MeshStats;
use newtopo_core::ops::Op;
use newtopo_kernel::{BodyId, FaceId};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug)]
pub struct LayerRow {
    pub id: LayerId,
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    pub active: bool,
}

#[derive(Clone, Debug)]
pub struct BodyRow {
    pub id: BodyId,
    pub name: String,
    pub layer: LayerId,
    pub visible: bool,
    pub locked: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PaintColumn {
    LayerEye,
    LayerLock,
    BodyEye,
    BodyLock,
}

#[derive(Default)]
pub struct OutlinerState {
    pub filter: String,
    pub collapsed: BTreeSet<LayerId>,
    pub renaming: Option<(RenameTarget, String)>,
    paint: Option<(PaintColumn, bool)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenameTarget {
    Layer(LayerId),
    Body(BodyId),
}

impl AppState {
    pub fn layer_rows(&self) -> Vec<LayerRow> {
        if let Some(scene) = &self.doc.scene {
            let Ok((layers, active)) = guard(|| (scene.doc().layers.clone(), scene.doc().active_layer)) else { return vec![] };
            return layers
                .into_iter()
                .map(|l| LayerRow {
                    visible: l.visible && !self.local.layer_hidden.contains(&l.id),
                    locked: l.locked || self.local.layer_locked.contains(&l.id),
                    active: l.id == active,
                    id: l.id,
                    name: l.name,
                })
                .collect();
        }
        if let Some(demo) = &self.demo {
            return demo
                .layers
                .iter()
                .map(|(n, id)| LayerRow {
                    id: *id,
                    name: n.clone(),
                    visible: !self.local.layer_hidden.contains(id),
                    locked: self.local.layer_locked.contains(id),
                    active: id.0 == 0,
                })
                .collect();
        }
        vec![]
    }

    pub fn body_rows(&self) -> Vec<BodyRow> {
        if let Some(scene) = &self.doc.scene {
            let Ok(bodies) = guard(|| scene.doc().bodies.clone()) else { return vec![] };
            return bodies
                .into_iter()
                .map(|b| BodyRow {
                    visible: b.visible && !self.view.hidden.contains(&b.id),
                    locked: b.locked || self.local.body_locked.contains(&b.id),
                    id: b.id,
                    name: b.name,
                    layer: b.layer,
                })
                .collect();
        }
        if let Some(demo) = &self.demo {
            return demo
                .bodies
                .iter()
                .map(|b| {
                    let id = BodyId(b.id);
                    BodyRow {
                        id,
                        name: b.name.clone(),
                        layer: demo.layers.iter().find(|(n, _)| *n == b.layer).map(|l| l.1).unwrap_or_default(),
                        visible: !self.view.hidden.contains(&id) && !self.local.body_hidden_demo.contains(&id),
                        locked: self.local.body_locked.contains(&id),
                    }
                })
                .collect();
        }
        vec![]
    }

    fn set_body_visible(&mut self, id: BodyId, visible: bool) {
        if self.doc.has_scene() {
            if visible {
                self.view.hidden.remove(&id);
            }
            self.apply(Op::SetBodyVisible { body: id, visible });
        } else if visible {
            self.view.hidden.remove(&id);
            self.local.body_hidden_demo.remove(&id);
        } else {
            self.local.body_hidden_demo.insert(id);
        }
    }

    fn rename_body(&mut self, id: BodyId, name: String) {
        if self.doc.has_scene() {
            self.apply(Op::RenameBody { body: id, name });
        } else if let Some(demo) = &mut self.demo
            && let Some(b) = demo.bodies.iter_mut().find(|b| b.id == id.0)
        {
            b.name = name;
            self.invalidate_view();
        }
    }

    fn rename_layer(&mut self, id: LayerId, name: String) {
        if let Some(demo) = &mut self.demo
            && !self.doc.has_scene()
        {
            if let Some(l) = demo.layers.iter_mut().find(|l| l.1 == id) {
                let old = std::mem::replace(&mut l.0, name.clone());
                for b in demo.bodies.iter_mut().filter(|b| b.layer == old) {
                    b.layer = name.clone();
                }
            }
            return;
        }
        self.toast(ToastKind::Warn, format!("{}: rename layer", S::TOAST_NOT_YET));
    }

    fn create_layer(&mut self) {
        if let Some(demo) = &mut self.demo
            && !self.doc.has_scene()
        {
            let id = LayerId(demo.layers.iter().map(|l| l.1.0 + 1).max().unwrap_or(0));
            demo.layers.push((format!("Layer {}", id.0 + 1), id));
            return;
        }
        self.toast(ToastKind::Warn, format!("{}: create layer", S::TOAST_NOT_YET));
    }

    fn delete_layer(&mut self, id: LayerId) {
        if let Some(demo) = &mut self.demo
            && !self.doc.has_scene()
        {
            if demo.layers.len() <= 1 {
                return;
            }
            let Some(pos) = demo.layers.iter().position(|l| l.1 == id) else { return };
            let (old, _) = demo.layers.remove(pos);
            let target = demo.layers[0].0.clone();
            for b in demo.bodies.iter_mut().filter(|b| b.layer == old) {
                b.layer = target.clone();
            }
            self.invalidate_view();
            return;
        }
        self.toast(ToastKind::Warn, format!("{}: delete layer", S::TOAST_NOT_YET));
    }

    fn move_body_to_layer(&mut self, body: BodyId, layer: LayerId) {
        if let Some(demo) = &mut self.demo
            && !self.doc.has_scene()
        {
            let Some(name) = demo.layers.iter().find(|l| l.1 == layer).map(|l| l.0.clone()) else { return };
            if let Some(b) = demo.bodies.iter_mut().find(|b| b.id == body.0) {
                b.layer = name;
            }
            self.invalidate_view();
            return;
        }
        self.toast(ToastKind::Warn, format!("{}: move to layer", S::TOAST_NOT_YET));
    }

    // ------------------------------------------------------------ outliner

    pub fn outliner_ui(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        // Header: search + buttons.
        ui.horizontal(|ui| {
            let mut te = egui::TextEdit::singleline(&mut self.ui.outliner.filter).hint_text(S::OUTLINER_SEARCH).desired_width(ui.available_width() - 64.0);
            te = te.margin(egui::Margin::symmetric(6, 4));
            let r = ui.add(te);
            if self.ui.focus_outliner_search {
                r.request_focus();
                self.ui.focus_outliner_search = false;
            }
            if icons::icon_button(ui, Icon::Plus, false, 22.0).on_hover_text(S::OUTLINER_NEW_LAYER).clicked() {
                self.create_layer();
            }
            let all_collapsed = !self.ui.outliner.collapsed.is_empty();
            let icon = if all_collapsed { Icon::ChevronRight } else { Icon::ChevronDown };
            let tip = if all_collapsed { S::OUTLINER_EXPAND_ALL } else { S::OUTLINER_COLLAPSE_ALL };
            if icons::icon_button(ui, icon, false, 22.0).on_hover_text(tip).clicked() {
                if all_collapsed {
                    self.ui.outliner.collapsed.clear();
                } else {
                    self.ui.outliner.collapsed = self.layer_rows().iter().map(|l| l.id).collect();
                }
            }
        });
        ui.add_space(4.0);

        let layers = self.layer_rows();
        let bodies = self.body_rows();
        if layers.is_empty() && bodies.is_empty() {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(S::EMPTY_OUTLINER).color(pal.text_dim));
            });
            return;
        }
        if !ui.input(|i| i.pointer.primary_down()) {
            self.ui.outliner.paint = None;
        }
        let filter = self.ui.outliner.filter.to_lowercase();
        let mut actions: Vec<OutlinerAction> = Vec::new();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for l in &layers {
                let members: Vec<&BodyRow> =
                    bodies.iter().filter(|b| b.layer == l.id && (filter.is_empty() || b.name.to_lowercase().contains(&filter))).collect();
                if !filter.is_empty() && members.is_empty() && !l.name.to_lowercase().contains(&filter) {
                    continue;
                }
                let collapsed = self.ui.outliner.collapsed.contains(&l.id) && filter.is_empty();
                self.layer_row(ui, l, members.len(), collapsed, &mut actions);
                if !collapsed {
                    for b in members {
                        self.body_row(ui, b, &mut actions);
                    }
                }
                ui.add_space(2.0);
            }
        });
        for a in actions {
            match a {
                OutlinerAction::ToggleCollapse(id) => {
                    if !self.ui.outliner.collapsed.remove(&id) {
                        self.ui.outliner.collapsed.insert(id);
                    }
                }
                OutlinerAction::LayerVisible(id, v) => {
                    if v {
                        self.local.layer_hidden.remove(&id);
                    } else {
                        self.local.layer_hidden.insert(id);
                    }
                }
                OutlinerAction::LayerLocked(id, v) => {
                    if v {
                        self.local.layer_locked.insert(id);
                    } else {
                        self.local.layer_locked.remove(&id);
                    }
                }
                OutlinerAction::BodyVisible(id, v) => self.set_body_visible(id, v),
                OutlinerAction::BodyLocked(id, v) => {
                    if v {
                        self.local.body_locked.insert(id);
                        self.sel.objects.remove(&id);
                    } else {
                        self.local.body_locked.remove(&id);
                    }
                }
                OutlinerAction::Select(id, how) => {
                    if self.mode != Mode::Object {
                        self.set_mode(Mode::Object);
                    }
                    combine(&mut self.sel.objects, [id], how);
                    self.on_selection_changed();
                }
                OutlinerAction::RenameBody(id, name) => self.rename_body(id, name),
                OutlinerAction::RenameLayer(id, name) => self.rename_layer(id, name),
                OutlinerAction::DeleteLayer(id) => self.delete_layer(id),
                OutlinerAction::Move(b, l) => self.move_body_to_layer(b, l),
                OutlinerAction::Frame(id) => {
                    let set: BTreeSet<BodyId> = [id].into();
                    let b = self.vd.bounds_of(&set);
                    self.frame_bounds(b);
                }
            }
        }
    }

    fn paint_toggle(&mut self, ui: &mut Ui, col: PaintColumn, on: bool, icon_on: Icon, icon_off: Icon, tip: &str) -> Option<bool> {
        let (rect, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click_and_drag());
        let pal = self.pal;
        let hovered = ui.rect_contains_pointer(rect);
        let mut result = None;
        if resp.drag_started() || resp.clicked() {
            let target = !on;
            self.ui.outliner.paint = Some((col, target));
            result = Some(target);
        } else if let Some((c, v)) = self.ui.outliner.paint
            && c == col
            && hovered
            && ui.input(|i| i.pointer.primary_down())
            && on != v
        {
            // Click-drag down the column sets many rows at once.
            result = Some(v);
        }
        let color = if on { pal.text } else { pal.text_dim.gamma_multiply(0.6) };
        if hovered {
            ui.painter().rect_filled(rect, 4.0, pal.widget_hover);
        }
        icons::paint(ui.painter(), rect.shrink(4.0), if on { icon_on } else { icon_off }, color);
        resp.on_hover_text(tip);
        result
    }

    fn layer_row(&mut self, ui: &mut Ui, l: &LayerRow, count: usize, collapsed: bool, actions: &mut Vec<OutlinerAction>) {
        let pal = self.pal;
        let row = ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let chev = if collapsed { Icon::ChevronRight } else { Icon::ChevronDown };
            if icons::icon_button(ui, chev, false, 18.0).clicked() {
                actions.push(OutlinerAction::ToggleCollapse(l.id));
            }
            let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
            icons::paint(ui.painter(), r.shrink(2.0), Icon::Layer, if l.active { pal.accent } else { pal.text_dim });
            ui.add_space(4.0);
            if let Some((RenameTarget::Layer(id), buf)) = &mut self.ui.outliner.renaming
                && *id == l.id
            {
                let r = ui.add(egui::TextEdit::singleline(buf).desired_width(ui.available_width() - 56.0));
                r.request_focus();
                if r.lost_focus() {
                    if ui.input(|i| i.key_pressed(egui::Key::Enter)) && !buf.trim().is_empty() {
                        actions.push(OutlinerAction::RenameLayer(l.id, buf.trim().to_string()));
                    }
                    self.ui.outliner.renaming = None;
                }
            } else {
                let name = RichText::new(&l.name).font(crate::theme::medium(13.0)).color(pal.text_strong);
                let r = ui.add(egui::Label::new(name).sense(Sense::click()).truncate());
                if r.double_clicked() {
                    self.ui.outliner.renaming = Some((RenameTarget::Layer(l.id), l.name.clone()));
                }
                ui.label(RichText::new(format!("{count}")).small().color(pal.text_dim));
                if l.active {
                    ui.label(RichText::new(S::OUTLINER_ACTIVE).small().color(pal.accent));
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                if let Some(v) = self.paint_toggle(ui, PaintColumn::LayerLock, l.locked, Icon::Lock, Icon::Unlock, S::OUTLINER_LOCK) {
                    actions.push(OutlinerAction::LayerLocked(l.id, v));
                }
                if let Some(v) = self.paint_toggle(ui, PaintColumn::LayerEye, l.visible, Icon::Eye, Icon::EyeOff, S::OUTLINER_VISIBLE) {
                    actions.push(OutlinerAction::LayerVisible(l.id, v));
                }
            });
        });
        let resp = row.response.interact(Sense::click());
        // Drop objects onto a layer row.
        if let Some(payload) = resp.dnd_release_payload::<BodyId>() {
            actions.push(OutlinerAction::Move(*payload, l.id));
        }
        if resp.dnd_hover_payload::<BodyId>().is_some() {
            ui.painter().rect_stroke(row.response.rect, 4.0, egui::Stroke::new(1.5, pal.accent), egui::StrokeKind::Inside);
        }
        resp.context_menu(|ui| {
            if ui.button(S::OUTLINER_RENAME).clicked() {
                self.ui.outliner.renaming = Some((RenameTarget::Layer(l.id), l.name.clone()));
                ui.close();
            }
            if ui.button(S::OUTLINER_DELETE_LAYER).on_hover_text(S::OUTLINER_LAYER_NOT_EMPTY).clicked() {
                actions.push(OutlinerAction::DeleteLayer(l.id));
                ui.close();
            }
        });
    }

    fn body_row(&mut self, ui: &mut Ui, b: &BodyRow, actions: &mut Vec<OutlinerAction>) {
        let pal = self.pal;
        let selected = self.sel.objects.contains(&b.id);
        let full = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(full, 24.0), Sense::hover());
        let id = ui.id().with(("body-row", b.id));
        let resp = ui.interact(rect, id, Sense::click_and_drag());
        if selected {
            ui.painter().rect_filled(rect, 4.0, pal.selection_bg);
        } else if resp.hovered() {
            ui.painter().rect_filled(rect, 4.0, pal.widget.gamma_multiply(0.6));
        }
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(2.0, 1.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
        child.add_space(26.0);
        let (ir, _) = child.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
        icons::paint(child.painter(), ir, Icon::Body, if selected { pal.accent } else { pal.text_dim });
        child.add_space(4.0);
        if let Some((RenameTarget::Body(rid), buf)) = &mut self.ui.outliner.renaming
            && *rid == b.id
        {
            let r = child.add(egui::TextEdit::singleline(buf).desired_width(rect.width() - 110.0));
            r.request_focus();
            if r.lost_focus() {
                if child.input(|i| i.key_pressed(egui::Key::Enter)) && !buf.trim().is_empty() {
                    actions.push(OutlinerAction::RenameBody(b.id, buf.trim().to_string()));
                }
                self.ui.outliner.renaming = None;
            }
        } else {
            let col = if !b.visible { pal.text_dim.gamma_multiply(0.7) } else if selected { pal.text_strong } else { pal.text };
            child.add(egui::Label::new(RichText::new(&b.name).color(col)).truncate().selectable(false));
        }
        child.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            if let Some(v) = self.paint_toggle(ui, PaintColumn::BodyLock, b.locked, Icon::Lock, Icon::Unlock, S::OUTLINER_LOCK) {
                actions.push(OutlinerAction::BodyLocked(b.id, v));
            }
            if let Some(v) = self.paint_toggle(ui, PaintColumn::BodyEye, b.visible, Icon::Eye, Icon::EyeOff, S::OUTLINER_VISIBLE) {
                actions.push(OutlinerAction::BodyVisible(b.id, v));
            }
        });
        if resp.clicked() && !b.locked {
            let m = ui.input(|i| i.modifiers);
            let how = if m.shift {
                Combine::Add
            } else if m.ctrl || m.command {
                Combine::Remove
            } else {
                Combine::Replace
            };
            actions.push(OutlinerAction::Select(b.id, how));
        }
        if resp.double_clicked() {
            actions.push(OutlinerAction::Frame(b.id));
        }
        resp.dnd_set_drag_payload(b.id);
        resp.context_menu(|ui| {
            if ui.button(S::OUTLINER_RENAME).clicked() {
                self.ui.outliner.renaming = Some((RenameTarget::Body(b.id), b.name.clone()));
                ui.close();
            }
            if ui.button("Frame").clicked() {
                actions.push(OutlinerAction::Frame(b.id));
                ui.close();
            }
        });
    }

    // ------------------------------------------------------------ statistics

    fn topo_stats(&self, filter: impl Fn(BodyId, Option<FaceId>) -> bool) -> MeshStats {
        let mut s = MeshStats::default();
        for (b, bt) in &self.vd.topo {
            let mut verts = BTreeSet::new();
            let mut edges = BTreeSet::new();
            for (pi, poly) in bt.mesh.polys.iter().enumerate() {
                if !filter(*b, bt.poly_face.get(pi).copied()) {
                    continue;
                }
                s.faces += 1;
                match poly.len() {
                    3 => s.tris += 1,
                    4 => s.quads += 1,
                    _ => s.ngons += 1,
                }
                for k in 0..poly.len() {
                    verts.insert(poly[k]);
                    let (a, c) = (poly[k], poly[(k + 1) % poly.len()]);
                    edges.insert((a.min(c), a.max(c)));
                }
            }
            s.verts += verts.len();
            s.edges += edges.len();
        }
        s
    }

    fn total_stats(&mut self) -> MeshStats {
        if let Some(scene) = self.doc.scene.as_mut()
            && let Ok(s) = guard(|| scene.stats())
        {
            return s;
        }
        self.topo_stats(|_, _| true)
    }

    fn selection_stats(&self) -> MeshStats {
        match self.mode {
            Mode::Object => self.topo_stats(|b, _| self.sel.objects.contains(&b)),
            Mode::Face => self.topo_stats(|b, f| f.is_some_and(|f| self.sel.faces.contains(&(b, f)))),
            Mode::Edge => MeshStats { edges: self.sel.edges.len(), ..Default::default() },
            Mode::Vertex => MeshStats { verts: self.sel.verts.len(), ..Default::default() },
        }
    }

    pub fn stats_ui(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        if !self.has_content() {
            ui.label(RichText::new(S::NO_SCENE).color(pal.text_dim));
            return;
        }
        let total = self.total_stats();
        let sel = self.selection_stats();
        let dormant = self.doc.scene.as_mut().and_then(|s| guard(|| s.dormant_edits().len()).ok()).unwrap_or(0);
        let show = self.prefs.stats.show;
        let p = self.prefs.stats.clone();
        let rows: Vec<(&str, usize, usize, bool)> = vec![
            (S::STAT_VERTS, total.verts, sel.verts, p.verts),
            (S::STAT_EDGES, total.edges, sel.edges, p.edges),
            (S::STAT_FACES, total.faces, sel.faces, p.faces),
            (S::STAT_TRIS, total.tris, sel.tris, p.tris),
            (S::STAT_QUADS, total.quads, sel.quads, p.quads),
            (S::STAT_NGONS, total.ngons, sel.ngons, p.ngons),
        ];
        ui.label(RichText::new(S::STAT_SOURCE_PREVIEW).small().color(pal.text_dim));
        ui.add_space(4.0);
        egui::Grid::new("stats").num_columns(3).spacing(vec2(16.0, 5.0)).show(ui, |ui| {
            ui.label("");
            if show != StatsShow::Selection {
                ui.label(RichText::new(S::STAT_TOTAL).small().color(pal.text_dim));
            }
            if show != StatsShow::Totals {
                ui.label(RichText::new(S::STAT_SELECTED).small().color(pal.text_dim));
            }
            ui.end_row();
            for (label, t, s, on) in rows {
                if !on {
                    continue;
                }
                ui.label(RichText::new(label).color(pal.text));
                if show != StatsShow::Selection {
                    ui.label(RichText::new(fmt_count(t)).color(pal.text_strong).monospace());
                }
                if show != StatsShow::Totals {
                    ui.label(RichText::new(fmt_count(s)).color(if s > 0 { pal.accent } else { pal.text_dim }).monospace());
                }
                ui.end_row();
            }
            if p.dormant {
                ui.label(RichText::new(S::STAT_DORMANT).color(pal.text));
                ui.label(RichText::new(fmt_count(dormant)).color(if dormant > 0 { pal.warn } else { pal.text_dim }).monospace());
                ui.end_row();
            }
        });
        // Quad ratio bar: a quick read of how game-ready the mesh is.
        if total.faces > 0 {
            ui.add_space(10.0);
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), Sense::hover());
            let f = total.faces as f32;
            let segs = [(total.quads as f32 / f, pal.accent), (total.tris as f32 / f, pal.warn), (total.ngons as f32 / f, pal.error)];
            ui.painter().rect_filled(r, 3.0, pal.inset);
            let mut x = r.left();
            for (frac, col) in segs {
                let w = r.width() * frac;
                if w > 0.5 {
                    ui.painter().rect_filled(egui::Rect::from_min_size(egui::pos2(x, r.top()), vec2(w, r.height())), 3.0, col);
                }
                x += w;
            }
            ui.add_space(3.0);
            ui.label(
                RichText::new(format!("{:.0}% quads · {:.0}% tris · {:.0}% n-gons", segs[0].0 * 100.0, segs[1].0 * 100.0, segs[2].0 * 100.0))
                    .small()
                    .color(pal.text_dim),
            );
        }
    }

    // ------------------------------------------------------------ history

    pub fn history_ui(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        let Some(scene) = self.doc.scene.as_mut() else {
            ui.add_space(8.0);
            ui.label(RichText::new(S::EMPTY_HISTORY).color(pal.text_dim));
            return;
        };
        let data = guard(|| (scene.doc().rules.clone(), scene.doc().edits.clone(), scene.doc().phase_overrides.clone()));
        let dormant: BTreeSet<u32> = guard(|| scene.dormant_edits()).map(|v| v.into_iter().collect()).unwrap_or_default();
        let (rules, edits, phase) = match data {
            Ok(d) => d,
            Err(e) => {
                ui.label(RichText::new(e).color(pal.warn));
                return;
            }
        };
        if rules.is_empty() && edits.is_empty() && phase.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new(S::EMPTY_HISTORY).color(pal.text_dim));
            return;
        }
        let mut delete_rule = None;
        section(ui, &pal, S::HIST_RULES, rules.len());
        for r in &rules {
            let (icon, detail) = rule_summary(&r.rule);
            let resp = history_row(ui, &pal, icon, r.rule.kind_name(), &detail, false);
            resp.context_menu(|ui| {
                if ui.button(S::HIST_DELETE_RULE).clicked() {
                    delete_rule = Some(r.id);
                    ui.close();
                }
            });
        }
        ui.add_space(6.0);
        section(ui, &pal, S::HIST_EDITS, edits.len());
        for e in &edits {
            let (icon, name) = match &e.edit {
                Edit::RemoveEdge { .. } => (Icon::Remove, "Remove edge"),
                Edit::RemoveCadEdge { .. } => (Icon::Remove, "Remove CAD edge"),
                Edit::Collapse { .. } => (Icon::Collapse, "Collapse"),
                Edit::Weld { .. } => (Icon::Weld, "Weld"),
                Edit::SetHard { hard, .. } => (Icon::Edge, if *hard { "Mark hard" } else { "Mark smooth" }),
            };
            let d = dormant.contains(&e.id);
            history_row(ui, &pal, icon, name, if d { S::HIST_DORMANT } else { "" }, d);
        }
        if !dormant.is_empty() {
            ui.add_space(4.0);
            if ui.button(format!("{}  ({})", S::HIST_PURGE, dormant.len())).clicked() {
                self.apply(Op::PurgeDormant);
            }
        }
        if !phase.is_empty() {
            ui.add_space(6.0);
            section(ui, &pal, S::HIST_PHASE, phase.len());
            for (t, _) in &phase {
                let label = match t {
                    newtopo_core::doc::PhaseTarget::Face(b, f) => format!("Body {b} · face {f}"),
                    newtopo_core::doc::PhaseTarget::Edge(b, e) => format!("Body {b} · edge {e}"),
                };
                history_row(ui, &pal, Icon::Link, "Phase reference", &label, false);
            }
        }
        if let Some(id) = delete_rule {
            self.apply(Op::DeleteRule { rule: id });
        }
    }

    // ------------------------------------------------------------ tool panel

    pub fn tool_panel_ui(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        // Tool switcher.
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            for (t, icon, cmd) in [
                (Tool::Connect, Icon::Connect, Cmd::Connect),
                (Tool::Grid, Icon::Grid, Cmd::Grid),
                (Tool::Slice, Icon::Slice, Cmd::Slice),
                (Tool::Iso, Icon::Iso, Cmd::Iso),
                (Tool::Cut, Icon::Cut, Cmd::Cut),
                (Tool::Weld, Icon::Weld, Cmd::Weld),
            ] {
                let r = icons::icon_button(ui, icon, self.tool.active == t, 30.0)
                    .on_hover_text(format!("{}  ({})", cmd.label(), self.keymap.label(cmd)));
                if r.clicked() {
                    if self.tool.active == t {
                        self.tool.active = Tool::Select;
                    } else {
                        self.activate_tool(t);
                    }
                }
            }
            ui.add_space(6.0);
            for (icon, cmd) in [(Icon::Collapse, Cmd::Collapse), (Icon::Remove, Cmd::Remove)] {
                if icons::icon_button(ui, icon, false, 30.0).on_hover_text(format!("{}  ({})", cmd.label(), self.keymap.label(cmd))).clicked() {
                    self.run(cmd);
                }
            }
        });
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(4.0);
        match self.tool.active {
            Tool::Select => {
                ui.label(RichText::new(S::EMPTY_TOOL).color(pal.text_dim));
            }
            Tool::Connect | Tool::Grid | Tool::Slice => self.segment_panel(ui),
            Tool::Iso => {
                ui.horizontal(|ui| {
                    ui.label(S::ISO_DIR);
                    let mut d = self.tool.iso_dir;
                    ui.selectable_value(&mut d, IsoDirSel::U, "U");
                    ui.selectable_value(&mut d, IsoDirSel::V, "V");
                    if d != self.tool.iso_dir {
                        self.tool.iso_dir = d;
                        self.on_hover_changed();
                    }
                    ui.label(RichText::new("Tab").small().color(pal.text_dim));
                });
                ui.checkbox(&mut self.tool.iso_split, S::ISO_SPLIT);
                ui.checkbox(&mut self.tool.iso_through, S::SEG_THROUGH);
                ui.checkbox(&mut self.tool.snap, S::ISO_SNAP);
                if self.tool.iso_split {
                    ui.label(RichText::new("Split lines divide the face into patches with their own counts.").small().color(pal.text_dim));
                }
            }
            Tool::Cut => {
                ui.label(format!("{}: {}", S::CUT_POINTS, self.tool.cut.len()));
                ui.horizontal(|ui| {
                    if ui.add_enabled(self.tool.cut.len() >= 2, egui::Button::new(S::CUT_FINISH)).clicked() {
                        self.cut_finish(false);
                    }
                    if ui.add_enabled(!self.tool.cut.is_empty(), egui::Button::new(S::CUT_UNDO_POINT)).clicked() {
                        self.tool.cut.pop();
                    }
                });
                ui.checkbox(&mut self.tool.snap, S::ISO_SNAP);
            }
            Tool::Weld => {
                let from = self.tool.weld_from.is_some();
                ui.label(RichText::new(if from { "Source picked. Click the target vertex." } else { "Click the source vertex." }).color(pal.text));
                ui.label(RichText::new("The source moves to the target and keeps the target's anchor.").small().color(pal.text_dim));
            }
        }
    }

    fn segment_panel(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        let tool = self.tool.active;
        let before = self.tool.seg.clone();
        let mut dragging = false;
        let mut count_row = |ui: &mut Ui, label: &str, v: &mut u32, color: Color32| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(4.0, 18.0), Sense::hover());
                ui.painter().rect_filled(r, 2.0, color);
                ui.label(RichText::new(label).font(crate::theme::medium(13.0)));
                ui.add_space(4.0);
                let s = ui.add(egui::Slider::new(v, 0..=64).clamping(egui::SliderClamping::Never).show_value(false));
                let d = ui.add(egui::DragValue::new(v).range(0..=999).speed(0.1));
                dragging |= s.dragged() || d.dragged();
            });
        };
        let c = self.colors();
        let ca = Color32::from_rgb((c.arrow_a[0] * 255.0) as u8, (c.arrow_a[1] * 255.0) as u8, (c.arrow_a[2] * 255.0) as u8);
        let cb = Color32::from_rgb((c.arrow_b[0] * 255.0) as u8, (c.arrow_b[1] * 255.0) as u8, (c.arrow_b[2] * 255.0) as u8);
        ui.label(RichText::new(S::SEG_TITLE).font(crate::theme::medium(14.0)).color(pal.text_strong));
        ui.add_space(2.0);
        match tool {
            Tool::Connect => {
                count_row(ui, S::SEG_A, &mut self.tool.seg.a, ca);
                count_row(ui, S::SEG_B, &mut self.tool.seg.b, cb);
                ui.checkbox(&mut self.tool.seg.start_on_seam, S::SEG_START_ON_SEAM);
            }
            Tool::Grid => {
                count_row(ui, S::SEG_U, &mut self.tool.seg.u, ca);
                count_row(ui, S::SEG_V, &mut self.tool.seg.v, cb);
                ui.horizontal(|ui| {
                    ui.label(S::SEG_SPACING);
                    ui.selectable_value(&mut self.tool.seg.spacing, Spacing::ArcLength, S::SEG_ARC);
                    ui.selectable_value(&mut self.tool.seg.spacing, Spacing::Parametric, S::SEG_PARAM);
                });
                ui.checkbox(&mut self.tool.seg.start_on_seam, S::SEG_START_ON_SEAM);
            }
            Tool::Slice => {
                count_row(ui, S::SEG_COUNT, &mut self.tool.seg.slice, ca);
                ui.checkbox(&mut self.tool.seg.through, S::SEG_THROUGH);
                ui.horizontal(|ui| {
                    ui.label(S::SEG_GUIDE);
                    let label = match self.tool.seg.guide {
                        Some((b, e)) => format!("edge {e} on body {b}"),
                        None => S::SEG_GUIDE_AUTO.to_string(),
                    };
                    ui.label(RichText::new(label).color(pal.text_dim));
                });
                ui.horizontal(|ui| {
                    if ui.selectable_label(self.tool.picking_guide, S::SEG_GUIDE_PICK).clicked() {
                        self.tool.picking_guide = !self.tool.picking_guide;
                        self.set_mode(Mode::Edge);
                    }
                    if self.tool.seg.guide.is_some() && ui.button(S::SEG_GUIDE_AUTO).clicked() {
                        self.tool.seg.guide = None;
                    }
                });
                let scope = if self.mode == Mode::Object { "Scope: whole objects" } else { "Scope: selected patches" };
                ui.label(RichText::new(scope).small().color(pal.text_dim));
            }
            _ => {}
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(S::SEG_TOTAL).color(pal.text_dim));
            let t = self.tool.last_lines_added.map(|n| n.to_string()).unwrap_or_else(|| "—".into());
            ui.label(RichText::new(t).monospace().color(pal.text_strong));
        });
        let n = if tool == Tool::Slice && self.mode == Mode::Object { self.sel.objects.len() } else { self.sel.faces.len() };
        ui.label(RichText::new(format!("{n} selected")).small().color(pal.text_dim));
        ui.add_space(4.0);
        if ui.button(S::SEG_CLEAR).clicked() {
            match tool {
                Tool::Connect => {
                    self.tool.seg.a = 0;
                    self.tool.seg.b = 0;
                }
                Tool::Grid => {
                    self.tool.seg.u = 0;
                    self.tool.seg.v = 0;
                }
                _ => self.tool.seg.slice = 0,
            }
        }
        ui.add_space(6.0);
        ui.label(RichText::new(S::SEG_HINT_WHEEL).small().color(pal.text_dim));
        if self.tool.seg != before && self.doc.has_scene() {
            self.apply_segment(true);
        } else if self.tool.seg != before && self.is_demo() && !dragging {
            self.toast(ToastKind::Info, S::TOAST_DEMO_READONLY);
        }
    }
}

enum OutlinerAction {
    ToggleCollapse(LayerId),
    LayerVisible(LayerId, bool),
    LayerLocked(LayerId, bool),
    BodyVisible(BodyId, bool),
    BodyLocked(BodyId, bool),
    Select(BodyId, Combine),
    RenameBody(BodyId, String),
    RenameLayer(LayerId, String),
    DeleteLayer(LayerId),
    Move(BodyId, LayerId),
    Frame(BodyId),
}

pub fn fmt_count(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push('\u{202f}');
        }
        out.push(ch);
    }
    out
}

fn section(ui: &mut Ui, pal: &crate::theme::Palette, title: &str, n: usize) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title.to_uppercase()).small().color(pal.text_dim));
        ui.label(RichText::new(n.to_string()).small().color(pal.text_dim.gamma_multiply(0.7)));
    });
}

fn history_row(ui: &mut Ui, pal: &crate::theme::Palette, icon: Icon, name: &str, detail: &str, dim: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 4.0, pal.widget.gamma_multiply(0.6));
    }
    let col = if dim { pal.text_dim.gamma_multiply(0.6) } else { pal.text };
    let ir = egui::Rect::from_min_size(rect.min + vec2(4.0, 4.0), vec2(16.0, 16.0));
    icons::paint(ui.painter(), ir, icon, if dim { col } else { pal.accent });
    ui.painter().text(rect.min + vec2(26.0, 12.0), egui::Align2::LEFT_CENTER, name, egui::FontId::proportional(13.0), col);
    ui.painter().text(rect.right_center() - vec2(4.0, 0.0), egui::Align2::RIGHT_CENTER, detail, egui::FontId::proportional(11.5), pal.text_dim);
    resp
}

fn rule_summary(r: &Rule) -> (Icon, String) {
    match r {
        Rule::Connect { patches, count_a, count_b, .. } => (Icon::Connect, format!("A {count_a} · B {count_b} · {} patches", patches.len())),
        Rule::Grid { count_u, count_v, .. } => (Icon::Grid, format!("{count_u} × {count_v}")),
        Rule::Slice { scope, count, through, .. } => {
            let s = match scope {
                SliceScope::Body(b) => format!("body {b}"),
                SliceScope::Patches(p) => format!("{} patches", p.len()),
            };
            (Icon::Slice, format!("{count} planes · {s}{}", if *through { " · through" } else { "" }))
        }
        Rule::Iso { pieces, through } => (Icon::Iso, format!("{} piece{}{}", pieces.len(), if pieces.len() == 1 { "" } else { "s" }, if *through { " · through" } else { "" })),
        Rule::Cut { points, closed } => (Icon::Cut, format!("{} points{}", points.len(), if *closed { " · closed" } else { "" })),
        Rule::Join { .. } => (Icon::Weld, "2 vertices".into()),
    }
}

/// Group rows by layer for tests and other panels.
pub fn group_by_layer(bodies: &[BodyRow]) -> BTreeMap<LayerId, Vec<BodyId>> {
    let mut m: BTreeMap<LayerId, Vec<BodyId>> = BTreeMap::new();
    for b in bodies {
        m.entry(b.layer).or_default().push(b.id);
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_are_grouped() {
        assert_eq!(fmt_count(0), "0");
        assert_eq!(fmt_count(1234567), "1\u{202f}234\u{202f}567");
    }

    #[test]
    fn grouping() {
        let rows = vec![
            BodyRow { id: BodyId(1), name: "a".into(), layer: LayerId(0), visible: true, locked: false },
            BodyRow { id: BodyId(2), name: "b".into(), layer: LayerId(1), visible: true, locked: false },
            BodyRow { id: BodyId(3), name: "c".into(), layer: LayerId(0), visible: true, locked: false },
        ];
        let g = group_by_layer(&rows);
        assert_eq!(g[&LayerId(0)], vec![BodyId(1), BodyId(3)]);
    }
}
