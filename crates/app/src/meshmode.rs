//! Mesh editing mode (SPEC §9.5): a separate editing mode with its own
//! tools and selection, switched with the `CAD | Mesh` control in the top
//! bar.
//!
//! - Entering bakes every body's output into an editable mesh (nothing is
//!   written yet). The first edit of a body stores its mesh in the
//!   document, which locks the body's CAD stage until "Discard mesh edits".
//! - Edits apply at once, each one undo step; there is no preview and no
//!   confirmation. Changing a tool value redoes the last edit from the mesh
//!   it started on with the new value (the step is amended, still one undo
//!   step).
//! - The CAD selection and component mode are kept aside while mesh
//!   editing and come back on return (the two selections never mix).
//! - Mesh-mode selection uses the shared `Selection`: vertices and edges as
//!   `Index` entries (vertex index; index into `EditMesh::edges()`), faces
//!   as `(body, polygon index)`, so picking, box/lasso, select all and
//!   framing work unchanged.

use crate::app::AppState;
use crate::commands::Cmd;
use crate::docstate::guard;
use crate::kit;
use crate::selection::{Combine, EdgeSel, Mode, Selection, VertSel, combine};
use crate::strings as S;
use crate::toasts::ToastKind;
use crate::viewport::Hover;
use egui::{Color32, Pos2, RichText, Stroke, Ui, vec2};
use glam::{DVec2, DVec3};
use newtopo_core::meshedit::{self as me, EditMesh, EditOutcome};
use newtopo_core::ops::Refusal;
use newtopo_kernel::{BodyId, FaceId};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// CAD editing (rules over the CAD model) or mesh editing (the finished
/// mesh, destructive).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum EditMode {
    #[default]
    Cad,
    Mesh,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MeshTool {
    #[default]
    Select,
    VertexSlide,
    EdgeSlide,
    Relax,
    Space,
    TargetMerge,
    MergeByDistance,
    LoopCleanup,
    Resample,
    Decimate,
}

impl MeshTool {
    pub const MOVE: [MeshTool; 4] = [
        MeshTool::VertexSlide,
        MeshTool::EdgeSlide,
        MeshTool::Relax,
        MeshTool::Space,
    ];
    pub const MERGE: [MeshTool; 5] = [
        MeshTool::TargetMerge,
        MeshTool::MergeByDistance,
        MeshTool::LoopCleanup,
        MeshTool::Resample,
        MeshTool::Decimate,
    ];

    pub fn title(self) -> &'static str {
        match self {
            MeshTool::Select => "Select",
            MeshTool::VertexSlide => "Vertex Slide",
            MeshTool::EdgeSlide => "Edge Slide",
            MeshTool::Relax => "Relax",
            MeshTool::Space => "Space",
            MeshTool::TargetMerge => "Target Merge",
            MeshTool::MergeByDistance => "Merge by Distance",
            MeshTool::LoopCleanup => "Loop Cleanup",
            MeshTool::Resample => "Resample",
            MeshTool::Decimate => "Decimate",
        }
    }

    pub fn cmd(self) -> Option<Cmd> {
        Some(match self {
            MeshTool::Select => return None,
            MeshTool::VertexSlide => Cmd::MeshVertexSlide,
            MeshTool::EdgeSlide => Cmd::MeshEdgeSlide,
            MeshTool::Relax => Cmd::MeshRelax,
            MeshTool::Space => Cmd::MeshSpace,
            MeshTool::TargetMerge => Cmd::MeshTargetMerge,
            MeshTool::MergeByDistance => Cmd::MeshMergeByDistance,
            MeshTool::LoopCleanup => Cmd::MeshLoopCleanup,
            MeshTool::Resample => Cmd::MeshResample,
            MeshTool::Decimate => Cmd::MeshDecimate,
        })
    }

    pub fn hint(self) -> &'static str {
        match self {
            MeshTool::Select => S::HINT_MESH_SELECT,
            MeshTool::VertexSlide => S::HINT_MESH_VERTEX_SLIDE,
            MeshTool::EdgeSlide => S::HINT_MESH_EDGE_SLIDE,
            MeshTool::TargetMerge => S::HINT_MESH_TARGET_MERGE,
            _ => S::HINT_MESH_RUN,
        }
    }

    /// Runs on the selection as soon as it is picked (the others wait for
    /// a drag or clicks in the viewport).
    pub fn instant(self) -> bool {
        matches!(
            self,
            MeshTool::Relax
                | MeshTool::Space
                | MeshTool::MergeByDistance
                | MeshTool::LoopCleanup
                | MeshTool::Resample
                | MeshTool::Decimate
        )
    }

    /// Has the "Stay on CAD surface" option.
    pub fn moves(self) -> bool {
        MeshTool::MOVE.contains(&self)
    }

    fn stay_slot(self) -> Option<usize> {
        MeshTool::MOVE.iter().position(|t| *t == self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResampleBy {
    Even,
    Count,
    Length,
}

/// Values of the mesh tool panel. Distances are percentages of the mesh's
/// bounding-box diagonal, so one set of values suits any model size.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshParams {
    /// Vertices on CAD and hard edges move only along them.
    pub keep_features: bool,
    /// "Stay on CAD surface" per move tool (`MeshTool::MOVE` order).
    pub stay: [bool; 4],
    pub vslide_factor: f64,
    pub vslide_even: bool,
    pub eslide_factor: f64,
    pub eslide_even: bool,
    pub relax_iterations: u32,
    pub relax_cubic: bool,
    pub relax_even: bool,
    pub merge_pct: f64,
    pub merge_selected: bool,
    pub merge_centroid: bool,
    pub cleanup_merge_pct: f64,
    pub cleanup_short_pct: f64,
    pub cleanup_straight_deg: f64,
    pub cleanup_slide: bool,
    pub cleanup_tolerance_pct: f64,
    pub resample_by: ResampleBy,
    pub resample_count: u32,
    pub resample_length_pct: f64,
    pub resample_corner_deg: f64,
    pub decimate_planar: bool,
    pub decimate_pct: f64,
    pub decimate_keep_quads: bool,
    pub decimate_planar_deg: f64,
}

impl Default for MeshParams {
    fn default() -> Self {
        MeshParams {
            keep_features: true,
            stay: [false; 4],
            vslide_factor: 0.5,
            vslide_even: false,
            eslide_factor: 0.5,
            eslide_even: false,
            relax_iterations: 5,
            relax_cubic: true,
            relax_even: true,
            merge_pct: 0.01,
            merge_selected: false,
            merge_centroid: true,
            cleanup_merge_pct: 0.01,
            cleanup_short_pct: 0.5,
            cleanup_straight_deg: 5.0,
            cleanup_slide: true,
            cleanup_tolerance_pct: 0.1,
            resample_by: ResampleBy::Even,
            resample_count: 12,
            resample_length_pct: 2.0,
            resample_corner_deg: 30.0,
            decimate_planar: false,
            decimate_pct: 50.0,
            decimate_keep_quads: true,
            decimate_planar_deg: 5.0,
        }
    }
}

impl MeshParams {
    fn preserve(&self) -> me::Preserve {
        me::Preserve {
            cad_edges: self.keep_features,
            hard_edges: self.keep_features,
        }
    }
    pub fn stay(&self, t: MeshTool) -> bool {
        t.stay_slot().is_some_and(|i| self.stay[i])
    }
}

/// What a mesh tool ran on, per body.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    Edges(Vec<[u32; 2]>),
    Vertices(Vec<u32>),
    /// Vertex Slide: each vertex towards its neighbour (chosen from the
    /// drag direction when the slide started).
    Toward(Vec<[u32; 2]>),
    /// Target Merge: from, to.
    Pair(u32, u32),
    /// The whole mesh (Decimate, Merge by Distance without "Selected only").
    Whole,
}

/// The last mesh edit, for redoing it with new tool values.
#[derive(Clone, Debug)]
pub struct LastOp {
    pub tool: MeshTool,
    /// Per body: the mesh before, whether the body already had mesh edits,
    /// and what the tool ran on.
    pub bodies: Vec<(BodyId, Arc<EditMesh>, bool, Input)>,
    /// Merge by Distance: the vertices selected when it ran, per body, so a
    /// redo can switch between the whole mesh and these ("Selected only").
    pub selected_verts: BTreeMap<BodyId, Vec<u32>>,
    /// Document revision right after the edit (or its last redo); for a
    /// slide drag that has not applied yet, the revision it started on.
    pub rev: u64,
    /// The edit made an undo step that a redo amends. False while its
    /// results changed nothing: a redo with new values then makes its own
    /// step instead of rewriting the step before it.
    pub stepped: bool,
}

