//! Rules → generated lines per face (SPEC §4.3, §4.6 step 2, §6.2–§6.6).
//! Lines are UV polylines in the periodic cover of the face's domain; the
//! clipping stage cuts them to patches.

use super::clip::Who;
use super::ctx::{BodyCtx, FaceGeom};
use crate::doc::{CutPoint, Document, IsoDir, PatchBound, PatchId, Plane, Rule, SliceOrient, SliceScope, Spacing};
use crate::keys::{LineKey, RuleId, VertexKey};
use glam::{DVec2, DVec3};
use newtopo_kernel::{BodyId, CurveKind, EdgeId, FaceId, SurfaceKind};
use std::collections::HashMap;
use std::f64::consts::PI;

/// Rule id used for a face's implicit grid (SPEC §4.1).
pub fn implicit_rule(face: FaceId) -> RuleId {
    RuleId(0xFF00_0000 | face.0)
}
pub fn is_implicit(r: RuleId) -> bool {
    r.0 & 0xFF00_0000 == 0xFF00_0000
}

/// Line index offset of the second (constant-v / B) family.
pub const B_FAMILY: u32 = 1 << 16;

#[derive(Clone, Debug)]
pub struct GLine {
    pub who: Who,
    pub face: FaceId,
    pub pts: Vec<DVec2>,
    pub closed: bool,
    pub allowed: Option<Vec<usize>>,
    pub through: bool,
    pub seed: Option<DVec2>,
    pub end_keys: [Option<VertexKey>; 2],
    /// Plane to refine crossings onto (slice lines).
    pub plane: Option<(DVec3, DVec3)>,
}

/// One patch of a face.
#[derive(Clone, Debug)]
pub struct Patch {
    pub id: PatchId,
    pub face: FaceId,
    pub index: usize,
    pub cells: Vec<usize>,
    /// Lifted bounding box in the cover.
    pub lo: DVec2,
    pub hi: DVec2,
    /// Wraps fully around the period in u / v.
    pub wraps: [bool; 2],
    /// Approximate area (metric-scaled UV).
    pub area: f64,
    /// Bounded by a split line.
    pub split_bounded: bool,
    pub sample: DVec2,
}

impl Patch {
    /// No outer boundary: implicit grid and count clamps apply (SPEC §4.1, §4.2).
    pub fn closed_patch(&self, fg: &FaceGeom) -> bool {
        !fg.has_outer && !self.split_bounded
    }
}

/// Bend class directions of a patch (SPEC §6.2): which iso families carry
/// the A and B counts. `Radial` = planar ring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bend {
    Flat,
    Radial,
    /// A = constant-u lines (spread around u), no B.
    OneU,
    /// A = constant-v lines.
    OneV,
    /// A = constant-u, B = constant-v.
    Two,
}

pub fn bend_class(ctx: &BodyCtx, fg: &FaceGeom) -> Bend {
    match fg.info.kind {
        SurfaceKind::Plane => {
            if fg.ring.is_some() {
                Bend::Radial
            } else {
                Bend::Flat
            }
        }
        SurfaceKind::Cylinder | SurfaceKind::Cone => Bend::OneU,
        SurfaceKind::Sphere | SurfaceKind::Torus => Bend::Two,
        _ => {
            // Sample normal variation along u and v.
            let n = 6;
            let mut pts = Vec::new();
            for i in 0..=n {
                for j in 0..=n {
                    pts.push(DVec2::new(
                        fg.lo.x + (fg.hi.x - fg.lo.x) * i as f64 / n as f64,
                        fg.lo.y + (fg.hi.y - fg.lo.y) * j as f64 / n as f64,
                    ));
                }
            }
            let ev = ctx.face_eval(fg.id, &pts);
            if ev.len() != pts.len() {
                return Bend::Flat;
            }
            let (mut bu, mut bv) = (0.0f64, 0.0f64);
            for i in 0..n {
                for j in 0..=n {
                    let a = ev[i * (n + 1) + j].n;
                    let b = ev[(i + 1) * (n + 1) + j].n;
                    bu = bu.max(a.dot(b).clamp(-1.0, 1.0).acos() * n as f64);
                }
            }
            for i in 0..=n {
                for j in 0..n {
                    let a = ev[i * (n + 1) + j].n;
                    let b = ev[i * (n + 1) + j + 1].n;
                    bv = bv.max(a.dot(b).clamp(-1.0, 1.0).acos() * n as f64);
                }
            }
            let th = 0.2;
            match (bu > th, bv > th) {
                (true, true) => Bend::Two,
                (true, false) => Bend::OneU,
                (false, true) => Bend::OneV,
                (false, false) => Bend::Flat,
            }
        }
    }
}

/// Default phase reference: world X projected off the axis (Y, then Z if
/// X is within 10° of the axis) (SPEC §4.1).
pub fn default_phase_dir(axis: DVec3) -> DVec3 {
    let a = axis.normalize_or_zero();
    for cand in [DVec3::X, DVec3::Y, DVec3::Z] {
        if a.dot(cand).abs() < (80f64).to_radians().cos() {
            return (cand - a * a.dot(cand)).normalize();
        }
    }
    DVec3::X
}

