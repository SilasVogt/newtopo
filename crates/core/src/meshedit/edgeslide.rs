//! Edge Slide (SPEC §9.5.10) and Target Merge (SPEC §9.5.13). Own design.
//!
//! **Edge Slide** moves the vertices of selected edge runs along their
//! cross edges, all to the same side: positive factors towards the side
//! where the run's edges are wound forward in their polygon (the side that
//! is on the left walking along the run, seen from outside), negative
//! factors towards the other side. Per vertex the cross edge is the edge
//! beside the run edge in the polygon on that side, so a loop on a quad
//! strip slides along the strip. The sliding itself is Vertex Slide
//! ([`SlidePlan`]): the same feature rules, validity checks and stop short
//! of the largest valid factor.
//!
//! **Target Merge** merges one vertex into another: the first vertex
//! disappears and its polygons use the second, which stays where it is.
//! Vertices that share a polygon merge under the §9.5.2 rules (feature
//! curve vertices only along their curve, corners never); vertices that
//! share no polygon weld (open boundaries joined, Merge by Distance's rules).

use super::merge::{MergeMode, MergeParams, MergePosition, VertexScope, merge_by_distance};
use super::slide::{SlideEdge, SlidePlan};
use super::smooth::check_limit;
use super::work::{VClass, Work};
use super::{EditOutcome, Preserve, Selection, check_input, mark_selection, runs};
use crate::meshedit::mesh::{EditMesh, ekey};
use crate::ops::Refusal;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct EdgeSlideParams {
    /// −1 to 1: the fraction of each cross edge; the sign picks the side.
    pub factor: f64,
    /// Every vertex moves the same distance (the factor times the shortest
    /// cross edge) instead of the same fraction.
    pub even: bool,
    pub preserve: Preserve,
    pub max_normal_change_deg: f64,
}

impl Default for EdgeSlideParams {
    fn default() -> Self {
        EdgeSlideParams {
            factor: 0.5,
            even: false,
            preserve: Preserve::default(),
            max_normal_change_deg: 60.0,
        }
    }
}

/// The cross edges of each selected run's vertices on one side:
/// `[vertex, neighbour]` pairs, and the vertices without one.
pub(crate) fn cross_edges(w: &Work, positive: bool) -> (Vec<[u32; 2]>, Vec<String>) {
    let mut pairs = Vec::new();
    let mut skipped = Vec::new();
    let mut on_run: BTreeSet<u32> = BTreeSet::new();
    let all = runs(w);
    for (chain, _) in &all {
        on_run.extend(chain.iter().copied());
    }
    let mut done: BTreeSet<u32> = BTreeSet::new();
    for (chain, closed) in &all {
        let n = chain.len();
        for i in 0..n {
            let v = chain[i];
            if !done.insert(v) {
                continue;
            }
            // The run edge to look beside: the next one, or for the last
            // vertex of an open run the previous one.
            let (a, b, v_first) = if *closed || i + 1 < n {
                (v, chain[(i + 1) % n], true)
            } else {
                (chain[i - 1], v, false)
            };
            let side = w
                .edge_polys(a, b)
                .into_iter()
                .find(|&q| super::work::has_directed(&w.polys[q as usize].verts, a, b) == positive);
            let Some(q) = side else {
                skipped.push(format!("vertex {v} has no polygon on that side of the run"));
                continue;
            };
            let verts = &w.polys[q as usize].verts;
            let k = verts.len();
            let at = verts.iter().position(|&x| x == v).expect("in polygon");
            // Walking a→b forward in the polygon: beside `a` is the corner
            // before it, beside `b` the corner after it. Backwards (the
            // other side) it is the other way round.
            let before = verts[(at + k - 1) % k];
            let after = verts[(at + 1) % k];
            let x = match (positive, v_first) {
                (true, true) | (false, false) => before,
                (true, false) | (false, true) => after,
            };
            if on_run.contains(&x) {
                skipped.push(format!(
                    "vertex {v}: its cross edge on that side is on the run"
                ));
                continue;
            }
            pairs.push([v, x]);
        }
    }
    (pairs, skipped)
}

