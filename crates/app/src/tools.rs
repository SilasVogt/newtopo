//! Tool state machines (SPEC §6) wired to `Scene::apply(Op::…)`.

use crate::app::AppState;
use crate::docstate::guard;
use crate::selection::{EdgeSel, Mode, VertSel};
use crate::strings as S;
use crate::toasts::ToastKind;
use crate::viewdata::EdgeKind;
use crate::viewport::Hover;
use egui::{Key, Modifiers, Pos2};
use glam::{DVec2, DVec3, Vec3};
use newtopo_core::doc::{CutPoint, IsoDir, PatchId, Rule, SliceScope, Spacing};
use newtopo_core::ops::{Op, PatchSel};
use newtopo_kernel::{BodyId, EdgeId, FaceId, UvClass};
use newtopo_render::OverlayColors;
use newtopo_render::pick::FaceRef;
use newtopo_render::scene::{RenderScene, with_alpha};
use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};

/// A wheel gesture coalesces into one undo step once it stops for this long.
pub const COALESCE_SECS: f64 = 0.4;
/// Second Delete on a CAD edge must follow within this window (SPEC §5.8).
pub const CAD_DELETE_SECS: f64 = 1.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Tool {
    #[default]
    Select,
    Connect,
    Grid,
    Slice,
    Iso,
    Cut,
    Weld,
}