/// An element clicked in mesh mode (for shortest paths).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    Vert(BodyId, u32),
    Edge(BodyId, [u32; 2]),
}

impl Pick {
    fn body(self) -> BodyId {
        match self {
            Pick::Vert(b, _) | Pick::Edge(b, _) => b,
        }
    }
}

/// A slide drag in progress.
#[derive(Clone, Debug)]
pub struct SlideDrag {
    pub start: Pos2,
    /// Screen-space slide vector per side (pixels for factor 1); for
    /// Vertex Slide only `pos` is used.
    pub pos: Option<DVec2>,
    pub neg: Option<DVec2>,
    pub started: bool,
    /// The factor last applied.
    pub factor: Option<f64>,
}

#[derive(Default)]
pub struct MeshState {
    pub tool: MeshTool,
    pub params: MeshParams,
    /// The selection and component mode of the mode not shown.
    pub other_sel: Selection,
    pub other_mode: Mode,
    /// The entry warning was shown this session.
    pub warned: bool,
    /// The entry warning is open, with its tick box.
    pub dialog: bool,
    pub dont_show: bool,
    /// Editable mesh per body, for the document revision `meshes_rev`.
    pub meshes: BTreeMap<BodyId, Arc<EditMesh>>,
    pub meshes_rev: Option<u64>,
    /// `EditMesh::edges()` per body (edge selection indices).
    pub edges: BTreeMap<BodyId, Vec<(u32, u32)>>,
    /// Bodies that can't be edited as meshes, with the reason.
    pub unavailable: Vec<(BodyId, String)>,
    pub last: Option<LastOp>,
    /// The last clicked elements, newest last (at most two).
    pub picks: Vec<Pick>,
    pub merge_from: Option<(BodyId, u32)>,
    pub slide: Option<SlideDrag>,
}

impl AppState {
    pub fn in_mesh_mode(&self) -> bool {
        self.edit == EditMode::Mesh
    }

    // ---------------------------------------------------------------- switch

    /// Switch editing mode (the top-bar control, `M`).
    pub fn request_edit_mode(&mut self, m: EditMode) {
        if m == self.edit {
            return;
        }
        match m {
            EditMode::Cad => self.leave_mesh_mode(),
            EditMode::Mesh => {
                if !self.doc.has_scene() {
                    self.toast(ToastKind::Info, S::TOAST_MESH_NO_SCENE);
                    return;
                }
                if self.doc.in_preview() {
                    self.toast(ToastKind::Info, S::PREVIEW_PENDING);
                    return;
                }
                if self.prefs.mesh_warning && !self.mm.warned {
                    self.mm.dialog = true;
                    self.mm.dont_show = false;
                    return;
                }
                self.enter_mesh_mode();
            }
        }
    }

    pub fn toggle_edit_mode(&mut self) {
        let m = match self.edit {
            EditMode::Cad => EditMode::Mesh,
            EditMode::Mesh => EditMode::Cad,
        };
        self.request_edit_mode(m);
    }

    fn enter_mesh_mode(&mut self) {
        self.mm.warned = true;
        self.edit = EditMode::Mesh;
        std::mem::swap(&mut self.sel, &mut self.mm.other_sel);
        std::mem::swap(&mut self.mode, &mut self.mm.other_mode);
        if self.mode == Mode::Object {
            self.mode = Mode::Edge;
        }
        self.tool.cancel_pending();
        self.tool.active = crate::tools::Tool::Select;
        self.drag = None;
        self.hover = None;
        self.end_compare();
        self.mm.last = None;
        self.mm.picks.clear();
        self.mm.merge_from = None;
        self.mm.meshes_rev = None;
        self.ensure_meshes();
        if !self.mm.unavailable.is_empty() {
            let names: Vec<String> = self
                .mm
                .unavailable
                .iter()
                .map(|(_, why)| why.clone())
                .collect();
            self.toast(ToastKind::Warn, names.join(" · "));
        }
        self.invalidate_view();
        self.theme_applied = None;
    }

    fn leave_mesh_mode(&mut self) {
        self.edit = EditMode::Cad;
        std::mem::swap(&mut self.sel, &mut self.mm.other_sel);
        std::mem::swap(&mut self.mode, &mut self.mm.other_mode);
        self.mm.tool = MeshTool::Select;
        self.mm.slide = None;
        self.mm.merge_from = None;
        self.drag = None;
        self.hover = None;
        self.invalidate_view();
        self.theme_applied = None;
    }

    /// Refresh the editable meshes after the document changed. A body whose
    /// mesh changed by anything but a mesh tool (undo, redo, discard, a CAD
    /// edit) loses its mesh-mode selection, whose indices no longer fit.
    pub fn ensure_meshes(&mut self) {
        if self.mm.meshes_rev == Some(self.doc.revision) {
            return;
        }
        self.mm.meshes_rev = Some(self.doc.revision);
        let Some(scene) = self.doc.scene.as_mut() else {
            self.mm.meshes.clear();
            self.mm.edges.clear();
            return;
        };
        let ids = scene.body_ids();
        let mut meshes = BTreeMap::new();
        let mut unavailable = Vec::new();
        for b in ids {
            match guard(|| scene.editable_mesh(b)) {
                Ok(Ok(m)) => {
                    meshes.insert(b, m);
                }
                Ok(Err(r)) => unavailable.push((b, r.reason)),
                Err(e) => unavailable.push((b, e)),
            }
        }
        let changed: BTreeSet<BodyId> = meshes
            .iter()
            .filter(|(b, m)| self.mm.meshes.get(b).is_none_or(|o| !Arc::ptr_eq(o, m)))
            .map(|(b, _)| *b)
            .chain(
                self.mm
                    .meshes
                    .keys()
                    .filter(|b| !meshes.contains_key(b))
                    .copied(),
            )
            .collect();
        let had_any = !self.mm.meshes.is_empty();
        self.mm.edges = meshes.iter().map(|(b, m)| (*b, m.edges())).collect();
        self.mm.meshes = meshes;
        self.mm.unavailable = unavailable;
        if had_any && !changed.is_empty() {
            let sel = if self.in_mesh_mode() {
                &mut self.sel
            } else {
                &mut self.mm.other_sel
            };
            clear_bodies(sel, &changed);
            self.mm.picks.retain(|p| !changed.contains(&p.body()));
            if self
                .mm
                .merge_from
                .is_some_and(|(b, _)| changed.contains(&b))
            {
                self.mm.merge_from = None;
            }
        }
    }

    // ------------------------------------------------------------- selection

    fn edge_of(&self, b: BodyId, i: u32) -> Option<[u32; 2]> {
        self.mm
            .edges
            .get(&b)
            .and_then(|t| t.get(i as usize))
            .map(|&(x, y)| [x, y])
    }

    fn edge_sel(&self, b: BodyId, e: [u32; 2]) -> Option<EdgeSel> {
        let k = (e[0].min(e[1]), e[0].max(e[1]));
        let i = self.mm.edges.get(&b)?.binary_search(&k).ok()?;
        Some(EdgeSel::Index(b, i as u32))
    }

    /// Selected edges per body (vertex mode: edges with both ends selected).
    pub fn mesh_sel_edges(&self) -> BTreeMap<BodyId, Vec<[u32; 2]>> {
        let mut out: BTreeMap<BodyId, Vec<[u32; 2]>> = BTreeMap::new();
        match self.mode {
            Mode::Edge => {
                for e in &self.sel.edges {
                    if let EdgeSel::Index(b, i) = e
                        && let Some(p) = self.edge_of(*b, *i)
                    {
                        out.entry(*b).or_default().push(p);
                    }
                }
            }
            Mode::Vertex => {
                let verts = self.mesh_sel_verts();
                for (b, vs) in verts {
                    let set: BTreeSet<u32> = vs.into_iter().collect();
                    let edges: Vec<[u32; 2]> = self.mm.edges[&b]
                        .iter()
                        .filter(|(x, y)| set.contains(x) && set.contains(y))
                        .map(|&(x, y)| [x, y])
                        .collect();
                    if !edges.is_empty() {
                        out.insert(b, edges);
                    }
                }
            }
            _ => {}
        }
        out
    }

