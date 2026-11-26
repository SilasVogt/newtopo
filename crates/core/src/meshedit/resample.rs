//! Resample (SPEC §9.5.5): respace the vertices of edge runs evenly by arc
//! length, optionally to a new vertex count or target edge length. Run ends,
//! pinned vertices, feature junctions and corners sharper than an angle
//! stay fixed; the pieces between them are resampled independently. Own
//! design; the new vertex count is distributed over the pieces by length
//! with the largest-remainder method, and old vertices are matched to new
//! positions by a monotone assignment (dynamic program) so as few vertices
//! as possible are created or removed.

use super::refill::fill_touched_ngons;
use super::work::{Prov, VClass, Work, polygon_shape_ok};
use super::{
    EditOutcome, EditReport, Preserve, Selection, check_input, mark_selection, run_neighbors, runs,
};
use crate::meshedit::mesh::{EditMesh, area_vec};
use crate::ops::Refusal;
use glam::DVec3;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Spacing {
    /// Keep the vertex count, space evenly.
    Even,
    /// Total vertices on each run (an open run counts both ends).
    Count(u32),
    /// Target edge length (body units); each piece gets the nearest whole
    /// number of edges, at least one.
    EdgeLength(f64),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResampleParams {
    pub spacing: Spacing,
    /// Run vertices where the run turns by more than this (degrees) stay
    /// fixed as corners.
    pub corner_angle_deg: f64,
    /// Refill polygons that grew beyond four sides with quads and
    /// triangles.
    pub fill_ngons: bool,
    pub preserve: Preserve,
    pub max_normal_change_deg: f64,
}

impl Default for ResampleParams {
    fn default() -> Self {
        ResampleParams {
            spacing: Spacing::Even,
            corner_angle_deg: 30.0,
            fill_ngons: true,
            preserve: Preserve::default(),
            max_normal_change_deg: 60.0,
        }
    }
}

pub fn resample(
    mesh: &EditMesh,
    sel: &Selection,
    p: &ResampleParams,
) -> Result<EditOutcome, Refusal> {
    check_input(mesh)?;
    match p.spacing {
        Spacing::Count(n) if n < 2 => {
            return Err(Refusal::new("A run needs at least two vertices"));
        }
        Spacing::EdgeLength(l) if !(l > 0.0 && l.is_finite()) => {
            return Err(Refusal::new("The edge length must be positive"));
        }
        _ => {}
    }
    if !(p.corner_angle_deg > 0.0 && p.corner_angle_deg <= 180.0) {
        return Err(Refusal::new(
            "The corner angle must be above 0° and at most 180°",
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
    mark_selection(&mut w, sel)?;
    let cos_corner = p.corner_angle_deg.to_radians().cos();
    let all = runs(&w);
    let added = all
        .iter()
        .map(|(chain, closed)| new_vertices(&w, chain, *closed, p.spacing, cos_corner))
        .fold(0usize, usize::saturating_add);
    #[cfg(test)]
    LAST_ESTIMATE.with(|c| c.set(added));
    if added > MAX_NEW_VERTICES {
        return Err(Refusal::new(format!(
            "Resampling would add about {added} vertices, more than the \
             {MAX_NEW_VERTICES} this tool adds at once: select fewer edges, or \
             use a smaller count or a longer edge length"
        )));
    }
    let mut skipped = Vec::new();
    let mut done = 0;
    w.log_changes = true;
    for (chain, closed) in &all {
        // `check_around` looks at what this run changes. A run that fails
        // is undone from the undo record: a copy of the whole working mesh
        // per run made long selections (Ctrl+A) quadratic.
        w.changed.clear();
        #[cfg(test)]
        let copy = w.clone();
        w.begin_undo();
        match resample_run(&mut w, chain, *closed, p, cos_corner) {
            Ok(changed) => {
                w.commit_undo();
                done += changed as usize;
            }
            Err(reason) => {
                w.rollback();
                #[cfg(test)]
                assert!(w.same_state(&copy), "the undo differs from a copy");
                skipped.push(format!(
                    "run from vertex {} ({} vertices): {reason}",
                    chain[0],
                    chain.len()
                ));
            }
        }
    }
    if done == 0 && !skipped.is_empty() {
        return Err(Refusal::new(format!("Can't resample: {}", skipped[0])));
    }
    let (out, map) = w.finish();
    let selection = w.selected_edges(&map);
    let after = out.stats();
    Ok(EditOutcome {
        vertex_map: map[..mesh.positions.len()].to_vec(),
        mesh: out,
        selection,
        selected_vertices: Vec::new(),
        report: EditReport {
            before,
            after,
            reached_target: true,
            message: format!("Resampled {done} run(s), skipped {}", skipped.len()),
            skipped,
        },
    })
}

/// One run split at its fixed vertices: the vertex sequence, the pieces
/// (index ranges into it) and the number of edges each piece gets.
type RunPlan = (Vec<u32>, Vec<(usize, usize)>, Vec<usize>);

/// How a run of at least two vertices would be resampled. Reads only.
fn run_plan(
    w: &Work,
    chain: &[u32],
    closed: bool,
    spacing: Spacing,
    cos_corner: f64,
) -> Result<RunPlan, String> {
    let n = chain.len();
    let at = |i: usize| chain[i % n];
    let fixed_at = |i: usize| -> bool {
        if !closed && (i == 0 || i == n - 1) {
            return true;
        }
        let (p, x, q) = (at(i + n - 1), at(i), at(i + 1));
        // Where the selection doesn't run straight through (a junction a
        // closed run starts at, shared with other runs), the vertex stays.
        if run_neighbors(w, x).len() != 2 {
            return true;
        }
        let class_fixed = match w.vclass(x) {
            VClass::Corner => true,
            VClass::Free => false,
            VClass::Curve([a, b]) => !((a, b) == (p, q) || (a, b) == (q, p)),
        };
        if class_fixed {
            return true;
        }
        let d1 = (w.pos[x as usize] - w.pos[p as usize]).normalize_or_zero();
        let d2 = (w.pos[q as usize] - w.pos[x as usize]).normalize_or_zero();
        d1.dot(d2) < cos_corner
    };
    let mut fixed: Vec<usize> = (0..n).filter(|&i| fixed_at(i)).collect();
    if closed && fixed.is_empty() {
        fixed.push(0);
    }
    // Sequence of vertices from fixed[0] round to it again (closed) or the
    // open chain.
    let seq: Vec<u32> = if closed {
        (0..=n).map(|k| at(fixed[0] + k)).collect()
    } else {
        chain.to_vec()
    };
    let fixed_pos: Vec<usize> = if closed {
        let mut f: Vec<usize> = fixed.iter().map(|&i| (i + n - fixed[0]) % n).collect();
        f.push(n);
        f
    } else {
        fixed.clone()
    };
    // Pieces between consecutive fixed vertices.
    let pieces: Vec<(usize, usize)> = fixed_pos.windows(2).map(|w| (w[0], w[1])).collect();
    let lens: Vec<f64> = pieces
        .iter()
        .map(|&(a, b)| {
            (a..b)
                .map(|k| w.pos[seq[k] as usize].distance(w.pos[seq[k + 1] as usize]))
                .sum()
        })
        .collect();
    let mut segs: Vec<usize> = match spacing {
        Spacing::Even => pieces.iter().map(|&(a, b)| b - a).collect(),
        Spacing::EdgeLength(l) => lens
            .iter()
            .map(|&len| ((len / l).round() as usize).max(1))
            .collect(),
        Spacing::Count(c) => {
            let total = if closed {
                c as usize
            } else {
                (c as usize).saturating_sub(1)
            };
            if closed && c < 3 {
                return Err("a closed run needs at least three vertices".into());
            }
            if total < pieces.len() {
                return Err(format!(
                    "{c} vertices is fewer than the run's {} fixed corner(s) allow",
                    fixed.len()
                ));
            }
            largest_remainder(total, &lens)
        }
    };
    // A closed run keeps at least three vertices: give the missing edges to
    // the longest pieces.
    while closed && segs.iter().sum::<usize>() < 3 {
        let i = (0..segs.len())
            .max_by(|&a, &b| (lens[a] / segs[a] as f64).total_cmp(&(lens[b] / segs[b] as f64)))
            .unwrap_or(0);
        segs[i] += 1;
    }
    let edges: usize = segs.iter().sum();
    if edges > MAX_RUN_EDGES {
        return Err(format!(
            "{edges} edges on one run is more than the {MAX_RUN_EDGES} this tool makes"
        ));
    }
    Ok((seq, pieces, segs))
}

/// Resample one run. `Ok(false)`: nothing to change.
fn resample_run(
    w: &mut Work,
    chain: &[u32],
    closed: bool,
    params: &ResampleParams,
    cos_corner: f64,
) -> Result<bool, String> {
    let spacing = params.spacing;
    let n = chain.len();
    if n < 2 {
        return Ok(false);
    }
    // Runs are found up front; one that an earlier run changed (it can only
    // share fixed vertices with it, so this is a safety net) is skipped.
    if chain.iter().any(|&v| !w.valive[v as usize]) {
        return Err("an earlier run removed one of its vertices".into());
    }
    let (seq, pieces, segs) = run_plan(w, chain, closed, spacing, cos_corner)?;
    let mut changed = false;
    for (pi, &(a, b)) in pieces.iter().enumerate() {
        changed |= resample_piece(w, &seq[a..=b], segs[pi])?;
    }
    if changed {
        // Refill first: a triangle that gained a vertex on the run is a
        // quad with a straight corner until it is refilled.
        if params.fill_ngons {
            fill_touched_ngons(w);
        }
        check_around(w, &seq)?;
    }
    Ok(changed)
}

/// Most edges one run may get (keeps the matching tables small).
const MAX_RUN_EDGES: usize = 10_000;

/// Most vertices one Resample may add, estimated before it starts (SPEC
/// §9.5.5). The app runs the tool on its UI thread, again on every
/// parameter change; a large count on a whole-mesh selection would
/// otherwise stall it.
pub(crate) const MAX_NEW_VERTICES: usize = 100_000;

#[cfg(test)]
thread_local! {
    /// The estimate the last `resample` on this thread made (tests).
    pub(crate) static LAST_ESTIMATE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Vertices resampling one run would add, piece by piece as the run is
/// really split at its fixed vertices. Pieces that lose vertices do not
/// offset the ones that gain, so this never underestimates what
/// [`MAX_NEW_VERTICES`] guards against. A run that would be skipped adds
/// nothing.
fn new_vertices(w: &Work, chain: &[u32], closed: bool, spacing: Spacing, cos_corner: f64) -> usize {
    if chain.len() < 2 {
        return 0;
    }
    match run_plan(w, chain, closed, spacing, cos_corner) {
        Ok((_, pieces, segs)) => pieces
            .iter()
            .zip(&segs)
            .map(|(&(a, b), &k)| k.saturating_sub(b - a))
            .fold(0usize, usize::saturating_add),
        Err(_) => 0,
    }
}

/// Resample the open piece `verts` (ends fixed) to `k` edges.
fn resample_piece(w: &mut Work, verts: &[u32], k: usize) -> Result<bool, String> {
    let m = verts.len() - 2; // old interior vertices
    let pts: Vec<DVec3> = verts.iter().map(|&v| w.pos[v as usize]).collect();
    let provs: Vec<Prov> = verts.iter().map(|&v| w.prov[v as usize].clone()).collect();
    let mut cum = vec![0.0];
    for i in 0..pts.len() - 1 {
        cum.push(cum[i] + pts[i].distance(pts[i + 1]));
    }
    let total = *cum.last().unwrap();
    if total <= 1e-12 * w.diag {
        return Err("the run has zero length".into());
    }
    let t_old: Vec<f64> = (1..=m).map(|i| cum[i] / total).collect();
    let s_new: Vec<f64> = (1..k).map(|j| j as f64 / k as f64).collect();
    // Target positions on the original polyline.
    let targets: Vec<(DVec3, Prov)> = s_new
        .iter()
        .map(|&s| {
            let d = s * total;
            let i = (0..pts.len() - 1)
                .find(|&i| cum[i + 1] >= d)
                .unwrap_or(pts.len() - 2);
            let seg = (cum[i + 1] - cum[i]).max(1e-300);
            let t = ((d - cum[i]) / seg).clamp(0.0, 1.0);
            (
                pts[i].lerp(pts[i + 1], t),
                Prov::lerp(&provs[i], &provs[i + 1], t),
            )
        })
        .collect();
    let already = m == k - 1
        && t_old
            .iter()
            .zip(&s_new)
            .all(|(t, s)| (t - s).abs() * total <= 1e-9 * w.diag);
    if already {
        return Ok(false);
    }
    let pairs = match_slots(&t_old, &s_new)?; // (old index, slot index)
    let kept: Vec<usize> = pairs.iter().map(|p| p.0).collect();
    // 1. Remove unmatched old vertices by merging them into the previous
    //    surviving vertex of the piece (or the next one).
    let mut alive: Vec<u32> = verts.to_vec(); // current piece sequence
    for i in 0..m {
        if kept.contains(&i) {
            continue;
        }
        let x = verts[i + 1];
        let pos = alive.iter().position(|&v| v == x).expect("in piece");
        let (prev, next) = (alive[pos - 1], alive[pos + 1]);
        let ok = [prev, next].into_iter().any(|t| {
            if !w.merge_allowed(x, t) {
                return false;
            }
            match w.plan_merge(x, t) {
                Some(plan) => {
                    w.apply_merge(&plan, Vec::new());
                    true
                }
                None => false,
            }
        });
        if !ok {
            return Err(format!(
                "vertex {x} can't be removed without changing the topology"
            ));
        }
        alive.remove(pos);
    }
    // 2. Insert new vertices for unmatched slots, between the surviving
    //    vertices around them.
    let mut slot_vertex: Vec<Option<u32>> = vec![None; s_new.len()];
    for &(i, j) in &pairs {
        slot_vertex[j] = Some(verts[i + 1]);
    }
    let mut prev_v = verts[0];
    let mut j = 0;
    while j < s_new.len() {
        if let Some(v) = slot_vertex[j] {
            prev_v = v;
            j += 1;
            continue;
        }
        let start = j;
        while j < s_new.len() && slot_vertex[j].is_none() {
            j += 1;
        }
        let next_v = if j < s_new.len() {
            slot_vertex[j].unwrap()
        } else {
            *verts.last().unwrap()
        };
        if w.edge_polys(prev_v, next_v).is_empty() {
            return Err(format!("{prev_v}-{next_v} is no longer an edge"));
        }
        let ids = w.split_edge(prev_v, next_v, &targets[start..j]);
        if ids.len() != j - start {
            return Err("internal: split failed".into());
        }
        for (o, id) in ids.into_iter().enumerate() {
            slot_vertex[start + o] = Some(id);
        }
    }
    // 3. Move matched old vertices to their slots.
    for &(i, jj) in &pairs {
        let (p, pr) = targets[jj].clone();
        w.set_position(verts[i + 1], p, pr);
    }
    Ok(true)
}

/// Every polygon touching the run, and every polygon the run changed,
/// keeps a valid shape and orientation. (Polygons earlier runs changed
/// passed this when they last changed.)
fn check_around(w: &Work, seq: &[u32]) -> Result<(), String> {
    let mut polys: Vec<u32> = Vec::new();
    for &v in seq {
        if w.valive[v as usize] {
            polys.extend_from_slice(&w.vpolys[v as usize]);
        }
    }
    // Inserted vertices are reached through the change log.
    polys.extend_from_slice(&w.changed);
    polys.sort_unstable();
    polys.dedup();
    for q in polys {
        let wp = &w.polys[q as usize];
        if !wp.alive {
            continue;
        }
        let pos = w.poly_pos(&wp.verts);
        let nrm = area_vec(&pos).normalize_or_zero();
        let shape = if wp.verts.len() <= 4 {
            polygon_shape_ok(&pos, w.diag)
        } else {
            area_vec(&pos).length() > 1e-12 * w.diag * w.diag && simple_ngon(&pos)
        };
        if !shape || nrm.dot(w.reference_normal(&wp.verts, wp)) < w.min_cos {
            return Err("the new spacing would fold or flatten a neighbouring polygon".into());
        }
    }
    Ok(())
}

/// No two non-adjacent edges of the polygon touch (in its best-fit
/// plane): a respaced n-gon may be concave, never self-intersecting.
fn simple_ngon(pos: &[DVec3]) -> bool {
    use crate::fill::geom::{project_best_fit, segments_touch};
    let Some(uv) = project_best_fit(pos) else {
        return false;
    };
    let n = uv.len();
    for i in 0..n {
        for j in i + 2..n {
            if i == 0 && j == n - 1 {
                continue; // adjacent through the closing edge
            }
            if segments_touch(uv[i], uv[i + 1], uv[j], uv[(j + 1) % n]) {
                return false;
            }
        }
    }
    true
}

/// Distribute `total` items over parts by weight, at least one each
/// (largest-remainder / Hamilton method). Needs `total >= parts`.
fn largest_remainder(total: usize, weights: &[f64]) -> Vec<usize> {
    let n = weights.len();
    let spare = total - n;
    let sum: f64 = weights.iter().sum::<f64>().max(1e-300);
    let quotas: Vec<f64> = weights.iter().map(|w| w / sum * spare as f64).collect();
    let mut out: Vec<usize> = quotas.iter().map(|q| 1 + q.floor() as usize).collect();
    let mut left = total - out.iter().sum::<usize>();
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        let (ra, rb) = (quotas[a] - quotas[a].floor(), quotas[b] - quotas[b].floor());
        rb.total_cmp(&ra).then(a.cmp(&b))
    });
    for i in order {
        if left == 0 {
            break;
        }
        out[i] += 1;
        left -= 1;
    }
    out
}

/// Most cells of the matching table (about 36 MB).
const MAX_MATCH_CELLS: usize = 4_000_000;

/// Old vertices matched to new slots. Equal counts have one monotone
/// match, index to index; otherwise the matching table must stay small
/// (the output edge limit does not bound the input length).
fn match_slots(t: &[f64], s: &[f64]) -> Result<Vec<(usize, usize)>, String> {
    if t.len() == s.len() {
        return Ok((0..t.len()).map(|i| (i, i)).collect());
    }
    if (t.len() + 1).saturating_mul(s.len() + 1) > MAX_MATCH_CELLS {
        return Err(format!(
            "{} vertices to match against {} new ones is more than this tool handles at once",
            t.len(),
            s.len()
        ));
    }
    Ok(monotone_match(t, s))
}

/// Monotone matching of `min(a, b)` pairs between sorted parameter lists,
/// minimising the summed distance. Returns (index into `t`, index into `s`).
fn monotone_match(t: &[f64], s: &[f64]) -> Vec<(usize, usize)> {
    let (a, b) = (t.len(), s.len());
    if a == 0 || b == 0 {
        return Vec::new();
    }
    // dp[i][j]: best cost using t[..i] and s[..j] with min(i, j) pairs.
    let inf = f64::INFINITY;
    let mut dp = vec![vec![inf; b + 1]; a + 1];
    let mut choice = vec![vec![0u8; b + 1]; a + 1]; // 1 pair, 2 skip t, 3 skip s
    for i in 0..=a {
        for j in 0..=b {
            if i.min(j) == 0 {
                dp[i][j] = 0.0;
                choice[i][j] = if i > 0 {
                    2
                } else if j > 0 {
                    3
                } else {
                    0
                };
                continue;
            }
            let mut best = (dp[i - 1][j - 1] + (t[i - 1] - s[j - 1]).abs(), 1u8);
            if a > b && i > j {
                // Extra old vertices may be skipped.
                let c = dp[i - 1][j];
                if c < best.0 {
                    best = (c, 2);
                }
            }
            if b > a && j > i {
                let c = dp[i][j - 1];
                if c < best.0 {
                    best = (c, 3);
                }
            }
            dp[i][j] = best.0;
            choice[i][j] = best.1;
        }
    }
    let (mut i, mut j) = (a, b);
    let mut out = Vec::new();
    while i > 0 && j > 0 {
        match choice[i][j] {
            1 => {
                out.push((i - 1, j - 1));
                i -= 1;
                j -= 1;
            }
            2 => i -= 1,
            _ => j -= 1,
        }
    }
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hamilton() {
        assert_eq!(largest_remainder(10, &[3.0, 7.0]), vec![3, 7]);
        assert_eq!(largest_remainder(3, &[1.0, 1.0, 1.0]), vec![1, 1, 1]);
        assert_eq!(largest_remainder(4, &[1.0, 100.0]).iter().sum::<usize>(), 4);
    }

    #[test]
    fn matching() {
        let m = monotone_match(&[0.1, 0.2, 0.7, 0.9], &[0.25, 0.75]);
        assert_eq!(m, vec![(1, 0), (2, 1)]);
        let m = monotone_match(&[0.5], &[0.2, 0.4, 0.6, 0.8]);
        assert_eq!(m.len(), 1);
        assert!(m[0].1 == 1 || m[0].1 == 2);
        let m = monotone_match(&[0.3, 0.6], &[0.33, 0.66]);
        assert_eq!(m, vec![(0, 0), (1, 1)]);
    }

    #[test]
    fn matching_stays_small_for_long_runs() {
        // Equal counts (Even spacing): index to index, the only full
        // monotone match, without a table (20 000² cells would be 3.6 GB).
        let n = 20_000;
        let t: Vec<f64> = (0..n)
            .map(|i| (i as f64 + 0.3) / (n as f64 + 1.0))
            .collect();
        let s: Vec<f64> = (0..n)
            .map(|i| (i as f64 + 1.0) / (n as f64 + 1.0))
            .collect();
        let m = match_slots(&t, &s).unwrap();
        assert_eq!(m.len(), n);
        assert!(m.iter().enumerate().all(|(i, &p)| p == (i, i)));
        let small = [0.1, 0.35, 0.8];
        assert_eq!(
            match_slots(&small, &[0.25, 0.5, 0.75]).unwrap(),
            monotone_match(&small, &[0.25, 0.5, 0.75])
        );
        // Unequal counts on a long run are refused before any allocation.
        assert!(match_slots(&t, &s[..n / 2]).is_err());
        assert_eq!(match_slots(&small, &[0.5]).unwrap().len(), 1);
    }
}