/// Canonical start of the periodic direction `dir` on a face: where the
/// face's phase reference direction meets the surface.
pub fn canonical(ctx: &BodyCtx, doc: &Document, body: BodyId, fg: &FaceGeom, dir: usize, other: f64) -> f64 {
    let axis = fg.info.axis_dir.unwrap_or(DVec3::Z).normalize_or_zero();
    let origin = fg.info.axis_origin.unwrap_or(DVec3::ZERO);
    let rf = doc
        .phase_overrides
        .get(&crate::doc::PhaseTarget::Face(body, fg.id))
        .copied()
        .map(|d| (d - axis * axis.dot(d)).normalize_or_zero())
        .filter(|d| d.length() > 0.5)
        .unwrap_or_else(|| default_phase_dir(axis));
    let per = if dir == 0 { fg.uper.or(fg.info.u_period) } else { fg.vper.or(fg.info.v_period) }.unwrap_or(2.0 * PI);
    let base = if dir == 0 { fg.lo.x } else { fg.lo.y };
    let side = axis.cross(rf);
    // Signed angle of the surface's radial (u) or normal (v) direction from
    // the reference, around the axis; its zero is the canonical angle.
    let ang = |ts: &[f64]| -> Vec<f64> {
        let uvs: Vec<DVec2> = ts.iter().map(|&t| if dir == 0 { DVec2::new(t, other) } else { DVec2::new(other, t) }).collect();
        let ev = ctx.face_eval(fg.id, &uvs);
        ev.iter()
            .map(|e| {
                if dir == 0 {
                    let r = e.p - origin;
                    let r = r - axis * axis.dot(r);
                    r.dot(side).atan2(r.dot(rf))
                } else {
                    e.n.dot(axis).atan2(e.n.dot(rf))
                }
            })
            .collect()
    };
    let n = 72;
    let ts: Vec<f64> = (0..n).map(|i| base + per * i as f64 / n as f64).collect();
    let a = ang(&ts);
    if a.len() != n {
        return base;
    }
    let i = (0..n).min_by(|&x, &y| a[x].abs().total_cmp(&a[y].abs())).unwrap();
    let step = per / n as f64;
    // Secant / bisection on the angle near the best sample.
    let (mut lo, mut hi) = (ts[i] - step, ts[i] + step);
    let f = |t: f64| ang(&[t]).first().copied().unwrap_or(0.0);
    let (mut flo, fhi) = (f(lo), f(hi));
    let mut t = ts[i];
    if flo.signum() != fhi.signum() && (flo - fhi).abs() < PI {
        for _ in 0..100 {
            let m = 0.5 * (lo + hi);
            let fm = f(m);
            if fm == 0.0 || (hi - lo) < 1e-15 * per {
                t = m;
                break;
            }
            if fm.signum() == flo.signum() {
                lo = m;
                flo = fm;
            } else {
                hi = m;
            }
            t = 0.5 * (lo + hi);
        }
    }
    // Snap onto the seam when within a hair of it.
    let rel = ((t - base) / per).rem_euclid(1.0);
    if rel < 1e-7 || rel > 1.0 - 1e-7 { base } else { base + rel * per }
}

/// Positions of `count` iso lines along `dir` across a patch (SPEC §6.2,
/// §6.3). Periodic (patch wraps): N lines → N segments from `phase`.
/// Open: N lines → N + 1 segments.
#[allow(clippy::too_many_arguments)]
pub fn spread(ctx: &BodyCtx, fg: &FaceGeom, p: &Patch, dir: usize, count: u32, phase: Option<f64>, spacing: Spacing) -> Vec<f64> {
    if count == 0 {
        return vec![];
    }
    let n = count as usize;
    let (a, b) = if dir == 0 { (p.lo.x, p.hi.x) } else { (p.lo.y, p.hi.y) };
    let (oa, ob) = if dir == 0 { (p.lo.y, p.hi.y) } else { (p.lo.x, p.hi.x) };
    let wraps = p.wraps[dir];
    let per = if dir == 0 { fg.uper } else { fg.vper };
    let (start, len, fracs): (f64, f64, Vec<f64>) = if wraps {
        let per = per.unwrap_or(b - a);
        (phase.unwrap_or(a), per, (0..n).map(|i| i as f64 / n as f64).collect())
    } else {
        (a, b - a, (0..n).map(|i| (i + 1) as f64 / (n + 1) as f64).collect())
    };
    if spacing == Spacing::Parametric || len <= 0.0 {
        return fracs.iter().map(|f| start + len * f).collect();
    }
    // Arc length along the direction, averaged over three positions.
    let m = 48;
    let mut cum = vec![0.0; m + 1];
    for q in [0.25, 0.5, 0.75] {
        let other = oa + (ob - oa) * q;
        let pts: Vec<DVec2> = (0..=m)
            .map(|i| {
                let t = start + len * i as f64 / m as f64;
                if dir == 0 { DVec2::new(t, other) } else { DVec2::new(other, t) }
            })
            .collect();
        let ev = ctx.face_eval(fg.id, &pts);
        if ev.len() != pts.len() {
            return fracs.iter().map(|f| start + len * f).collect();
        }
        let mut acc = 0.0;
        for i in 1..=m {
            acc += (ev[i].p - ev[i - 1].p).length();
            cum[i] += acc;
        }
    }
    let total = cum[m];
    if total <= 0.0 {
        return fracs.iter().map(|f| start + len * f).collect();
    }
    fracs
        .iter()
        .map(|f| {
            let target = f * total;
            let mut i = 0;
            while i < m && cum[i + 1] < target {
                i += 1;
            }
            let seg = (cum[(i + 1).min(m)] - cum[i]).max(1e-300);
            let fr = ((target - cum[i]) / seg).clamp(0.0, 1.0);
            start + len * (i as f64 + fr) / m as f64
        })
        .collect()
}