    /// Selected vertices per body (edge mode: the edges' ends; face mode:
    /// the faces' corners).
    pub fn mesh_sel_verts(&self) -> BTreeMap<BodyId, Vec<u32>> {
        let mut out: BTreeMap<BodyId, BTreeSet<u32>> = BTreeMap::new();
        match self.mode {
            Mode::Vertex => {
                for v in &self.sel.verts {
                    if let VertSel::Index(b, i) = v {
                        out.entry(*b).or_default().insert(*i);
                    }
                }
            }
            Mode::Edge => {
                for e in &self.sel.edges {
                    if let EdgeSel::Index(b, i) = e
                        && let Some(p) = self.edge_of(*b, *i)
                    {
                        out.entry(*b).or_default().extend(p);
                    }
                }
            }
            Mode::Face => {
                for (b, f) in &self.sel.faces {
                    if let Some(m) = self.mm.meshes.get(b)
                        && let Some(p) = m.polygons.get(f.0 as usize)
                    {
                        out.entry(*b).or_default().extend(p.verts.iter().copied());
                    }
                }
            }
            Mode::Object => {}
        }
        out.into_iter()
            .map(|(b, s)| (b, s.into_iter().collect()))
            .collect()
    }

    /// Replace one body's mesh-mode selection (in the current component
    /// mode) with these edges and vertices of its mesh `edges` table.
    fn set_body_selection(&mut self, b: BodyId, edges: &[[u32; 2]], verts: &[u32]) {
        clear_bodies(&mut self.sel, &BTreeSet::from([b]));
        match self.mode {
            Mode::Edge => {
                for e in edges {
                    if let Some(s) = self.edge_sel(b, *e) {
                        self.sel.edges.insert(s);
                    }
                }
            }
            Mode::Vertex => {
                for v in verts.iter().copied().chain(edges.iter().flatten().copied()) {
                    self.sel.verts.insert(VertSel::Index(b, v));
                }
            }
            _ => {}
        }
    }

    fn push_pick(&mut self, p: Pick) {
        self.mm.picks.retain(|q| *q != p);
        self.mm.picks.push(p);
        if self.mm.picks.len() > 2 {
            self.mm.picks.remove(0);
        }
    }

    fn pick_of_hover(&self, h: Hover) -> Option<Pick> {
        match h {
            Hover::Vert(i) => match self.vd.verts.get(i)?.sel {
                VertSel::Index(b, v) => Some(Pick::Vert(b, v)),
                _ => None,
            },
            Hover::Edge(i, _) => match self.vd.edges.get(i)?.sel {
                Some(EdgeSel::Index(b, e)) => Some(Pick::Edge(b, self.edge_of(b, e)?)),
                _ => None,
            },
            _ => None,
        }
    }

    /// Add elements to (or, with `Remove`, take them out of) the selection
    /// in the current mode.
    fn combine_elements(&mut self, b: BodyId, edges: &[[u32; 2]], verts: &[u32], how: Combine) {
        match self.mode {
            Mode::Edge => {
                let s: Vec<EdgeSel> = edges.iter().filter_map(|e| self.edge_sel(b, *e)).collect();
                combine(&mut self.sel.edges, s, how);
            }
            Mode::Vertex => {
                let mut vs: BTreeSet<u32> = verts.iter().copied().collect();
                vs.extend(edges.iter().flatten().copied());
                combine(
                    &mut self.sel.verts,
                    vs.into_iter().map(|v| VertSel::Index(b, v)),
                    how,
                );
            }
            _ => {}
        }
    }

    /// Alt-click: the edge loop through the edge under the pointer.
    pub fn mesh_loop_at(&mut self, at: Pos2, how: Combine) {
        if self.mode == Mode::Face {
            self.toast(
                ToastKind::Info,
                "Loop selection works in vertex and edge mode.",
            );
            return;
        }
        let Some(Hover::Edge(i, _)) = self.pick_at(at, Mode::Edge) else {
            return;
        };
        let Some(Pick::Edge(b, e)) = self.pick_of_hover(Hover::Edge(i, Default::default())) else {
            return;
        };
        let Some(m) = self.mm.meshes.get(&b).cloned() else {
            return;
        };
        match me::edge_loop(&m, e[0], e[1]) {
            Ok(l) => {
                self.combine_elements(b, &l, &[], how);
                self.push_pick(Pick::Edge(b, e));
                self.on_selection_changed();
            }
            Err(r) => self.toast(ToastKind::Warn, r.reason),
        }
    }

    /// Loop (L): the loops through the selected edges (or the last one
    /// clicked).
    pub fn mesh_select_loops(&mut self) {
        let mut edges = self.mesh_sel_edges();
        if edges.is_empty()
            && let Some(Pick::Edge(b, e)) = self.mm.picks.last().copied()
        {
            edges.insert(b, vec![e]);
        }
        if edges.is_empty() {
            self.toast(ToastKind::Info, "Select an edge first (or Alt-click one).");
            return;
        }
        for (b, es) in edges {
            let Some(m) = self.mm.meshes.get(&b).cloned() else {
                continue;
            };
            let mut all = Vec::new();
            for e in es {
                if let Ok(l) = me::edge_loop(&m, e[0], e[1]) {
                    all.extend(l);
                }
            }
            self.combine_elements(b, &all, &[], Combine::Add);
        }
        self.on_selection_changed();
    }

    /// Ctrl-click: the shortest path from the last clicked element to the
    /// one under the pointer, added to the selection.
    fn mesh_path_to(&mut self, target: Pick) {
        let Some(from) = self.mm.picks.last().copied() else {
            self.push_pick(target);
            self.combine_pick(target, Combine::Add);
            return;
        };
        self.mesh_path(from, target);
    }

    fn mesh_path(&mut self, from: Pick, to: Pick) {
        if from.body() != to.body() {
            self.toast(ToastKind::Info, "A path can't run between two objects.");
            return;
        }
        let b = to.body();
        let Some(m) = self.mm.meshes.get(&b).cloned() else {
            return;
        };
        let r = match (from, to) {
            (Pick::Vert(_, a), Pick::Vert(_, c)) => me::shortest_path(&m, a, c)
                .map(|p| p.windows(2).map(|w| [w[0], w[1]]).collect::<Vec<_>>()),
            (Pick::Edge(_, a), Pick::Edge(_, c)) => me::shortest_edge_path(&m, a, c),
            (Pick::Vert(_, a), Pick::Edge(_, c)) | (Pick::Edge(_, c), Pick::Vert(_, a)) => {
                me::shortest_edge_path(&m, c, c).and_then(|mut e| {
                    let p = me::shortest_path(&m, a, c[0])?;
                    e.extend(p.windows(2).map(|w| [w[0], w[1]]));
                    Ok(e)
                })
            }
        };
        match r {
            Ok(path) => {
                let verts: Vec<u32> = match (from, to) {
                    (Pick::Vert(_, a), Pick::Vert(_, c)) => vec![a, c],
                    _ => vec![],
                };
                self.combine_elements(b, &path, &verts, Combine::Add);
                self.push_pick(to);
                self.on_selection_changed();
            }
            Err(r) => self.toast(ToastKind::Warn, r.reason),
        }
    }

    /// The palette / panel command: the path between the last two clicks.
    pub fn mesh_shortest_path_cmd(&mut self) {
        match self.mm.picks.as_slice() {
            [a, b] => {
                let (a, b) = (*a, *b);
                self.mesh_path(a, b);
            }
            _ => self.toast(
                ToastKind::Info,
                "Click two vertices or edges first (or Ctrl-click the second one).",
            ),
        }
    }

    fn combine_pick(&mut self, p: Pick, how: Combine) {
        match p {
            Pick::Vert(b, v) => self.combine_elements(b, &[], &[v], how),
            Pick::Edge(b, e) => self.combine_elements(b, &[e], &[], how),
        }
    }

    fn is_picked_selected(&self, p: Pick) -> bool {
        match p {
            Pick::Vert(b, v) => self.sel.verts.contains(&VertSel::Index(b, v)),
            Pick::Edge(b, e) => self
                .edge_sel(b, e)
                .is_some_and(|s| self.sel.edges.contains(&s)),
        }
    }

