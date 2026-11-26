//! The 3D viewport: navigation presets, hover, selection (click, box,
//! lasso, double-click loops), view cube, overlays and the paint callback.

use crate::app::AppState;
use crate::icons::{self, Icon};
use crate::prefs::NavPreset;
use crate::selection::{Combine, EdgeSel, Mode, TopoMesh, VertSel, combine};
use crate::strings as S;
use crate::toasts::ToastKind;
use crate::tools::Tool;
use crate::viewdata::EdgeKind;
use egui::{Color32, Pos2, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use glam::{DVec2, DVec3, Vec3};
use newtopo_kernel::{BodyId, FaceId};
use newtopo_render::egui_viewport::{ViewportCallback, paint_callback};
use newtopo_render::pick::{FaceRef, Region, nearest_point, nearest_polyline};
use newtopo_render::scene::{RenderScene, with_alpha};
use newtopo_render::{CameraMatrices, NamedView, RenderOptions, ShadeMode};
use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hover {
    Object(BodyId, DVec3),
    Face(FaceRef, DVec3),
    Edge(usize, DVec3),
    Vert(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavKind {
    Orbit,
    Pan,
    Zoom,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DragState {
    Nav(NavKind),
    Box { start: Pos2, how: Combine },
    Lasso { pts: Vec<Pos2>, how: Combine },
    Weld { from: usize },
}

/// Which navigation a pointer button + modifiers means for a preset.
pub fn nav_for(
    preset: NavPreset,
    button: egui::PointerButton,
    m: egui::Modifiers,
) -> Option<NavKind> {
    use egui::PointerButton::*;
    let ctrl = m.ctrl || m.command;
    match preset {
        NavPreset::Blender => match button {
            Middle if m.shift => Some(NavKind::Pan),
            Middle if ctrl => Some(NavKind::Zoom),
            Middle => Some(NavKind::Orbit),
            _ => None,
        },
        NavPreset::Maya => match button {
            Primary if m.alt => Some(NavKind::Orbit),
            Middle if m.alt => Some(NavKind::Pan),
            Secondary if m.alt => Some(NavKind::Zoom),
            Middle => Some(NavKind::Pan),
            _ => None,
        },
        NavPreset::Max => match button {
            Middle if m.alt && ctrl => Some(NavKind::Zoom),
            Middle if m.alt => Some(NavKind::Orbit),
            Middle => Some(NavKind::Pan),
            _ => None,
        },
    }
}

fn combine_from(m: egui::Modifiers) -> Combine {
    if m.shift {
        Combine::Add
    } else if m.ctrl || m.command {
        Combine::Remove
    } else {
        Combine::Replace
    }
}

impl AppState {
    pub fn camera_matrices(&self) -> CameraMatrices {
        let (c, r) = self.scene_sphere();
        self.view.camera.matrices(
            [
                self.view.rect.width().max(1.0),
                self.view.rect.height().max(1.0),
            ],
            c,
            r,
        )
    }

    pub fn scene_sphere(&self) -> (DVec3, f64) {
        match self.vd.bounds {
            Some((a, b)) => (
                ((a + b) * 0.5).as_dvec3(),
                ((b - a).length() * 0.5).max(1e-3) as f64,
            ),
            None => (DVec3::ZERO, 50.0),
        }
    }

    fn local(&self, p: Pos2) -> DVec2 {
        DVec2::new(
            (p.x - self.view.rect.left()) as f64,
            (p.y - self.view.rect.top()) as f64,
        )
    }

    pub fn viewport_ui(&mut self, ui: &mut Ui) {
        let rect = ui.available_rect_before_wrap();
        self.view.rect = rect;
        self.view.shown = true;
        let resp = ui.allocate_rect(rect, Sense::click_and_drag());
        self.ensure_view_data();
        if self.view.needs_frame && self.vd.bounds.is_some() && rect.width() > 10.0 {
            self.view.needs_frame = false;
            self.named_view(NamedView::Iso);
            self.frame_bounds(None);
        }

        // Compare split: both halves follow the camera; picking and
        // editing wait until the compare is closed.
        let comparing = self.view.compare && self.doc.in_preview();
        if self.has_content() {
            self.navigate(ui, &resp);
            if comparing {
                self.hover = None;
            } else {
                self.update_hover(ui, &resp);
                self.handle_clicks(ui, &resp);
            }
        }
        self.rebuild_overlay();
        if comparing {
            self.paint_compare(ui, rect);
        } else {
            self.paint_viewport(ui, rect);
        }
        self.view_cube(ui, rect);
        self.hud(ui, rect);
        if !self.has_content() {
            self.empty_state(ui, rect);
        }
        if ui.ctx().input(|i| !i.raw.hovered_files.is_empty()) {
            let p = ui.painter();
            p.rect_filled(rect.shrink(8.0), 12.0, self.pal.accent.gamma_multiply(0.08));
            p.rect_stroke(
                rect.shrink(8.0),
                12.0,
                Stroke::new(2.0, self.pal.accent),
                egui::StrokeKind::Inside,
            );
            p.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                S::VP_DROP,
                crate::theme::medium(20.0),
                self.pal.accent,
            );
        }
    }

    // ------------------------------------------------------------ navigation

    fn navigate(&mut self, ui: &mut Ui, resp: &egui::Response) {
        let (mods, delta, pressed_btn, any_down, hover) = ui.input(|i| {
            let pressed = [
                egui::PointerButton::Primary,
                egui::PointerButton::Middle,
                egui::PointerButton::Secondary,
            ]
            .into_iter()
            .find(|b| i.pointer.button_pressed(*b));
            (
                i.modifiers,
                i.pointer.delta(),
                pressed,
                i.pointer.any_down(),
                i.pointer.hover_pos(),
            )
        });
        let inside = hover.is_some_and(|p| self.view.rect.contains(p)) && resp.hovered();
        if let Some(b) = pressed_btn
            && inside
            && let Some(kind) = nav_for(self.prefs.nav, b, mods)
        {
            self.drag = Some(DragState::Nav(kind));
        }
        if let Some(DragState::Nav(kind)) = self.drag.clone() {
            if !any_down {
                self.drag = None;
            } else {
                let h = self.view.rect.height().max(1.0) as f64;
                match kind {
                    NavKind::Orbit => {
                        if delta != egui::Vec2::ZERO {
                            self.leave_auto_ortho();
                        }
                        self.view
                            .camera
                            .orbit(-delta.x as f64 * 0.008, delta.y as f64 * 0.008)
                    }
                    NavKind::Pan => self.view.camera.pan_px(delta.x as f64, delta.y as f64, h),
                    NavKind::Zoom => self
                        .view
                        .camera
                        .zoom((1.0 + delta.y as f64 * 0.006).max(0.1), None),
                }
                ui.ctx().request_repaint();
            }
        }

        // Wheel: modifiers go to the active segment tool (SPEC §6.2), plain wheel zooms.
        if inside {
            let wheels: Vec<(egui::Vec2, egui::MouseWheelUnit, egui::Modifiers)> = ui.input(|i| {
                i.events
                    .iter()
                    .filter_map(|e| match e {
                        egui::Event::MouseWheel {
                            unit,
                            delta,
                            modifiers,
                            ..
                        } => Some((*delta, *unit, *modifiers)),
                        _ => None,
                    })
                    .collect()
            });
            for (d, unit, m) in wheels {
                let raw = if d.y.abs() >= d.x.abs() { d.y } else { d.x };
                let steps = match unit {
                    egui::MouseWheelUnit::Line | egui::MouseWheelUnit::Page => raw,
                    egui::MouseWheelUnit::Point => raw / 40.0,
                };
                if steps == 0.0 {
                    continue;
                }
                let ctrl = m.ctrl || m.command;
                if self.tool.active.is_segment() && (m.shift || ctrl) {
                    self.wheel_count(steps, m.shift);
                    continue;
                }
                let anchor = hover.map(|p| {
                    let (sc, sr) = self.scene_sphere();
                    let ray = self.view.camera.ray_in_scene(
                        self.local(p),
                        [self.view.rect.width(), self.view.rect.height()],
                        sc,
                        sr,
                    );
                    match self.vd.pick.raycast(&ray, |_| false) {
                        Some(h) => h.point,
                        None => {
                            let n = self.view.camera.forward();
                            let t = (self.view.camera.target - ray.origin).dot(n)
                                / ray.dir.dot(n).max(1e-9);
                            ray.at(t)
                        }
                    }
                });
                self.view.camera.zoom(0.88f64.powf(steps as f64), anchor);
            }
        }
    }

    pub fn frame_bounds(&mut self, b: Option<(Vec3, Vec3)>) {
        let Some((min, max)) = b.or(self.vd.bounds) else {
            return;
        };
        let aspect = (self.view.rect.width() / self.view.rect.height().max(1.0)).max(0.1) as f64;
        self.view
            .camera
            .frame_aabb(min.as_dvec3(), max.as_dvec3(), aspect);
    }

    /// Frame the selection in the current mode, or everything (SPEC §5.2).
    pub fn frame_selection(&mut self) {
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        let mut add = |p: Vec3| {
            min = min.min(p);
            max = max.max(p);
        };
        match self.mode {
            Mode::Object => {
                if let Some((a, b)) = self
                    .vd
                    .bounds_of(&self.sel.objects)
                    .filter(|_| !self.sel.objects.is_empty())
                {
                    add(a);
                    add(b);
                }
            }
            Mode::Face => {
                for (b, f) in &self.sel.faces {
                    if let Some(tris) = self.vd.face_tris.get(&FaceRef {
                        body: b.0,
                        face: f.0,
                    }) {
                        for &t in tris {
                            for i in self.vd.pick.tris[t as usize] {
                                add(self.vd.pick.positions[i as usize]);
                            }
                        }
                    }
                }
            }
            Mode::Edge => {
                for e in &self.sel.edges {
                    if let Some(i) = self.vd.edge_index(e) {
                        self.vd.edges[i].path.iter().for_each(|p| add(*p));
                    }
                }
            }
            Mode::Vertex => {
                for v in &self.sel.verts {
                    if let Some(i) = self.vd.vert_index(v) {
                        add(self.vd.verts[i].pos);
                    }
                }
            }
        }
        if min.x <= max.x {
            let pad = ((max - min).length() * 0.1).max(1.0);
            self.frame_bounds(Some((min - pad, max + pad)));
        } else {
            self.frame_bounds(None);
        }
    }

    // ------------------------------------------------------------ hover & picking

    fn pick_kind(&self) -> Mode {
        if self.in_mesh_mode() {
            return match self.mm.tool {
                crate::meshmode::MeshTool::TargetMerge => Mode::Vertex,
                _ => self.mode,
            };
        }
        match self.tool.active {
            Tool::Weld => Mode::Vertex,
            Tool::Iso | Tool::Cut => Mode::Face,
            Tool::Connect | Tool::Grid => Mode::Face,
            Tool::Slice if self.mode == Mode::Object => Mode::Object,
            Tool::Slice => Mode::Face,
            Tool::Select => self.mode,
        }
    }

    fn visible_fn(&self, cam: &CameraMatrices) -> impl Fn(DVec3) -> bool + '_ {
        let eps = self.scene_sphere().1 * 0.004;
        let cam = *cam;
        move |p| self.vd.pick.is_visible(&cam, p, eps)
    }

    pub fn pick_at(&self, p: Pos2, kind: Mode) -> Option<Hover> {
        let cam = self.camera_matrices();
        let local = self.local(p);
        let size = [self.view.rect.width(), self.view.rect.height()];
        match kind {
            Mode::Object | Mode::Face => {
                let (sc, sr) = self.scene_sphere();
                let ray = self.view.camera.ray_in_scene(local, size, sc, sr);
                let h = self
                    .vd
                    .pick
                    .raycast(&ray, |f| self.is_locked(BodyId(f.body)))?;
                Some(if kind == Mode::Object {
                    Hover::Object(BodyId(h.face.body), h.point)
                } else {
                    Hover::Face(h.face, h.point)
                })
            }
            Mode::Edge => {
                let allow_fill = self.prefs.select_fill_edges;
                let vis = self.visible_fn(&cam);
                let n = nearest_polyline(
                    &cam,
                    local,
                    8.0,
                    self.vd
                        .edges
                        .iter()
                        .enumerate()
                        .filter(|(_, e)| {
                            e.sel.is_some()
                                && (allow_fill || e.kind != EdgeKind::Fill)
                                && !self.is_locked(e.body)
                        })
                        .map(|(i, e)| (i, e.path.as_slice())),
                    vis,
                )?;
                Some(Hover::Edge(n.index, n.point))
            }
            Mode::Vertex => {
                let vis = self.visible_fn(&cam);
                let (i, _) = nearest_point(
                    &cam,
                    local,
                    10.0,
                    self.vd
                        .verts
                        .iter()
                        .enumerate()
                        .filter(|(_, v)| !self.is_locked(v.body))
                        .map(|(i, v)| (i, v.pos)),
                    vis,
                )?;
                Some(Hover::Vert(i))
            }
        }
    }

    fn update_hover(&mut self, ui: &mut Ui, resp: &egui::Response) {
        if matches!(
            self.drag,
            Some(DragState::Nav(_)) | Some(DragState::Box { .. }) | Some(DragState::Lasso { .. })
        ) {
            return;
        }
        let pos = ui.input(|i| i.pointer.hover_pos()).filter(|p| {
            self.view.rect.contains(*p) && !cube_area(self.view.rect).contains(*p) && resp.hovered()
        });
        self.ui_pointer = pos;
        let new = pos.and_then(|p| match self.tool.active {
            Tool::Cut => {
                let c = self.cut_pick(p);
                let h = c.as_ref().map(|c| c.hover);
                self.tool.cut_hover = c;
                h
            }
            _ => self.pick_at(p, self.pick_kind()),
        });
        if new != self.hover {
            self.hover = new;
            self.on_hover_changed();
        }
    }

    // ------------------------------------------------------------ clicks, box, lasso

    fn handle_clicks(&mut self, ui: &mut Ui, resp: &egui::Response) {
        // The view cube and its buttons own their area.
        let at = ui.input(|i| {
            i.pointer
                .press_origin()
                .or(i.pointer.interact_pos())
                .or(i.pointer.hover_pos())
        });
        if at.is_some_and(|p| cube_area(self.view.rect).contains(p)) && self.has_content() {
            return;
        }
        let mods = ui.input(|i| i.modifiers);
        let nav_primary = nav_for(self.prefs.nav, egui::PointerButton::Primary, mods).is_some();
        let nav_secondary = nav_for(self.prefs.nav, egui::PointerButton::Secondary, mods).is_some();
        if self.in_mesh_mode() && self.mesh_clicks(ui, resp, nav_primary) {
            return;
        }

        // Target Weld drag from one vertex to another.
        if self.tool.active == Tool::Weld
            && resp.drag_started_by(egui::PointerButton::Primary)
            && !nav_primary
            && let Some(Hover::Vert(i)) = self.hover
        {
            self.drag = Some(DragState::Weld { from: i });
            self.tool.weld_from = Some(self.vd.verts[i].sel.clone());
        }
        if let Some(DragState::Weld { from }) = self.drag.clone() {
            if resp.drag_stopped() {
                self.drag = None;
                if let Some(Hover::Vert(to)) = self.hover
                    && to != from
                {
                    self.weld(from, to);
                }
            }
            return;
        }

        // Box (primary drag) and lasso (secondary drag) selection.
        let selecting = matches!(
            self.tool.active,
            Tool::Select | Tool::Connect | Tool::Grid | Tool::Slice
        );
        if selecting && resp.drag_started_by(egui::PointerButton::Primary) && !nav_primary {
            let start = ui.input(|i| i.pointer.press_origin()).unwrap_or_default();
            self.drag = Some(DragState::Box {
                start,
                how: combine_from(mods),
            });
        }
        if selecting && resp.drag_started_by(egui::PointerButton::Secondary) && !nav_secondary {
            let start = ui.input(|i| i.pointer.press_origin()).unwrap_or_default();
            self.drag = Some(DragState::Lasso {
                pts: vec![start],
                how: combine_from(mods),
            });
        }
        if let Some(DragState::Lasso { pts, .. }) = &mut self.drag
            && let Some(p) = ui.input(|i| i.pointer.hover_pos())
            && pts.last().is_none_or(|l| l.distance(p) > 3.0)
        {
            pts.push(p);
        }
        if resp.drag_stopped() {
            match self.drag.take() {
                Some(DragState::Box { start, how }) => {
                    let end = ui.input(|i| i.pointer.hover_pos()).unwrap_or(start);
                    let r = Rect::from_two_pos(start, end);
                    let region = Region::Rect {
                        min: self.local(r.min),
                        max: self.local(r.max),
                    };
                    self.select_region(&region, how);
                }
                Some(DragState::Lasso { pts, how }) => {
                    if pts.len() >= 3 {
                        let region = Region::Lasso(pts.iter().map(|p| self.local(*p)).collect());
                        self.select_region(&region, how);
                    }
                }
                other => self.drag = other.filter(|d| matches!(d, DragState::Nav(_))),
            }
        }

        if resp.double_clicked() && !nav_primary && selecting {
            self.double_click_select(combine_from(mods));
            return;
        }
        if resp.clicked() && !nav_primary {
            match self.tool.active {
                Tool::Select | Tool::Connect | Tool::Grid | Tool::Slice => {
                    if self.tool.picking_guide {
                        self.pick_guide_edge();
                    } else {
                        self.click_select(combine_from(mods));
                    }
                }
                Tool::Iso => self.iso_click(),
                Tool::Cut => {
                    if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
                        self.cut_click(p);
                    }
                }
                Tool::Weld => {
                    if let Some(Hover::Vert(i)) = self.hover {
                        let from = self
                            .tool
                            .weld_from
                            .as_ref()
                            .and_then(|s| self.vd.vert_index(s));
                        match from {
                            None => self.tool.weld_from = Some(self.vd.verts[i].sel.clone()),
                            Some(f) if f != i => self.weld(f, i),
                            _ => self.tool.weld_from = None,
                        }
                    }
                }
            }
        }
    }

    fn click_select(&mut self, how: Combine) {
        let before = self.sel.clone();
        let kind = self.pick_kind();
        match (kind, self.hover) {
            (Mode::Object, Some(Hover::Object(b, _))) => combine(&mut self.sel.objects, [b], how),
            (Mode::Face, Some(Hover::Face(f, _))) => {
                combine(&mut self.sel.faces, [(BodyId(f.body), FaceId(f.face))], how)
            }
            (Mode::Edge, Some(Hover::Edge(i, _))) => {
                if let Some(s) = self.vd.edges[i].sel.clone() {
                    combine(&mut self.sel.edges, [s], how)
                }
            }
            (Mode::Vertex, Some(Hover::Vert(i))) => {
                combine(&mut self.sel.verts, [self.vd.verts[i].sel.clone()], how)
            }
            (m, None) if how == Combine::Replace => self.sel.clear_mode(m),
            _ => {}
        }
        if self.sel != before {
            self.on_selection_changed();
        }
    }

    fn select_region(&mut self, region: &Region, how: Combine) {
        let cam = self.camera_matrices();
        let vis_only = self.view.visible_only;
        let eps = self.scene_sphere().1 * 0.004;
        let visible = |p: Vec3| !vis_only || self.vd.pick.is_visible(&cam, p.as_dvec3(), eps);
        let inside = |p: Vec3| region.contains_world(&cam, p.as_dvec3()) && visible(p);
        let before = self.sel.clone();
        match self.pick_kind() {
            Mode::Object => {
                let hits: BTreeSet<BodyId> = self
                    .vd
                    .face_centers
                    .iter()
                    .filter(|(f, c)| inside(**c) && !self.is_locked(BodyId(f.body)))
                    .map(|(f, _)| BodyId(f.body))
                    .collect();
                combine(&mut self.sel.objects, hits, how);
            }
            Mode::Face => {
                let hits: Vec<(BodyId, FaceId)> = self
                    .vd
                    .face_centers
                    .iter()
                    .filter(|(f, c)| inside(**c) && !self.is_locked(BodyId(f.body)))
                    .map(|(f, _)| (BodyId(f.body), FaceId(f.face)))
                    .collect();
                combine(&mut self.sel.faces, hits, how);
            }
            Mode::Edge => {
                let allow_fill = self.prefs.select_fill_edges;
                let hits: Vec<EdgeSel> = self
                    .vd
                    .edges
                    .iter()
                    .filter(|e| (allow_fill || e.kind != EdgeKind::Fill) && !self.is_locked(e.body))
                    .filter(|e| {
                        let mid =
                            e.path[e.path.len() / 2].lerp(e.path[(e.path.len() - 1) / 2], 0.5);
                        inside(mid)
                    })
                    .filter_map(|e| e.sel.clone())
                    .collect();
                combine(&mut self.sel.edges, hits, how);
            }
            Mode::Vertex => {
                let hits: Vec<VertSel> = self
                    .vd
                    .verts
                    .iter()
                    .filter(|v| inside(v.pos) && !self.is_locked(v.body))
                    .map(|v| v.sel.clone())
                    .collect();
                combine(&mut self.sel.verts, hits, how);
            }
        }
        if self.sel != before {
            self.on_selection_changed();
        }
    }

    fn double_click_select(&mut self, how: Combine) {
        match (self.pick_kind(), self.hover) {
            (Mode::Edge, Some(Hover::Edge(i, _))) => {
                let edges = self.loop_from(i, false, false);
                combine(
                    &mut self.sel.edges,
                    edges,
                    if how == Combine::Replace {
                        Combine::Replace
                    } else {
                        how
                    },
                );
                self.on_selection_changed();
            }
            (Mode::Face, Some(Hover::Face(f, p))) => {
                let body = BodyId(f.body);
                let Some(bt) = self.vd.topo.get(&body) else {
                    return;
                };
                // The output polygon nearest the click on that face.
                let face = FaceId(f.face);
                let pos_of = |v: u32| -> Option<Vec3> {
                    let ti = bt.mesh.edges.iter().position(|&(a, b)| a == v || b == v)?;
                    let item = bt.edge_item.get(&(ti as u32))?;
                    let e = &self.vd.edges[*item];
                    let (a, _) = bt.mesh.edges[ti];
                    Some(if a == v { e.path[0] } else { *e.path.last()? })
                };
                let best = bt
                    .mesh
                    .polys
                    .iter()
                    .enumerate()
                    .filter(|(pi, _)| bt.poly_face.get(*pi) == Some(&face))
                    .map(|(pi, poly)| {
                        let c: Vec3 = poly.iter().filter_map(|&v| pos_of(v)).sum::<Vec3>()
                            / poly.len().max(1) as f32;
                        (pi, c.as_dvec3().distance(p))
                    })
                    .min_by(|a, b| a.1.total_cmp(&b.1));
                if let Some((pi, _)) = best {
                    // Real scenes: the engine's face loop (`Scene::face_strip`
                    // wraps the same `select::Adjacency::face_loop`).
                    let core_strip =
                        self.vd
                            .outputs
                            .iter()
                            .find(|o| o.body == body)
                            .and_then(|mesh| {
                                let poly = mesh.polygons.get(pi)?;
                                if poly.verts.len() != 4 {
                                    return None;
                                }
                                let adj = newtopo_core::select::Adjacency::new(mesh);
                                let (a, b) = (poly.verts[1], poly.verts[2]);
                                let e = *adj.edge_of.get(&(a.min(b), a.max(b)))?;
                                Some(
                                    adj.face_loop(pi, e)
                                        .into_iter()
                                        .map(|x| x as u32)
                                        .collect::<Vec<u32>>(),
                                )
                            });
                    let strip = core_strip.unwrap_or_else(|| bt.mesh.face_strip(pi as u32));
                    let faces: BTreeSet<(BodyId, FaceId)> = strip
                        .iter()
                        .filter_map(|&q| bt.poly_face.get(q as usize))
                        .map(|f| (body, *f))
                        .collect();
                    combine(&mut self.sel.faces, faces, how);
                    self.on_selection_changed();
                }
            }
            _ => {}
        }
    }

    /// Loop or ring through view edge `i`, optionally every other.
    fn loop_from(&self, i: usize, ring: bool, alternate: bool) -> Vec<EdgeSel> {
        let e = &self.vd.edges[i];
        // Real scenes: the engine's walks (SPEC §6.14) over the output mesh.
        if let (Some(oi), Some(mesh)) = (e.out, self.vd.outputs.iter().find(|o| o.body == e.body)) {
            let oi = oi as usize;
            // Same walk as `Scene::edge_loop/edge_ring` (which wrap
            // `select::Adjacency`), run on the cached output so `&self` suffices.
            let adj = newtopo_core::select::Adjacency::new(mesh);
            let mut seq = if ring {
                adj.edge_ring(oi)
            } else {
                adj.edge_loop(oi)
            };
            if alternate {
                // Keep the clicked edge on the "every other" phase.
                let pos = seq.iter().position(|&x| x == oi).unwrap_or(0);
                seq = seq
                    .iter()
                    .enumerate()
                    .filter(|(k, _)| (*k as isize - pos as isize).rem_euclid(2) == 0)
                    .map(|(_, x)| *x)
                    .collect();
            }
            if e.kind != EdgeKind::Fill {
                seq = newtopo_core::select::editable_only(mesh, &seq);
            }
            let wanted: std::collections::HashSet<u32> = seq.iter().map(|&x| x as u32).collect();
            return self
                .vd
                .edges
                .iter()
                .filter(|ed| ed.body == e.body && ed.out.is_some_and(|o| wanted.contains(&o)))
                .filter_map(|ed| ed.sel.clone())
                .collect();
        }
        let (Some(t), Some(bt)) = (e.topo, self.vd.topo.get(&e.body)) else {
            return e.sel.clone().into_iter().collect();
        };
        let mut seq = if ring {
            bt.mesh.edge_ring(t)
        } else {
            bt.mesh.edge_loop(t)
        };
        if alternate {
            seq = TopoMesh::every_other(&seq, t);
        }
        let start_kind = e.kind;
        seq.iter()
            .filter_map(|te| bt.edge_item.get(te))
            .map(|&ix| &self.vd.edges[ix])
            // Walks pass through fill edges but only real edges get selected (SPEC §6.7).
            .filter(|ed| ed.kind != EdgeKind::Fill || start_kind == EdgeKind::Fill)
            .filter_map(|ed| ed.sel.clone())
            .collect()
    }

    pub fn select_loop(&mut self, ring: bool, alternate: bool) {
        if self.mode != Mode::Edge {
            self.toast(ToastKind::Info, S::TOAST_SELECT_EDGES);
            return;
        }
        let starts: Vec<usize> = self
            .sel
            .edges
            .iter()
            .filter_map(|s| self.vd.edge_index(s))
            .collect();
        if starts.is_empty() {
            self.toast(ToastKind::Info, S::TOAST_SELECT_EDGES);
            return;
        }
        let mut out = BTreeSet::new();
        for i in starts {
            out.extend(self.loop_from(i, ring, alternate));
        }
        self.sel.edges = out;
        self.on_selection_changed();
    }

    pub fn select_all(&mut self) {
        match self.mode {
            Mode::Object => {
                self.sel.objects = self
                    .vd
                    .bodies
                    .iter()
                    .map(|b| b.id)
                    .filter(|b| !self.is_locked(*b))
                    .collect()
            }
            Mode::Face => {
                self.sel.faces = self
                    .vd
                    .face_tris
                    .keys()
                    .filter(|f| !self.is_locked(BodyId(f.body)))
                    .map(|f| (BodyId(f.body), FaceId(f.face)))
                    .collect()
            }
            Mode::Edge => {
                let fill = self.prefs.select_fill_edges;
                self.sel.edges = self
                    .vd
                    .edges
                    .iter()
                    .filter(|e| (fill || e.kind != EdgeKind::Fill) && !self.is_locked(e.body))
                    .filter_map(|e| e.sel.clone())
                    .collect()
            }
            Mode::Vertex => {
                self.sel.verts = self
                    .vd
                    .verts
                    .iter()
                    .filter(|v| !self.is_locked(v.body))
                    .map(|v| v.sel.clone())
                    .collect()
            }
        }
        self.on_selection_changed();
    }

    /// Invert within the objects being edited (component modes) or the
    /// visible scene (object mode) — SPEC §5.3.
    pub fn invert_selection(&mut self) {
        let mut bodies = self.sel.bodies(self.mode);
        bodies.retain(|b| !self.is_locked(*b));
        match self.mode {
            Mode::Object => {
                let all: BTreeSet<BodyId> = self
                    .vd
                    .bodies
                    .iter()
                    .map(|b| b.id)
                    .filter(|b| !self.is_locked(*b))
                    .collect();
                self.sel.objects = all.difference(&self.sel.objects).copied().collect();
            }
            Mode::Face => {
                let all: BTreeSet<(BodyId, FaceId)> = self
                    .vd
                    .face_tris
                    .keys()
                    .map(|f| (BodyId(f.body), FaceId(f.face)))
                    .filter(|(b, _)| bodies.contains(b))
                    .collect();
                self.sel.faces = all.difference(&self.sel.faces).copied().collect();
            }
            Mode::Edge => {
                let fill = self.prefs.select_fill_edges;
                let all: BTreeSet<EdgeSel> = self
                    .vd
                    .edges
                    .iter()
                    .filter(|e| bodies.contains(&e.body) && (fill || e.kind != EdgeKind::Fill))
                    .filter_map(|e| e.sel.clone())
                    .collect();
                self.sel.edges = all.difference(&self.sel.edges).cloned().collect();
            }
            Mode::Vertex => {
                let all: BTreeSet<VertSel> = self
                    .vd
                    .verts
                    .iter()
                    .filter(|v| bodies.contains(&v.body))
                    .map(|v| v.sel.clone())
                    .collect();
                self.sel.verts = all.difference(&self.sel.verts).cloned().collect();
            }
        }
        self.on_selection_changed();
    }

    pub fn hide_selected(&mut self) {
        let bodies = self.sel.bodies(self.mode);
        if bodies.is_empty() {
            self.toast(ToastKind::Info, S::TOAST_SELECT_OBJECTS);
            return;
        }
        self.view.hidden.extend(bodies.iter().copied());
        self.sel.objects.retain(|b| !bodies.contains(b));
        self.sel.faces.retain(|f| !bodies.contains(&f.0));
        self.sel.edges.retain(|e| !bodies.contains(&e.body()));
        self.sel.verts.retain(|v| !bodies.contains(&v.body()));
    }

    /// `/`: isolate the selected objects; again: step through them.
    pub fn isolate_step(&mut self) {
        match &mut self.view.isolate {
            Some(iso) => {
                let n = iso.original.len();
                iso.step = Some(match iso.step {
                    None => 0,
                    Some(i) => (i + 1) % n.max(1),
                });
            }
            None => {
                let bodies: Vec<BodyId> = self.sel.bodies(self.mode).into_iter().collect();
                if bodies.is_empty() {
                    self.toast(ToastKind::Info, S::TOAST_SELECT_OBJECTS);
                    return;
                }
                self.view.isolate = Some(crate::app::Isolate {
                    original: bodies,
                    step: None,
                });
            }
        }
    }

    // ------------------------------------------------------------ overlay

    fn overlay_signature(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.vd_rev.hash(&mut h);
        self.sel.objects.hash(&mut h);
        self.sel.faces.hash(&mut h);
        self.sel.edges.hash(&mut h);
        self.sel.verts.hash(&mut h);
        self.mode.hash(&mut h);
        format!("{:?}", self.hover).hash(&mut h);
        self.tool.signature().hash(&mut h);
        self.scene_toggles()
            .map(|s| (s.symmetry, s.link, format!("{:?}", s.symmetry_axes)))
            .hash(&mut h);
        self.prefs.show_symmetry_planes.hash(&mut h);
        self.edit.hash(&mut h);
        self.mm.merge_from.hash(&mut h);
        h.finish()
    }

    /// Faces that follow the selected ones through Link and Symmetry.
    pub fn partner_faces(&mut self) -> BTreeSet<(BodyId, FaceId)> {
        let mut out = BTreeSet::new();
        let Some(st) = self.scene_toggles() else {
            return out;
        };
        if !(st.link || st.symmetry) || self.sel.faces.is_empty() {
            return out;
        }
        let sel = self.sel.faces.clone();
        let Some(scene) = self.doc.scene.as_mut() else {
            return out;
        };
        for (b, f) in &sel {
            if st.link
                && let Ok(v) = crate::docstate::guard(|| scene.link_set(*b, *f))
            {
                out.extend(v.into_iter().map(|x| (*b, x)));
            }
            if st.symmetry
                && let Ok(v) = crate::docstate::guard(|| scene.symmetry_partners(*b, *f))
            {
                out.extend(v.into_iter().map(|x| (*b, x)));
            }
        }
        out.retain(|x| !sel.contains(x));
        out
    }

    fn rebuild_overlay(&mut self) {
        let sig = self.overlay_signature();
        if sig == self.overlay_sig {
            return;
        }
        self.overlay_sig = sig;
        self.overlay_rev = newtopo_render::egui_viewport::next_revision();
        let partners = if self.mode == Mode::Face {
            self.partner_faces()
        } else {
            BTreeSet::new()
        };
        let c = self.colors();
        let mut o = RenderScene::default();
        let tint =
            |o: &mut RenderScene, f: FaceRef, col: [f32; 4], vd: &crate::viewdata::ViewData| {
                if let Some(tris) = vd.face_tris.get(&f) {
                    for &t in tris {
                        let p = vd.pick.tris[t as usize].map(|i| vd.pick.positions[i as usize]);
                        o.translucent.push_triangle(p, [Vec3::Z; 3], col);
                    }
                }
            };
        // Selection.
        match self.mode {
            Mode::Object => {
                for f in self.vd.face_tris.keys() {
                    if self.sel.objects.contains(&BodyId(f.body)) {
                        tint(&mut o, *f, with_alpha(c.selection, 0.16), &self.vd);
                    }
                }
                for e in self
                    .vd
                    .edges
                    .iter()
                    .filter(|e| self.sel.objects.contains(&e.body) && e.kind == EdgeKind::Cad)
                {
                    o.add_polyline(&e.path, with_alpha(c.selection, 0.9), 2.0);
                }
            }
            Mode::Face => {
                // Link sets and symmetry partners of the selection (SPEC §6.11-6.12).
                for (b, f) in &partners {
                    tint(
                        &mut o,
                        FaceRef {
                            body: b.0,
                            face: f.0,
                        },
                        c.linked_tint,
                        &self.vd,
                    );
                }
                for (b, f) in &self.sel.faces {
                    tint(
                        &mut o,
                        FaceRef {
                            body: b.0,
                            face: f.0,
                        },
                        with_alpha(c.selection, 0.32),
                        &self.vd,
                    );
                }
            }
            Mode::Edge => {
                // A contrasting halo under the selection keeps it readable
                // over CAD edges and generated lines in both themes.
                let bg = c.background_top;
                let light = 0.2126 * bg[0] + 0.7152 * bg[1] + 0.0722 * bg[2] > 0.5;
                let halo = if light {
                    [1.0, 1.0, 1.0, 0.95]
                } else {
                    [0.0, 0.0, 0.0, 0.55]
                };
                for e in &self.sel.edges {
                    if let Some(i) = self.vd.edge_index(e) {
                        o.add_polyline(&self.vd.edges[i].path, halo, 6.5);
                    }
                }
                for e in &self.sel.edges {
                    if let Some(i) = self.vd.edge_index(e) {
                        o.add_polyline(&self.vd.edges[i].path, c.selection, 4.0);
                    }
                }
            }
            Mode::Vertex => {
                for v in &self.sel.verts {
                    if let Some(i) = self.vd.vert_index(v) {
                        o.add_point(self.vd.verts[i].pos, c.selection, 9.0);
                    }
                }
            }
        }
        // Hover.
        match self.hover {
            Some(Hover::Object(b, _)) => {
                for f in self.vd.face_tris.keys().filter(|f| f.body == b.0) {
                    tint(&mut o, *f, with_alpha(c.hover, 0.07), &self.vd);
                }
            }
            Some(Hover::Face(f, _))
                if self.tool.active != Tool::Iso && self.tool.active != Tool::Cut =>
            {
                tint(&mut o, f, with_alpha(c.hover, 0.12), &self.vd);
            }
            Some(Hover::Edge(i, _)) => o.add_polyline(&self.vd.edges[i].path, c.hover, 3.2),
            Some(Hover::Vert(i)) => o.add_point(self.vd.verts[i].pos, c.hover, 10.0),
            _ => {}
        }
        // Armed CAD-edge delete.
        if let Some((armed, _)) = &self.tool.cad_delete_armed {
            for e in armed {
                if let Some(i) = self.vd.edge_index(e) {
                    o.add_polyline(&self.vd.edges[i].path, with_alpha(c.warning, 1.0), 4.0);
                }
            }
        }
        if self.in_mesh_mode() {
            // Target Merge: the vertex picked first.
            if let Some((b, v)) = self.mm.merge_from
                && let Some(i) = self.vd.vert_index(&VertSel::Index(b, v))
            {
                o.add_point(self.vd.verts[i].pos, c.weld_from, 11.0);
            }
            self.overlay = Arc::new(o);
            return;
        }
        self.connect_arrows(&mut o, &c);
        self.symmetry_planes(&mut o, &c);
        self.tool_overlay(&mut o, &c);
        self.overlay = Arc::new(o);
    }

    /// Connect direction arrows on selected patches (SPEC §5.2, §6.2).
    /// Directions come from the sample model; for real scenes the facade
    /// has no bend-direction query yet (docs/notes/app.md).
    fn connect_arrows(&self, o: &mut RenderScene, c: &newtopo_render::OverlayColors) {
        if self.tool.active != Tool::Connect {
            return;
        }
        if self.doc.has_scene() {
            let r = self.scene_sphere().1 as f32;
            for key in &self.sel.faces {
                for ar in self.vd.arrows.get(key).into_iter().flatten() {
                    let len = (r * 0.12).min(ar.size * 0.6).max(r * 0.03);
                    let lift = ar.normal * r * 0.004;
                    if let Some(a) = ar.a {
                        let o0 = ar.a_origin.unwrap_or(ar.origin) + lift;
                        o.add_arrow(
                            o0 - a * len * 0.5,
                            a,
                            ar.normal.cross(a),
                            len,
                            c.arrow_a,
                            3.0,
                        );
                    }
                    if let Some(b) = ar.b {
                        let o0 = ar.b_origin.unwrap_or(ar.origin) + lift;
                        o.add_arrow(
                            o0 - b * len * 0.5,
                            b,
                            ar.normal.cross(b),
                            len,
                            c.arrow_b,
                            3.0,
                        );
                    }
                }
            }
            return;
        }
        let Some(demo) = &self.demo else { return };
        let r = self.scene_sphere().1 as f32;
        let eye = self.view.camera.eye().as_vec3();
        for (b, f) in &self.sel.faces {
            let Some(face) = demo
                .bodies
                .iter()
                .find(|x| x.id == b.0)
                .and_then(|x| x.faces.iter().find(|y| y.id == f.0))
            else {
                continue;
            };
            if face.polys.is_empty() || face.surface == "plane" || face.surface == "extrusion" {
                continue;
            }
            // The quad that faces the camera best carries the arrows.
            let best = face
                .polys
                .iter()
                .zip(&face.poly_normals)
                .filter(|(p, _)| p.len() >= 4)
                .max_by(|a, b| {
                    let s = |(p, n): &(&Vec<Vec3>, &Vec3)| {
                        let c = p.iter().copied().sum::<Vec3>() / p.len() as f32;
                        n.dot((eye - c).normalize_or_zero())
                    };
                    s(a).total_cmp(&s(b))
                });
            let Some((poly, n)) = best else { continue };
            let centre = poly.iter().copied().sum::<Vec3>() / poly.len() as f32 + *n * r * 0.004;
            let len = r * 0.16;
            let du = (poly[1] - poly[0]).normalize_or_zero();
            o.add_arrow(
                centre - du * len * 0.5,
                du,
                n.cross(du),
                len,
                c.arrow_a,
                3.0,
            );
            if face.surface == "torus" {
                let dv = (poly[3] - poly[0]).normalize_or_zero();
                o.add_arrow(
                    centre - dv * len * 0.5,
                    dv,
                    n.cross(dv),
                    len,
                    c.arrow_b,
                    3.0,
                );
            }
        }
    }

    fn symmetry_planes(&self, o: &mut RenderScene, c: &newtopo_render::OverlayColors) {
        if !self.prefs.show_symmetry_planes {
            return;
        }
        let Some(s) = self.scene_toggles() else {
            return;
        };
        if !s.symmetry {
            return;
        }
        let r = self.scene_sphere().1 as f32 * 1.1;
        for a in &s.symmetry_axes {
            let n = match a {
                newtopo_core::doc::Axis::X => Vec3::X,
                newtopo_core::doc::Axis::Y => Vec3::Y,
                newtopo_core::doc::Axis::Z => Vec3::Z,
            };
            let centre = self.scene_sphere().0.as_vec3();
            let centre = centre - n * centre.dot(n);
            o.add_plane(centre, n, r, c.symmetry_plane);
        }
    }

    // ------------------------------------------------------------ painting

    /// Before | After, side by side, same camera (SPEC §5.10).
    fn paint_compare(&mut self, ui: &mut Ui, rect: Rect) {
        self.ensure_before_view();
        let pal = self.pal;
        let ppp = ui.ctx().pixels_per_point();
        let c = self.colors();
        let mid = rect.center().x;
        let left = Rect::from_min_max(rect.min, pos2(mid - 1.0, rect.max.y));
        let right = Rect::from_min_max(pos2(mid + 1.0, rect.min.y), rect.max);
        if self.gpu_ok {
            let (centre, radius) = self.scene_sphere();
            let opts = RenderOptions {
                shade: if self.view.preview && self.view.flat {
                    ShadeMode::Flat
                } else {
                    ShadeMode::Smooth
                },
                background_top: c.background_top,
                background_bottom: c.background_bottom,
                pixels_per_point: ppp,
                ..Default::default()
            };
            let size = |r: Rect| {
                [
                    (r.width() * ppp).round().max(1.0) as u32,
                    (r.height() * ppp).round().max(1.0) as u32,
                ]
            };
            let cam = |r: Rect| {
                self.view
                    .camera
                    .matrices([r.width().max(1.0), r.height().max(1.0)], centre, radius)
            };
            if let Some((_, before, rev)) = &self.before_vd {
                ui.painter().add(paint_callback(
                    left,
                    ViewportCallback {
                        layers: vec![(*rev, before.base.clone())],
                        camera: cam(left),
                        scene_radius: radius as f32,
                        options: opts,
                        size_px: size(left),
                        slot: 1,
                    },
                ));
            }
            ui.painter().add(paint_callback(
                right,
                ViewportCallback {
                    // No selection/tool overlay: both halves show the same
                    // things, so only the change differs.
                    layers: vec![(self.vd_rev, self.vd.base.clone())],
                    camera: cam(right),
                    scene_radius: radius as f32,
                    options: opts,
                    size_px: size(right),
                    slot: 0,
                },
            ));
        } else {
            ui.painter().rect_filled(rect, 0.0, pal.inset);
        }
        let p = ui.painter();
        p.rect_filled(
            Rect::from_min_max(pos2(mid - 1.0, rect.min.y), pos2(mid + 1.0, rect.max.y)),
            0.0,
            pal.accent,
        );
        // Labels, and what auto-fill does when that's the change.
        let autofill_change = self.doc.scene.as_ref().and_then(|s| {
            crate::docstate::guard(|| {
                s.preview_base()
                    .map(|b| b.settings.autofill != s.doc().settings.autofill)
            })
            .ok()
            .flatten()
        }) == Some(true);
        for (r, label) in [(left, S::COMPARE_BEFORE), (right, S::COMPARE_AFTER)] {
            let g = p.layout_no_wrap(
                label.to_string(),
                crate::theme::medium(13.0),
                pal.text_strong,
            );
            let chip = Rect::from_min_size(
                pos2(r.center().x - g.size().x * 0.5 - 10.0, r.top() + 12.0),
                g.size() + vec2(20.0, 8.0),
            );
            p.rect_filled(chip, 8.0, pal.panel_raised.gamma_multiply(0.92));
            p.rect_stroke(
                chip,
                8.0,
                Stroke::new(1.0, pal.border),
                egui::StrokeKind::Inside,
            );
            p.galley(chip.min + vec2(10.0, 4.0), g, pal.text_strong);
        }
        if autofill_change {
            let w = (rect.width() - 80.0).clamp(200.0, 560.0);
            let g = p.layout(
                S::AUTOFILL_EXPLAIN.to_string(),
                egui::FontId::proportional(12.5),
                pal.text,
                w,
            );
            let r = Rect::from_min_size(
                pos2(
                    rect.center().x - (g.size().x + 24.0) * 0.5,
                    rect.top() + 46.0,
                ),
                g.size() + vec2(24.0, 14.0),
            );
            p.rect_filled(r, 8.0, pal.panel_raised.gamma_multiply(0.95));
            p.rect_stroke(
                r,
                8.0,
                Stroke::new(1.0, pal.border),
                egui::StrokeKind::Inside,
            );
            p.galley(r.min + vec2(12.0, 7.0), g, pal.text);
        }
    }

    fn paint_viewport(&mut self, ui: &mut Ui, rect: Rect) {
        let ppp = ui.ctx().pixels_per_point();
        let c = self.colors();
        if !self.gpu_ok {
            ui.painter().rect_filled(rect, 0.0, self.pal.inset);
            return;
        }
        let cam = self.camera_matrices();
        let (_, radius) = self.scene_sphere();
        let opts = RenderOptions {
            shade: if self.view.preview && self.view.flat {
                ShadeMode::Flat
            } else {
                ShadeMode::Smooth
            },
            background_top: c.background_top,
            background_bottom: c.background_bottom,
            pixels_per_point: ppp,
            ..Default::default()
        };
        let size_px = [
            (rect.width() * ppp).round().max(1.0) as u32,
            (rect.height() * ppp).round().max(1.0) as u32,
        ];
        let cb = ViewportCallback {
            layers: vec![
                (self.vd_rev, self.vd.base.clone()),
                (self.overlay_rev, self.overlay.clone()),
            ],
            camera: cam,
            scene_radius: radius as f32,
            options: opts,
            size_px,
            slot: 0,
        };
        ui.painter().add(paint_callback(rect, cb));

        // 2D overlays: box / lasso.
        let p = ui.painter();
        let accent = self.pal.accent;
        match &self.drag {
            Some(DragState::Box { start, .. }) => {
                if let Some(end) = ui.input(|i| i.pointer.hover_pos()) {
                    let r = Rect::from_two_pos(*start, end);
                    p.rect_filled(r, 2.0, accent.gamma_multiply(0.10));
                    p.rect_stroke(r, 2.0, Stroke::new(1.0, accent), egui::StrokeKind::Inside);
                }
            }
            Some(DragState::Lasso { pts, .. }) if pts.len() > 1 => {
                p.add(egui::Shape::closed_line(
                    pts.clone(),
                    Stroke::new(1.2, accent),
                ));
            }
            _ => {}
        }
    }

    fn hud(&mut self, ui: &mut Ui, rect: Rect) {
        let pal = self.pal;
        let p = ui.painter();
        let mut y = rect.top() + 10.0;
        let x = rect.left() + 12.0;
        let font = egui::FontId::proportional(12.0);
        let txt = if self.view.camera.ortho {
            S::VP_ORTHO
        } else {
            S::VP_PERSP
        };
        let label = |p: &egui::Painter, y: f32, s: &str, col: Color32| {
            p.text(pos2(x, y), egui::Align2::LEFT_TOP, s, font.clone(), col);
        };
        if self.has_content() {
            label(p, y, txt, pal.text_dim);
            y += 17.0;
            if self.in_mesh_mode() {
                label(p, y, S::MESH_HUD, pal.mesh_accent);
                y += 17.0;
            } else if self.view.preview {
                label(
                    p,
                    y,
                    if self.view.wireframe {
                        "Preview · wireframe"
                    } else {
                        "Preview"
                    },
                    pal.accent,
                );
                y += 17.0;
            }
            if let Some(iso) = &self.view.isolate {
                let s = match iso.step {
                    None => format!("{} ({})", S::VP_ISOLATED, iso.original.len()),
                    Some(i) => format!("{} {}/{}", S::VP_ISOLATED, i + 1, iso.original.len()),
                };
                label(p, y, &s, pal.warn);
                y += 17.0;
            }
            if self.is_demo() {
                label(p, y, S::DEMO_BADGE, pal.text_dim);
            }
        }
        if self.in_mesh_mode() && self.has_content() {
            self.mesh_badge(ui, rect);
        }
        // Tool chip at the bottom centre.
        if self.tool.active != Tool::Select {
            let text = self.tool.chip_text();
            let galley = p.layout_no_wrap(text, egui::FontId::proportional(12.5), pal.text_strong);
            let size = galley.size() + vec2(28.0, 12.0);
            let r = Rect::from_center_size(pos2(rect.center().x, rect.bottom() - 24.0), size);
            p.rect_filled(r, 14.0, pal.panel_raised.gamma_multiply(0.94));
            p.rect_stroke(
                r,
                14.0,
                Stroke::new(1.0, pal.border),
                egui::StrokeKind::Inside,
            );
            p.galley(r.min + vec2(14.0, 6.0), galley, pal.text_strong);
            self.view.chip_rect = Some(r);
        }
    }

    fn view_cube(&mut self, ui: &mut Ui, rect: Rect) {
        if !self.has_content() {
            return;
        }
        let pal = self.pal;
        let size = CUBE_SIZE;
        let center = pos2(
            rect.right() - size * 0.5 - 16.0,
            rect.top() + size * 0.5 + 14.0,
        );
        let cam = &self.view.camera;
        let (bx, by, bz) = cam.world_basis();
        let right = cam.right();
        let up = cam.cam_up();
        let fwd = cam.forward();
        let s = size * 0.3;
        let proj = |v: DVec3| center + vec2(v.dot(right) as f32, -v.dot(up) as f32) * s;
        // (normal, label, view)
        let faces = [
            (-by, S::VIEW_FRONT, NamedView::Front),
            (by, S::VIEW_BACK, NamedView::Back),
            (bx, S::VIEW_RIGHT, NamedView::Right),
            (-bx, S::VIEW_LEFT, NamedView::Left),
            (bz, S::VIEW_TOP, NamedView::Top),
            (-bz, S::VIEW_BOTTOM, NamedView::Bottom),
        ];
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let clicked = ui.input(|i| i.pointer.primary_clicked());
        let mut drawn: Vec<(f64, Vec<Pos2>, &str, NamedView, f64)> = Vec::new();
        for (n, label, view) in faces {
            let facing = -n.dot(fwd);
            if facing <= 0.02 {
                continue;
            }
            let (t1, t2) = if n.cross(bz).length() < 1e-6 {
                (bx, by)
            } else {
                (bz, n.cross(bz).normalize())
            };
            let corners = [n + t1 + t2, n + t1 - t2, n - t1 - t2, n - t1 + t2].map(&proj);
            drawn.push((facing, corners.to_vec(), label, view, facing));
        }
        drawn.sort_by(|a, b| a.0.total_cmp(&b.0));
        let p = ui.painter();
        p.circle_filled(center, size * 0.62, pal.panel.gamma_multiply(0.55));
        let mut hovered_view = None;
        for (_, pts, label, view, facing) in &drawn {
            let hovered = pointer.is_some_and(|q| point_in_quad(q, pts));
            if hovered {
                hovered_view = Some(*view);
            }
            let shade = 0.55 + 0.45 * *facing as f32;
            let fill = if hovered {
                pal.accent.gamma_multiply(0.85)
            } else {
                lerp_color(pal.panel, pal.widget_active, shade)
            };
            p.add(egui::Shape::convex_polygon(
                pts.clone(),
                fill,
                Stroke::new(1.0, pal.border),
            ));
            if *facing > 0.35 {
                let c = pts.iter().fold(egui::Vec2::ZERO, |a, q| a + q.to_vec2()) / 4.0;
                let col = if hovered { pal.accent_text } else { pal.text };
                p.text(
                    c.to_pos2(),
                    egui::Align2::CENTER_CENTER,
                    *label,
                    egui::FontId::proportional(10.5),
                    col.gamma_multiply(0.4 + 0.6 * *facing as f32),
                );
            }
        }
        // Axis stubs from the back-bottom-left corner.
        let corner = -(DVec3::X + DVec3::Y + DVec3::Z);
        for (axis, col) in [
            (DVec3::X, Color32::from_rgb(232, 92, 92)),
            (DVec3::Y, Color32::from_rgb(120, 200, 110)),
            (DVec3::Z, Color32::from_rgb(90, 150, 240)),
        ] {
            p.line_segment(
                [proj(corner), proj(corner + axis * 0.9)],
                Stroke::new(2.0, col),
            );
        }
        // Buttons under the cube: frame all, ortho/persp.
        let by0 = center.y + size * 0.62 + 6.0;
        let b1 = Rect::from_center_size(pos2(center.x - 14.0, by0 + 11.0), vec2(24.0, 22.0));
        let b2 = Rect::from_center_size(pos2(center.x + 14.0, by0 + 11.0), vec2(24.0, 22.0));
        let r1 = ui
            .interact(b1, ui.id().with("vc_frame"), Sense::click())
            .on_hover_text("Frame all  (F with nothing selected)");
        let r2 = ui
            .interact(b2, ui.id().with("vc_ortho"), Sense::click())
            .on_hover_text(format!("{} / {}  (O)", S::VP_PERSP, S::VP_ORTHO));
        for (r, rr, icon, on) in [
            (b1, &r1, Icon::Frame, false),
            (b2, &r2, Icon::Cube, self.view.camera.ortho),
        ] {
            let bg = if rr.hovered() {
                pal.widget_hover
            } else {
                pal.panel.gamma_multiply(0.7)
            };
            ui.painter().rect_filled(r, 5.0, bg);
            icons::paint(
                ui.painter(),
                r.shrink(5.0),
                icon,
                if on { pal.accent } else { pal.text },
            );
        }
        if r1.clicked() {
            self.frame_bounds(None);
        }
        if r2.clicked() {
            self.toggle_ortho();
        }
        if clicked && let Some(v) = hovered_view {
            self.named_view(v);
        }
    }

    /// Jump to a named view. The six axis views switch a perspective camera
    /// to orthographic until the next orbit; an orthographic camera the user
    /// chose stays as it is.
    pub(crate) fn named_view(&mut self, v: NamedView) {
        self.view.camera.set_view(v);
        if v == NamedView::Iso {
            self.leave_auto_ortho();
        } else if !self.view.camera.ortho {
            self.view.camera.ortho = true;
            self.view.auto_ortho = true;
        }
    }

    /// The user's own choice of projection; it is never switched back.
    pub(crate) fn toggle_ortho(&mut self) {
        self.view.camera.ortho = !self.view.camera.ortho;
        self.view.auto_ortho = false;
    }

    fn leave_auto_ortho(&mut self) {
        if std::mem::take(&mut self.view.auto_ortho) {
            self.view.camera.ortho = false;
        }
    }

    fn empty_state(&mut self, ui: &mut Ui, rect: Rect) {
        let pal = self.pal;
        let card = Rect::from_center_size(
            rect.center(),
            vec2(440.0_f32.min(rect.width() - 32.0), 250.0),
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(card)
                .layout(egui::Layout::top_down(egui::Align::Center)),
        );
        let p = child.painter().clone();
        // Illustration: a CAD face being split into quads.
        let ill =
            Rect::from_center_size(pos2(card.center().x, card.top() + 40.0), vec2(96.0, 56.0));
        let skew = |x: f32, y: f32| {
            pos2(
                ill.left() + x * ill.width() + (1.0 - y) * 14.0,
                ill.top() + y * ill.height(),
            )
        };
        let outline = [
            skew(0.0, 0.0),
            skew(0.86, 0.0),
            skew(0.86, 1.0),
            skew(0.0, 1.0),
        ];
        p.add(egui::Shape::convex_polygon(
            outline.to_vec(),
            pal.widget,
            Stroke::new(1.5, pal.text_dim),
        ));
        for k in 1..4 {
            let t = k as f32 / 4.0 * 0.86;
            p.line_segment([skew(t, 0.0), skew(t, 1.0)], Stroke::new(1.3, pal.accent));
        }
        p.line_segment(
            [skew(0.0, 0.5), skew(0.86, 0.5)],
            Stroke::new(1.3, pal.accent.gamma_multiply(0.6)),
        );
        child.add_space(84.0);
        child.label(
            RichText::new(S::EMPTY_TITLE)
                .font(crate::theme::medium(19.0))
                .color(pal.text_strong),
        );
        child.add_space(2.0);
        child.label(RichText::new(S::EMPTY_BODY).color(pal.text_dim));
        child.add_space(14.0);
        child.horizontal(|ui| {
            let w = 3.0 * 116.0 + 16.0;
            ui.add_space(((card.width() - w) * 0.5).max(0.0));
            let primary = egui::Button::new(RichText::new(S::EMPTY_IMPORT).color(pal.accent_text))
                .fill(pal.accent)
                .min_size(vec2(116.0, 30.0));
            if ui.add(primary).clicked() {
                self.import_dialog();
            }
            if ui
                .add(egui::Button::new(S::EMPTY_OPEN).min_size(vec2(116.0, 30.0)))
                .clicked()
            {
                self.run(crate::commands::Cmd::Open);
            }
            if ui
                .add(egui::Button::new(S::EMPTY_DEMO).min_size(vec2(116.0, 30.0)))
                .clicked()
            {
                self.load_sample();
            }
        });
        child.add_space(14.0);
        child.label(RichText::new(S::EMPTY_HINT).small().color(pal.text_dim));
    }
}

/// Screen area of the view cube and the buttons under it.
fn cube_area(rect: Rect) -> Rect {
    let size = CUBE_SIZE;
    let center = pos2(
        rect.right() - size * 0.5 - 16.0,
        rect.top() + size * 0.5 + 14.0,
    );
    Rect::from_center_size(
        center + vec2(0.0, 16.0),
        vec2(size * 1.3, size * 1.3 + 32.0),
    )
}

const CUBE_SIZE: f32 = 84.0;

fn point_in_quad(p: Pos2, q: &[Pos2]) -> bool {
    let pts: Vec<DVec2> = q
        .iter()
        .map(|x| DVec2::new(x.x as f64, x.y as f64))
        .collect();
    newtopo_render::pick::point_in_polygon(DVec2::new(p.x as f64, p.y as f64), &pts)
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Modifiers, PointerButton};

    #[test]
    fn navigation_presets() {
        let none = Modifiers::NONE;
        assert_eq!(
            nav_for(NavPreset::Blender, PointerButton::Middle, none),
            Some(NavKind::Orbit)
        );
        assert_eq!(
            nav_for(NavPreset::Blender, PointerButton::Middle, Modifiers::SHIFT),
            Some(NavKind::Pan)
        );
        assert_eq!(
            nav_for(NavPreset::Blender, PointerButton::Primary, none),
            None
        );
        assert_eq!(
            nav_for(NavPreset::Maya, PointerButton::Primary, Modifiers::ALT),
            Some(NavKind::Orbit)
        );
        assert_eq!(
            nav_for(NavPreset::Maya, PointerButton::Secondary, Modifiers::ALT),
            Some(NavKind::Zoom)
        );
        assert_eq!(
            nav_for(NavPreset::Max, PointerButton::Middle, none),
            Some(NavKind::Pan)
        );
        assert_eq!(
            nav_for(NavPreset::Max, PointerButton::Middle, Modifiers::ALT),
            Some(NavKind::Orbit)
        );
    }
}