/// An iso line across the face at `value` (cover coordinates).
pub fn iso_line(fg: &FaceGeom, dir: usize, value: f64, along: Option<(f64, f64)>) -> (Vec<DVec2>, bool) {
    // dir 0: constant u (runs along v).
    let (closed_along, lo, hi) = if dir == 0 { (fg.vper, fg.lo.y, fg.hi.y) } else { (fg.uper, fg.lo.x, fg.hi.x) };
    if let Some(p) = closed_along {
        let a = lo;
        let pts = if dir == 0 { vec![DVec2::new(value, a), DVec2::new(value, a + p)] } else { vec![DVec2::new(a, value), DVec2::new(a + p, value)] };
        return (pts, true);
    }
    let (a, b) = along.unwrap_or((lo, hi));
    let m = 0.05 * (b - a).abs().max(fg.size_uv() * 0.01);
    let (a, b) = (a.min(lo) - m, b.max(hi) + m);
    let pts = if dir == 0 { vec![DVec2::new(value, a), DVec2::new(value, b)] } else { vec![DVec2::new(a, value), DVec2::new(b, value)] };
    (pts, false)
}

fn phase_for(ctx: &BodyCtx, doc: &Document, body: BodyId, fg: &FaceGeom, p: &Patch, dir: usize, start_on_seam: bool) -> Option<f64> {
    if !p.wraps[dir] {
        return None;
    }
    let base = if dir == 0 { fg.lo.x } else { fg.lo.y };
    if start_on_seam {
        return Some(base);
    }
    let other = if dir == 0 { 0.5 * (p.lo.y + p.hi.y) } else { 0.5 * (p.lo.x + p.hi.x) };
    Some(canonical(ctx, doc, body, fg, dir, other))
}

/// Iso-family lines for a patch.
#[allow(clippy::too_many_arguments)]
fn family(
    ctx: &BodyCtx,
    doc: &Document,
    body: BodyId,
    fg: &FaceGeom,
    p: &Patch,
    dir: usize,
    count: u32,
    start_on_seam: bool,
    spacing: Spacing,
    rule: RuleId,
    index_base: u32,
    sub: u32,
    out: &mut Vec<GLine>,
) {
    let mut count = count;
    if p.closed_patch(fg) && (1..3).contains(&count) && p.wraps[dir] {
        count = 3;
    }
    let phase = phase_for(ctx, doc, body, fg, p, dir, start_on_seam);
    let vals = spread(ctx, fg, p, dir, count, phase, spacing);
    let along = if dir == 0 { (p.lo.y, p.hi.y) } else { (p.lo.x, p.hi.x) };
    for (i, v) in vals.into_iter().enumerate() {
        let (pts, closed) = iso_line(fg, dir, v, Some(along));
        out.push(GLine {
            who: Who::Line(LineKey { rule, index: index_base + i as u32, sub }),
            face: fg.id,
            pts,
            closed,
            allowed: Some(vec![p.index]),
            through: false,
            seed: None,
            end_keys: [None, None],
            plane: None,
        });
    }
}

/// Radial lines on a planar ring patch.
#[allow(clippy::too_many_arguments)]
fn radial(ctx: &BodyCtx, doc: &Document, body: BodyId, fg: &FaceGeom, p: &Patch, count: u32, rule: RuleId, sub: u32, out: &mut Vec<GLine>) {
    let Some(c) = fg.ring else { return };
    let ev = ctx.face_eval(fg.id, &[c, c + DVec2::X, c + DVec2::Y]);
    if ev.len() != 3 {
        return;
    }
    let (o, eu, evv) = (ev[0].p, ev[1].p - ev[0].p, ev[2].p - ev[0].p);
    let axis = eu.cross(evv).normalize_or_zero();
    let rf = doc
        .phase_overrides
        .get(&crate::doc::PhaseTarget::Face(body, fg.id))
        .copied()
        .unwrap_or_else(|| default_phase_dir(axis));
    let rf = (rf - axis * axis.dot(rf)).normalize_or_zero();
    let side = axis.cross(rf);
    // 3D direction → UV direction (plane param is affine).
    let m = glam::DMat2::from_cols(DVec2::new(eu.dot(eu), eu.dot(evv)), DVec2::new(evv.dot(eu), evv.dot(evv)));
    let big = (fg.hi - fg.lo).length() * 2.0 + 1.0;
    let _ = o;
    for i in 0..count {
        let a = 2.0 * PI * i as f64 / count as f64;
        let d3 = rf * a.cos() + side * a.sin();
        let rhs = DVec2::new(d3.dot(eu), d3.dot(evv));
        let duv = m.inverse() * rhs;
        let duv = duv.normalize_or_zero();
        out.push(GLine {
            who: Who::Line(LineKey { rule, index: i, sub }),
            face: fg.id,
            pts: vec![c, c + duv * big],
            closed: false,
            allowed: Some(vec![p.index]),
            through: false,
            seed: None,
            end_keys: [None, None],
            plane: None,
        });
    }
}