    /// Mesh-mode clicks and drags in the viewport. Returns true if it used
    /// the input (the CAD handling then doesn't run).
    pub fn mesh_clicks(&mut self, ui: &Ui, resp: &egui::Response, nav_primary: bool) -> bool {
        let mods = ui.input(|i| i.modifiers);
        let pointer = ui.input(|i| i.pointer.interact_pos().or(i.pointer.hover_pos()));
        // Slide drags.
        if matches!(self.mm.tool, MeshTool::VertexSlide | MeshTool::EdgeSlide) {
            if resp.drag_started_by(egui::PointerButton::Primary) && !nav_primary {
                let start = ui.input(|i| i.pointer.press_origin()).unwrap_or_default();
                self.mm.slide = Some(SlideDrag {
                    start,
                    pos: None,
                    neg: None,
                    started: false,
                    factor: None,
                });
                return true;
            }
            if self.mm.slide.is_some() {
                if resp.drag_stopped() {
                    self.mm.slide = None;
                } else if let Some(p) = ui.input(|i| i.pointer.hover_pos()) {
                    self.slide_drag_to(p);
                }
                return true;
            }
        }
        if resp.double_clicked() && !nav_primary {
            if let Some(Hover::Edge(i, _)) = self.hover
                && let Some(Pick::Edge(b, e)) =
                    self.pick_of_hover(Hover::Edge(i, Default::default()))
                && let Some(m) = self.mm.meshes.get(&b).cloned()
                && let Ok(l) = me::edge_loop(&m, e[0], e[1])
            {
                let how = if mods.shift {
                    Combine::Add
                } else {
                    Combine::Replace
                };
                if how == Combine::Replace {
                    self.sel.clear_mode(self.mode);
                }
                self.combine_elements(b, &l, &[], Combine::Add);
                self.on_selection_changed();
            }
            return true;
        }
        if !resp.clicked() || nav_primary {
            return false;
        }
        if self.mm.tool == MeshTool::TargetMerge {
            if let Some(Hover::Vert(i)) = self.hover
                && let Some(Pick::Vert(b, v)) = self.pick_of_hover(Hover::Vert(i))
            {
                match self.mm.merge_from {
                    Some((fb, f)) if fb == b && f != v => {
                        self.mm.merge_from = None;
                        self.mesh_run(MeshTool::TargetMerge, Some((b, Input::Pair(f, v))));
                    }
                    Some((fb, f)) if fb == b && f == v => self.mm.merge_from = None,
                    _ => self.mm.merge_from = Some((b, v)),
                }
            }
            return true;
        }
        let ctrl = mods.ctrl || mods.command;
        if mods.alt {
            if let Some(p) = pointer {
                let how = if mods.shift {
                    Combine::Add
                } else {
                    Combine::Replace
                };
                if how == Combine::Replace && self.mode != Mode::Face {
                    self.sel.clear_mode(self.mode);
                }
                self.mesh_loop_at(p, Combine::Add);
            }
            return true;
        }
        let picked = self.hover.and_then(|h| self.pick_of_hover(h));
        if ctrl {
            if let Some(p) = picked {
                self.mesh_path_to(p);
            }
            return true;
        }
        match picked {
            Some(p) => {
                if mods.shift {
                    let how = if self.is_picked_selected(p) {
                        Combine::Remove
                    } else {
                        Combine::Add
                    };
                    self.combine_pick(p, how);
                } else {
                    self.sel.clear_mode(self.mode);
                    self.combine_pick(p, Combine::Add);
                }
                self.push_pick(p);
                self.on_selection_changed();
                true
            }
            // Faces (and clicks on nothing) use the CAD handling.
            None => false,
        }
    }

    // ---------------------------------------------------------------- tools

    /// Pick a mesh tool (panel button, shortcut, palette). Instant tools run
    /// on the selection right away.
    pub fn mesh_pick_tool(&mut self, t: MeshTool) {
        self.mm.tool = t;
        self.mm.merge_from = None;
        self.mm.slide = None;
        if t.instant() {
            self.mesh_run(t, None);
        }
    }

    /// What `t` runs on, per body, from the current selection.
    fn mesh_inputs(&self, t: MeshTool) -> Result<Vec<(BodyId, Input)>, String> {
        let p = &self.mm.params;
        let edges = || -> Result<Vec<(BodyId, Input)>, String> {
            let e = self.mesh_sel_edges();
            if e.is_empty() {
                return Err(format!(
                    "{}: select an edge loop or chain first (Alt-click an edge for its loop)",
                    t.title()
                ));
            }
            Ok(e.into_iter().map(|(b, v)| (b, Input::Edges(v))).collect())
        };
        match t {
            MeshTool::EdgeSlide
            | MeshTool::Relax
            | MeshTool::Space
            | MeshTool::LoopCleanup
            | MeshTool::Resample => edges(),
            MeshTool::MergeByDistance if p.merge_selected => {
                let v = self.mesh_sel_verts();
                if v.is_empty() {
                    return Err(
                        "Merge by Distance: select vertices first, or switch off \"Selected only\""
                            .into(),
                    );
                }
                Ok(v.into_iter()
                    .map(|(b, v)| (b, Input::Vertices(v)))
                    .collect())
            }
            MeshTool::MergeByDistance | MeshTool::Decimate => {
                let bodies = self.sel.bodies(self.mode);
                if bodies.is_empty() {
                    return Err(format!(
                        "{}: select something on the object(s) to work on (Ctrl+A selects everything)",
                        t.title()
                    ));
                }
                Ok(bodies.into_iter().map(|b| (b, Input::Whole)).collect())
            }
            MeshTool::VertexSlide => {
                let v = self.mesh_sel_verts();
                if v.is_empty() {
                    return Err("Vertex Slide: select vertices first".into());
                }
                Ok(v.into_iter()
                    .map(|(b, v)| (b, Input::Vertices(v)))
                    .collect())
            }
            MeshTool::TargetMerge | MeshTool::Select => Ok(vec![]),
        }
    }

    /// Run `t` once on one body's mesh (pure, plus the projection back to
    /// the CAD surface when asked).
    fn mesh_compute(
        &self,
        t: MeshTool,
        b: BodyId,
        m: &EditMesh,
        input: &Input,
    ) -> Result<EditOutcome, Refusal> {
        let p = &self.mm.params;
        let preserve = p.preserve();
        let edges = |i: &Input| match i {
            Input::Edges(e) => me::Selection::Edges(e.clone()),
            _ => me::Selection::Edges(vec![]),
        };
        let d = m.diagonal().max(1e-12);
        let out = match t {
            MeshTool::Select => return Err(Refusal::new("No tool")),
            MeshTool::VertexSlide => {
                let edge = match input {
                    Input::Toward(pairs) => me::SlideEdge::Toward(pairs.clone()),
                    _ => return Err(Refusal::new("Drag in the viewport to slide the vertices")),
                };
                me::vertex_slide(
                    m,
                    &edge,
                    &me::SlideParams {
                        factor: p.vslide_factor,
                        even: p.vslide_even,
                        clamp: true,
                        preserve,
                        ..Default::default()
                    },
                )?
            }
            MeshTool::EdgeSlide => me::edge_slide(
                m,
                &edges(input),
                &me::EdgeSlideParams {
                    factor: p.eslide_factor,
                    even: p.eslide_even,
                    preserve,
                    ..Default::default()
                },
            )?,
            MeshTool::Relax => me::relax(
                m,
                &edges(input),
                &me::RelaxParams {
                    iterations: p.relax_iterations,
                    interpolation: if p.relax_cubic {
                        me::Interpolation::Cubic
                    } else {
                        me::Interpolation::Linear
                    },
                    even: p.relax_even,
                    preserve,
                    ..Default::default()
                },
            )?,
            MeshTool::Space => me::space(
                m,
                &edges(input),
                &me::SpaceParams {
                    preserve,
                    ..Default::default()
                },
            )?,
            MeshTool::TargetMerge => {
                let Input::Pair(f, to) = *input else {
                    return Err(Refusal::new(
                        "Click a vertex, then the vertex it merges into",
                    ));
                };
                me::target_merge(
                    m,
                    f,
                    to,
                    &me::TargetMergeParams {
                        preserve,
                        ..Default::default()
                    },
                )?
            }
            MeshTool::MergeByDistance => {
                let scope = match input {
                    Input::Vertices(v) => me::VertexScope::Vertices(v.clone()),
                    _ => me::VertexScope::All,
                };
                me::merge_by_distance(
                    m,
                    &scope,
                    &me::MergeParams {
                        distance: p.merge_pct * 0.01 * d,
                        position: if p.merge_centroid {
                            me::MergePosition::Centroid
                        } else {
                            me::MergePosition::Survivor
                        },
                        preserve,
                        ..me::MergeParams::for_mesh(m)
                    },
                )?
            }
            MeshTool::LoopCleanup => me::cleanup(
                m,
                &edges(input),
                &me::CleanupParams {
                    merge_distance: p.cleanup_merge_pct * 0.01 * d,
                    short_edge: p.cleanup_short_pct * 0.01 * d,
                    straight_angle_deg: p.cleanup_straight_deg,
                    slide: p.cleanup_slide,
                    tolerance: p.cleanup_tolerance_pct * 0.01 * d,
                    preserve,
                    ..me::CleanupParams::for_mesh(m)
                },
            )?,
            MeshTool::Resample => me::resample(
                m,
                &edges(input),
                &me::ResampleParams {
                    spacing: match p.resample_by {
                        ResampleBy::Even => me::Spacing::Even,
                        ResampleBy::Count => me::Spacing::Count(p.resample_count),
                        ResampleBy::Length => {
                            me::Spacing::EdgeLength(p.resample_length_pct * 0.01 * d)
                        }
                    },
                    corner_angle_deg: p.resample_corner_deg,
                    preserve,
                    ..Default::default()
                },
            )?,
            MeshTool::Decimate => me::decimate(
                m,
                &me::DecimateParams {
                    mode: if p.decimate_planar {
                        me::DecimateMode::Planar
                    } else {
                        me::DecimateMode::Collapse
                    },
                    target: me::Target::Ratio((p.decimate_pct * 0.01).clamp(0.0, 1.0)),
                    keep_quads: p.decimate_keep_quads,
                    planar_angle_deg: p.decimate_planar_deg,
                    preserve,
                    ..Default::default()
                },
            )?,
        };
        if t.moves()
            && p.stay(t)
            && let Some(scene) = self.doc.scene.as_ref()
        {
            // The move tools all run with the default normal limit.
            return Ok(me::stay_on_surface(
                m,
                out,
                me::SlideParams::default().max_normal_change_deg,
                &mut |f: FaceId, pts: &[DVec3]| scene.project_to_cad(b, f, pts),
            ));
        }
        Ok(out)
    }

