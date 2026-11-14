//! Per-face pipeline: patch arrangement (boundary + split lines), line
//! clipping, line crossings, final arrangement with the topology guard.

use super::Owner;
use super::arr::{ACurve, AEvent, Arr, Glued};
use super::clip::{ClipInput, STOP, Who, clip, project_par};
use super::ctx::{BodyCtx, FaceGeom};
use super::geom2::*;
use super::lines::{GLine, Patch};
use crate::doc::{IsoDir, PatchBound, PatchId, SplitKind, SplitLine};
use crate::keys::{SplitId, VertexKey};
use glam::DVec2;
use newtopo_kernel::{BodyId, UvClass};
use std::collections::HashMap;

pub struct Stage1 {
    pub curves: Vec<ACurve>,
    pub events: Vec<AEvent>,
    pub arr: Arr,
    pub glued: Glued,
    pub patches: Vec<Patch>,
    /// Inside cell → patch index.
    pub cell_patch: Vec<Option<usize>>,
}

impl Stage1 {
    pub fn locate(&self, fg: &FaceGeom, uv: DVec2) -> Option<usize> {
        let c = self.arr.locate(fg.wrap(uv))?;
        self.cell_patch[c]
    }
}

pub fn classify_inside(ctx: &BodyCtx, fg: &FaceGeom, pts: &[DVec2]) -> Vec<bool> {
    match ctx.kernel.face_classify_uv(ctx.kbody, fg.id, pts) {
        Ok(v) => v.into_iter().map(|c| c != UvClass::Out).collect(),
        Err(_) => vec![false; pts.len()],
    }
}

/// Add incidences for events lying on curves they don't reference yet
/// (T-junctions, lines through vertices, lines on seams).
fn snap_events(fg: &FaceGeom, curves: &[ACurve], events: &mut [AEvent]) {
    let tol = fg.eps * 4.0;
    // Segment grid in metric space.
    let cell = (fg.size_uv() * fg.metric.max_element() / 64.0).max(tol * 8.0);
    let mut grid: HashMap<(i64, i64), Vec<(usize, usize)>> = HashMap::new();
    for (ci, c) in curves.iter().enumerate() {
        for j in 0..c.pts.len().saturating_sub(1) {
            let a = fg.m(c.pts[j]);
            let b = fg.m(c.pts[j + 1]);
            let lo = a.min(b) - DVec2::splat(tol);
            let hi = a.max(b) + DVec2::splat(tol);
            let (x0, y0) = ((lo.x / cell).floor() as i64, (lo.y / cell).floor() as i64);
            let (x1, y1) = ((hi.x / cell).floor() as i64, (hi.y / cell).floor() as i64);
            if (x1 - x0 + 1) * (y1 - y0 + 1) > 4096 {
                continue;
            }
            for x in x0..=x1 {
                for y in y0..=y1 {
                    grid.entry((x, y)).or_default().push((ci, j));
                }
            }
        }
    }
    for e in events.iter_mut() {
        let m = fg.m(e.uv);
        let key = ((m.x / cell).floor() as i64, (m.y / cell).floor() as i64);
        let Some(list) = grid.get(&key) else { continue };
        let mut add: Vec<(usize, f64)> = Vec::new();
        for &(ci, j) in list {
            if e.inc.iter().any(|(c, _)| *c == ci) || add.iter().any(|(c, _)| *c == ci) {
                continue;
            }
            let c = &curves[ci];
            let (d, t) = dist_point_seg(m, fg.m(c.pts[j]), fg.m(c.pts[j + 1]));
            if d <= tol {
                add.push((ci, c.par[j] + (c.par[j + 1] - c.par[j]) * t));
            }
        }
        e.inc.extend(add);
    }
}

fn boundary_curves_and_events(fg: &FaceGeom) -> (Vec<ACurve>, Vec<AEvent>) {
    let mut curves = Vec::new();
    let mut events = Vec::new();
    for b in &fg.curves {
        let ci = curves.len();
        curves.push(ACurve { owner: b.owner, seam: b.seam, pts: b.pts.clone(), par: b.par.clone(), closed: false });
        for (k, end) in b.ends.iter().enumerate() {
            let (uv, p) = if k == 0 { (b.pts[0], b.par[0]) } else { (b.pts[b.pts.len() - 1], b.par[b.par.len() - 1]) };
            let key = match end {
                Some(v) => VertexKey::Cad(*v),
                None => VertexKey::Pole(fg.id, k as u8),
            };
            events.push(AEvent { uv, key, inc: vec![(ci, p)], artificial: false, seam_rel: false });
        }
    }
    (curves, events)
}