/// Lines a rule generates on one face.
#[allow(clippy::too_many_arguments)]
pub fn rule_lines(
    ctx: &BodyCtx,
    doc: &Document,
    body: BodyId,
    fg: &FaceGeom,
    patches: &[Patch],
    rule: RuleId,
    r: &Rule,
    plans: &BodyPlans,
    out: &mut Vec<GLine>,
) {
    let find = |pid: &PatchId| patches.iter().find(|p| p.id == *pid);
    match r {
        Rule::Grid { patch, count_u, count_v, start_on_seam, spacing } => {
            if patch.body != body || patch.face != fg.id {
                return;
            }
            let Some(p) = find(patch) else { return };
            family(ctx, doc, body, fg, p, 0, *count_u, *start_on_seam, *spacing, rule, 0, 0, out);
            family(ctx, doc, body, fg, p, 1, *count_v, *start_on_seam, *spacing, rule, B_FAMILY, 0, out);
        }
        Rule::Connect { patches: list, count_a, count_b, start_on_seam, flipped } => {
            let bend = bend_class(ctx, fg);
            for (sub, pid) in list.iter().enumerate() {
                if pid.body != body || pid.face != fg.id {
                    continue;
                }
                let Some(p) = find(pid) else { continue };
                let flip = flipped.contains(pid);
                let sub = sub as u32;
                match bend {
                    Bend::Flat => {}
                    Bend::Radial => radial(ctx, doc, body, fg, p, *count_a, rule, sub, out),
                    Bend::OneU | Bend::OneV => {
                        let d = if (bend == Bend::OneU) != flip { 0 } else { 1 };
                        family(ctx, doc, body, fg, p, d, *count_a, *start_on_seam, Spacing::ArcLength, rule, 0, sub, out);
                    }
                    Bend::Two => {
                        let (da, db) = if flip { (1, 0) } else { (0, 1) };
                        family(ctx, doc, body, fg, p, da, *count_a, *start_on_seam, Spacing::ArcLength, rule, 0, sub, out);
                        family(ctx, doc, body, fg, p, db, *count_b, *start_on_seam, Spacing::ArcLength, rule, B_FAMILY, sub, out);
                    }
                }
            }
        }
        Rule::Slice { scope, through, .. } => {
            let allowed: Option<Vec<usize>> = match scope {
                SliceScope::Body(b) => {
                    if *b != body {
                        return;
                    }
                    None
                }
                SliceScope::Patches(list) => {
                    let v: Vec<usize> = list.iter().filter(|p| p.body == body && p.face == fg.id).filter_map(|pid| find(pid).map(|p| p.index)).collect();
                    if v.is_empty() {
                        return;
                    }
                    Some(v)
                }
            };
            let Some(planes) = plans.slice_planes.get(&rule) else { return };
            for (k, pl) in planes.iter().enumerate() {
                let polys = section(ctx, fg, pl.origin, pl.normal);
                for (ci, (pts, closed)) in polys.into_iter().enumerate() {
                    out.push(GLine {
                        who: Who::Line(LineKey { rule, index: k as u32, sub: ci as u32 }),
                        face: fg.id,
                        pts,
                        closed,
                        allowed: allowed.clone(),
                        through: *through,
                        seed: None,
                        end_keys: [None, None],
                        plane: Some((pl.origin, pl.normal)),
                    });
                }
            }
        }
        Rule::Iso { pieces, through } => {
            for (i, pc) in pieces.iter().enumerate() {
                if pc.body != body || pc.face != fg.id {
                    continue;
                }
                let dir = if pc.dir == IsoDir::U { 0 } else { 1 };
                let value = iso_abs(fg, dir, pc.value);
                let (pts, closed) = iso_line(fg, dir, value, None);
                let seed = if *through {
                    None
                } else {
                    pc.seed.map(|s| {
                        let t = iso_abs(fg, 1 - dir, s);
                        if dir == 0 { DVec2::new(value, t) } else { DVec2::new(t, value) }
                    })
                };
                out.push(GLine {
                    who: Who::Line(LineKey { rule, index: i as u32, sub: 0 }),
                    face: fg.id,
                    pts,
                    closed,
                    allowed: None,
                    through: *through,
                    seed,
                    end_keys: [None, None],
                    plane: None,
                });
            }
        }
        Rule::Cut { .. } | Rule::Join { .. } => {
            if let Some(segs) = plans.cut_segments.get(&rule) {
                for s in segs.iter().filter(|s| s.face == fg.id) {
                    out.push(GLine {
                        who: Who::Line(LineKey { rule, index: s.index, sub: s.sub }),
                        face: fg.id,
                        pts: vec![s.a, s.b],
                        closed: false,
                        allowed: None,
                        through: true,
                        seed: None,
                        end_keys: s.keys.clone(),
                        plane: None,
                    });
                }
            }
        }
    }
}