    /// Run a mesh tool on the selection (or on `only`), as one undo step.
    pub fn mesh_run(&mut self, t: MeshTool, only: Option<(BodyId, Input)>) {
        if !self.in_mesh_mode() {
            return;
        }
        self.ensure_meshes();
        let inputs = match only {
            Some(x) => vec![x],
            None => match self.mesh_inputs(t) {
                Ok(i) => i,
                Err(e) => {
                    self.toast(ToastKind::Info, e);
                    return;
                }
            },
        };
        let mut bodies = Vec::new();
        for (b, input) in inputs {
            let Some(m) = self.mm.meshes.get(&b).cloned() else {
                continue;
            };
            let had = self.doc.scene.as_ref().is_some_and(|s| s.has_mesh_edits(b));
            bodies.push((b, m, had, input));
        }
        if bodies.is_empty() {
            self.toast(
                ToastKind::Info,
                "Nothing to work on: the selected objects have no mesh.",
            );
            return;
        }
        let selected_verts = if t == MeshTool::MergeByDistance {
            self.mesh_sel_verts()
        } else {
            BTreeMap::new()
        };
        let op = LastOp {
            tool: t,
            bodies,
            selected_verts,
            rev: 0,
            stepped: false,
        };
        self.mesh_apply(op, false);
    }

    /// Compute `op` from its starting meshes and store the results: a new
    /// undo step, or (`amend`) a replacement of the step `op` made. An op
    /// whose results changed nothing has no step of its own, so its redo
    /// makes a new one.
    fn mesh_apply(&mut self, mut op: LastOp, amend: bool) {
        let amend = amend && op.stepped;
        // Merge by Distance: the scope follows "Selected only" as it is now.
        if op.tool == MeshTool::MergeByDistance {
            let selected = self.mm.params.merge_selected;
            for (b, _, _, input) in op.bodies.iter_mut() {
                *input = if selected {
                    Input::Vertices(op.selected_verts.get(b).cloned().unwrap_or_default())
                } else {
                    Input::Whole
                };
            }
        }
        let mut changes = Vec::new();
        let mut results = Vec::new();
        let mut refusals = Vec::new();
        for (b, m, had, input) in &op.bodies {
            match guard(|| self.mesh_compute(op.tool, *b, m, input)) {
                Ok(Ok(out)) => {
                    changes.push((*b, Some(out.mesh.clone())));
                    results.push((*b, out));
                }
                Ok(Err(r)) => {
                    refusals.push(r.reason);
                    // A redo that is refused puts the body back as it was.
                    if amend {
                        changes.push((*b, had.then(|| (**m).clone())));
                    }
                }
                Err(e) => refusals.push(e),
            }
        }
        if results.is_empty() && !amend {
            if let Some(r) = refusals.first() {
                self.toast(ToastKind::Warn, r.clone());
            }
            return;
        }
        match self.doc.set_meshes(changes, amend) {
            Ok(made) => op.stepped = amend || made,
            Err(e) => {
                self.toast(ToastKind::Warn, e.message());
                if amend {
                    self.mm.last = None;
                }
                return;
            }
        }
        self.ensure_meshes();
        // The selection follows the edit (whole-mesh tools clear it).
        for (b, out) in &results {
            let whole = op
                .bodies
                .iter()
                .any(|(x, _, _, i)| x == b && *i == Input::Whole);
            if whole || self.mode == Mode::Face {
                clear_bodies(&mut self.sel, &BTreeSet::from([*b]));
            } else {
                self.set_body_selection(*b, &out.selection, &out.selected_vertices);
            }
        }
        self.mm.picks.clear();
        if !amend {
            let msg: Vec<String> = results
                .iter()
                .map(|(_, o)| o.report.message.clone())
                .collect();
            if !msg.is_empty() {
                self.toast(ToastKind::Info, msg.join(" · "));
            }
        }
        if let Some(r) = refusals.first() {
            self.toast(ToastKind::Warn, r.clone());
        }
        op.rev = self.doc.revision;
        self.mm.last = Some(op);
        self.invalidate_view();
    }

    /// A tool value changed in the panel: redo the last edit with it, if
    /// the last edit was this tool and nothing has changed since (one undo
    /// step: the edit's own, made now if its first result changed nothing).
    pub fn mesh_params_changed(&mut self) {
        let t = self.mm.tool;
        if let Some(op) = self.mm.last.clone()
            && op.tool == t
            && op.rev == self.doc.revision
            && (!op.stepped || self.doc.can_amend_meshes())
        {
            self.mesh_apply(op, true);
        } else if t == MeshTool::EdgeSlide {
            // Edge Slide needs no drag: the factor alone slides the loop.
            self.mesh_run(t, None);
        }
    }

    /// The slide drag moved to `p` (screen).
    fn slide_drag_to(&mut self, p: Pos2) {
        let Some(mut drag) = self.mm.slide.clone() else {
            return;
        };
        if drag.started && self.slide_cancel_if_stale() {
            return;
        }
        let d = p - drag.start;
        if !drag.started {
            if d.length() < 4.0 {
                return;
            }
            let Some((op, pos, neg)) = self.slide_setup(vec2(d.x, d.y)) else {
                self.mm.slide = None;
                return;
            };
            drag.pos = pos;
            drag.neg = neg;
            drag.started = true;
            self.mm.slide = Some(drag.clone());
            self.mm.last = Some(op);
        }
        let dd = DVec2::new(d.x as f64, d.y as f64);
        let along = |v: Option<DVec2>| {
            v.filter(|v| v.length_squared() > 1e-6)
                .map(|v| dd.dot(v) / v.length_squared())
        };
        let (fp, fneg) = (along(drag.pos), along(drag.neg));
        let factor = match self.mm.tool {
            MeshTool::VertexSlide => fp.unwrap_or(0.0).clamp(0.0, 1.0),
            _ => match (fp, fneg) {
                (Some(a), Some(b)) if b > a => -b.clamp(0.0, 1.0),
                (Some(a), _) => a.clamp(0.0, 1.0),
                (None, Some(b)) => -b.clamp(0.0, 1.0),
                (None, None) => 0.0,
            },
        };
        let factor = (factor * 1000.0).round() / 1000.0;
        if drag.factor == Some(factor) {
            return;
        }
        drag.factor = Some(factor);
        self.mm.slide = Some(drag);
        if self.mm.tool == MeshTool::VertexSlide {
            self.mm.params.vslide_factor = factor;
        } else {
            self.mm.params.eslide_factor = factor;
        }
        let Some(op) = self.mm.last.clone() else {
            return;
        };
        self.mesh_apply(op, true);
    }

