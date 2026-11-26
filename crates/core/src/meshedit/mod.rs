//! Mesh mode (SPEC §9.5): after Finish (bake) the rule-stage output becomes
//! a plain [`EditMesh`], and these tools edit it directly:
//!
//! - [`decimate`]: reduce the polygon count (quadric-ordered merges, or
//!   planar merging);
//! - [`cleanup`]: tidy dense or jagged edge runs (merge close vertices,
//!   collapse short edges, slide vertices to straighten the run, dissolve
//!   straight-through vertices);
//! - [`resample`]: respace edge runs evenly by arc length, optionally to a
//!   new vertex count or edge length;
//! - [`vertex_slide`] / [`SlidePlan`]: move vertices along one of their
//!   edges (a plan is cheap to apply again and again while dragging);
//! - [`merge_by_distance`]: merge vertices closer than a distance;
//! - [`edge_slide`]: slide selected edge runs along their cross edges;
//! - [`relax`] / [`space`]: smooth selected runs, or space their vertices
//!   evenly along them;
//! - [`target_merge`]: merge one vertex into another;
//! - [`edge_loop`], [`shortest_path`], [`shortest_edge_path`]: selection
//!   helpers;
//! - [`stay_on_surface`]: put moved vertices back on the CAD surface.
//!
//! All are pure functions: input mesh + parameters → new mesh, or a
//! [`Refusal`] with a reason. None of them adds holes, flips a face, makes
//! the mesh non-manifold, moves a pinned vertex, or bends a preserved
//! feature edge. All but Merge by Distance and Target Merge keep the
//! topology (genus, boundary loops, connected parts); those two change it
//! only by welding open boundaries together.

mod cleanup;
mod decimate;
mod edgeslide;
mod merge;
pub mod mesh;
mod pick;
mod project;
mod quadric;
mod refill;
mod resample;
mod slide;
mod smooth;
pub mod synth;
mod work;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_mode;

pub use cleanup::{CleanupParams, cleanup};
pub use decimate::{DecimateMode, DecimateParams, Target, decimate};
pub use edgeslide::{
    EdgeSlideParams, TargetMergeParams, edge_slide, edge_slide_cross, target_merge,
};
pub use merge::{MergeMode, MergeParams, MergePosition, VertexScope, merge_by_distance};
pub use mesh::{EdgeFlags, EditMesh, EditPolygon, FlaggedEdge};
pub use pick::{edge_loop, edge_loops, shortest_edge_path, shortest_path};
pub use project::{Projector, stay_on_surface};
pub use resample::{ResampleParams, Spacing, resample};
pub use slide::{SlideEdge, SlideMove, SlideParams, SlidePlan, vertex_slide};
pub use smooth::{Interpolation, MAX_RELAX_ITERATIONS, RelaxParams, SpaceParams, relax, space};

use crate::mesh::MeshStats;
use crate::ops::Refusal;
use serde::{Deserialize, Serialize};
use work::{F_SEL, Work};

/// Which edges are protected as feature edges, besides the always-kept
/// ones (open boundaries, UV seams, pinned vertices).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preserve {
    /// Edges that lie on CAD edges (including smooth, tangent ones).
    pub cad_edges: bool,
    /// Hard edges.
    pub hard_edges: bool,
}

impl Default for Preserve {
    fn default() -> Self {
        Preserve {
            cad_edges: true,
            hard_edges: true,
        }
    }
}

/// The edges a selection tool works on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Selection {
    /// Selected edges as vertex pairs (any order). They form one or more
    /// runs: chains split where three or more selected edges meet.
    Edges(Vec<[u32; 2]>),
    /// Every feature run of the mesh: open boundaries and preserved CAD,
    /// hard and seam edges.
    AllFeatures,
}

