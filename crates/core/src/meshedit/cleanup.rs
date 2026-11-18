//! Cleanup (SPEC §9.4.4): tidy dense or jagged edge runs, for example
//! where two parts of a model were joined. Own design:
//!
//! 1. merge vertices that share a polygon and are closer than a distance;
//! 2. collapse selected edges shorter than a length;
//! 3. optionally slide run vertices along one of their cross edges towards
//!    the straight line through their run neighbours (the vertex stays on
//!    the old surface because it moves along an existing edge);
//! 4. dissolve run vertices where the run goes straight through (merge
//!    them into a run neighbour).
//!
//! The steps repeat until nothing changes. Every step keeps every input
//! vertex within `tolerance` of the result and refuses to flip a face,
//! bend a preserved feature edge or change the topology.

use super::work::{F_SEL, VClass, Work, point_segment_distance};
use super::{
    EditOutcome, EditReport, Preserve, Selection, check_input, mark_selection, run_neighbors,
};
use crate::meshedit::mesh::EditMesh;
use crate::ops::Refusal;
use glam::DVec3;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CleanupParams {
    /// Vertices sharing a polygon and closer than this are merged (body
    /// units). At least one of them must be on the selection.
    pub merge_distance: f64,
    /// Selected edges shorter than this are collapsed (body units).
    pub short_edge: f64,
    /// Run vertices where the run turns by at most this (degrees) are
    /// dissolved; 0 turns dissolving off.
    pub straight_angle_deg: f64,
    /// Slide run vertices along their cross edges to straighten the run.
    pub slide: bool,
    /// No input vertex may end up further than this from the result (body
    /// units).
    pub tolerance: f64,
    pub preserve: Preserve,
    pub max_normal_change_deg: f64,
}

impl CleanupParams {
    /// Defaults scaled to the mesh: merge distance 0.01 %, short edge
    /// 0.5 % and tolerance 0.1 % of the bounding-box diagonal.
    pub fn for_mesh(mesh: &EditMesh) -> CleanupParams {
        let d = mesh.diagonal().max(1e-12);
        CleanupParams {
            merge_distance: 1e-4 * d,
            short_edge: 5e-3 * d,
            tolerance: 1e-3 * d,
            ..CleanupParams::default()
        }
    }
}

impl Default for CleanupParams {
    /// The [`CleanupParams::for_mesh`] values for a mesh with a unit
    /// diagonal.
    fn default() -> Self {
        CleanupParams {
            merge_distance: 1e-4,
            short_edge: 5e-3,
            straight_angle_deg: 5.0,
            slide: true,
            tolerance: 1e-3,
            preserve: Preserve::default(),
            max_normal_change_deg: 60.0,
        }
    }
}

const MAX_ROUNDS: usize = 32;
/// Shape guard: no step may leave a polygon whose worst corner is below
/// this (relative sine, about 14° for a quad), unless it was already worse.
const MIN_QUALITY: f64 = 0.25;

pub fn cleanup(
    mesh: &EditMesh,
    sel: &Selection,
    p: &CleanupParams,
) -> Result<EditOutcome, Refusal> {
    check_input(mesh)?;
    for (name, v) in [
        ("merge distance", p.merge_distance),
        ("short-edge length", p.short_edge),
        ("tolerance", p.tolerance),
    ] {
        if !(v >= 0.0 && v.is_finite()) {
            return Err(Refusal::new(format!("The {name} must be zero or positive")));
        }
    }
    if !(0.0..90.0).contains(&p.straight_angle_deg) {
        return Err(Refusal::new(
            "The straight angle must be from 0° to below 90°",
        ));
    }
    if !(p.max_normal_change_deg > 0.0 && p.max_normal_change_deg < 90.0) {
        return Err(Refusal::new(
            "The normal-change limit must be between 0° and 90°",
        ));
    }
    let before = mesh.stats();
    let mut w = Work::new(mesh, p.preserve.mask());
    w.min_cos = p.max_normal_change_deg.to_radians().cos();
    w.track_deviation(p.tolerance);
    mark_selection(&mut w, sel)?;
    let cos_straight = p.straight_angle_deg.to_radians().cos();
    let (mut merged, mut short, mut slid, mut dissolved) = (0, 0, 0, 0);
    for _ in 0..MAX_ROUNDS {
        let mut changed = false;
        let m = merge_close(&mut w, p.merge_distance);
        let s = collapse_short(&mut w, p.short_edge);
        // Merging close vertices and short edges removes degenerate shapes
        // and may pass through worse ones; sliding and dissolving must
        // keep shapes reasonable.
        w.min_quality = MIN_QUALITY;
        let sl = if p.slide { slide(&mut w) } else { 0 };
        let d = dissolve_straight(&mut w, cos_straight, p.tolerance);
        w.min_quality = 0.0;
        merged += m;
        short += s;
        slid += sl;
        dissolved += d;
        changed |= m + s + sl + d > 0;
        if !changed {
            break;
        }
    }
    let (out, map) = w.finish();
    let selection = w.selected_edges(&map);
    let after = out.stats();
    Ok(EditOutcome {
        vertex_map: map[..mesh.positions.len()].to_vec(),
        mesh: out,
        selection,
        report: EditReport {
            before,
            after,
            reached_target: true,
            message: format!(
                "Merged {merged} close vertex pair(s), collapsed {short} short edge(s), slid {slid} and dissolved {dissolved} vertex(es)"
            ),
            skipped: Vec::new(),
        },
    })
}