/// Normalised iso value (0..1 of the face's UV range) → absolute.
pub fn iso_abs(fg: &FaceGeom, dir: usize, value: f64) -> f64 {
    let (a, b) = if dir == 0 { (fg.info.u_range.0, fg.info.u_range.1) } else { (fg.info.v_range.0, fg.info.v_range.1) };
    a + (b - a) * value
}

/// Inverse of [`iso_abs`].
pub fn iso_norm(fg: &FaceGeom, dir: usize, abs: f64) -> f64 {
    let (a, b) = if dir == 0 { (fg.info.u_range.0, fg.info.u_range.1) } else { (fg.info.v_range.0, fg.info.v_range.1) };
    if (b - a).abs() < 1e-300 { 0.0 } else { (abs - a) / (b - a) }
}

/// Implicit grid lines for patches with no spread rule and no outer
/// boundary (SPEC §4.1).
pub fn implicit_lines(ctx: &BodyCtx, doc: &Document, body: BodyId, fg: &FaceGeom, p: &Patch, out: &mut Vec<GLine>) {
    let n = doc.settings.min_closed_segments.max(3);
    let rule = implicit_rule(fg.id);
    let sub = p.index as u32;
    let (nu, nv) = if fg.info.kind == SurfaceKind::Sphere { (n, (n / 2).max(1)) } else { (n, n) };
    family(ctx, doc, body, fg, p, 0, nu, false, Spacing::ArcLength, rule, 0, sub, out);
    family(ctx, doc, body, fg, p, 1, nv, false, Spacing::ArcLength, rule, B_FAMILY, sub, out);
}

// ---------------------------------------------------------------------------
// Plane sections (marching squares on the signed distance, refined).