/// Edge Slide: slide the selected runs along their cross edges.
pub fn edge_slide(
    mesh: &EditMesh,
    sel: &Selection,
    p: &EdgeSlideParams,
) -> Result<EditOutcome, Refusal> {
    check_input(mesh)?;
    check_limit(p.max_normal_change_deg)?;
    if !(p.factor.is_finite() && (-1.0..=1.0).contains(&p.factor)) {
        return Err(Refusal::new("The slide factor must be between −1 and 1"));
    }
    let mut w = Work::new(mesh, p.preserve.mask());
    mark_selection(&mut w, sel)?;
    let run_edges: Vec<(u32, u32)> = runs(&w)
        .iter()
        .flat_map(|(c, closed)| {
            let n = c.len();
            let m = if *closed { n } else { n - 1 };
            (0..m).map(move |i| ekey(c[i], c[(i + 1) % n]))
        })
        .collect();
    let (pairs, mut skipped) = cross_edges(&w, p.factor >= 0.0);
    if pairs.is_empty() {
        return Err(Refusal::new(format!(
            "Can't slide: {}",
            skipped
                .first()
                .map(String::as_str)
                .unwrap_or("no run vertex has a cross edge on that side")
        )));
    }
    let plan = SlidePlan::new(
        mesh,
        &SlideEdge::Toward(pairs),
        p.preserve,
        p.max_normal_change_deg,
    )?;
    skipped.extend(plan.skipped().iter().cloned());
    let mut out = plan.apply(p.factor.abs(), p.even, true)?;
    // The selection follows the run.
    let mut selection: Vec<[u32; 2]> = run_edges
        .iter()
        .filter_map(|&(a, b)| {
            let (x, y) = (out.vertex_map[a as usize]?, out.vertex_map[b as usize]?);
            (x != y).then_some(if x < y { [x, y] } else { [y, x] })
        })
        .collect();
    selection.sort_unstable();
    selection.dedup();
    out.selection = selection;
    out.selected_vertices = Vec::new();
    out.report.skipped = skipped;
    Ok(out)
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TargetMergeParams {
    pub preserve: Preserve,
    pub max_normal_change_deg: f64,
}

impl Default for TargetMergeParams {
    fn default() -> Self {
        TargetMergeParams {
            preserve: Preserve::default(),
            max_normal_change_deg: 60.0,
        }
    }
}

/// Target Merge: merge `from` into `to` (`to` stays where it is).
pub fn target_merge(
    mesh: &EditMesh,
    from: u32,
    to: u32,
    p: &TargetMergeParams,
) -> Result<EditOutcome, Refusal> {
    check_input(mesh)?;
    let min_cos = check_limit(p.max_normal_change_deg)?;
    let nv = mesh.positions.len();
    let mut w = Work::new(mesh, p.preserve.mask());
    w.min_cos = min_cos;
    for v in [from, to] {
        if v as usize >= nv || !w.valive[v as usize] {
            return Err(Refusal::new(format!("Vertex {v} is not in this mesh")));
        }
    }
    if from == to {
        return Err(Refusal::new("Pick two different vertices"));
    }
    let shared = w.vpolys[from as usize]
        .iter()
        .any(|&q| w.polys[q as usize].verts.contains(&to));
    if !shared {
        return weld(mesh, from, to, p);
    }
    if !w.merge_allowed(from, to) {
        let why = match w.vclass(from) {
            VClass::Corner => "it is a corner (pinned, a feature end or a junction) and stays",
            VClass::Curve(_) => {
                "it is on a feature edge (boundary, seam, CAD or hard edge) and may only merge along it into a neighbour"
            }
            VClass::Free => "a feature loop would get fewer than three vertices",
        };
        return Err(Refusal::new(format!("Can't merge vertex {from}: {why}")));
    }
    let Some(plan) = w.plan_merge(from, to) else {
        return Err(Refusal::new(
            "Can't merge them: the mesh would change its topology or become non-manifold",
        ));
    };
    if !w.merge_geometry_ok(&plan) {
        return Err(Refusal::new(
            "Can't merge them: a polygon around them would fold, flip or flatten",
        ));
    }
    w.apply_merge(&plan, Vec::new());
    let (out, map) = w.finish();
    let before = mesh.stats();
    let after = out.stats();
    let to_out = map[to as usize];
    Ok(EditOutcome {
        vertex_map: map[..nv].to_vec(),
        selection: Vec::new(),
        selected_vertices: to_out.into_iter().collect(),
        report: super::EditReport {
            before,
            after,
            reached_target: true,
            message: format!("Merged vertex {from} into {to}"),
            skipped: Vec::new(),
        },
        mesh: out,
    })
}

/// Target Merge of two vertices that share no polygon: put `from` on `to`
/// and weld them (Merge by Distance, unconnected pairs only).
fn weld(
    mesh: &EditMesh,
    from: u32,
    to: u32,
    p: &TargetMergeParams,
) -> Result<EditOutcome, Refusal> {
    if mesh.pinned[from as usize] {
        return Err(Refusal::new(format!(
            "Can't merge vertex {from}: it is pinned (a CAD corner) and stays"
        )));
    }
    let mut moved = mesh.clone();
    moved.positions[from as usize] = mesh.positions[to as usize];
    let params = MergeParams {
        distance: 1e-9 * mesh.diagonal().max(1e-12),
        mode: MergeMode::Unconnected,
        position: MergePosition::Survivor,
        with_unselected: false,
        preserve: p.preserve,
        max_normal_change_deg: p.max_normal_change_deg,
    };
    let mut out = merge_by_distance(&moved, &VertexScope::Vertices(vec![from, to]), &params)
        .map_err(|r| Refusal::new(format!("Can't weld them: {}", r.reason)))?;
    let merged = out.vertex_map[from as usize].is_none() || out.vertex_map[to as usize].is_none();
    if !merged {
        let why =
            out.report.skipped.first().cloned().unwrap_or_else(|| {
                "they share no polygon and are not both on an open boundary".into()
            });
        return Err(Refusal::new(format!("Can't weld them: {why}")));
    }
    out.report.before = mesh.stats();
    out.report.message = format!("Welded vertex {from} onto {to}");
    // Whichever of the two survived sits where `to` was.
    out.selected_vertices = out.vertex_map[to as usize]
        .or(out.vertex_map[from as usize])
        .into_iter()
        .collect();
    Ok(out)
}