/// Which vertex of a pair to keep: corners over feature vertices over free
/// ones, run vertices over others, then the lower index.
fn keep_rank(w: &Work, x: u32) -> (u8, u8, std::cmp::Reverse<u32>) {
    let c = match w.vclass(x) {
        VClass::Corner => 3,
        VClass::Curve(_) => 2,
        VClass::Free => 1,
    };
    let on_run = !run_neighbors(w, x).is_empty() as u8;
    (c, on_run, std::cmp::Reverse(x))
}

/// Merge `a` and `b`, keeping the higher-ranked one if possible.
fn merge_pair(w: &mut Work, a: u32, b: u32) -> bool {
    let (keep, drop) = if keep_rank(w, a) >= keep_rank(w, b) {
        (a, b)
    } else {
        (b, a)
    };
    w.try_merge(drop, keep) || w.try_merge(keep, drop)
}

fn sel_vertices(w: &Work) -> BTreeSet<u32> {
    let mut s = BTreeSet::new();
    for (&k, &f) in &w.flags {
        if f & F_SEL != 0 && w.valive[k.0 as usize] && w.valive[k.1 as usize] {
            s.insert(k.0);
            s.insert(k.1);
        }
    }
    s
}

fn merge_close(w: &mut Work, dist: f64) -> usize {
    if dist <= 0.0 {
        return 0;
    }
    let sv = sel_vertices(w);
    let mut pairs: Vec<(f64, u32, u32)> = Vec::new();
    let mut seen = BTreeSet::new();
    for &x in &sv {
        for &q in &w.vpolys[x as usize] {
            for &y in &w.polys[q as usize].verts {
                let k = super::mesh::ekey(x, y);
                if x == y || !seen.insert(k) {
                    continue;
                }
                let d = w.pos[x as usize].distance(w.pos[y as usize]);
                if d < dist {
                    pairs.push((d, k.0, k.1));
                }
            }
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0).then((a.1, a.2).cmp(&(b.1, b.2))));
    let mut n = 0;
    for (_, a, b) in pairs {
        if !w.valive[a as usize] || !w.valive[b as usize] {
            continue;
        }
        let share = w.vpolys[a as usize]
            .iter()
            .any(|&q| w.polys[q as usize].verts.contains(&b));
        if share && w.pos[a as usize].distance(w.pos[b as usize]) < dist && merge_pair(w, a, b) {
            n += 1;
        }
    }
    n
}

fn collapse_short(w: &mut Work, len: f64) -> usize {
    if len <= 0.0 {
        return 0;
    }
    let mut edges: Vec<(f64, u32, u32)> = w
        .flags
        .iter()
        .filter(|(k, f)| **f & F_SEL != 0 && w.valive[k.0 as usize] && w.valive[k.1 as usize])
        .map(|(k, _)| (w.pos[k.0 as usize].distance(w.pos[k.1 as usize]), k.0, k.1))
        .filter(|e| e.0 < len)
        .collect();
    edges.sort_by(|a, b| a.0.total_cmp(&b.0).then((a.1, a.2).cmp(&(b.1, b.2))));
    let mut n = 0;
    for (_, a, b) in edges {
        if !w.valive[a as usize]
            || !w.valive[b as usize]
            || w.flag(a, b) & F_SEL == 0
            || w.edge_polys(a, b).is_empty()
            || w.pos[a as usize].distance(w.pos[b as usize]) >= len
        {
            continue;
        }
        // Keep run ends and junctions in place.
        let ra = run_neighbors(w, a).len() != 2;
        let rb = run_neighbors(w, b).len() != 2;
        let ok = match (ra, rb) {
            (true, true) => false,
            (true, false) => w.try_merge(b, a),
            (false, true) => w.try_merge(a, b),
            (false, false) => merge_pair(w, a, b),
        };
        if ok {
            n += 1;
        }
    }
    n
}