pub fn section(ctx: &BodyCtx, fg: &FaceGeom, o: DVec3, n: DVec3) -> Vec<(Vec<DVec2>, bool)> {
    let n = n.normalize_or_zero();
    let ext = fg.hi - fg.lo;
    let h = ctx.size / 96.0;
    let nu = (((ext.x * fg.metric.x) / h).ceil() as usize).clamp(8, 160);
    let nv = (((ext.y * fg.metric.y) / h).ceil() as usize).clamp(8, 160);
    let pu = fg.uper.is_some();
    let pv = fg.vper.is_some();
    let mut uvs = Vec::with_capacity((nu + 1) * (nv + 1));
    for i in 0..=nu {
        for j in 0..=nv {
            uvs.push(DVec2::new(fg.lo.x + ext.x * i as f64 / nu as f64, fg.lo.y + ext.y * j as f64 / nv as f64));
        }
    }
    let ev = ctx.face_eval(fg.id, &uvs);
    if ev.len() != uvs.len() {
        return vec![];
    }
    let tiny = 1e-10 * ctx.size;
    let mut f: Vec<f64> = ev.iter().map(|e| (e.p - o).dot(n)).collect();
    if f.iter().all(|x| x.abs() <= tiny * 10.0) {
        return vec![]; // face lies in the plane
    }
    for x in &mut f {
        if x.abs() <= tiny {
            *x = tiny;
        }
    }
    let at = |i: usize, j: usize| f[i * (nv + 1) + j];
    let uv = |i: usize, j: usize| uvs[i * (nv + 1) + j];
    // Edge ids: (kind, i, j) with periodic identification.
    let norm_i = |i: usize| if pu && i == nu { 0 } else { i };
    let norm_j = |j: usize| if pv && j == nv { 0 } else { j };
    type EKey = (u8, usize, usize);
    let mut point: HashMap<EKey, DVec2> = HashMap::new();
    let mut adj: HashMap<EKey, Vec<EKey>> = HashMap::new();
    let mut cross = |k: EKey, a: DVec2, b: DVec2, fa: f64, fb: f64, point: &mut HashMap<EKey, DVec2>| {
        let t = fa / (fa - fb);
        point.entry(k).or_insert(a + (b - a) * t);
    };
    for i in 0..nu {
        for j in 0..nv {
            let c = [at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)];
            let p = [uv(i, j), uv(i + 1, j), uv(i + 1, j + 1), uv(i, j + 1)];
            // Edges: 0 bottom (i,j)-(i+1,j), 1 right, 2 top, 3 left.
            let keys: [EKey; 4] = [(0, norm_i(i), norm_j(j)), (1, norm_i(i + 1), norm_j(j)), (0, norm_i(i), norm_j(j + 1)), (1, norm_i(i), norm_j(j))];
            let mut hits = Vec::new();
            for e in 0..4 {
                let (a, b) = (e, (e + 1) % 4);
                if (c[a] > 0.0) != (c[b] > 0.0) {
                    cross(keys[e], p[a], p[b], c[a], c[b], &mut point);
                    hits.push(keys[e]);
                }
            }
            let pairs: Vec<(EKey, EKey)> = match hits.len() {
                2 => vec![(hits[0], hits[1])],
                4 => {
                    let center = (c[0] + c[1] + c[2] + c[3]) * 0.25;
                    if (center > 0.0) == (c[0] > 0.0) {
                        vec![(hits[0], hits[3]), (hits[1], hits[2])]
                    } else {
                        vec![(hits[0], hits[1]), (hits[2], hits[3])]
                    }
                }
                _ => vec![],
            };
            for (a, b) in pairs {
                adj.entry(a).or_default().push(b);
                adj.entry(b).or_default().push(a);
            }
        }
    }
    // Trace polylines.
    let mut keys: Vec<EKey> = adj.keys().copied().collect();
    keys.sort();
    let mut used: std::collections::HashSet<(EKey, EKey)> = Default::default();
    let mut polys: Vec<(Vec<EKey>, bool)> = Vec::new();
    let mut visited: std::collections::HashSet<EKey> = Default::default();
    // Open chains first (start at degree-1 nodes), then loops.
    let mut starts: Vec<EKey> = keys.iter().copied().filter(|k| adj[k].len() == 1).collect();
    starts.extend(keys.iter().copied().filter(|k| adj[k].len() != 1));
    for s in starts {
        if visited.contains(&s) {
            continue;
        }
        let mut chain = vec![s];
        visited.insert(s);
        let mut cur = s;
        let mut closed = false;
        loop {
            let nxt = adj[&cur].iter().copied().find(|&x| !used.contains(&(cur, x)) && !used.contains(&(x, cur)));
            let Some(x) = nxt else { break };
            used.insert((cur, x));
            if x == s {
                closed = true;
                break;
            }
            if visited.contains(&x) {
                break;
            }
            visited.insert(x);
            chain.push(x);
            cur = x;
        }
        if chain.len() >= 2 {
            polys.push((chain, closed));
        }
    }
    let per = fg.periods();
    let mut out = Vec::new();
    for (chain, closed) in polys {
        let mut pts: Vec<DVec2> = Vec::with_capacity(chain.len() + 1);
        for k in &chain {
            let mut p = point[k];
            if let Some(&prev) = pts.last() {
                p = fg.lift_near(p, prev);
            }
            pts.push(p);
        }
        if closed {
            let first = pts[0];
            let last = *pts.last().unwrap();
            let back = fg.lift_near(first, last);
            pts.push(back);
        }
        let _ = per;
        // Newton refinement onto the plane.
        for _ in 0..4 {
            let ev = ctx.face_eval(fg.id, &pts);
            if ev.len() != pts.len() {
                break;
            }
            for (p, e) in pts.iter_mut().zip(&ev) {
                let fv = (e.p - o).dot(n);
                let g = DVec2::new(e.du.dot(n), e.dv.dot(n));
                let g2 = g.length_squared();
                if g2 > 1e-300 {
                    *p -= g * (fv / g2);
                }
            }
        }
        if closed {
            // Keep the closing point consistent with the start.
            let k = pts.len() - 1;
            let shift = pts[k] - pts[0];
            let snapped = DVec2::new(
                if fg.uper.is_some() { (shift.x / fg.uper.unwrap()).round() * fg.uper.unwrap() } else { 0.0 },
                if fg.vper.is_some() { (shift.y / fg.vper.unwrap()).round() * fg.vper.unwrap() } else { 0.0 },
            );
            pts[k] = pts[0] + snapped;
        }
        out.push((pts, closed));
    }
    // Deterministic order.
    out.sort_by(|a, b| {
        let ka = a.0.iter().fold(DVec2::splat(f64::INFINITY), |m, p| m.min(*p));
        let kb = b.0.iter().fold(DVec2::splat(f64::INFINITY), |m, p| m.min(*p));
        ka.x.total_cmp(&kb.x).then(ka.y.total_cmp(&kb.y))
    });
    out
}

// ---------------------------------------------------------------------------
// Per-body plans (computed before faces): slice planes, cut segments.

#[derive(Clone, Debug)]
pub struct CutSeg {
    pub face: FaceId,
    pub index: u32,
    pub sub: u32,
    pub a: DVec2,
    pub b: DVec2,
    pub keys: [Option<VertexKey>; 2],
}

#[derive(Default)]
pub struct BodyPlans {
    pub slice_planes: HashMap<RuleId, Vec<Plane>>,
    pub cut_segments: HashMap<RuleId, Vec<CutSeg>>,
    /// Rules that could not be fully resolved (dormant parts).
    pub unresolved: Vec<RuleId>,
}

/// Scope faces of a slice rule.
pub fn slice_faces(ctx: &BodyCtx, body: BodyId, scope: &SliceScope) -> Vec<FaceId> {
    match scope {
        SliceScope::Body(b) => {
            if *b == body {
                ctx.topo.face_ids().collect()
            } else {
                vec![]
            }
        }
        SliceScope::Patches(list) => {
            let mut v: Vec<FaceId> = list.iter().filter(|p| p.body == body).map(|p| p.face).collect();
            v.sort();
            v.dedup();
            v
        }
    }
}