/// Append clip output: local curves get offset, STOP refs map to `stop_base + i`.
fn absorb(curves: &mut Vec<ACurve>, events: &mut Vec<AEvent>, out: super::clip::ClipOut, stop_base: usize) -> Vec<usize> {
    let base = curves.len();
    let ids: Vec<usize> = (base..base + out.curves.len()).collect();
    curves.extend(out.curves);
    for mut e in out.events {
        for inc in &mut e.inc {
            if inc.0 >= STOP {
                inc.0 = inc.0 - STOP + stop_base;
            } else {
                inc.0 += base;
            }
        }
        events.push(e);
    }
    ids
}

/// Crossings among a set of curves (pairwise), keyed by `key_fn`.
#[allow(clippy::type_complexity)]
fn crossings(fg: &FaceGeom, curves: &[ACurve], set: &[usize], key_fn: &dyn Fn(Owner, Owner, u32) -> Option<VertexKey>, events: &mut Vec<AEvent>) {
    let eps_uv = fg.eps / fg.metric.max_element();
    // (owner a, owner b) -> hits (sa, sb, uv, ca, cb)
    let mut hits: HashMap<(Owner, Owner), Vec<(f64, f64, DVec2, usize, usize)>> = HashMap::new();
    let bbox = |c: &ACurve| {
        let mut lo = DVec2::splat(f64::INFINITY);
        let mut hi = DVec2::splat(f64::NEG_INFINITY);
        for p in &c.pts {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        (lo, hi)
    };
    let boxes: Vec<(DVec2, DVec2)> = set.iter().map(|&i| bbox(&curves[i])).collect();
    for x in 0..set.len() {
        for y in x + 1..set.len() {
            let (ia, ib) = (set[x], set[y]);
            let (la, ha) = boxes[x];
            let (lb, hb) = boxes[y];
            if la.x > hb.x + eps_uv || lb.x > ha.x + eps_uv || la.y > hb.y + eps_uv || lb.y > ha.y + eps_uv {
                continue;
            }
            let (ca, cb) = (&curves[ia], &curves[ib]);
            // Order owners canonically.
            let swap = cb.owner < ca.owner;
            for i in 0..ca.pts.len() - 1 {
                for j in 0..cb.pts.len() - 1 {
                    for (fa, fb) in seg_hits(ca.pts[i], ca.pts[i + 1], cb.pts[j], cb.pts[j + 1], eps_uv) {
                        let sa = ca.par[i] + (ca.par[i + 1] - ca.par[i]) * fa;
                        let sb = cb.par[j] + (cb.par[j + 1] - cb.par[j]) * fb;
                        let uv = ca.pts[i] + (ca.pts[i + 1] - ca.pts[i]) * fa;
                        if swap {
                            hits.entry((cb.owner, ca.owner)).or_default().push((sb, sa, uv, ib, ia));
                        } else {
                            hits.entry((ca.owner, cb.owner)).or_default().push((sa, sb, uv, ia, ib));
                        }
                    }
                }
            }
        }
    }
    let mut keys: Vec<(Owner, Owner)> = hits.keys().copied().collect();
    keys.sort();
    for k in keys {
        let mut list = hits.remove(&k).unwrap();
        list.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
        // Same logical crossing (seam copies) share an occurrence number.
        let mut occ = 0u32;
        let mut last: Option<(f64, f64)> = None;
        for (sa, sb, uv, ca, cb) in list {
            if let Some((la, lb)) = last
                && ((sa - la).abs() > 1e-9 || (sb - lb).abs() > 1e-9)
            {
                occ += 1;
            }
            last = Some((sa, sb));
            if let Some(key) = key_fn(k.0, k.1, occ) {
                events.push(AEvent { uv, key, inc: vec![(ca, sa), (cb, sb)], artificial: false, seam_rel: false });
            }
        }
    }
}

/// Split line → cover polyline on its face.
pub fn split_pts(fg: &FaceGeom, s: &SplitLine) -> (Vec<DVec2>, bool, Option<DVec2>) {
    match s.kind {
        SplitKind::Iso { dir, value, seed } => {
            let d = if dir == IsoDir::U { 0 } else { 1 };
            let v = super::lines::iso_abs(fg, d, value);
            let (pts, closed) = super::lines::iso_line(fg, d, v, None);
            let seed = seed.map(|t| {
                let t = super::lines::iso_abs(fg, 1 - d, t);
                if d == 0 { DVec2::new(v, t) } else { DVec2::new(t, v) }
            });
            (pts, closed, seed)
        }
    }
}

/// Stage 1: patches of a face.
pub fn stage1(ctx: &BodyCtx, body: BodyId, fg: &FaceGeom, splits: &[&SplitLine]) -> Stage1 {
    let (mut curves, mut events) = boundary_curves_and_events(fg);
    let nb = curves.len();
    let boundary: Vec<ACurve> = curves.clone();
    let mut split_curves: Vec<usize> = Vec::new();
    for s in splits {
        let (pts, closed, seed) = split_pts(fg, s);
        let input = ClipInput {
            who: Who::Split(s.id),
            pts: &pts,
            closed,
            allowed: None,
            through: seed.is_none(),
            seed,
            end_keys: [None, None],
        };
        let mut loc = |uv: DVec2| -> Option<usize> { if classify_inside(ctx, fg, &[uv])[0] { Some(0) } else { None } };
        let out = clip(fg, &input, &boundary, &mut loc);
        split_curves.extend(absorb(&mut curves, &mut events, out, 0));
    }
    let key_fn = |a: Owner, b: Owner, occ: u32| -> Option<VertexKey> {
        match (a, b) {
            (Owner::Split(x), Owner::Split(y)) if x != y => Some(VertexKey::SplitCross { a: x.min(y), b: x.max(y), occurrence: occ }),
            _ => None,
        }
    };
    crossings(fg, &curves, &split_curves, &key_fn, &mut events);
    snap_events(fg, &curves, &mut events);
    let _ = nb;
    let arr = Arr::build(curves.clone(), events.clone(), fg, &mut |p| classify_inside(ctx, fg, p));
    let glued = Glued::build(&arr, false);
    // Patches = regions.
    let nreg = glued.regions.len();
    let mut cell_patch = vec![None; arr.cells.len()];
    let mut cells_of: Vec<Vec<usize>> = vec![vec![]; nreg];
    for (ci, &r) in glued.region_of_cell.iter().enumerate() {
        if r != usize::MAX {
            cell_patch[ci] = Some(r);
            cells_of[r].push(ci);
        }
    }
    let per = fg.periods();
    let mut patches: Vec<Patch> = Vec::new();
    for r in 0..nreg {
        // Bounds from split half-edges on the region's walks.
        let mut lo_u: Option<(f64, SplitId)> = None;
        let mut hi_u: Option<(f64, SplitId)> = None;
        let mut lo_v: Option<(f64, SplitId)> = None;
        let mut hi_v: Option<(f64, SplitId)> = None;
        let mut split_bounded = false;
        let mut sample = DVec2::splat(f64::INFINITY);
        for w in &glued.regions[r] {
            for &gh in w {
                let ge = &glued.gedges[gh / 2];
                let pts = glued.g_pts(gh);
                for p in &pts {
                    if p.x < sample.x || (p.x == sample.x && p.y < sample.y) {
                        sample = *p;
                    }
                }
                let Owner::Split(sid) = ge.owner else { continue };
                split_bounded = true;
                let Some(sl) = splits.iter().find(|s| s.id == sid) else { continue };
                let SplitKind::Iso { dir, .. } = sl.kind;
                let d = pts[pts.len() - 1] - pts[0];
                let val = if dir == IsoDir::U { pts[0].x } else { pts[0].y };
                let upd_max = |slot: &mut Option<(f64, SplitId)>| {
                    if slot.is_none_or(|(v, _)| val > v) {
                        *slot = Some((val, sid));
                    }
                };
                let upd_min = |slot: &mut Option<(f64, SplitId)>| {
                    if slot.is_none_or(|(v, _)| val < v) {
                        *slot = Some((val, sid));
                    }
                };
                match dir {
                    IsoDir::U => {
                        if d.y > 0.0 {
                            upd_min(&mut hi_u)
                        } else {
                            upd_max(&mut lo_u)
                        }
                    }
                    IsoDir::V => {
                        if d.x > 0.0 {
                            upd_max(&mut lo_v)
                        } else {
                            upd_min(&mut hi_v)
                        }
                    }
                }
            }
        }
        let b = |x: Option<(f64, SplitId)>| x.map(|(_, s)| PatchBound::Split(s)).unwrap_or(PatchBound::FaceBoundary);
        let id = PatchId { body, face: fg.id, u_lo: b(lo_u), u_hi: b(hi_u), v_lo: b(lo_v), v_hi: b(hi_v), region: 0 };
        // Lift cells across dissolved seams.
        let cells = cells_of[r].clone();
        let mut off: HashMap<usize, DVec2> = HashMap::new();
        let mut wraps = [false, false];
        if let Some(&c0) = cells.first() {
            off.insert(c0, DVec2::ZERO);
            let mut changed = true;
            while changed {
                changed = false;
                for (e, ge) in glued.gedges.iter().enumerate() {
                    if !glued.dissolved[e] || ge.chains.len() < 2 {
                        continue;
                    }
                    let Owner::Seam(dir) = ge.owner else { continue };
                    // Cell on the copy-0 side and on the copy-1 side.
                    let mut c_lo = None;
                    let mut c_hi = None;
                    for &ch in &ge.chains {
                        let copy = arr.chains[ch].seam.map(|s| s.1).unwrap_or(0);
                        let cell = arr.left_cell(ch * 2).or(arr.left_cell(ch * 2 + 1));
                        if copy == 0 { c_lo = cell } else { c_hi = cell }
                    }
                    let (Some(a), Some(bc)) = (c_lo, c_hi) else { continue };
                    let shift = if dir == 0 { DVec2::new(per.x, 0.0) } else { DVec2::new(0.0, per.y) };
                    match (off.get(&a).copied(), off.get(&bc).copied()) {
                        (Some(oa), Some(ob)) => {
                            if (oa - (ob + shift)).length() > 1e-9 {
                                wraps[dir as usize] = true;
                            }
                        }
                        (None, Some(ob)) => {
                            off.insert(a, ob + shift);
                            changed = true;
                        }
                        (Some(oa), None) => {
                            off.insert(bc, oa - shift);
                            changed = true;
                        }
                        _ => {}
                    }
                }
            }
        }
        let mut lo = DVec2::splat(f64::INFINITY);
        let mut hi = DVec2::splat(f64::NEG_INFINITY);
        let mut area = 0.0;
        for &c in &cells {
            let o = off.get(&c).copied().unwrap_or(DVec2::ZERO);
            let poly = arr.orbit_poly(arr.cells[c].outer);
            for p in &poly {
                lo = lo.min(*p + o);
                hi = hi.max(*p + o);
            }
            area += arr.area[arr.cells[c].outer];
            for &h in &arr.cells[c].holes {
                area += arr.area[h];
            }
        }
        if wraps[0] {
            lo.x = fg.lo.x;
            hi.x = fg.lo.x + per.x;
        }
        if wraps[1] {
            lo.y = fg.lo.y;
            hi.y = fg.lo.y + per.y;
        }
        patches.push(Patch {
            id,
            face: fg.id,
            index: r,
            cells,
            lo,
            hi,
            wraps,
            area: area * fg.metric.x * fg.metric.y,
            split_bounded,
            sample,
        });
    }
    // Region numbers among equal bounds, by sample point.
    let mut order: Vec<usize> = (0..patches.len()).collect();
    order.sort_by(|&a, &b| {
        let (pa, pb) = (&patches[a], &patches[b]);
        (pa.id.u_lo, pa.id.u_hi, pa.id.v_lo, pa.id.v_hi)
            .cmp(&(pb.id.u_lo, pb.id.u_hi, pb.id.v_lo, pb.id.v_hi))
            .then(pa.sample.x.total_cmp(&pb.sample.x))
            .then(pa.sample.y.total_cmp(&pb.sample.y))
    });
    let mut prev: Option<PatchId> = None;
    let mut cnt = 0u32;
    for &i in &order {
        let mut id = patches[i].id;
        id.region = 0;
        if prev == Some(id) {
            cnt += 1;
        } else {
            cnt = 0;
        }
        prev = Some(id);
        patches[i].id.region = cnt;
    }
    Stage1 { curves, events, arr, glued, patches, cell_patch }
}

/// Stage 2+3: clip lines, compute crossings, build and glue the final
/// arrangement.
pub fn stage3(ctx: &BodyCtx, fg: &FaceGeom, s1: &Stage1, lines: &[GLine]) -> (Arr, Glued) {
    let mut curves = s1.curves.clone();
    let mut events = s1.events.clone();
    let stoppers = s1.curves.clone();
    let mut line_curves: Vec<usize> = Vec::new();
    for gl in lines {
        let input = ClipInput {
            who: gl.who,
            pts: &gl.pts,
            closed: gl.closed,
            allowed: gl.allowed.as_deref(),
            through: gl.through,
            seed: gl.seed,
            end_keys: gl.end_keys.clone(),
        };
        let mut loc = |uv: DVec2| s1.locate(fg, uv);
        let mut out = clip(fg, &input, &stoppers, &mut loc);
        refine_boundary_hits(ctx, fg, gl, &stoppers, &mut out);
        line_curves.extend(absorb(&mut curves, &mut events, out, 0));
    }
    let key_fn = |a: Owner, b: Owner, occ: u32| -> Option<VertexKey> {
        match (a, b) {
            (Owner::Line(x), Owner::Line(y)) => Some(VertexKey::Crossing { a: x.min(y), b: x.max(y), occurrence: occ }),
            _ => None,
        }
    };
    crossings(fg, &curves, &line_curves, &key_fn, &mut events);
    snap_events(fg, &curves, &mut events);
    let arr = Arr::build(curves, events, fg, &mut |p| classify_inside(ctx, fg, p));
    let glued = Glued::build(&arr, true);
    (arr, glued)
}

/// Refine where a line meets a CAD edge so the event sits exactly on the
/// edge (and on the line: iso value, straight segment, or slice plane).
fn refine_boundary_hits(ctx: &BodyCtx, fg: &FaceGeom, gl: &GLine, stoppers: &[ACurve], out: &mut super::clip::ClipOut) {
    for e in &mut out.events {
        let Some(&(si, t0)) = e.inc.iter().find(|(c, _)| *c >= STOP) else { continue };
        let st = &stoppers[si - STOP];
        let Owner::Cad(edge) = st.owner else { continue };
        let (a, b) = (gl.pts[0], gl.pts[gl.pts.len() - 1]);
        let residual = |t: f64| -> Option<(f64, DVec2)> {
            let uv = ctx.kernel.edge_pcurve_eval(ctx.kbody, edge, fg.id, false, &[t]).ok()?[0];
            if let Some((o, n)) = gl.plane {
                let p = ctx.edge_point(edge, t);
                return Some(((p - o).dot(n), uv));
            }
            if gl.pts.len() == 2 {
                let uvl = fg.lift_near(uv, a);
                let d = (b - a).normalize_or_zero();
                return Some((d.perp_dot(uvl - a), uv));
            }
            None
        };
        let span = (st.par[st.par.len() - 1] - st.par[0]).abs().max(1e-12);
        let Some((mut f0, _)) = residual(t0) else { continue };
        let mut x0 = t0;
        let mut x1 = t0 + span * 1e-4;
        let Some((mut f1, _)) = residual(x1) else { continue };
        let (lo, hi) = (st.par[0].min(st.par[st.par.len() - 1]), st.par[0].max(st.par[st.par.len() - 1]));
        for _ in 0..30 {
            if f1.abs() < 1e-15 || (f1 - f0).abs() < 1e-300 {
                break;
            }
            let x2 = (x1 - f1 * (x1 - x0) / (f1 - f0)).clamp(lo, hi);
            x0 = x1;
            f0 = f1;
            x1 = x2;
            match residual(x1) {
                Some((f, _)) => f1 = f,
                None => break,
            }
            if (x1 - x0).abs() < 1e-14 * span {
                break;
            }
        }
        if (x1 - t0).abs() > span * 0.05 {
            continue;
        }
        let Some((fr, uv)) = residual(x1) else { continue };
        let Some((f_orig, _)) = residual(t0) else { continue };
        if fr.abs() > f_orig.abs() {
            continue;
        }
        let uvw = if (uv - e.uv).length() > (fg.wrap(uv) - e.uv).length() { fg.wrap(uv) } else { uv };
        if (fg.m(uvw) - fg.m(e.uv)).length() > fg.eps * 1e4 {
            continue;
        }
        e.uv = uvw;
        for inc in &mut e.inc {
            if inc.0 >= STOP && inc.0 == si {
                inc.1 = x1;
            }
        }
        let _ = project_par;
    }
}