    /// The document changed under a started slide drag (undo, redo): its
    /// starting meshes are stale, so the drag ends instead of editing from
    /// them. True if it was cancelled.
    fn slide_cancel_if_stale(&mut self) -> bool {
        let live = self.mm.last.as_ref().is_some_and(|op| {
            op.rev == self.doc.revision && (!op.stepped || self.doc.can_amend_meshes())
        });
        if live {
            return false;
        }
        self.mm.slide = None;
        self.mm.last = None;
        self.toast(ToastKind::Info, S::TOAST_MESH_SLIDE_CANCELLED);
        true
    }

    /// Start a slide drag: the edges each vertex slides along, and the
    /// screen vectors that turn the drag into a factor.
    fn slide_setup(&mut self, drag: egui::Vec2) -> Option<(LastOp, Option<DVec2>, Option<DVec2>)> {
        let t = self.mm.tool;
        let inputs = match self.mesh_inputs(t) {
            Ok(i) => i,
            Err(e) => {
                self.toast(ToastKind::Info, e);
                return None;
            }
        };
        let cam = self.camera_matrices();
        let preserve = self.mm.params.preserve();
        let doc = self.doc.scene.as_ref()?.doc().clone();
        // Screen vector of a set of [from, to] pairs in a body.
        let screen = |b: BodyId, m: &EditMesh, pairs: &[[u32; 2]]| -> Option<DVec2> {
            let xf = doc.body(b)?.transform;
            let mut acc = DVec2::ZERO;
            let mut n = 0.0;
            for [v, w] in pairs {
                let a = cam
                    .project(xf.transform_point3(m.positions[*v as usize]))?
                    .0;
                let c = cam
                    .project(xf.transform_point3(m.positions[*w as usize]))?
                    .0;
                acc += c - a;
                n += 1.0;
            }
            (n > 0.0).then(|| acc / n)
        };
        let mut bodies = Vec::new();
        let (mut pos, mut neg) = (DVec2::ZERO, DVec2::ZERO);
        let (mut np, mut nn) = (0.0, 0.0);
        for (b, input) in inputs {
            let m = self.mm.meshes.get(&b)?.clone();
            let had = self.doc.scene.as_ref().is_some_and(|s| s.has_mesh_edits(b));
            match (&input, t) {
                (Input::Vertices(vs), MeshTool::VertexSlide) => {
                    // The drag direction in the body's space picks each
                    // vertex's edge.
                    let xf = doc.body(b)?.transform;
                    let dir_world = self.view.camera.right() * drag.x as f64
                        - self.view.camera.cam_up() * drag.y as f64;
                    let dir = xf.inverse().transform_vector3(dir_world);
                    let plan = me::SlidePlan::new(
                        &m,
                        &me::SlideEdge::Direction {
                            vertices: vs.clone(),
                            dir,
                        },
                        preserve,
                        60.0,
                    );
                    let pairs: Vec<[u32; 2]> = match plan {
                        Ok(p) => p.moves().iter().map(|mv| [mv.vertex, mv.toward]).collect(),
                        Err(r) => {
                            self.toast(ToastKind::Warn, r.reason);
                            continue;
                        }
                    };
                    if let Some(s) = screen(b, &m, &pairs) {
                        pos += s;
                        np += 1.0;
                    }
                    bodies.push((b, m, had, Input::Toward(pairs)));
                }
                (Input::Edges(es), MeshTool::EdgeSlide) => {
                    let sel = me::Selection::Edges(es.clone());
                    for (positive, acc, n) in
                        [(true, &mut pos, &mut np), (false, &mut neg, &mut nn)]
                    {
                        if let Ok(c) = me::edge_slide_cross(&m, &sel, preserve, positive)
                            && let Some(s) = screen(b, &m, &c)
                        {
                            *acc += s;
                            *n += 1.0;
                        }
                    }
                    bodies.push((b, m, had, input));
                }
                _ => {}
            }
        }
        if bodies.is_empty() {
            return None;
        }
        let op = LastOp {
            tool: t,
            bodies,
            selected_verts: BTreeMap::new(),
            rev: self.doc.revision,
            stepped: false,
        };
        Some((
            op,
            (np > 0.0).then(|| pos / np),
            (nn > 0.0).then(|| neg / nn),
        ))
    }

    /// Esc in mesh mode: a slide drag, then the first Target Merge pick,
    /// then the tool. True if something was cancelled.
    pub fn mesh_cancel(&mut self) -> bool {
        if self.mm.slide.take().is_some() {
            return true;
        }
        if self.mm.merge_from.take().is_some() {
            return true;
        }
        if self.mm.tool != MeshTool::Select {
            self.mm.tool = MeshTool::Select;
            return true;
        }
        false
    }

    /// Discard the mesh edits of the selected objects (one undo step).
    pub fn mesh_discard(&mut self) {
        let bodies: Vec<BodyId> = self.sel.bodies(self.mode).into_iter().collect();
        let edited: Vec<BodyId> = self
            .doc
            .scene
            .as_ref()
            .map(|s| s.mesh_edited_bodies())
            .unwrap_or_default();
        let targets: Vec<BodyId> = if bodies.is_empty() && edited.len() == 1 {
            edited.clone()
        } else {
            bodies.into_iter().filter(|b| edited.contains(b)).collect()
        };
        if targets.is_empty() {
            self.toast(
                ToastKind::Info,
                if edited.is_empty() {
                    "No object has mesh edits."
                } else {
                    "Select the objects whose mesh edits to discard."
                },
            );
            return;
        }
        match self.doc.discard_meshes(&targets) {
            Ok(n) => {
                self.mm.last = None;
                self.ensure_meshes();
                self.invalidate_view();
                self.toast(
                    ToastKind::Info,
                    format!("Discarded the mesh edits of {n} object(s): CAD editing is unlocked"),
                );
            }
            Err(e) => self.toast(ToastKind::Warn, e.message()),
        }
    }

    // ------------------------------------------------------------------ UI