/// Planes for a slice rule (SPEC §6.4).
pub fn compute_slice_planes(ctx: &BodyCtx, faces: &[FaceId], count: u32, orient: &SliceOrient) -> Vec<Plane> {
    if count == 0 || faces.is_empty() {
        return vec![];
    }
    // Scope points (tessellation vertices).
    let mut pts: Vec<DVec3> = Vec::new();
    for f in faces {
        pts.extend(ctx.face_meshes[f.idx()].positions.iter().copied());
    }
    let radial = |origin: DVec3, axis: DVec3| -> Vec<Plane> {
        let axis = axis.normalize_or_zero();
        let rf = default_phase_dir(axis);
        let side = axis.cross(rf);
        (0..count)
            .map(|k| {
                let a = (k as f64 + 0.5) * PI / count as f64;
                let d = rf * a.cos() + side * a.sin();
                Plane { origin, normal: axis.cross(d).normalize() }
            })
            .collect()
    };
    let stack = |n: DVec3| -> Vec<Plane> {
        let n = n.normalize_or_zero();
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for p in &pts {
            let d = p.dot(n);
            lo = lo.min(d);
            hi = hi.max(d);
        }
        (0..count)
            .map(|k| {
                let d = lo + (hi - lo) * (k + 1) as f64 / (count + 1) as f64;
                Plane { origin: n * d, normal: n }
            })
            .collect()
    };
    match orient {
        SliceOrient::GuideEdge(_, e) => {
            let et = ctx.topo.edge(*e);
            let poly = ctx.edge_polyline(*e);
            if et.curve == CurveKind::Line || poly.len() < 3 {
                let d = poly[poly.len() - 1] - poly[0];
                return stack(d);
            }
            // Circle-ish: axis from three points.
            let (a, b, c) = (poly[0], poly[poly.len() / 3], poly[2 * poly.len() / 3]);
            let axis = (b - a).cross(c - a);
            let centre = circumcentre(a, b, c);
            radial(centre, axis)
        }
        SliceOrient::Auto => {
            // Shared axis of revolution among curved faces?
            let mut axes: Vec<(DVec3, DVec3)> = Vec::new();
            let mut ok = true;
            for f in faces {
                let fg = ctx.face(*f);
                match fg.info.kind {
                    SurfaceKind::Plane => {}
                    SurfaceKind::Cylinder | SurfaceKind::Cone | SurfaceKind::Sphere | SurfaceKind::Torus | SurfaceKind::Revolution => {
                        match (fg.info.axis_origin, fg.info.axis_dir) {
                            (Some(o), Some(d)) => axes.push((o, d.normalize())),
                            _ => ok = false,
                        }
                    }
                    _ => ok = false,
                }
            }
            if ok && !axes.is_empty() {
                let (o0, d0) = axes[0];
                let coaxial = axes.iter().all(|(o, d)| {
                    let par = d.cross(d0).length() < 1e-6;
                    let off = (*o - o0) - d0 * d0.dot(*o - o0);
                    par && off.length() < 1e-6 * ctx.size
                });
                if coaxial {
                    return radial(o0, d0);
                }
            }
            // Longest straight CAD edge in scope.
            let mut best: Option<(f64, EdgeId)> = None;
            for f in faces {
                for w in &ctx.topo.face(*f).wires {
                    for c in &w.coedges {
                        let et = ctx.topo.edge(c.edge);
                        if et.curve == CurveKind::Line && !et.seam && best.is_none_or(|(l, _)| et.length > l) {
                            best = Some((et.length, c.edge));
                        }
                    }
                }
            }
            if let Some((_, e)) = best {
                let poly = ctx.edge_polyline(e);
                return stack(poly[poly.len() - 1] - poly[0]);
            }
            // Fallback: the longest bounding-box axis.
            let ext = ctx.bbox.max - ctx.bbox.min;
            let n = if ext.x >= ext.y && ext.x >= ext.z {
                DVec3::X
            } else if ext.y >= ext.z {
                DVec3::Y
            } else {
                DVec3::Z
            };
            stack(n)
        }
    }
}

fn circumcentre(a: DVec3, b: DVec3, c: DVec3) -> DVec3 {
    let ab = b - a;
    let ac = c - a;
    let n = ab.cross(ac);
    let d = 2.0 * n.length_squared();
    if d < 1e-300 {
        return (a + b + c) / 3.0;
    }
    a + (n.cross(ab) * ac.length_squared() + ac.cross(n) * ab.length_squared()) / d
}

