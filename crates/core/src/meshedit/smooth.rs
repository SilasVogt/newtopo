//! Relax and Space (SPEC §9.5.11, §9.5.12): reshape selected edge runs.
//! Own design.
//!
//! - **Relax** smooths each run: every movable run vertex moves towards a
//!   point predicted from its run neighbours (their midpoint, or with
//!   Cubic the cubic through the two neighbours on each side, which keeps
//!   the run's curvature and shrinks it less), for a number of iterations,
//!   optionally respacing the run evenly after each one. Vertices leave the
//!   old surface: that is what mesh mode is for.
//! - **Space** keeps each run's shape and spaces its movable vertices
//!   evenly by arc length along it (vertices stay on the old run).
//!
//! Fixed vertices: open run ends, junctions, pinned vertices and other
//! corners, and vertices with feature edges off the run (Relax also keeps
//! vertices on a feature curve; Space moves them only along their own
//! curve, when the run is that curve). Every move keeps the polygons around
//! it valid: one that was fine stays convex, unflipped and within the
//! normal limit; one that was already bad keeps a non-zero area and its
//! orientation. A Relax step that would break a polygon is halved, then
//! halved again, then skipped; a Space run that would is left unchanged and
//! reported.

use super::mesh::{EditMesh, area_vec};
use super::work::{Prov, VClass, Work, polygon_shape_ok};
use super::{
    EditOutcome, EditReport, Preserve, Selection, check_input, mark_selection, run_neighbors, runs,
};
use crate::ops::Refusal;
use glam::DVec3;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// How Relax predicts where a run vertex belongs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Interpolation {
    /// The midpoint of its two run neighbours.
    Linear,
    /// The cubic through two run vertices on each side (falls back to the
    /// midpoint near run ends).
    Cubic,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RelaxParams {
    /// 1 to 1000.
    pub iterations: u32,
    pub interpolation: Interpolation,
    /// Respace the run evenly after every iteration.
    pub even: bool,
    pub preserve: Preserve,
    pub max_normal_change_deg: f64,
}