    /// The `CAD | Mesh` control in the top bar.
    pub fn edit_mode_switch(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        let mut want = None;
        egui::Frame::new()
            .fill(pal.inset)
            .stroke(Stroke::new(1.0, pal.border))
            .corner_radius(pal.rounding.round() as u8)
            .inner_margin(egui::Margin::same(2))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.horizontal(|ui| {
                    for (m, label) in [(EditMode::Cad, S::EDIT_CAD), (EditMode::Mesh, S::EDIT_MESH)]
                    {
                        let on = self.edit == m;
                        let text = RichText::new(label).color(if on {
                            pal.accent_text
                        } else {
                            pal.text_dim
                        });
                        let text = if on { text.strong() } else { text };
                        let b = egui::Button::new(text)
                            .fill(if on { pal.accent } else { Color32::TRANSPARENT })
                            .stroke(Stroke::NONE)
                            .min_size(vec2(52.0, 22.0));
                        if ui
                            .add(b)
                            .on_hover_text(format!(
                                "{}  ({})",
                                S::EDIT_SWITCH_TIP,
                                self.keymap.label(Cmd::EditModeToggle)
                            ))
                            .clicked()
                        {
                            want = Some(m);
                        }
                    }
                });
            });
        if let Some(m) = want {
            self.request_edit_mode(m);
        }
    }

    /// The entry warning (SPEC §9.5), shown once per session unless turned
    /// off with its tick box.
    pub fn mesh_warning_ui(&mut self, ctx: &egui::Context) {
        let pal = self.pal;
        let amber = pal.mesh_accent;
        let mut choice = None;
        kit::modal("mesh-warning", &pal).show(ctx, |ui| {
            ui.set_width(440.0);
            if kit::dialog_header(ui, &pal, S::MESH_DLG_TITLE, None) {
                choice = Some(false);
            }
            ui.label(RichText::new(S::MESH_DLG_BODY).color(pal.text));
            ui.add_space(6.0);
            for p in [
                S::MESH_DLG_POINT_1,
                S::MESH_DLG_POINT_2,
                S::MESH_DLG_POINT_3,
            ] {
                ui.horizontal_top(|ui| {
                    ui.label(RichText::new("•").color(amber));
                    ui.add(egui::Label::new(RichText::new(p).color(pal.text_strong)).wrap());
                });
            }
            ui.add_space(10.0);
            kit::checkbox(ui, &mut self.mm.dont_show, S::MESH_DLG_DONT_SHOW);
            ui.add_space(12.0);
            let (r, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), 1.0), egui::Sense::hover());
            ui.painter().rect_filled(r, 0.0, pal.separator);
            ui.add_space(12.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let ok = egui::Button::new(
                    RichText::new(S::MESH_DLG_OK)
                        .color(Color32::from_rgb(0x1c, 0x14, 0x04))
                        .strong(),
                )
                .fill(amber)
                .min_size(vec2(96.0, 28.0));
                if ui.add(ok).clicked() {
                    choice = Some(true);
                }
                if ui
                    .add(egui::Button::new(S::DLG_CANCEL).min_size(vec2(72.0, 28.0)))
                    .clicked()
                {
                    choice = Some(false);
                }
            });
        });
        match choice {
            Some(true) => self.confirm_mesh_warning(),
            Some(false) => self.mm.dialog = false,
            None => {}
        }
    }

    pub fn confirm_mesh_warning(&mut self) {
        self.mm.dialog = false;
        if self.mm.dont_show {
            self.prefs.mesh_warning = false;
            self.dirty_settings = true;
        }
        self.enter_mesh_mode();
    }

    /// The always-visible warning in the viewport corner.
    pub fn mesh_badge(&self, ui: &Ui, rect: egui::Rect) {
        let pal = self.pal;
        let amber = pal.mesh_accent;
        let p = ui.painter();
        let font = egui::FontId::proportional(12.5);
        let g = p.layout_no_wrap(S::MESH_BADGE.to_string(), font, amber);
        let tri = 12.0;
        let size = vec2(g.size().x + tri + 30.0, g.size().y + 12.0);
        let r = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 12.0, rect.bottom() - 12.0 - size.y),
            size,
        );
        p.rect_filled(
            r,
            7.0,
            amber.gamma_multiply(if pal.dark { 0.16 } else { 0.14 }),
        );
        p.rect_stroke(
            r,
            7.0,
            Stroke::new(1.0, amber.gamma_multiply(0.7)),
            egui::StrokeKind::Inside,
        );
        let c = egui::pos2(r.left() + 10.0 + tri * 0.5, r.center().y);
        let h = tri * 0.9;
        p.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(c.x, c.y - h * 0.5),
                egui::pos2(c.x + tri * 0.5, c.y + h * 0.5),
                egui::pos2(c.x - tri * 0.5, c.y + h * 0.5),
            ],
            amber,
            Stroke::NONE,
        ));
        p.text(
            egui::pos2(c.x, c.y + h * 0.12),
            egui::Align2::CENTER_CENTER,
            "!",
            egui::FontId::proportional(9.0),
            Color32::from_rgb(0x1c, 0x14, 0x04),
        );
        p.galley(egui::pos2(r.left() + tri + 18.0, r.top() + 6.0), g, amber);
    }

    /// The mesh tool panel (replaces the CAD tool panel in mesh mode).
    pub fn mesh_panel_ui(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        let section = |ui: &mut Ui, s: &str| {
            ui.label(
                RichText::new(s.to_uppercase())
                    .small()
                    .strong()
                    .color(pal.text_dim),
            );
        };
        ui.spacing_mut().item_spacing.y = 4.0;
        section(ui, S::MESH_SEC_SELECT);
        let select = [
            (
                "Loop".to_string(),
                false,
                format!(
                    "Select the loops through the selected edges  (Alt-click, {})",
                    self.keymap.label(Cmd::Loop)
                ),
            ),
            (
                "Shortest Path".to_string(),
                false,
                "Select the shortest path between the last two clicked elements  (Ctrl-click)"
                    .to_string(),
            ),
        ];
        let run_cmd = match tool_grid(ui, &pal, &select) {
            Some(0) => Some(Cmd::Loop),
            Some(_) => Some(Cmd::MeshShortestPath),
            None => None,
        };
        ui.add_space(6.0);
        let mut picked = None;
        for (title, tools) in [
            (S::MESH_SEC_MOVE, &MeshTool::MOVE[..]),
            (S::MESH_SEC_MERGE, &MeshTool::MERGE[..]),
        ] {
            section(ui, title);
            let items: Vec<(String, bool, String)> = tools
                .iter()
                .map(|&t| {
                    let key = t.cmd().map(|c| self.keymap.label(c)).unwrap_or_default();
                    let tip = if key.is_empty() {
                        t.title().to_string()
                    } else {
                        format!("{}  ({key})", t.title())
                    };
                    (t.title().to_string(), self.mm.tool == t, tip)
                })
                .collect();
            if let Some(i) = tool_grid(ui, &pal, &items) {
                picked = Some(tools[i]);
            }
            ui.add_space(6.0);
        }
        if let Some(c) = run_cmd {
            self.run(c);
        }
        if let Some(t) = picked {
            self.mesh_pick_tool(t);
        }
        ui.separator();
        self.mesh_options_ui(ui);
        ui.add_space(6.0);
        ui.separator();
        self.mesh_status_ui(ui);
    }

    fn mesh_options_ui(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        let t = self.mm.tool;
        ui.label(
            RichText::new(t.title())
                .font(crate::theme::medium(14.0))
                .color(pal.text_strong),
        );
        ui.add_space(4.0);
        let before = self.mm.params.clone();
        let p = &mut self.mm.params;
        let grid = |ui: &mut Ui, id: &str, add: &mut dyn FnMut(&mut Ui)| {
            egui::Grid::new(id)
                .num_columns(2)
                .spacing(vec2(12.0, 6.0))
                .show(ui, |ui| add(ui));
        };
        match t {
            MeshTool::Select => {
                ui.label(
                    RichText::new(S::HINT_MESH_SELECT)
                        .small()
                        .color(pal.text_dim),
                );
            }
            MeshTool::VertexSlide => {
                let made = self.mm.last.as_ref().is_some_and(|l| l.tool == t);
                grid(ui, "vslide", &mut |ui| {
                    ui.label("Factor");
                    ui.add_enabled(
                        made,
                        egui::Slider::new(&mut p.vslide_factor, 0.0..=1.0).fixed_decimals(2),
                    );
                    ui.end_row();
                    ui.label("");
                    kit::checkbox(ui, &mut p.vslide_even, "Even");
                    ui.end_row();
                });
                ui.label(
                    RichText::new("Drag in the viewport to slide; then fine-tune the factor here.")
                        .small()
                        .color(pal.text_dim),
                );
            }
            MeshTool::EdgeSlide => {
                grid(ui, "eslide", &mut |ui| {
                    ui.label("Factor");
                    ui.add(egui::Slider::new(&mut p.eslide_factor, -1.0..=1.0).fixed_decimals(2));
                    ui.end_row();
                    ui.label("");
                    kit::checkbox(ui, &mut p.eslide_even, "Even");
                    ui.end_row();
                });
                ui.label(
                    RichText::new(
                        "Drag in the viewport, or set the factor: the sign picks the side.",
                    )
                    .small()
                    .color(pal.text_dim),
                );
            }
            MeshTool::Relax => {
                grid(ui, "relax", &mut |ui| {
                    ui.label("Iterations");
                    kit::segmented(ui, &pal, |ui| {
                        for n in [1u32, 3, 5, 10, 25] {
                            ui.selectable_value(&mut p.relax_iterations, n, n.to_string());
                        }
                    });
                    ui.end_row();
                    ui.label("Interpolation");
                    kit::segmented(ui, &pal, |ui| {
                        ui.selectable_value(&mut p.relax_cubic, true, "Cubic");
                        ui.selectable_value(&mut p.relax_cubic, false, "Linear");
                    });
                    ui.end_row();
                    ui.label("");
                    kit::checkbox(ui, &mut p.relax_even, "Keep even spacing");
                    ui.end_row();
                });
            }
            MeshTool::Space => {
                ui.label(
                    RichText::new(
                        "Spaces the selected loop's vertices evenly along its current shape.",
                    )
                    .small()
                    .color(pal.text_dim),
                );
            }
            MeshTool::TargetMerge => {
                let from = self.mm.merge_from.is_some();
                ui.label(
                    RichText::new(if from {
                        "Now click the vertex it merges into."
                    } else {
                        "Click the vertex to merge away."
                    })
                    .color(pal.text),
                );
                ui.label(
                    RichText::new("The first vertex disappears; the second stays where it is.")
                        .small()
                        .color(pal.text_dim),
                );
            }
            MeshTool::MergeByDistance => {
                grid(ui, "mbd", &mut |ui| {
                    ui.label("Distance");
                    ui.add(
                        egui::DragValue::new(&mut p.merge_pct)
                            .range(0.0001..=10.0)
                            .speed(0.001)
                            .max_decimals(4)
                            .suffix(" % of size"),
                    );
                    ui.end_row();
                    ui.label("");
                    kit::checkbox(ui, &mut p.merge_selected, "Selected only");
                    ui.end_row();
                    ui.label("");
                    kit::checkbox(ui, &mut p.merge_centroid, "Move to the middle");
                    ui.end_row();
                });
            }
            MeshTool::LoopCleanup => {
                grid(ui, "cleanup", &mut |ui| {
                    ui.label("Merge closer than");
                    ui.add(
                        egui::DragValue::new(&mut p.cleanup_merge_pct)
                            .range(0.0..=5.0)
                            .speed(0.001)
                            .max_decimals(3)
                            .suffix(" %"),
                    );
                    ui.end_row();
                    ui.label("Collapse edges under");
                    ui.add(
                        egui::DragValue::new(&mut p.cleanup_short_pct)
                            .range(0.0..=10.0)
                            .speed(0.01)
                            .max_decimals(2)
                            .suffix(" %"),
                    );
                    ui.end_row();
                    ui.label("Straight angle");
                    ui.add(
                        egui::DragValue::new(&mut p.cleanup_straight_deg)
                            .range(0.0..=45.0)
                            .speed(0.1)
                            .suffix("°"),
                    );
                    ui.end_row();
                    ui.label("Tolerance");
                    ui.add(
                        egui::DragValue::new(&mut p.cleanup_tolerance_pct)
                            .range(0.0..=10.0)
                            .speed(0.01)
                            .max_decimals(2)
                            .suffix(" %"),
                    );
                    ui.end_row();
                    ui.label("");
                    kit::checkbox(ui, &mut p.cleanup_slide, "Slide to straighten");
                    ui.end_row();
                });
                ui.label(RichText::new("Tidies the selected loop where two surfaces meet. Distances are % of the object's size.").small().color(pal.text_dim));
            }
            MeshTool::Resample => {
                grid(ui, "resample", &mut |ui| {
                    ui.label("Spacing");
                    kit::segmented(ui, &pal, |ui| {
                        ui.selectable_value(&mut p.resample_by, ResampleBy::Even, "Even");
                        ui.selectable_value(&mut p.resample_by, ResampleBy::Count, "Count");
                        ui.selectable_value(&mut p.resample_by, ResampleBy::Length, "Length");
                    });
                    ui.end_row();
                    match p.resample_by {
                        ResampleBy::Count => {
                            ui.label("Vertices");
                            ui.add(egui::DragValue::new(&mut p.resample_count).range(2..=10_000));
                            ui.end_row();
                        }
                        ResampleBy::Length => {
                            ui.label("Edge length");
                            ui.add(
                                egui::DragValue::new(&mut p.resample_length_pct)
                                    .range(0.01..=50.0)
                                    .speed(0.05)
                                    .max_decimals(2)
                                    .suffix(" %"),
                            );
                            ui.end_row();
                        }
                        ResampleBy::Even => {}
                    }
                    ui.label("Keep corners over");
                    ui.add(
                        egui::DragValue::new(&mut p.resample_corner_deg)
                            .range(1.0..=180.0)
                            .speed(0.5)
                            .suffix("°"),
                    );
                    ui.end_row();
                });
            }
            MeshTool::Decimate => {
                grid(ui, "decimate", &mut |ui| {
                    ui.label("Mode");
                    kit::segmented(ui, &pal, |ui| {
                        ui.selectable_value(&mut p.decimate_planar, false, "Collapse");
                        ui.selectable_value(&mut p.decimate_planar, true, "Planar");
                    });
                    ui.end_row();
                    if p.decimate_planar {
                        ui.label("Planar angle");
                        ui.add(
                            egui::DragValue::new(&mut p.decimate_planar_deg)
                                .range(0.1..=45.0)
                                .speed(0.1)
                                .suffix("°"),
                        );
                    } else {
                        ui.label("Target");
                        ui.add(
                            egui::Slider::new(&mut p.decimate_pct, 1.0..=100.0)
                                .suffix(" %")
                                .fixed_decimals(0),
                        );
                    }
                    ui.end_row();
                    ui.label("");
                    kit::checkbox(ui, &mut p.decimate_keep_quads, "Keep quads");
                    ui.end_row();
                });
            }
        }
        if t != MeshTool::Select {
            ui.add_space(4.0);
            if let Some(i) = t.stay_slot() {
                kit::checkbox(ui, &mut p.stay[i], S::MESH_STAY_ON_SURFACE)
                    .on_hover_text(S::MESH_STAY_TIP);
            }
            kit::checkbox(ui, &mut p.keep_features, S::MESH_KEEP_FEATURES)
                .on_hover_text(S::MESH_KEEP_FEATURES_TIP);
            if t.instant() || matches!(t, MeshTool::VertexSlide | MeshTool::EdgeSlide) {
                ui.add_space(2.0);
                ui.label(RichText::new(S::MESH_LIVE_NOTE).small().color(pal.text_dim));
            }
            if t.instant() {
                ui.add_space(4.0);
                if ui
                    .add(
                        egui::Button::new(format!("Apply {} again", t.title()))
                            .min_size(vec2(0.0, 26.0)),
                    )
                    .on_hover_text("Run it once more on the current selection (a new undo step)")
                    .clicked()
                {
                    self.mesh_run(t, None);
                    return;
                }
            }
        }
        // Slider drags change values every frame: each change redoes the
        // last edit (amended, so still one step).
        if self.mm.params != before {
            self.mesh_params_changed();
        }
    }

    fn mesh_status_ui(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        let (edited, names): (Vec<BodyId>, Vec<(BodyId, String)>) = match self.doc.scene.as_ref() {
            Some(s) => (
                s.mesh_edited_bodies(),
                s.doc()
                    .bodies
                    .iter()
                    .map(|b| (b.id, b.name.clone()))
                    .collect(),
            ),
            None => (vec![], vec![]),
        };
        if edited.is_empty() {
            ui.label(RichText::new(S::MESH_NO_EDITS).small().color(pal.text_dim));
            return;
        }
        for b in &edited {
            let name = names
                .iter()
                .find(|(i, _)| i == b)
                .map(|(_, n)| n.as_str())
                .unwrap_or("?");
            ui.label(
                RichText::new(format!("\"{name}\" {}", S::MESH_HAS_EDITS))
                    .small()
                    .color(pal.mesh_accent),
            );
        }
        ui.add_space(4.0);
        if ui
            .add(egui::Button::new(S::MESH_DISCARD).min_size(vec2(0.0, 26.0)))
            .on_hover_text(S::MESH_DISCARD_TIP)
            .clicked()
        {
            self.mesh_discard();
        }
    }
}