/// Resolved location of a cut point: candidate (face, uv) pairs.
pub fn cut_point_locations(ctx: &BodyCtx, cp: &CutPoint, known: &HashMap<VertexKey, Vec<(FaceId, DVec2)>>) -> Vec<(FaceId, DVec2)> {
    match cp {
        CutPoint::Face { face, uv, .. } => vec![(*face, *uv)],
        CutPoint::CadEdge { edge, t, .. } => {
            let et = ctx.topo.edge(*edge);
            et.faces
                .iter()
                .filter_map(|f| {
                    ctx.kernel.edge_pcurve_eval(ctx.kbody, *edge, *f, false, &[*t]).ok().and_then(|v| v.first().map(|uv| (*f, *uv)))
                })
                .collect()
        }
        CutPoint::Vertex { key, .. } => {
            if let VertexKey::Cad(v) = key {
                let mut out = Vec::new();
                for f in ctx.topo.face_ids() {
                    for c in &ctx.face(f).curves {
                        for (k, end) in c.ends.iter().enumerate() {
                            if *end == Some(*v) {
                                let uv = if k == 0 { c.pts[0] } else { c.pts[c.pts.len() - 1] };
                                if !out.iter().any(|(ff, _)| *ff == f) {
                                    out.push((f, uv));
                                }
                            }
                        }
                    }
                }
                return out;
            }
            known.get(key).cloned().unwrap_or_default()
        }
    }
}

/// Plan cut / join segments of a rule. Returns None when unresolvable.
pub fn plan_cut(ctx: &BodyCtx, rule: RuleId, points: &[CutPoint], closed: bool, known: &HashMap<VertexKey, Vec<(FaceId, DVec2)>>) -> Option<Vec<CutSeg>> {
    let locs: Vec<Vec<(FaceId, DVec2)>> = points.iter().map(|p| cut_point_locations(ctx, p, known)).collect();
    if locs.iter().any(|l| l.is_empty()) {
        return None;
    }
    let key_of = |i: usize| -> Option<VertexKey> {
        match &points[i] {
            CutPoint::Face { .. } => Some(VertexKey::CutPoint { rule, i: i as u32 }),
            CutPoint::CadEdge { .. } => Some(VertexKey::CutPoint { rule, i: i as u32 }),
            CutPoint::Vertex { key, .. } => Some(key.clone()),
        }
    };
    let n = points.len();
    let nseg = if closed { n } else { n.saturating_sub(1) };
    let mut segs = Vec::new();
    for i in 0..nseg {
        let j = (i + 1) % n;
        let (la, lb) = (&locs[i], &locs[j]);
        // Common face?
        let mut done = false;
        for (fa, ua) in la {
            if let Some((_, ub)) = lb.iter().find(|(fb, _)| fb == fa) {
                let fg = ctx.face(*fa);
                let ub = fg.lift_near(*ub, *ua);
                segs.push(CutSeg { face: *fa, index: i as u32, sub: 0, a: *ua, b: ub, keys: [key_of(i), key_of(j)] });
                done = true;
                break;
            }
        }
        if done {
            continue;
        }
        // Adjacent faces through a shared edge.
        let mut best: Option<(f64, FaceId, DVec2, FaceId, DVec2, EdgeId, f64)> = None;
        for (fa, ua) in la {
            for (fb, ub) in lb {
                for e in ctx.topo.edge_ids() {
                    let et = ctx.topo.edge(e);
                    if et.seam || et.degenerate || !(et.faces.contains(fa) && et.faces.contains(fb)) {
                        continue;
                    }
                    let pa = ctx.point(*fa, *ua);
                    let pb = ctx.point(*fb, *ub);
                    let cost = |t: f64| (ctx.edge_point(e, t) - pa).length() + (ctx.edge_point(e, t) - pb).length();
                    let (t0, t1) = et.t_range;
                    let m = 32;
                    let mut bt = (f64::INFINITY, t0);
                    for k in 0..=m {
                        let t = t0 + (t1 - t0) * k as f64 / m as f64;
                        let c = cost(t);
                        if c < bt.0 {
                            bt = (c, t);
                        }
                    }
                    let st = (t1 - t0) / m as f64;
                    let (mut lo, mut hi) = ((bt.1 - st).max(t0.min(t1)), (bt.1 + st).min(t0.max(t1)));
                    for _ in 0..50 {
                        let m1 = lo + (hi - lo) / 3.0;
                        let m2 = hi - (hi - lo) / 3.0;
                        if cost(m1) < cost(m2) {
                            hi = m2
                        } else {
                            lo = m1
                        }
                    }
                    let t = 0.5 * (lo + hi);
                    let c = cost(t);
                    if best.is_none_or(|b| c < b.0) {
                        best = Some((c, *fa, *ua, *fb, *ub, e, t));
                    }
                }
            }
        }
        let (_, fa, ua, fb, ub, e, t) = best?;
        let ea = ctx.kernel.edge_pcurve_eval(ctx.kbody, e, fa, false, &[t]).ok()?[0];
        let eb = ctx.kernel.edge_pcurve_eval(ctx.kbody, e, fb, false, &[t]).ok()?[0];
        let fga = ctx.face(fa);
        let fgb = ctx.face(fb);
        segs.push(CutSeg { face: fa, index: i as u32, sub: 0, a: ua, b: fga.lift_near(ea, ua), keys: [key_of(i), None] });
        let eb2 = fgb.lift_near(eb, ub);
        segs.push(CutSeg { face: fb, index: i as u32, sub: 1, a: eb2, b: ub, keys: [None, key_of(j)] });
    }
    Some(segs)
}

/// Patch bound helper for ids (SPEC §4.2).
pub fn bound_eq(a: PatchBound, b: PatchBound) -> bool {
    a == b
}