/// Result of a mesh tool.
#[derive(Clone, Debug, PartialEq)]
pub struct EditOutcome {
    pub mesh: EditMesh,
    /// Input vertex index → output index; `None` if the vertex was merged
    /// away. A moved vertex keeps its entry.
    pub vertex_map: Vec<Option<u32>>,
    /// The edge selection after the edit, in output indices (sorted; empty
    /// for Decimate and Merge by Distance; the slide edges for Vertex
    /// Slide).
    pub selection: Vec<[u32; 2]>,
    /// The vertex selection after the edit, in output indices (sorted;
    /// empty for the edge tools).
    pub selected_vertices: Vec<u32>,
    pub report: EditReport,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EditReport {
    pub before: MeshStats,
    pub after: MeshStats,
    /// Decimate: the target was reached. Other tools: always true.
    pub reached_target: bool,
    /// One-line summary for the status bar.
    pub message: String,
    /// Runs the tool left unchanged, with the reason.
    pub skipped: Vec<String>,
}

/// Refuse meshes the tools can't work on.
pub(crate) fn check_input(mesh: &EditMesh) -> Result<(), Refusal> {
    if mesh.polygons.is_empty() {
        return Err(Refusal::new("The mesh is empty"));
    }
    let issues = mesh.issues();
    if let Some(first) = issues.first() {
        return Err(Refusal::new(format!(
            "The mesh has {} problem(s) the mesh tools can't work around, first: {first}",
            issues.len()
        )));
    }
    Ok(())
}

/// Mark the selection (`F_SEL`) in a working copy. `AllFeatures` marks
/// every feature edge.
pub(crate) fn mark_selection(w: &mut Work, sel: &Selection) -> Result<usize, Refusal> {
    let edges: Vec<(u32, u32)> = match sel {
        Selection::Edges(e) => {
            if e.is_empty() {
                return Err(Refusal::new("Select one or more edges first"));
            }
            let mut out = Vec::new();
            for &[a, b] in e {
                if a as usize >= w.pos.len()
                    || b as usize >= w.pos.len()
                    || a == b
                    || w.edge_polys(a, b).is_empty()
                {
                    return Err(Refusal::new(format!("{a}-{b} is not an edge of this mesh")));
                }
                out.push(mesh::ekey(a, b));
            }
            out
        }
        Selection::AllFeatures => {
            let mut out = Vec::new();
            for p in w.alive_polys().collect::<Vec<_>>() {
                let v = &w.polys[p as usize].verts;
                for i in 0..v.len() {
                    let k = mesh::ekey(v[i], v[(i + 1) % v.len()]);
                    if w.is_feature(k.0, k.1) {
                        out.push(k);
                    }
                }
            }
            out.sort_unstable();
            out.dedup();
            if out.is_empty() {
                return Err(Refusal::new("The mesh has no feature edges to work on"));
            }
            out
        }
    };
    for &k in &edges {
        *w.flags.entry(k).or_default() |= F_SEL;
    }
    Ok(edges.len())
}

/// Selected neighbours of `x` (edges with `F_SEL`), sorted.
pub(crate) fn run_neighbors(w: &Work, x: u32) -> Vec<u32> {
    w.neighbors(x)
        .into_iter()
        .filter(|&y| w.flag(x, y) & F_SEL != 0)
        .collect()
}

/// Runs of selected edges: chains split at vertices where the selection
/// doesn't continue through (not exactly two selected edges). Closed runs
/// start at their lowest vertex and head to its lower neighbour; a run that
/// leaves a junction and comes back to it is closed and starts there. Returns
/// (vertices, closed); a closed run doesn't repeat its first vertex.
pub(crate) fn runs(w: &Work) -> Vec<(Vec<u32>, bool)> {
    use std::collections::BTreeSet;
    let mut sel: BTreeSet<(u32, u32)> = BTreeSet::new();
    for (&k, &f) in &w.flags {
        if f & F_SEL != 0
            && w.valive[k.0 as usize]
            && w.valive[k.1 as usize]
            && !w.edge_polys(k.0, k.1).is_empty()
        {
            sel.insert(k);
        }
    }
    let mut adj: std::collections::BTreeMap<u32, Vec<u32>> = Default::default();
    for &(a, b) in &sel {
        adj.entry(a).or_default().push(b);
        adj.entry(b).or_default().push(a);
    }
    let mut used: BTreeSet<(u32, u32)> = BTreeSet::new();
    let mut out = Vec::new();
    let through = |x: u32| adj.get(&x).is_some_and(|n| n.len() == 2);
    // Open runs from every end or junction.
    for (&s, nb) in &adj {
        if through(s) {
            continue;
        }
        for &first in nb {
            if used.contains(&mesh::ekey(s, first)) {
                continue;
            }
            let mut chain = vec![s];
            let (mut prev, mut cur) = (s, first);
            used.insert(mesh::ekey(prev, cur));
            loop {
                chain.push(cur);
                if !through(cur) {
                    break;
                }
                let n = &adj[&cur];
                let next = if n[0] == prev { n[1] } else { n[0] };
                if used.contains(&mesh::ekey(cur, next)) {
                    break;
                }
                used.insert(mesh::ekey(cur, next));
                prev = cur;
                cur = next;
            }
            // A loop through a junction starts and ends at the same vertex.
            if chain.len() > 3 && chain.first() == chain.last() {
                chain.pop();
                out.push((chain, true));
            } else {
                out.push((chain, false));
            }
        }
    }
    // Closed runs.
    for (&s, nb) in &adj {
        if !through(s) || used.contains(&mesh::ekey(s, nb[0])) {
            continue;
        }
        let mut chain = vec![s];
        let (mut prev, mut cur) = (s, nb[0].min(nb[1]));
        used.insert(mesh::ekey(prev, cur));
        while cur != s {
            chain.push(cur);
            let n = &adj[&cur];
            let next = if n[0] == prev { n[1] } else { n[0] };
            used.insert(mesh::ekey(cur, next));
            prev = cur;
            cur = next;
        }
        out.push((chain, true));
    }
    out
}