impl Tool {
    pub fn title(self) -> &'static str {
        match self {
            Tool::Select => S::TAB_TOOL,
            Tool::Connect => "Connect",
            Tool::Grid => "Surface Grid",
            Tool::Slice => "Slice",
            Tool::Iso => "Iso",
            Tool::Cut => "Cut",
            Tool::Weld => "Target Weld",
        }
    }
    pub fn hint(self) -> &'static str {
        match self {
            Tool::Select => S::HINT_SELECT,
            Tool::Connect => S::HINT_CONNECT,
            Tool::Grid => S::HINT_GRID,
            Tool::Slice => S::HINT_SLICE,
            Tool::Iso => S::HINT_ISO,
            Tool::Cut => S::HINT_CUT,
            Tool::Weld => S::HINT_WELD,
        }
    }
    pub fn is_segment(self) -> bool {
        matches!(self, Tool::Connect | Tool::Grid | Tool::Slice)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SegParams {
    pub a: u32,
    pub b: u32,
    pub u: u32,
    pub v: u32,
    pub slice: u32,
    pub start_on_seam: bool,
    pub spacing: Spacing,
    pub through: bool,
    pub guide: Option<(BodyId, EdgeId)>,
}

impl Default for SegParams {
    fn default() -> Self {
        SegParams {
            a: 8,
            b: 0,
            u: 4,
            v: 4,
            slice: 8,
            start_on_seam: false,
            spacing: Spacing::ArcLength,
            through: false,
            guide: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct IsoPreview {
    pub body: BodyId,
    pub face: FaceId,
    pub value: f64,
    pub seed: f64,
    pub pts: Vec<Vec<Vec3>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CutPick {
    pub point: CutPoint,
    pub pos: Vec3,
}

/// Where a Cut click would land.
#[derive(Clone, Debug, PartialEq)]
pub struct CutCandidate {
    pub hover: Hover,
    pub point: Option<CutPoint>,
    pub pos: Vec3,
}

#[derive(Clone, Debug, Default)]
pub struct ToolState {
    pub active: Tool,
    pub seg: SegParams,
    pub last_lines_added: Option<u32>,
    pub picking_guide: bool,
    wheel_last: Option<f64>,
    wheel_applied: bool,
    pub iso_dir: IsoDirSel,
    pub iso_split: bool,
    pub iso_through: bool,
    pub iso_preview: Option<IsoPreview>,
    pub snap: bool,
    pub cut: Vec<CutPick>,
    pub cut_hover: Option<CutCandidate>,
    pub weld_from: Option<VertSel>,
    pub cad_delete_armed: Option<(Vec<EdgeSel>, f64)>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum IsoDirSel {
    #[default]
    U,
    V,
}

impl IsoDirSel {
    pub fn core(self) -> IsoDir {
        match self {
            IsoDirSel::U => IsoDir::U,
            IsoDirSel::V => IsoDir::V,
        }
    }
}

impl ToolState {
    /// Cancel a pending action (Esc step 1). True if something was pending.
    pub fn cancel_pending(&mut self) -> bool {
        if !self.cut.is_empty() {
            self.cut.clear();
            return true;
        }
        if self.weld_from.take().is_some() {
            return true;
        }
        if self.picking_guide {
            self.picking_guide = false;
            return true;
        }
        if self.cad_delete_armed.take().is_some() {
            return true;
        }
        false
    }

    pub fn signature(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.active.hash(&mut h);
        format!(
            "{:?}{:?}{:?}{:?}",
            self.iso_preview
                .as_ref()
                .map(|p| (p.face, p.value.to_bits())),
            self.cut,
            self.cut_hover,
            self.weld_from
        )
        .hash(&mut h);
        self.iso_split.hash(&mut h);
        h.finish()
    }

    pub fn chip_text(&self) -> String {
        let s = &self.seg;
        match self.active {
            Tool::Select => String::new(),
            Tool::Connect => format!("Connect   A {}   B {}", s.a, s.b),
            Tool::Grid => format!("Surface Grid   U {}   V {}", s.u, s.v),
            Tool::Slice => format!(
                "Slice   {} planes{}",
                s.slice,
                if s.through { "   through" } else { "" }
            ),
            Tool::Iso => format!(
                "Iso   {}{}{}",
                if self.iso_dir == IsoDirSel::U {
                    "U"
                } else {
                    "V"
                },
                if self.iso_split { "   split" } else { "" },
                if self.iso_through { "   through" } else { "" }
            ),
            Tool::Cut => format!(
                "Cut   {} point{}",
                self.cut.len(),
                if self.cut.len() == 1 { "" } else { "s" }
            ),
            Tool::Weld => {
                if self.weld_from.is_some() {
                    "Target Weld   pick the target".into()
                } else {
                    "Target Weld   pick the source".into()
                }
            }
        }
    }
}

impl AppState {
    pub fn activate_tool(&mut self, t: Tool) {
        if t == Tool::Connect && !self.has_content() {
            self.toast(ToastKind::Info, S::TOAST_NO_SCENE);
        }
        let switching_segment =
            self.tool.active.is_segment() && t.is_segment() && self.tool.active != t;
        self.tool.active = t;
        self.tool.cancel_pending();
        self.tool.iso_preview = None;
        match t {
            Tool::Connect | Tool::Grid => self.set_mode(Mode::Face),
            Tool::Slice if self.mode != Mode::Object => self.set_mode(Mode::Face),
            Tool::Weld => self.set_mode(Mode::Vertex),
            _ => {}
        }
        // SPEC §6.1: switching between spread tools re-applies the selection.
        if t.is_segment()
            && (switching_segment || self.has_segment_selection())
            && self.doc.has_scene()
        {
            self.apply_segment(false);
        }
    }

    fn has_segment_selection(&self) -> bool {
        match self.tool.active {
            Tool::Slice if self.mode == Mode::Object => !self.sel.objects.is_empty(),
            _ => !self.sel.faces.is_empty(),
        }
    }

    fn selected_patches(&self) -> Vec<PatchId> {
        self.sel
            .faces
            .iter()
            .map(|(b, f)| PatchId::whole(*b, *f))
            .collect()
    }

    /// The ops the active spread tool would apply to the selection.
    pub fn segment_ops(&self) -> Vec<Op> {
        let s = &self.tool.seg;
        let patches = PatchSel {
            body: None,
            faces: vec![],
            patches: self.selected_patches(),
        };
        match self.tool.active {
            Tool::Connect => {
                if s.a == 0 && s.b == 0 {
                    vec![Op::ClearPatches { patches }]
                } else {
                    vec![Op::Connect {
                        patches,
                        a: s.a,
                        b: s.b,
                        start_on_seam: s.start_on_seam,
                    }]
                }
            }
            Tool::Grid => {
                if s.u == 0 && s.v == 0 {
                    vec![Op::ClearPatches { patches }]
                } else {
                    vec![Op::Grid {
                        patches,
                        u: s.u,
                        v: s.v,
                        start_on_seam: s.start_on_seam,
                        spacing: s.spacing,
                    }]
                }
            }
            Tool::Slice => {
                let guide = s.guide.map(|g| g.1);
                if self.mode == Mode::Object {
                    self.sel
                        .objects
                        .iter()
                        .map(|b| {
                            if s.slice == 0 {
                                Op::ClearPatches {
                                    patches: PatchSel {
                                        body: Some(*b),
                                        faces: self.body_face_ids(*b),
                                        patches: vec![],
                                    },
                                }
                            } else {
                                Op::Slice {
                                    body: Some(*b),
                                    patches: None,
                                    count: s.slice,
                                    through: s.through,
                                    guide_edge: guide,
                                }
                            }
                        })
                        .collect()
                } else if s.slice == 0 {
                    vec![Op::ClearPatches { patches }]
                } else {
                    vec![Op::Slice {
                        body: None,
                        patches: Some(patches),
                        count: s.slice,
                        through: s.through,
                        guide_edge: guide,
                    }]
                }
            }
            _ => vec![],
        }
    }

    fn body_face_ids(&self, b: BodyId) -> Vec<u32> {
        self.vd
            .face_tris
            .keys()
            .filter(|f| f.body == b.0)
            .map(|f| f.face)
            .collect()
    }

    /// Apply the spread tool to the selection. `coalesce` merges this
    /// apply with the previous one if it came within [`COALESCE_SECS`].
    pub fn apply_segment(&mut self, coalesce: bool) {
        if !self.has_segment_selection() {
            self.toast(
                ToastKind::Info,
                if self.tool.active == Tool::Slice {
                    S::TOAST_SELECT_OBJECTS
                } else {
                    S::TOAST_SELECT_PATCHES
                },
            );
            return;
        }
        if self.is_demo() {
            self.toast(ToastKind::Info, S::TOAST_DEMO_READONLY);
            return;
        }
        let ops = self.segment_ops();
        // Coalescing workaround (facade has no begin/end gesture API):
        // undo the previous step of this gesture, then apply the new one.
        let merge = coalesce
            && self.tool.wheel_applied
            && self
                .tool
                .wheel_last
                .is_some_and(|t| self.time - t <= COALESCE_SECS);
        if merge {
            for _ in 0..ops.len() {
                self.undo();
            }
        }
        let mut total = 0;
        let mut ok = false;
        for op in ops {
            if let Some(o) = self.apply(op) {
                total += o.lines_added;
                ok = true;
            }
        }
        self.tool.last_lines_added = ok.then_some(total);
        if coalesce {
            self.tool.wheel_applied = ok;
            self.tool.wheel_last = Some(self.time);
        } else {
            self.tool.wheel_applied = false;
            self.tool.wheel_last = None;
        }
    }

    /// Modifier + wheel over the viewport changes the counts (SPEC §6.1/6.2).
    pub fn wheel_count(&mut self, steps: f32, shift: bool) {
        let d = if steps > 0.0 { 1i64 } else { -1 };
        let s = &mut self.tool.seg;
        let slot = match (self.tool.active, shift) {
            (Tool::Connect, true) => &mut s.a,
            (Tool::Connect, false) => &mut s.b,
            (Tool::Grid, true) => &mut s.u,
            (Tool::Grid, false) => &mut s.v,
            (Tool::Slice, _) => &mut s.slice,
            _ => return,
        };
        *slot = (*slot as i64 + d).clamp(0, 999) as u32;
        self.apply_segment(true);
    }

    /// Selecting patches with a rule loads its values (SPEC §6.1).
    pub fn on_selection_changed(&mut self) {
        if !self.tool.active.is_segment() {
            return;
        }
        let Some(scene) = self.doc.scene.as_ref() else {
            return;
        };
        let Ok(rules) = guard(|| scene.doc().rules.clone()) else {
            return;
        };
        let sel: BTreeSet<(BodyId, FaceId)> = self.sel.faces.clone();
        if sel.is_empty() && self.sel.objects.is_empty() {
            return;
        }
        let hits = |p: &PatchId| sel.contains(&(p.body, p.face));
        for r in rules.iter().rev() {
            match &r.rule {
                Rule::Connect {
                    patches,
                    count_a,
                    count_b,
                    start_on_seam,
                    ..
                } if patches.iter().any(hits) => {
                    self.tool.seg.a = *count_a;
                    self.tool.seg.b = *count_b;
                    self.tool.seg.start_on_seam = *start_on_seam;
                    if self.tool.active == Tool::Connect {
                        return;
                    }
                }
                Rule::Grid {
                    patch,
                    count_u,
                    count_v,
                    spacing,
                    start_on_seam,
                } if hits(patch) => {
                    self.tool.seg.u = *count_u;
                    self.tool.seg.v = *count_v;
                    self.tool.seg.spacing = *spacing;
                    self.tool.seg.start_on_seam = *start_on_seam;
                    if self.tool.active == Tool::Grid {
                        return;
                    }
                }
                Rule::Slice {
                    scope,
                    count,
                    through,
                    ..
                } => {
                    let applies = match scope {
                        SliceScope::Body(b) => {
                            self.sel.objects.contains(b) || sel.iter().any(|(x, _)| x == b)
                        }
                        SliceScope::Patches(ps) => ps.iter().any(hits),
                    };
                    if applies {
                        self.tool.seg.slice = *count;
                        self.tool.seg.through = *through;
                        if self.tool.active == Tool::Slice {
                            return;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    pub fn pick_guide_edge(&mut self) {
        self.tool.picking_guide = false;
        let edge = match self.pick_at_hover_edge() {
            Some((b, e)) => (b, e),
            None => {
                self.toast(ToastKind::Info, "Click a CAD edge to use as the guide.");
                return;
            }
        };
        self.tool.seg.guide = Some(edge);
        if self.has_segment_selection() {
            self.apply_segment(false);
        }
    }

    fn pick_at_hover_edge(&self) -> Option<(BodyId, EdgeId)> {
        let p = self.ui_pointer?;
        match self.pick_at(p, Mode::Edge)? {
            Hover::Edge(i, _) => {
                let e = &self.vd.edges[i];
                e.cad_edge.map(|c| (e.body, c))
            }
            _ => None,
        }
    }

    // ------------------------------------------------------------ in-tool keys

    /// Keys inside tools (SPEC §5.8). Returns true when consumed.
    pub fn tool_key(&mut self, key: Key, m: Modifiers) -> bool {
        if m.ctrl || m.command || m.alt || m.shift {
            return false;
        }
        match (self.tool.active, key) {
            (Tool::Iso, Key::Tab) => {
                self.tool.iso_dir = if self.tool.iso_dir == IsoDirSel::U {
                    IsoDirSel::V
                } else {
                    IsoDirSel::U
                };
                self.on_hover_changed();
                true
            }
            (Tool::Iso, Key::X) => {
                self.tool.iso_split = !self.tool.iso_split;
                true
            }
            (Tool::Iso, Key::A) => {
                self.tool.iso_through = !self.tool.iso_through;
                true
            }
            (Tool::Slice, Key::A) => {
                self.tool.seg.through = !self.tool.seg.through;
                if self.has_segment_selection() && self.doc.has_scene() {
                    self.apply_segment(false);
                }
                true
            }
            (Tool::Grid, Key::Tab) => {
                let s = &mut self.tool.seg;
                std::mem::swap(&mut s.u, &mut s.v);
                if self.has_segment_selection() && self.doc.has_scene() {
                    self.apply_segment(false);
                }
                true
            }
            (Tool::Connect, Key::Tab) => {
                // Flip A/B on the hovered patch: the facade has no flip op yet.
                self.toast(ToastKind::Warn, format!("{}: flip A/B", S::TOAST_NOT_YET));
                true
            }
            (Tool::Cut, Key::Enter) => {
                self.cut_finish(false);
                true
            }
            (Tool::Cut, Key::Backspace) => {
                self.tool.cut.pop();
                true
            }
            _ => false,
        }
    }

    // ------------------------------------------------------------ iso

    /// World → body coordinates for a body in the scene.
    fn to_body(&self, body: BodyId, p: DVec3) -> Option<DVec3> {
        let scene = self.doc.scene.as_ref()?;
        let m = guard(|| scene.doc().body(body).map(|b| b.transform)).ok()??;
        Some(m.inverse().transform_point3(p))
    }

    fn body_transform(&self, body: BodyId) -> Option<glam::DMat4> {
        let scene = self.doc.scene.as_ref()?;
        guard(|| scene.doc().body(body).map(|b| b.transform)).ok()?
    }

    /// Exact (u, v) on a face for a world point, through the kernel.
    pub fn face_uv(&self, body: BodyId, face: FaceId, world: DVec3) -> Option<DVec2> {
        let scene = self.doc.scene.as_ref()?;
        let p = self.to_body(body, world)?;
        let k = scene.kernel.clone();
        let r = guard(|| k.project_to_face(body, face, &[p])).ok()?.ok()?;
        r.first().copied().flatten().map(|(uv, _)| uv)
    }

    pub fn on_hover_changed(&mut self) {
        if self.tool.active != Tool::Iso {
            return;
        }
        self.tool.iso_preview = match self.hover {
            Some(Hover::Face(f, p)) => self.iso_preview_for(f, p),
            _ => None,
        };
    }

    fn iso_preview_for(&self, f: FaceRef, p: DVec3) -> Option<IsoPreview> {
        let (body, face) = (BodyId(f.body), FaceId(f.face));
        let uv = self.face_uv(body, face, p)?;
        let scene = self.doc.scene.as_ref()?;
        let surface = guard(|| {
            scene
                .body_view(body)
                .map(|bv| bv.topology.face(face).surface.clone())
        })
        .ok()??;
        let m = self.body_transform(body)?;
        let (value, seed, range) = match self.tool.iso_dir {
            IsoDirSel::U => (uv.x, uv.y, surface.v_range),
            IsoDirSel::V => (uv.y, uv.x, surface.u_range),
        };
        let n = 96;
        let uvs: Vec<DVec2> = (0..=n)
            .map(|i| {
                let t = range.0 + (range.1 - range.0) * i as f64 / n as f64;
                match self.tool.iso_dir {
                    IsoDirSel::U => DVec2::new(value, t),
                    IsoDirSel::V => DVec2::new(t, value),
                }
            })
            .collect();
        let k = scene.kernel.clone();
        let evals = guard(|| k.face_eval(body, face, &uvs)).ok()?.ok()?;
        let classes = guard(|| k.face_classify_uv(body, face, &uvs)).ok()?.ok()?;
        // Keep the inside runs (a trimmed iso line can be several pieces).
        let mut pieces: Vec<Vec<Vec3>> = Vec::new();
        let mut cur: Vec<Vec3> = Vec::new();
        for (e, c) in evals.iter().zip(&classes) {
            if *c == UvClass::Out {
                if cur.len() > 1 {
                    pieces.push(std::mem::take(&mut cur));
                }
                cur.clear();
            } else {
                cur.push(m.transform_point3(e.p).as_vec3());
            }
        }
        if cur.len() > 1 {
            pieces.push(cur);
        }
        Some(IsoPreview {
            body,
            face,
            value,
            seed,
            pts: pieces,
        })
    }

    pub fn iso_click(&mut self) {
        if self.is_demo() {
            self.toast(ToastKind::Info, S::TOAST_DEMO_READONLY);
            return;
        }
        let Some(prev) = self.tool.iso_preview.clone() else {
            if !self.doc.has_scene() {
                self.toast(ToastKind::Info, S::TOAST_NO_SCENE);
            }
            return;
        };
        let op = Op::Iso {
            body: Some(prev.body),
            face: prev.face.0,
            dir: self.tool.iso_dir.core(),
            value: prev.value,
            seed: if self.tool.iso_through {
                None
            } else {
                Some(prev.seed)
            },
            split: self.tool.iso_split,
            through: self.tool.iso_through,
        };
        self.apply(op);
    }

    // ------------------------------------------------------------ cut

    /// Snap cascade (SPEC §6.6): vertices, CAD-edge midpoints, CAD edges,
    /// then the face interior. Without snap only the face is used.
    pub fn cut_pick(&self, p: Pos2) -> Option<CutCandidate> {
        if self.tool.snap {
            if let Some(Hover::Vert(i)) = self.pick_at(p, Mode::Vertex) {
                let v = &self.vd.verts[i];
                let point = match &v.sel {
                    VertSel::Key(b, k) => Some(CutPoint::Vertex {
                        body: *b,
                        key: k.clone(),
                    }),
                    VertSel::Index(..) => None,
                };
                return Some(CutCandidate {
                    hover: Hover::Vert(i),
                    point,
                    pos: v.pos,
                });
            }
            if let Some(Hover::Edge(i, hit)) = self.pick_at(p, Mode::Edge) {
                let e = &self.vd.edges[i];
                if let Some(edge) = e.cad_edge {
                    // Midpoint snap if close to the middle, else nearest point.
                    let mid = e.path[e.path.len() / 2];
                    let cam = self.camera_matrices();
                    let near_mid = cam
                        .project(mid.as_dvec3())
                        .zip(cam.project(hit))
                        .is_some_and(|(a, b)| (a.0 - b.0).length() < 8.0);
                    let world = if near_mid { mid.as_dvec3() } else { hit };
                    let t = self
                        .to_body(e.body, world)
                        .and_then(|bp| self.doc.scene.as_ref().map(|s| (s.kernel.clone(), bp)))
                        .and_then(|(k, bp)| {
                            guard(|| k.project_to_edge(e.body, edge, &[bp])).ok()?.ok()
                        })
                        .and_then(|v| v.first().copied().flatten());
                    let point = t.map(|t| CutPoint::CadEdge {
                        body: e.body,
                        edge,
                        t,
                    });
                    return Some(CutCandidate {
                        hover: Hover::Edge(i, world),
                        point,
                        pos: world.as_vec3(),
                    });
                }
            }
        }
        match self.pick_at(p, Mode::Face)? {
            Hover::Face(f, hit) => {
                let point =
                    self.face_uv(BodyId(f.body), FaceId(f.face), hit)
                        .map(|uv| CutPoint::Face {
                            body: BodyId(f.body),
                            face: FaceId(f.face),
                            uv,
                        });
                Some(CutCandidate {
                    hover: Hover::Face(f, hit),
                    point,
                    pos: hit.as_vec3(),
                })
            }
            _ => None,
        }
    }

    pub fn cut_click(&mut self, p: Pos2) {
        if self.is_demo() {
            self.toast(ToastKind::Info, S::TOAST_DEMO_READONLY);
            return;
        }
        // Clicking the first point again closes the loop.
        if self.tool.cut.len() >= 2 {
            let cam = self.camera_matrices();
            let first = self.tool.cut[0].pos.as_dvec3();
            if let Some((s, _)) = cam.project(first) {
                let local = DVec2::new(
                    (p.x - self.view.rect.left()) as f64,
                    (p.y - self.view.rect.top()) as f64,
                );
                if (s - local).length() < 10.0 {
                    self.cut_finish(true);
                    return;
                }
            }
        }
        match self.cut_pick(p) {
            Some(CutCandidate {
                point: Some(point),
                pos,
                ..
            }) => self.tool.cut.push(CutPick { point, pos }),
            Some(_) => self.toast(ToastKind::Warn, "Couldn't place a point there."),
            None => {}
        }
    }

    pub fn cut_finish(&mut self, closed: bool) {
        if self.tool.cut.len() < 2 {
            self.toast(ToastKind::Info, "A cut needs at least two points.");
            return;
        }
        let points: Vec<CutPoint> = self.tool.cut.iter().map(|c| c.point.clone()).collect();
        if self.apply(Op::Cut { points, closed }).is_some() {
            self.tool.cut.clear();
        }
    }

    // ------------------------------------------------------------ weld, collapse, remove

    pub fn weld(&mut self, from: usize, to: usize) {
        self.tool.weld_from = None;
        let (a, b) = (&self.vd.verts[from], &self.vd.verts[to]);
        match (&a.sel, &b.sel) {
            (VertSel::Key(ba, ka), VertSel::Key(bb, kb)) if ba == bb => {
                let op = Op::Weld {
                    body: Some(*ba),
                    from: ka.clone(),
                    to: kb.clone(),
                    confirmed: false,
                };
                self.apply(op);
            }
            (VertSel::Key(..), VertSel::Key(..)) => {
                self.toast(ToastKind::Warn, "Both vertices must be on the same object.")
            }
            _ => self.toast(ToastKind::Info, S::TOAST_DEMO_READONLY),
        }
    }

    pub fn collapse_selected(&mut self) {
        if self.mode != Mode::Edge || self.sel.edges.is_empty() {
            self.toast(ToastKind::Info, S::TOAST_SELECT_EDGES);
            return;
        }
        let edges: Vec<EdgeSel> = self.sel.edges.iter().cloned().collect();
        for e in edges {
            match e {
                EdgeSel::Key(b, k) => {
                    self.apply(Op::Collapse {
                        body: Some(b),
                        edge: k,
                        confirmed: false,
                    });
                }
                _ => self.toast(ToastKind::Info, S::TOAST_DEMO_READONLY),
            }
        }
        self.sel.edges.clear();
    }

    /// Delete in edge mode (SPEC §6.7, §6.10).
    pub fn remove_selected(&mut self) {
        if self.mode != Mode::Edge || self.sel.edges.is_empty() {
            self.toast(ToastKind::Info, S::TOAST_SELECT_EDGES);
            return;
        }
        if self.is_demo() {
            self.toast(ToastKind::Info, S::TOAST_DEMO_READONLY);
            return;
        }
        let mut generated = Vec::new();
        let mut cad = Vec::new();
        let mut fill = false;
        for s in &self.sel.edges {
            let item = self.vd.edge_index(s).map(|i| &self.vd.edges[i]);
            match (s, item.map(|e| e.kind)) {
                (_, Some(EdgeKind::Fill)) => fill = true,
                (EdgeSel::Key(b, k), Some(EdgeKind::Cad)) | (EdgeSel::Key(b, k), None) => {
                    let cad_id = match k {
                        newtopo_core::keys::EdgeKey::Cad { edge, .. } => Some(*edge),
                        _ => item.and_then(|e| e.cad_edge),
                    };
                    match cad_id {
                        Some(edge) => cad.push((*b, edge, s.clone())),
                        None => generated.push((*b, k.clone())),
                    }
                }
                (EdgeSel::Key(b, k), _) => generated.push((*b, k.clone())),
                (EdgeSel::Cad(b, e), _) => cad.push((*b, *e, s.clone())),
                (EdgeSel::Index(..), _) => fill = true,
            }
        }
        if fill && generated.is_empty() && cad.is_empty() {
            self.toast(ToastKind::Warn, S::TOAST_FILL_EDGE);
            return;
        }
        for (b, k) in generated {
            if self
                .apply(Op::RemoveEdge {
                    body: Some(b),
                    edge: k.clone(),
                })
                .is_some()
            {
                self.sel.edges.remove(&EdgeSel::Key(b, k));
            }
        }
        if !cad.is_empty() {
            let sels: Vec<EdgeSel> = cad.iter().map(|c| c.2.clone()).collect();
            let armed = self
                .tool
                .cad_delete_armed
                .take()
                .filter(|(s, t)| *s == sels && self.time - t <= CAD_DELETE_SECS);
            if armed.is_some() {
                // Second press: confirmation dialog, then apply confirmed.
                let (b, e, _) = cad[0];
                let op = Op::RemoveCadEdge {
                    body: Some(b),
                    edge: e,
                    confirmed: true,
                };
                self.ui.confirm = Some(crate::app::Confirm {
                    title: S::DLG_CAD_EDGE_TITLE.into(),
                    body: S::DLG_CAD_EDGE_BODY.into(),
                    op,
                });
                if cad.len() > 1 {
                    self.toast(
                        ToastKind::Info,
                        "CAD edges are removed one at a time; repeat for the others.",
                    );
                }
            } else {
                self.tool.cad_delete_armed = Some((sels, self.time));
                self.toast(ToastKind::Warn, S::TOAST_CAD_EDGE_ARMED);
            }
        }
    }

    // ------------------------------------------------------------ overlay

    pub fn tool_overlay(&self, o: &mut RenderScene, c: &OverlayColors) {
        match self.tool.active {
            Tool::Iso => {
                if let Some(p) = &self.tool.iso_preview {
                    let col = if self.tool.iso_split {
                        c.split_preview
                    } else {
                        c.line
                    };
                    for piece in &p.pts {
                        o.add_polyline(piece, col, 2.4);
                    }
                }
            }
            Tool::Cut => {
                let col = c.line;
                for w in self.tool.cut.windows(2) {
                    o.add_segment(w[0].pos, w[1].pos, col, 2.2);
                }
                for (i, cp) in self.tool.cut.iter().enumerate() {
                    o.add_point(cp.pos, if i == 0 { c.selection } else { col }, 8.0);
                }
                if let Some(h) = &self.tool.cut_hover {
                    if let Some(last) = self.tool.cut.last() {
                        o.add_dashed_segment(last.pos, h.pos, with_alpha(col, 0.8), 1.6, 10.0);
                    }
                    o.add_point(h.pos, c.hover, 7.0);
                }
            }
            Tool::Weld => {
                if let Some(from) = self
                    .tool
                    .weld_from
                    .as_ref()
                    .and_then(|s| self.vd.vert_index(s))
                {
                    let a = self.vd.verts[from].pos;
                    o.add_point(a, c.weld_from, 11.0);
                    if let Some(Hover::Vert(i)) = self.hover
                        && i != from
                    {
                        let b = self.vd.verts[i].pos;
                        o.add_dashed_segment(a, b, c.weld_to, 2.0, 10.0);
                        o.add_point(b, c.weld_to, 11.0);
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_pending_order() {
        let mut t = ToolState {
            active: Tool::Cut,
            ..Default::default()
        };
        assert!(!t.cancel_pending());
        t.picking_guide = true;
        assert!(t.cancel_pending());
        assert!(!t.cancel_pending());
    }

    #[test]
    fn chip_text_mentions_counts() {
        let t = ToolState {
            active: Tool::Connect,
            ..Default::default()
        };
        assert!(t.chip_text().contains("A 8"));
    }
}