/// Remove the selection of some bodies (every component kind).
fn clear_bodies(sel: &mut Selection, bodies: &BTreeSet<BodyId>) {
    sel.verts.retain(|v| !bodies.contains(&v.body()));
    sel.edges.retain(|e| !bodies.contains(&e.body()));
    sel.faces.retain(|f| !bodies.contains(&f.0));
}

/// Buttons in rows of two equal columns. Returns the clicked one.
fn tool_grid(
    ui: &mut Ui,
    pal: &crate::theme::Palette,
    items: &[(String, bool, String)],
) -> Option<usize> {
    let gap = 5.0;
    let w = ((ui.available_width() - gap) / 2.0).floor().max(60.0);
    let mut out = None;
    for (r, row) in items.chunks(2).enumerate() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (k, (label, on, tip)) in row.iter().enumerate() {
                let text = RichText::new(label).color(if *on { pal.accent } else { pal.text });
                let b = egui::Button::new(text)
                    .fill(pal.widget)
                    .stroke(if *on {
                        Stroke::new(1.0, pal.accent)
                    } else {
                        Stroke::NONE
                    })
                    .truncate();
                if ui.add_sized(vec2(w, 26.0), b).on_hover_text(tip).clicked() {
                    out = Some(r * 2 + k);
                }
            }
        });
    }
    out
}