/// Run interior vertices that may change: exactly two run neighbours, and
/// no feature edges other than the run's own.
fn movable(w: &Work, x: u32) -> Option<(u32, u32)> {
    if !w.valive[x as usize] {
        return None;
    }
    let rn = run_neighbors(w, x);
    if rn.len() != 2 {
        return None;
    }
    let (p, q) = (rn[0], rn[1]);
    match w.vclass(x) {
        VClass::Corner => None,
        VClass::Free => Some((p, q)),
        VClass::Curve([a, b]) => ((a, b) == (p, q) || (a, b) == (q, p)).then_some((p, q)),
    }
}

fn slide(w: &mut Work) -> usize {
    let mut n = 0;
    let eps = 1e-9 * w.diag;
    for x in sel_vertices(w) {
        let Some((p, q)) = movable(w, x) else {
            continue;
        };
        if w.vclass(x) != VClass::Free {
            continue;
        }
        let (px, pp, pq) = (w.pos[x as usize], w.pos[p as usize], w.pos[q as usize]);
        let dir = (pq - pp).normalize_or_zero();
        if dir == DVec3::ZERO {
            continue;
        }
        let d0 = line_distance(px, pp, dir);
        if d0 <= eps {
            continue;
        }
        let mut best: Option<(f64, u32, f64, DVec3)> = None;
        for y in w.neighbors(x) {
            if y == p || y == q || w.is_feature(x, y) {
                continue;
            }
            let e = w.pos[y as usize] - px;
            let t = closest_param(px, e, pp, dir).clamp(0.0, 0.75);
            let np = px + e * t;
            let d = line_distance(np, pp, dir);
            if d < 0.5 * d0 && best.is_none_or(|b| d < b.0) {
                best = Some((d, y, t, np));
            }
        }
        if let Some((_, y, t, np)) = best {
            let prov = super::work::Prov::lerp(&w.prov[x as usize], &w.prov[y as usize], t);
            if w.try_move(x, np, prov) {
                n += 1;
            }
        }
    }
    n
}

fn dissolve_straight(w: &mut Work, cos: f64, tol: f64) -> usize {
    if cos >= 1.0 {
        return 0;
    }
    let mut cands: Vec<(f64, u32)> = Vec::new();
    for x in sel_vertices(w) {
        if let Some((p, q)) = movable(w, x) {
            let c = turn_cos(w, x, p, q);
            if c >= cos {
                cands.push((c, x));
            }
        }
    }
    cands.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut n = 0;
    for (_, x) in cands {
        let Some((p, q)) = movable(w, x) else {
            continue;
        };
        if turn_cos(w, x, p, q) < cos {
            continue;
        }
        let px = w.pos[x as usize];
        if point_segment_distance(px, w.pos[p as usize], w.pos[q as usize]) > tol {
            continue;
        }
        // Merge into the run neighbour that leaves the better shapes.
        let mut best: Option<(f64, u32)> = None;
        for t in [p, q] {
            if !w.merge_allowed(x, t) {
                continue;
            }
            let Some(plan) = w.plan_merge(x, t) else {
                continue;
            };
            if !w.merge_geometry_ok(&plan) || w.merge_deviation(&plan).is_none() {
                continue;
            }
            let after = w.merge_quality(&plan).1;
            if best.is_none_or(|b| after > b.0) {
                best = Some((after, t));
            }
        }
        if let Some((_, t)) = best
            && w.try_merge(x, t)
        {
            n += 1;
        }
    }
    n
}

fn turn_cos(w: &Work, x: u32, p: u32, q: u32) -> f64 {
    let a = (w.pos[x as usize] - w.pos[p as usize]).normalize_or_zero();
    let b = (w.pos[q as usize] - w.pos[x as usize]).normalize_or_zero();
    a.dot(b)
}

fn line_distance(x: DVec3, p: DVec3, dir: DVec3) -> f64 {
    let v = x - p;
    (v - dir * v.dot(dir)).length()
}

/// Parameter `t` on `x + t e` closest to the line `p + s dir` (unit dir).
fn closest_param(x: DVec3, e: DVec3, p: DVec3, dir: DVec3) -> f64 {
    // Minimise |(x + t e - p) - dir ((x + t e - p)·dir)|².
    let r = x - p;
    let ep = e - dir * e.dot(dir);
    let rp = r - dir * r.dot(dir);
    let den = ep.length_squared();
    if den <= 1e-300 {
        return 0.0;
    }
    -rp.dot(ep) / den
}
