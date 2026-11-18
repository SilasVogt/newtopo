//! Decimate (SPEC §9.5.3).
//!
//! **Collapse** mode: greedy vertex merges ordered by quadric error
//! (Garland & Heckbert 1997) with *subset placement*: a vertex is merged
//! into a neighbour and every remaining vertex keeps its baked position, so
//! the result still lies exactly on the CAD surface at its vertices and the
//! baked normals stay exact. Merge targets are edge neighbours and, to keep
//! quads, the opposite corner of a quad ("diagonal collapse", as used for
//! quad meshes by Daniels et al., "Quadrilateral mesh simplification",
//! SIGGRAPH Asia 2008, and Tarini et al., "Practical quad mesh
//! simplification", Eurographics 2010). Feature edges get constraint
//! planes as in Garland & Heckbert §6.1. A final pass joins triangle pairs
//! into quads.
//!
//! **Planar** mode: join neighbouring polygons whose normals are within an
//! angle (own design, region growing with a normal cone), dissolve
//! straight-through vertices, refill with the auto-fill decomposition.

use super::mesh::{EditMesh, area_vec, ekey};
use super::quadric::Quadric;
use super::refill::{fill_touched_ngons, pair_triangles, quad_quality};
use super::work::{F_CAD, F_HARD, VClass, Work, assign_witnesses};
use super::{EditOutcome, EditReport, Preserve, check_input};
use crate::ops::Refusal;
use glam::DVec3;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeSet, BinaryHeap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum DecimateMode {
    /// Quadric-ordered vertex merges down to a target.
    Collapse,
    /// Merge nearly coplanar polygons (no target).
    Planar,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Target {
    /// Fraction of the current triangle count, in (0, 1].
    Ratio(f64),
    /// Triangle count (an n-gon counts n - 2).
    Triangles(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DecimateParams {
    pub mode: DecimateMode,
    /// Collapse mode only.
    pub target: Target,
    /// Upper bound on how far any baked vertex may end up from the result
    /// (body units). `None`: only the target limits the reduction.
    pub max_deviation: Option<f64>,
    pub preserve: Preserve,
    /// Prefer merges that keep quads and join triangle pairs afterwards.
    pub keep_quads: bool,
    /// Planar mode: polygons whose normals differ by at most this many
    /// degrees are merged; vertices whose edges turn by at most this are
    /// dissolved.
    pub planar_angle_deg: f64,
    /// Largest change of a polygon's normal against the CAD normal allowed
    /// by any single step, in degrees.
    pub max_normal_change_deg: f64,
}

impl Default for DecimateParams {
    fn default() -> Self {
        DecimateParams {
            mode: DecimateMode::Collapse,
            target: Target::Ratio(0.5),
            max_deviation: None,
            preserve: Preserve::default(),
            keep_quads: true,
            planar_angle_deg: 5.0,
            max_normal_change_deg: 60.0,
        }
    }
}

/// Reduce the polygon count. Pure: the input is not changed.
pub fn decimate(mesh: &EditMesh, p: &DecimateParams) -> Result<EditOutcome, Refusal> {
    check_input(mesh)?;
    if !(p.max_normal_change_deg > 0.0 && p.max_normal_change_deg < 90.0) {
        return Err(Refusal::new(
            "The normal-change limit must be between 0° and 90°",
        ));
    }
    if let Some(d) = p.max_deviation
        && !(d >= 0.0 && d.is_finite())
    {
        return Err(Refusal::new("The deviation limit must be zero or positive"));
    }
    let before = mesh.stats();
    let mask = p.preserve.mask();
    let mut w = Work::new(mesh, mask);
    w.min_cos = p.max_normal_change_deg.to_radians().cos();
    if let Some(d) = p.max_deviation {
        w.track_deviation(d);
    }
    let (reached, message) = match p.mode {
        DecimateMode::Collapse => {
            let tris = mesh.tri_count();
            let target = match p.target {
                Target::Ratio(r) => {
                    if !(r > 0.0 && r <= 1.0) {
                        return Err(Refusal::new("The ratio must be above 0 and at most 1"));
                    }
                    (r * tris as f64).ceil() as usize
                }
                Target::Triangles(n) => n,
            };
            if target < 1 {
                return Err(Refusal::new("The target must keep at least one triangle"));
            }
            collapse(&mut w, target, p.keep_quads);
            let reached = w.tri_count() <= target;
            // Joining triangle pairs is part of reducing; a target the input
            // already meets leaves the mesh exactly as it was.
            if p.keep_quads && target < tris {
                pair_triangles(&mut w, 40.0);
            }
            let msg = if reached {
                format!("Decimated to {} triangles (target {target})", w.tri_count())
            } else {
                format!(
                    "Stopped at {} triangles (target {target}): every further merge would break a preserved edge, the shape limit or the topology",
                    w.tri_count()
                )
            };
            (reached, msg)
        }
        DecimateMode::Planar => {
            if !(p.planar_angle_deg > 0.0 && p.planar_angle_deg < 90.0) {
                return Err(Refusal::new("The planar angle must be between 0° and 90°"));
            }
            let (joined, dissolved) = planar(&mut w, p.planar_angle_deg);
            fill_touched_ngons(&mut w);
            if p.keep_quads {
                pair_triangles(&mut w, p.planar_angle_deg);
            }
            (
                true,
                format!(
                    "Merged {joined} coplanar polygon pair(s), dissolved {dissolved} vertex(es)"
                ),
            )
        }
    };
    let (out, map) = w.finish();
    let after = out.stats();
    Ok(EditOutcome {
        vertex_map: map[..mesh.positions.len()].to_vec(),
        mesh: out,
        selection: Vec::new(),
        report: EditReport {
            before,
            after,
            reached_target: reached,
            message,
            skipped: Vec::new(),
        },
    })
}

// ---------------------------------------------------------------- collapse

#[derive(Clone, Copy, Debug)]
struct Cand {
    cost: f64,
    u: u32,
    v: u32,
    stamp: u32,
}

impl PartialEq for Cand {
    fn eq(&self, o: &Self) -> bool {
        self.cmp(o) == Ordering::Equal
    }
}
impl Eq for Cand {}
impl PartialOrd for Cand {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Cand {
    /// Reversed: the heap pops the cheapest, ties by lower `(u, v)`.
    fn cmp(&self, o: &Self) -> Ordering {
        o.cost
            .total_cmp(&self.cost)
            .then(o.u.cmp(&self.u))
            .then(o.v.cmp(&self.v))
    }
}

/// Weight of a lost quad and of shape quality, in units of the local
/// squared edge length (quadric errors are normalised to the same units).
const QUAD_WEIGHT: f64 = 0.02;
const SHAPE_WEIGHT: f64 = 0.01;
/// Constraint-plane weight for feature edges (Garland & Heckbert §6.1).
const FEATURE_WEIGHT: f64 = 100.0;

struct Collapse {
    q: Vec<Quadric>,
    stamp: Vec<u32>,
    keep_quads: bool,
}

fn collapse(w: &mut Work, target: usize, keep_quads: bool) {
    let mean_area = {
        let (mut s, mut n) = (0.0, 0);
        for p in w.alive_polys() {
            s += area_vec(&w.poly_pos(&w.polys[p as usize].verts)).length() * 0.5;
            n += 1;
        }
        (s / n.max(1) as f64).max(1e-300)
    };
    let mean_len2 = mean_area.max(1e-300);
    let nv = w.pos.len();
    let mut q = vec![Quadric::default(); nv];
    for p in w.alive_polys().collect::<Vec<_>>() {
        let verts = w.polys[p as usize].verts.clone();
        let pos = w.poly_pos(&verts);
        let av = area_vec(&pos);
        let n = av.normalize_or_zero();
        if n == DVec3::ZERO {
            continue;
        }
        let c = pos.iter().copied().sum::<DVec3>() / pos.len() as f64;
        let pl = Quadric::plane(n, c, av.length() * 0.5 / mean_area);
        for &v in &verts {
            q[v as usize].add(&pl);
        }
        // Feature constraint planes, perpendicular to the polygon along
        // each feature edge.
        for i in 0..verts.len() {
            let (a, b) = (verts[i], verts[(i + 1) % verts.len()]);
            if !w.is_feature(a, b) {
                continue;
            }
            let d = w.pos[b as usize] - w.pos[a as usize];
            let m = d.cross(n).normalize_or_zero();
            if m == DVec3::ZERO {
                continue;
            }
            let pl = Quadric::plane(
                m,
                w.pos[a as usize],
                FEATURE_WEIGHT * d.length_squared() / mean_len2,
            );
            q[a as usize].add(&pl);
            q[b as usize].add(&pl);
        }
    }
    let mut st = Collapse {
        q,
        stamp: vec![0; nv],
        keep_quads,
    };
    let mut heap = BinaryHeap::new();
    for u in 0..nv as u32 {
        if w.valive[u as usize]
            && let Some(c) = best(w, &st, u)
        {
            heap.push(c);
        }
    }
    let mut tris = w.tri_count();
    while tris > target {
        let Some(c) = heap.pop() else { break };
        if c.stamp != st.stamp[c.u as usize] || !w.valive[c.u as usize] {
            continue;
        }
        // Re-verify against the current mesh.
        let ok = w.merge_allowed(c.u, c.v)
            && w.plan_merge(c.u, c.v)
                .and_then(|plan| {
                    if !w.merge_geometry_ok(&plan) {
                        return None;
                    }
                    let wit = w.merge_deviation(&plan)?;
                    Some((plan, wit))
                })
                .map(|(plan, wit)| {
                    let removed: usize = plan
                        .affected
                        .iter()
                        .map(|&p| w.polys[p as usize].verts.len() - 2)
                        .sum::<usize>()
                        - plan.kept_pieces().map(|k| k.len() - 2).sum::<usize>();
                    w.apply_merge(&plan, wit);
                    tris -= removed;
                })
                .is_some();
        if !ok {
            st.stamp[c.u as usize] += 1;
            if let Some(n) = best(w, &st, c.u) {
                heap.push(n);
            }
            continue;
        }
        let qu = st.q[c.u as usize];
        st.q[c.v as usize].add(&qu);
        // Refresh the ring around v.
        let mut ring: BTreeSet<u32> = BTreeSet::new();
        ring.insert(c.v);
        for &p in &w.vpolys[c.v as usize] {
            ring.extend(w.polys[p as usize].verts.iter().copied());
        }
        for x in ring {
            st.stamp[x as usize] += 1;
            if let Some(n) = best(w, &st, x) {
                heap.push(n);
            }
        }
    }
}

/// Cheapest valid merge of `u`.
fn best(w: &Work, st: &Collapse, u: u32) -> Option<Cand> {
    if w.vclass(u) == VClass::Corner {
        return None;
    }
    let mut targets: BTreeSet<u32> = w.neighbors(u).into_iter().collect();
    if st.keep_quads && w.vclass(u) == VClass::Free {
        targets.extend(w.diagonal_neighbors(u));
    }
    let pu = w.pos[u as usize];
    let ring = w.neighbors(u);
    let l2 = ring
        .iter()
        .map(|&x| w.pos[x as usize].distance_squared(pu))
        .sum::<f64>()
        / ring.len().max(1) as f64;
    let mut out: Option<Cand> = None;
    for v in targets {
        if !w.merge_allowed(u, v) {
            continue;
        }
        let Some(plan) = w.plan_merge(u, v) else {
            continue;
        };
        if !w.merge_geometry_ok(&plan) || w.merge_deviation(&plan).is_none() {
            continue;
        }
        let pv = w.pos[v as usize];
        let mut cost = st.q[u as usize].sum(&st.q[v as usize]).eval(pv);
        if st.keep_quads {
            let lost = plan.quads_before(w) as f64 - plan.quads_after() as f64;
            let before: f64 = plan
                .affected
                .iter()
                .map(|&p| 1.0 - quad_quality(&w.poly_pos(&w.polys[p as usize].verts)))
                .sum();
            let after: f64 = plan
                .kept_pieces()
                .map(|k| 1.0 - quad_quality(&w.poly_pos(k)))
                .sum();
            cost += l2 * (QUAD_WEIGHT * lost.max(0.0) + SHAPE_WEIGHT * (after - before));
        }
        let c = Cand {
            cost,
            u,
            v,
            stamp: st.stamp[u as usize],
        };
        if out.is_none_or(|o| c > o) {
            out = Some(c);
        }
    }
    out
}

// ------------------------------------------------------------------ planar

/// Returns (polygon joins, vertex dissolves).
fn planar(w: &mut Work, angle_deg: f64) -> (usize, usize) {
    let cos = angle_deg.to_radians().cos();
    let mut cone: Vec<Vec<DVec3>> = w
        .polys
        .iter()
        .map(|p| vec![area_vec(&w.poly_pos(&p.verts)).normalize_or_zero()])
        .collect();
    let mut joins = 0;
    loop {
        let mut cands: Vec<(f64, (u32, u32))> = Vec::new();
        let mut seen = HashSet::new();
        for p in w.alive_polys().collect::<Vec<_>>() {
            let v = &w.polys[p as usize].verts;
            for i in 0..v.len() {
                let e = ekey(v[i], v[(i + 1) % v.len()]);
                if !seen.insert(e) || w.is_feature(e.0, e.1) {
                    continue;
                }
                let ps = w.edge_polys(e.0, e.1);
                if ps.len() != 2 {
                    continue;
                }
                let n0 = area_vec(&w.poly_pos(&w.polys[ps[0] as usize].verts)).normalize_or_zero();
                let n1 = area_vec(&w.poly_pos(&w.polys[ps[1] as usize].verts)).normalize_or_zero();
                let d = n0.dot(n1);
                if d >= cos {
                    cands.push((d, e));
                }
            }
        }
        cands.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut changed = false;
        for (_, (a, b)) in cands {
            if !w.valive[a as usize] || !w.valive[b as usize] || w.is_feature(a, b) {
                continue;
            }
            let ps = w.edge_polys(a, b);
            if ps.len() != 2 {
                continue;
            }
            if let Some((merged, gone)) = plan_join(w, ps[0], ps[1], &cone, cos) {
                apply_join(w, ps[0], ps[1], merged, &gone, &mut cone);
                joins += 1;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    // Dissolve vertices with two edges that run straight through.
    let mut dissolved = 0;
    loop {
        let mut changed = false;
        for x in 0..w.pos.len() as u32 {
            if !w.valive[x as usize] || w.pinned[x as usize] {
                continue;
            }
            let nb = w.neighbors(x);
            if nb.len() != 2 {
                continue;
            }
            let (p, q) = (nb[0], nb[1]);
            let d1 = (w.pos[x as usize] - w.pos[p as usize]).normalize_or_zero();
            let d2 = (w.pos[q as usize] - w.pos[x as usize]).normalize_or_zero();
            if d1.dot(d2) < cos || w.feature_bits(x, p) != w.feature_bits(x, q) {
                continue;
            }
            if w.neighbors(p).contains(&q) {
                continue;
            }
            if w.try_merge(x, p) {
                dissolved += 1;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    (joins, dissolved)
}

/// Plan joining polygons `a` and `b` across their shared edges. They must
/// share one contiguous chain, the result must be a simple cycle, and its
/// normal must stay within the angle of every polygon merged into it.
/// Returns the merged cycle and the chain-interior vertices it drops.
fn plan_join(
    w: &Work,
    a: u32,
    b: u32,
    cone: &[Vec<DVec3>],
    cos: f64,
) -> Option<(Vec<u32>, Vec<u32>)> {
    // Polygons keep their group (material, export grouping): never join
    // across groups, even where the shared edge is not a preserved one.
    if a == b || w.polys[a as usize].group != w.polys[b as usize].group {
        return None;
    }
    let pa = &w.polys[a as usize].verts;
    let pb = &w.polys[b as usize].verts;
    let n = pa.len();
    let shared: Vec<bool> = (0..n)
        .map(|i| super::work::has_directed(pb, pa[(i + 1) % n], pa[i]))
        .collect();
    let count = shared.iter().filter(|&&s| s).count();
    if count == 0 || count == n {
        return None;
    }
    // Every shared edge disappears, so none of them may be a feature edge
    // (not just the candidate edge the join was found from).
    if (0..n).any(|i| shared[i] && w.is_feature(pa[i], pa[(i + 1) % n])) {
        return None;
    }
    // Start of the run: shared edge whose predecessor is not shared.
    let starts: Vec<usize> = (0..n)
        .filter(|&i| shared[i] && !shared[(i + n - 1) % n])
        .collect();
    if starts.len() != 1 {
        return None;
    }
    let k = starts[0];
    let s = pa[k];
    let e = pa[(k + count) % n];
    let interior: Vec<u32> = (1..count).map(|j| pa[(k + j) % n]).collect();
    if interior
        .iter()
        .any(|&x| w.vpolys[x as usize].len() != 2 || w.pinned[x as usize])
    {
        return None;
    }
    // P from e to s, then Q after s up to before e.
    let mut merged = Vec::new();
    let mut i = (k + count) % n;
    loop {
        merged.push(pa[i]);
        if pa[i] == s {
            break;
        }
        i = (i + 1) % n;
    }
    let m = pb.len();
    let js = pb.iter().position(|&x| x == s)?;
    let mut j = (js + 1) % m;
    while pb[j] != e {
        merged.push(pb[j]);
        j = (j + 1) % m;
        if j == js {
            return None;
        }
    }
    let uniq: BTreeSet<u32> = merged.iter().copied().collect();
    if uniq.len() != merged.len() || merged.len() < 3 {
        return None;
    }
    let pos = w.poly_pos(&merged);
    let nrm = area_vec(&pos).normalize_or_zero();
    if nrm == DVec3::ZERO {
        return None;
    }
    if cone[a as usize]
        .iter()
        .chain(cone[b as usize].iter())
        .any(|c| c.dot(nrm) < cos)
    {
        return None;
    }
    let parent = &w.polys[a as usize];
    if nrm.dot(w.reference_normal(&merged, parent)) < w.min_cos {
        return None;
    }
    if let Some(ws) = &w.witnesses {
        let mut wit: Vec<u32> = ws[a as usize].clone();
        wit.extend_from_slice(&ws[b as usize]);
        wit.extend(
            interior
                .iter()
                .copied()
                .filter(|&x| (x as usize) < w.src.positions.len()),
        );
        assign_witnesses(&wit, &[pos], &w.src.positions, w.max_dev)?;
    }
    Some((merged, interior))
}

fn apply_join(
    w: &mut Work,
    a: u32,
    b: u32,
    merged: Vec<u32>,
    gone: &[u32],
    cone: &mut [Vec<DVec3>],
) {
    let pb = w.polys[b as usize].verts.clone();
    let pa = w.polys[a as usize].verts.clone();
    // Shared edges disappear with their flags.
    for i in 0..pa.len() {
        let (x, y) = (pa[i], pa[(i + 1) % pa.len()]);
        if super::work::has_directed(&pb, y, x) {
            w.flags.remove(&ekey(x, y));
        }
    }
    for &x in &pb {
        w.vpolys[x as usize].retain(|&q| q != b);
    }
    w.polys[b as usize].alive = false;
    let wit_b = w
        .witnesses
        .as_mut()
        .map(|ws| std::mem::take(&mut ws[b as usize]))
        .unwrap_or_default();
    w.replace_polygon(a, vec![merged]);
    for &x in gone {
        w.valive[x as usize] = false;
        w.vpolys[x as usize].clear();
    }
    if let Some(ws) = &mut w.witnesses {
        ws[a as usize].extend(wit_b);
        ws[a as usize].extend(
            gone.iter()
                .copied()
                .filter(|&x| (x as usize) < w.src.positions.len()),
        );
    }
    let cb = std::mem::take(&mut cone[b as usize]);
    cone[a as usize].extend(cb);
}

impl Preserve {
    pub(crate) fn mask(&self) -> u8 {
        (if self.cad_edges { F_CAD } else { 0 }) | (if self.hard_edges { F_HARD } else { 0 })
    }
}