impl Default for RelaxParams {
    fn default() -> Self {
        RelaxParams {
            iterations: 5,
            interpolation: Interpolation::Cubic,
            even: true,
            preserve: Preserve::default(),
            max_normal_change_deg: 60.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpaceParams {
    pub preserve: Preserve,
    pub max_normal_change_deg: f64,
}

impl Default for SpaceParams {
    fn default() -> Self {
        SpaceParams {
            preserve: Preserve::default(),
            max_normal_change_deg: 60.0,
        }
    }
}

/// Most Relax iterations allowed.
pub const MAX_RELAX_ITERATIONS: u32 = 1000;

pub(crate) fn check_limit(deg: f64) -> Result<f64, Refusal> {
    if !(deg > 0.0 && deg < 90.0) {
        return Err(Refusal::new(
            "The normal-change limit must be between 0° and 90°",
        ));
    }
    Ok(deg.to_radians().cos())
}

/// The polygons around some vertices as they were before a change, to
/// check the change against (see the module docs).
pub(crate) struct Guard {
    polys: Vec<(u32, bool, DVec3)>,
}

impl Guard {
    pub fn new(w: &Work, verts: impl IntoIterator<Item = u32>) -> Guard {
        let mut set = BTreeSet::new();
        for v in verts {
            set.extend(w.vpolys[v as usize].iter().copied());
        }
        let polys = set
            .into_iter()
            .filter(|&q| w.polys[q as usize].alive)
            .map(|q| {
                let p = &w.polys[q as usize];
                let ok = w.piece_ok(&p.verts, p, DVec3::ZERO, None);
                let n = area_vec(&w.poly_pos(&p.verts)).normalize_or_zero();
                (q, ok, n)
            })
            .collect();
        Guard { polys }
    }

    /// Every guarded polygon is acceptable at the current positions.
    pub fn ok(&self, w: &Work) -> bool {
        self.polys.iter().all(|&(q, was_ok, old)| {
            let p = &w.polys[q as usize];
            if was_ok {
                w.piece_ok(&p.verts, p, old, None)
            } else {
                let pos = w.poly_pos(&p.verts);
                let a = area_vec(&pos);
                a.length() > 1e-14 * w.diag * w.diag && a.dot(old) > 0.0
            }
        })
    }
}

/// A run vertex that may move along the run (Space): free and passed
/// straight through by the run, or on a feature curve that is the run.
fn movable_along(w: &Work, v: u32) -> bool {
    let rn = run_neighbors(w, v);
    if rn.len() != 2 {
        return false;
    }
    match w.vclass(v) {
        VClass::Free => true,
        VClass::Curve([a, b]) => rn.contains(&a) && rn.contains(&b),
        VClass::Corner => false,
    }
}

/// A run vertex that may move freely (Relax).
fn movable_free(w: &Work, v: u32) -> bool {
    run_neighbors(w, v).len() == 2 && w.vclass(v) == VClass::Free
}

/// Positions and provenances that space `piece` (a run piece, both ends
/// fixed) evenly by arc length along its current polyline.
fn even_targets(w: &Work, piece: &[u32]) -> Vec<(u32, DVec3, Prov)> {
    let m = piece.len() - 1;
    if m < 2 {
        return Vec::new();
    }
    let pts: Vec<DVec3> = piece.iter().map(|&v| w.pos[v as usize]).collect();
    let mut cum = vec![0.0];
    for s in pts.windows(2) {
        cum.push(cum.last().unwrap() + s[0].distance(s[1]));
    }
    let total = cum[m];
    if total <= 0.0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut seg = 0;
    for k in 1..m {
        let s = total * k as f64 / m as f64;
        while seg + 1 < m && cum[seg + 1] < s {
            seg += 1;
        }
        let len = cum[seg + 1] - cum[seg];
        let t = if len > 0.0 {
            ((s - cum[seg]) / len).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let p = pts[seg].lerp(pts[seg + 1], t);
        let prov = Prov::lerp(
            &w.prov[piece[seg] as usize],
            &w.prov[piece[seg + 1] as usize],
            t,
        );
        out.push((piece[k], p, prov));
    }
    out
}

/// Split a run at its fixed vertices into pieces whose interior vertices
/// all move. A closed run without fixed vertices is cut at its first.
fn pieces(chain: &[u32], closed: bool, movable: &[bool]) -> Vec<Vec<u32>> {
    let n = chain.len();
    let mut out = Vec::new();
    if closed {
        let Some(start) = (0..n).find(|&i| !movable[i]).or(Some(0)) else {
            return out;
        };
        let mut cur = vec![chain[start]];
        for k in 1..=n {
            let i = (start + k) % n;
            cur.push(chain[i]);
            if !movable[i] || k == n {
                if cur.len() > 2 {
                    out.push(std::mem::take(&mut cur));
                }
                cur = vec![chain[i]];
            }
        }
    } else {
        let mut cur = vec![chain[0]];
        for (i, &v) in chain.iter().enumerate().skip(1) {
            cur.push(v);
            if !movable[i] || i == n - 1 {
                if cur.len() > 2 {
                    out.push(std::mem::take(&mut cur));
                }
                cur = vec![v];
            }
        }
    }
    out
}

/// Move a batch of vertices; keep it only if every polygon around them is
/// still acceptable.
fn try_batch(w: &mut Work, moves: &[(u32, DVec3, Prov)]) -> bool {
    if moves.is_empty() {
        return true;
    }
    let guard = Guard::new(w, moves.iter().map(|m| m.0));
    let saved: Vec<(u32, DVec3, Prov)> = moves
        .iter()
        .map(|m| (m.0, w.pos[m.0 as usize], w.prov[m.0 as usize].clone()))
        .collect();
    for (v, p, prov) in moves {
        w.set_position(*v, *p, prov.clone());
    }
    if guard.ok(w) {
        return true;
    }
    for (v, p, prov) in saved {
        w.pos[v as usize] = p;
        w.prov[v as usize] = prov;
    }
    false
}

/// Space each run's pieces evenly. Returns the number of vertices moved,
/// or the reason the run was left unchanged.
fn space_run(w: &mut Work, chain: &[u32], closed: bool, movable: &[bool]) -> Result<usize, String> {
    let mut moves = Vec::new();
    for piece in pieces(chain, closed, movable) {
        moves.extend(even_targets(w, &piece));
    }
    let moved = moves
        .iter()
        .filter(|(v, p, _)| w.pos[*v as usize].distance(*p) > 1e-12 * w.diag)
        .count();
    if moved == 0 {
        return Ok(0);
    }
    if try_batch(w, &moves) {
        Ok(moved)
    } else {
        Err("even spacing would fold, flip or flatten a polygon beside it".into())
    }
}

fn outcome(mesh: &EditMesh, w: &Work, message: String, skipped: Vec<String>) -> EditOutcome {
    let (out, map) = w.finish();
    let selection = w.selected_edges(&map);
    EditOutcome {
        vertex_map: map[..mesh.positions.len()].to_vec(),
        report: EditReport {
            before: mesh.stats(),
            after: out.stats(),
            reached_target: true,
            message,
            skipped,
        },
        mesh: out,
        selection,
        selected_vertices: Vec::new(),
    }
}

/// Space (SPEC §9.5.12): distribute the vertices of each selected run
/// evenly along its current shape.
pub fn space(mesh: &EditMesh, sel: &Selection, p: &SpaceParams) -> Result<EditOutcome, Refusal> {
    check_input(mesh)?;
    let min_cos = check_limit(p.max_normal_change_deg)?;
    let mut w = Work::new(mesh, p.preserve.mask());
    w.min_cos = min_cos;
    mark_selection(&mut w, sel)?;
    let mut skipped = Vec::new();
    let (mut done, mut moved) = (0, 0);
    for (chain, closed) in runs(&w) {
        let movable: Vec<bool> = chain.iter().map(|&v| movable_along(&w, v)).collect();
        match space_run(&mut w, &chain, closed, &movable) {
            Ok(0) => {}
            Ok(n) => {
                done += 1;
                moved += n;
            }
            Err(r) => skipped.push(format!(
                "run from vertex {} ({} vertices): {r}",
                chain[0],
                chain.len()
            )),
        }
    }
    if done == 0 && !skipped.is_empty() {
        return Err(Refusal::new(format!("Can't space: {}", skipped[0])));
    }
    let msg = if done == 0 {
        "Already evenly spaced".to_string()
    } else {
        format!("Spaced {done} run(s), moved {moved} vertices")
    };
    Ok(outcome(mesh, &w, msg, skipped))
}

/// Relax (SPEC §9.5.11): smooth each selected run.
pub fn relax(mesh: &EditMesh, sel: &Selection, p: &RelaxParams) -> Result<EditOutcome, Refusal> {
    check_input(mesh)?;
    let min_cos = check_limit(p.max_normal_change_deg)?;
    if p.iterations == 0 || p.iterations > MAX_RELAX_ITERATIONS {
        return Err(Refusal::new(format!(
            "Relax takes 1 to {MAX_RELAX_ITERATIONS} iterations"
        )));
    }
    let mut w = Work::new(mesh, p.preserve.mask());
    w.min_cos = min_cos;
    mark_selection(&mut w, sel)?;
    let all = runs(&w);
    let mut skipped = Vec::new();
    let mut touched: BTreeSet<u32> = BTreeSet::new();
    let mut any_movable = false;
    for (chain, closed) in &all {
        let n = chain.len();
        let movable: Vec<bool> = chain.iter().map(|&v| movable_free(&w, v)).collect();
        if !movable.iter().any(|&m| m) {
            continue;
        }
        any_movable = true;
        let at = |i: isize| -> Option<usize> {
            if *closed {
                Some(i.rem_euclid(n as isize) as usize)
            } else {
                (i >= 0 && (i as usize) < n).then_some(i as usize)
            }
        };
        let mut stuck = 0usize;
        for _ in 0..p.iterations {
            let snap: Vec<DVec3> = chain.iter().map(|&v| w.pos[v as usize]).collect();
            let sprov: Vec<Prov> = chain.iter().map(|&v| w.prov[v as usize].clone()).collect();
            for i in 0..n {
                if !movable[i] {
                    continue;
                }
                let ii = i as isize;
                let (Some(a), Some(b)) = (at(ii - 1), at(ii + 1)) else {
                    continue;
                };
                let mid = (snap[a] + snap[b]) * 0.5;
                let target = match (p.interpolation, at(ii - 2), at(ii + 2)) {
                    (Interpolation::Cubic, Some(a2), Some(b2))
                        if n >= 5 || !*closed && a2 != b2 =>
                    {
                        (-snap[a2] + 4.0 * snap[a] + 4.0 * snap[b] - snap[b2]) / 6.0
                    }
                    _ => mid,
                };
                let v = chain[i];
                let from = w.pos[v as usize];
                let reach = from.distance(mid).max(1e-300);
                let mut moved = false;
                // Damped: half way to the target per iteration (a full
                // step only flips a zig-zag over), halved again if that
                // would break a polygon.
                for t in [0.5, 0.25, 0.125] {
                    let q = from + (target - from) * t;
                    if q.distance(from) <= 1e-12 * w.diag {
                        moved = true;
                        break;
                    }
                    // Attributes blend towards the neighbours as far as
                    // the vertex moved towards them.
                    let blend = (q.distance(from) / reach).clamp(0.0, 1.0) * 0.5;
                    let prov = Prov::lerp(
                        &sprov[i],
                        &Prov::mean(&[sprov[a].clone(), sprov[b].clone()]),
                        blend,
                    );
                    if try_batch(&mut w, &[(v, q, prov)]) {
                        touched.insert(v);
                        moved = true;
                        break;
                    }
                }
                if !moved {
                    stuck += 1;
                }
            }
            if p.even {
                let moves: Vec<(u32, DVec3, Prov)> = pieces(chain, *closed, &movable)
                    .iter()
                    .flat_map(|pc| even_targets(&w, pc))
                    .collect();
                if try_batch(&mut w, &moves) {
                    touched.extend(moves.iter().map(|m| m.0));
                }
            }
        }
        if stuck > 0 {
            skipped.push(format!(
                "run from vertex {}: {stuck} step(s) held back to keep the polygons beside it valid",
                chain[0]
            ));
        }
    }
    if !any_movable {
        return Err(Refusal::new(
            "Nothing to relax: the selected runs have no free vertices between their ends (run ends, corners and feature-edge vertices stay)",
        ));
    }
    let moved = touched
        .iter()
        .filter(|&&v| w.pos[v as usize].distance(mesh.positions[v as usize]) > 1e-12 * w.diag)
        .count();
    let msg = format!(
        "Relaxed {} run(s) × {} iteration(s), moved {moved} vertices",
        all.len(),
        p.iterations
    );
    Ok(outcome(mesh, &w, msg, skipped))
}

/// Shape check used by the projection step: shape-valid polygons stay
/// shape-valid, bad ones keep their area and orientation.
pub(crate) fn polygon_acceptable(before: &[DVec3], after: &[DVec3], diag: f64) -> bool {
    let (ab, aa) = (area_vec(before), area_vec(after));
    if polygon_shape_ok(before, diag) {
        polygon_shape_ok(after, diag) && aa.dot(ab) > 0.0
    } else {
        aa.length() > 1e-14 * diag * diag && aa.dot(ab) > 0.0
    }
}
