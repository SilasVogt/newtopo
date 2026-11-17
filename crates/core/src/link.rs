//! Congruent faces and face-to-face mappings: Link sets (SPEC §6.11),
//! symmetry partners (§6.12) and copy/paste of rules (§6.13).
//!
//! Two faces are congruent when they share surface type, intrinsic
//! parameters, boundary structure, area and edge lengths, and a rigid
//! transform (rotation, optional reflection, translation) maps sampled points
//! of one onto the other's surface and boundary. Candidate transforms: the
//! caller's preferred ones (symmetry reflections, a body transform), a pure
//! translation, and the rotations aligning the two faces' principal axes
//! (PCA, own code; Jacobi eigen-decomposition).

#![allow(clippy::needless_range_loop, clippy::too_many_arguments)]

use crate::doc::{
    CutPoint, Document, IsoDir, IsoPiece, PatchBound, PatchId, PhaseTarget, Plane, Rule,
    SliceOrient, SliceScope, SplitKind, SplitLine,
};
use crate::eval::BodyCtx;
use crate::eval::lines::{compute_slice_planes, default_phase_dir, iso_abs, iso_norm, slice_faces};
use glam::{DAffine3, DMat3, DVec2, DVec3};
use newtopo_kernel::{BodyId, EdgeId, FaceId, VertexId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// Rules copied from a body (SPEC §6.13). Self-contained: it lists the
/// source body; pasting looks the source geometry up in the scene.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Clipboard {
    pub source: BodyId,
    pub rules: Vec<Rule>,
    pub split_lines: Vec<SplitLine>,
    pub phase_overrides: Vec<(PhaseTarget, DVec3)>,
    /// Number of edits copied (`edit_list.len()`).
    pub edits: usize,
    /// The source body's edits, in order. Pasting maps their keys onto the
    /// target's output by geometry; ones that cannot be mapped are reported.
    #[serde(default)]
    pub edit_list: Vec<crate::doc::Edit>,
}

/// Coarse signature used to bucket faces before the transform test.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SigKey {
    pub kind: u8,
    pub wires: usize,
    pub edges: usize,
}

#[derive(Clone, Debug)]
pub struct FaceSample {
    pub key: SigKey,
    pub params: Vec<f64>,
    pub area: f64,
    pub edge_lens: Vec<f64>,
    pub pts: Vec<DVec3>,
    /// Boundary samples (a few per edge).
    pub bnd: Vec<DVec3>,
    /// Boundary edge polylines (for distance tests).
    pub bnd_polys: Vec<Vec<DVec3>>,
    pub centroid: DVec3,
    pub axes: DMat3,
}

fn jacobi(m: DMat3) -> (DVec3, DMat3) {
    let mut a = [[0.0f64; 3]; 3];
    for (i, row) in a.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = m.col(j)[i];
        }
    }
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..50 {
        let (mut p, mut q, mut best) = (0, 1, 0.0);
        for i in 0..3 {
            for j in i + 1..3 {
                if a[i][j].abs() > best {
                    best = a[i][j].abs();
                    p = i;
                    q = j;
                }
            }
        }
        if best < 1e-14 {
            break;
        }
        let theta = 0.5 * (2.0 * a[p][q]).atan2(a[q][q] - a[p][p]);
        let (s, c) = theta.sin_cos();
        let mut r = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        r[p][p] = c;
        r[q][q] = c;
        r[p][q] = s;
        r[q][p] = -s;
        // a = rᵀ a r, v = v r
        let mut t = [[0.0; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                t[i][j] = (0..3).map(|k| a[i][k] * r[k][j]).sum();
            }
        }
        for i in 0..3 {
            for j in 0..3 {
                a[i][j] = (0..3).map(|k| r[k][i] * t[k][j]).sum();
            }
        }
        let mut nv = [[0.0; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                nv[i][j] = (0..3).map(|k| v[i][k] * r[k][j]).sum();
            }
        }
        v = nv;
    }
    let vals = DVec3::new(a[0][0], a[1][1], a[2][2]);
    let mut idx = [0usize, 1, 2];
    idx.sort_by(|&x, &y| vals[y].total_cmp(&vals[x]));
    let col = |k: usize| DVec3::new(v[0][k], v[1][k], v[2][k]);
    (
        DVec3::new(vals[idx[0]], vals[idx[1]], vals[idx[2]]),
        DMat3::from_cols(col(idx[0]), col(idx[1]), col(idx[2])),
    )
}

/// Sample a face for congruence tests.
pub fn face_sample(ctx: &BodyCtx, f: FaceId) -> FaceSample {
    let ft = ctx.topo.face(f);
    let mut edges: Vec<EdgeId> = Vec::new();
    for w in &ft.wires {
        for c in &w.coedges {
            let et = ctx.topo.edge(c.edge);
            if !et.seam && !et.degenerate && !edges.contains(&c.edge) {
                edges.push(c.edge);
            }
        }
    }
    let mut edge_lens: Vec<f64> = edges.iter().map(|e| ctx.topo.edge(*e).length).collect();
    edge_lens.sort_by(|a, b| a.total_cmp(b));
    let mesh = &ctx.face_meshes[f.idx()];
    let step = (mesh.positions.len() / 48).max(1);
    let pts: Vec<DVec3> = mesh.positions.iter().step_by(step).copied().collect();
    let mut bnd = Vec::new();
    let mut bnd_polys = Vec::new();
    for e in &edges {
        let poly = ctx.edge_polyline(*e);
        let st = (poly.len() / 8).max(1);
        bnd.extend(poly.iter().step_by(st).copied());
        bnd_polys.push(poly);
    }
    let all: Vec<DVec3> = mesh.positions.clone();
    let n = all.len().max(1) as f64;
    let centroid = all.iter().copied().sum::<DVec3>() / n;
    let mut cov = DMat3::ZERO;
    for p in &all {
        let d = *p - centroid;
        cov += DMat3::from_cols(d * d.x, d * d.y, d * d.z);
    }
    let (_, axes) = jacobi(cov * (1.0 / n));
    let kind = ft.surface.kind as u8;
    FaceSample {
        key: SigKey {
            kind,
            wires: ft.wires.len(),
            edges: edges.len(),
        },
        params: ft.surface.params.clone(),
        area: ft.area,
        edge_lens,
        pts,
        bnd,
        bnd_polys,
        centroid,
        axes,
    }
}

fn close(a: f64, b: f64, rel: f64, abs: f64) -> bool {
    (a - b).abs() <= rel * a.abs().max(b.abs()) + abs
}

/// Cheap intrinsic comparison (before any transform test).
pub fn similar(a: &FaceSample, b: &FaceSample, size: f64) -> bool {
    a.key == b.key
        && a.params.len() == b.params.len()
        && a.params
            .iter()
            .zip(&b.params)
            .all(|(x, y)| close(*x, *y, 1e-6, 1e-9 * size))
        && close(a.area, b.area, 2e-3, 1e-9 * size * size)
        && a.edge_lens.len() == b.edge_lens.len()
        && a.edge_lens
            .iter()
            .zip(&b.edge_lens)
            .all(|(x, y)| close(*x, *y, 1e-3, 1e-7 * size))
}

/// Does `t` map face A onto face B?
pub fn verify(
    ca: &BodyCtx,
    a: &FaceSample,
    cb: &BodyCtx,
    fb: FaceId,
    b: &FaceSample,
    t: &DAffine3,
) -> bool {
    let size = ca.size.max(cb.size);
    let tol_s = 1e-4 * size;
    let tol_b = 3e-3 * size;
    let pts: Vec<DVec3> = a.pts.iter().map(|p| t.transform_point3(*p)).collect();
    let Ok(pr) = cb.kernel.project_to_face(cb.kbody, fb, &pts) else {
        return false;
    };
    let dbg = std::env::var("NT_DEBUG_LINK").is_ok();
    for (p, r) in pts.iter().zip(pr) {
        match r {
            Some((_, q)) if (q - *p).length() <= tol_s => {}
            _ => {
                if dbg {
                    eprintln!(
                        "verify face {fb}: point {p} off by {:?}",
                        r.map(|x| (x.1 - *p).length())
                    );
                }
                return false;
            }
        }
    }
    // Boundary samples land on the other face's boundary (both ways).
    let near = |p: DVec3, polys: &[Vec<DVec3>]| {
        polys.iter().any(|poly| {
            poly.windows(2).any(|w| {
                let (x, y) = (w[0], w[1]);
                let d = y - x;
                let f = if d.length_squared() > 0.0 {
                    ((p - x).dot(d) / d.length_squared()).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                (x + d * f - p).length() <= tol_b
            }) || poly.len() == 1 && (poly[0] - p).length() <= tol_b
        })
    };
    a.bnd
        .iter()
        .all(|p| near(t.transform_point3(*p), &b.bnd_polys))
        && {
            let inv = t.inverse();
            b.bnd
                .iter()
                .all(|q| near(inv.transform_point3(*q), &a.bnd_polys))
        }
}

/// A rigid transform mapping face A (body A) onto face B (body B), trying
/// `preferred` first.
pub fn match_faces(
    ca: &BodyCtx,
    a: &FaceSample,
    cb: &BodyCtx,
    fb: FaceId,
    b: &FaceSample,
    preferred: &[DAffine3],
) -> Option<DAffine3> {
    if !similar(a, b, ca.size.max(cb.size)) {
        return None;
    }
    let mut cands: Vec<DAffine3> = preferred.to_vec();
    cands.push(DAffine3::from_translation(b.centroid - a.centroid));
    for sx in [1.0, -1.0] {
        for sy in [1.0, -1.0] {
            for sz in [1.0, -1.0] {
                let s = DMat3::from_diagonal(DVec3::new(sx, sy, sz));
                let r = b.axes * s * a.axes.transpose();
                cands.push(DAffine3::from_mat3_translation(
                    r,
                    b.centroid - r * a.centroid,
                ));
            }
        }
    }
    cands.into_iter().find(|t| verify(ca, a, cb, fb, b, t))
}

/// Match source faces to target faces, preferring one consistent rigid
/// transform: every face first tries the transforms already found (and
/// `preferred`), and only then the full candidate search.
pub fn match_bodies(
    ca: &BodyCtx,
    src: &[FaceId],
    cb: &BodyCtx,
    preferred: &[DAffine3],
) -> (FaceMap, Vec<FaceId>) {
    let sa: Vec<(FaceId, FaceSample)> = src.iter().map(|f| (*f, face_sample(ca, *f))).collect();
    let sb: Vec<FaceSample> = cb.topo.face_ids().map(|f| face_sample(cb, f)).collect();
    let mut known: Vec<DAffine3> = preferred.to_vec();
    let mut map = FaceMap::new();
    let mut taken = vec![false; sb.len()];
    let mut unmatched = Vec::new();
    let size = ca.size.max(cb.size);
    for (fa, a) in &sa {
        let mut found = None;
        'known: for t in known.clone() {
            for fb in cb.topo.face_ids() {
                if !taken[fb.idx()]
                    && similar(a, &sb[fb.idx()], size)
                    && verify(ca, a, cb, fb, &sb[fb.idx()], &t)
                {
                    found = Some((fb, t));
                    break 'known;
                }
            }
        }
        if found.is_none() {
            for fb in cb.topo.face_ids() {
                if taken[fb.idx()] {
                    continue;
                }
                if let Some(t) = match_faces(ca, a, cb, fb, &sb[fb.idx()], &[]) {
                    found = Some((fb, t));
                    break;
                }
            }
        }
        match found {
            Some((fb, t)) => {
                taken[fb.idx()] = true;
                map.insert(*fa, (fb, t));
                if !known.iter().any(|k| k.abs_diff_eq(t, 1e-9)) {
                    known.insert(0, t);
                }
            }
            None => unmatched.push(*fa),
        }
    }
    (map, unmatched)
}

/// Link sets of a body: groups of congruent faces, with transforms from
/// the group's first face to each member.
#[derive(Clone, Debug, Default)]
pub struct LinkSets {
    pub groups: Vec<Vec<(FaceId, DAffine3)>>,
    pub group_of: HashMap<FaceId, usize>,
}

impl LinkSets {
    pub fn compute(ctx: &BodyCtx) -> LinkSets {
        let samples: Vec<FaceSample> = ctx.topo.face_ids().map(|f| face_sample(ctx, f)).collect();
        let mut buckets: BTreeMap<SigKey, Vec<FaceId>> = BTreeMap::new();
        for f in ctx.topo.face_ids() {
            buckets
                .entry(samples[f.idx()].key.clone())
                .or_default()
                .push(f);
        }
        let mut out = LinkSets::default();
        for faces in buckets.values() {
            let mut left: Vec<FaceId> = faces.clone();
            while let Some(rep) = left.first().copied() {
                left.remove(0);
                let mut group = vec![(rep, DAffine3::IDENTITY)];
                let mut rest = Vec::new();
                for f in left {
                    match match_faces(ctx, &samples[rep.idx()], ctx, f, &samples[f.idx()], &[]) {
                        Some(t) => group.push((f, t)),
                        None => rest.push(f),
                    }
                }
                left = rest;
                if group.len() > 1 {
                    let gi = out.groups.len();
                    for (f, _) in &group {
                        out.group_of.insert(*f, gi);
                    }
                    out.groups.push(group);
                }
            }
        }
        out
    }

    /// Faces congruent to `f` (excluding `f`) with transforms `f → member`.
    pub fn partners(&self, f: FaceId) -> Vec<(FaceId, DAffine3)> {
        let Some(&g) = self.group_of.get(&f) else {
            return vec![];
        };
        let grp = &self.groups[g];
        let to_f = grp
            .iter()
            .find(|(x, _)| *x == f)
            .map(|(_, t)| *t)
            .unwrap_or(DAffine3::IDENTITY);
        let inv = to_f.inverse();
        grp.iter()
            .filter(|(x, _)| *x != f)
            .map(|(x, t)| (*x, *t * inv))
            .collect()
    }
}

/// World mirror plane reflection for an axis (plane through the origin
/// perpendicular to the axis), expressed in body coordinates.
pub fn mirror(axis: crate::doc::Axis, body_to_world: glam::DMat4) -> DAffine3 {
    let d = match axis {
        crate::doc::Axis::X => DVec3::new(-1.0, 1.0, 1.0),
        crate::doc::Axis::Y => DVec3::new(1.0, -1.0, 1.0),
        crate::doc::Axis::Z => DVec3::new(1.0, 1.0, -1.0),
    };
    let w = DAffine3::from_mat4(body_to_world);
    w.inverse() * DAffine3::from_mat3(DMat3::from_diagonal(d)) * w
}

/// Symmetry partners: face → (partner, reflection) for one mirror plane.
pub fn symmetry_partners(ctx: &BodyCtx, refl: &DAffine3) -> HashMap<FaceId, (FaceId, DAffine3)> {
    let samples: Vec<FaceSample> = ctx.topo.face_ids().map(|f| face_sample(ctx, f)).collect();
    let mut out = HashMap::new();
    for fa in ctx.topo.face_ids() {
        for fb in ctx.topo.face_ids() {
            if fa == fb || !similar(&samples[fa.idx()], &samples[fb.idx()], ctx.size) {
                continue;
            }
            if verify(ctx, &samples[fa.idx()], ctx, fb, &samples[fb.idx()], refl) {
                out.insert(fa, (fb, *refl));
                break;
            }
        }
    }
    out
}

/// Face mapping from one body (or face set) to another.
pub type FaceMap = HashMap<FaceId, (FaceId, DAffine3)>;

pub(crate) fn is_whole(p: &PatchId) -> bool {
    p.u_lo == PatchBound::FaceBoundary
        && p.u_hi == PatchBound::FaceBoundary
        && p.v_lo == PatchBound::FaceBoundary
        && p.v_hi == PatchBound::FaceBoundary
        && p.region == 0
}

pub(crate) fn map_patch(p: &PatchId, to: BodyId, map: &FaceMap) -> Option<PatchId> {
    if !is_whole(p) {
        return None;
    }
    map.get(&p.face).map(|(f, _)| PatchId::whole(to, *f))
}

/// Map an iso line (face, dir, value, seed) through a face mapping by its
/// 3D mid point and tangent.
pub fn map_iso(
    ca: &BodyCtx,
    fa: FaceId,
    dir: IsoDir,
    value: f64,
    seed: Option<f64>,
    cb: &BodyCtx,
    fb: FaceId,
    t: &DAffine3,
) -> Option<(IsoDir, f64, Option<f64>)> {
    let ga = ca.face(fa);
    let gb = cb.face(fb);
    let d = if dir == IsoDir::U { 0 } else { 1 };
    let v = iso_abs(ga, d, value);
    let s = iso_abs(ga, 1 - d, seed.unwrap_or(0.5));
    let uv = if d == 0 {
        DVec2::new(v, s)
    } else {
        DVec2::new(s, v)
    };
    let e = ca.face_eval(fa, &[uv]).into_iter().next()?;
    let tan = if d == 0 { e.dv } else { e.du };
    let p = t.transform_point3(e.p);
    let tb = t.matrix3 * tan;
    let (uvb, _) = cb
        .kernel
        .project_to_face(cb.kbody, fb, &[p])
        .ok()?
        .into_iter()
        .next()??;
    let eb = cb.face_eval(fb, &[uvb]).into_iter().next()?;
    // Constant-u lines run along dv.
    let along_v = tb.normalize_or_zero().dot(eb.dv.normalize_or_zero()).abs()
        >= tb.normalize_or_zero().dot(eb.du.normalize_or_zero()).abs();
    let (nd, val, sd) = if along_v {
        (IsoDir::U, uvb.x, uvb.y)
    } else {
        (IsoDir::V, uvb.y, uvb.x)
    };
    let di = if nd == IsoDir::U { 0 } else { 1 };
    Some((
        nd,
        iso_norm(gb, di, val),
        seed.map(|_| iso_norm(gb, 1 - di, sd)),
    ))
}

fn map_point_face(cb: &BodyCtx, fb: FaceId, p: DVec3) -> Option<DVec2> {
    cb.kernel
        .project_to_face(cb.kbody, fb, &[p])
        .ok()?
        .into_iter()
        .next()?
        .map(|(uv, _)| uv)
}

/// Map one rule from body A to body B through a face map. Returns None when
/// nothing of it maps.
pub fn map_rule(
    rule: &Rule,
    a: BodyId,
    ca: &BodyCtx,
    b: BodyId,
    cb: &BodyCtx,
    map: &FaceMap,
) -> Option<Rule> {
    map_rule_with(rule, a, ca, b, cb, map, &|p| map_patch(p, b, map))
}

/// [`map_rule`] with a caller-supplied patch mapping (used by paste to map
/// patches bounded by split lines).
pub fn map_rule_with(
    rule: &Rule,
    a: BodyId,
    ca: &BodyCtx,
    b: BodyId,
    cb: &BodyCtx,
    map: &FaceMap,
    mp: &dyn Fn(&PatchId) -> Option<PatchId>,
) -> Option<Rule> {
    match rule {
        Rule::Connect {
            patches,
            count_a,
            count_b,
            start_on_seam,
            flipped,
        } => {
            let ps: Vec<PatchId> = patches
                .iter()
                .filter(|p| p.body == a)
                .filter_map(mp)
                .collect();
            if ps.is_empty() {
                return None;
            }
            let fl: Vec<PatchId> = flipped.iter().filter_map(mp).collect();
            Some(Rule::Connect {
                patches: ps,
                count_a: *count_a,
                count_b: *count_b,
                start_on_seam: *start_on_seam,
                flipped: fl,
            })
        }
        Rule::Grid {
            patch,
            count_u,
            count_v,
            start_on_seam,
            spacing,
        } => {
            if patch.body != a {
                return None;
            }
            // Grid U/V follow the target's own parameter directions; swap
            // when the mapping exchanges them.
            let pb = mp(patch)?;
            let (fb, t) = map[&patch.face];
            let swap = map_iso(ca, patch.face, IsoDir::U, 0.5, None, cb, fb, &t)
                .is_some_and(|(d, _, _)| d == IsoDir::V);
            let (cu, cv) = if swap {
                (*count_v, *count_u)
            } else {
                (*count_u, *count_v)
            };
            Some(Rule::Grid {
                patch: pb,
                count_u: cu,
                count_v: cv,
                start_on_seam: *start_on_seam,
                spacing: *spacing,
            })
        }
        Rule::Slice {
            scope,
            count,
            orient,
            through,
            frozen_planes,
        } => {
            let t = map.values().next()?.1;
            let planes = match frozen_planes {
                Some(p) => p.clone(),
                None => compute_slice_planes(ca, &slice_faces(ca, a, scope), *count, orient),
            };
            let planes: Vec<Plane> = planes
                .iter()
                .map(|p| Plane {
                    origin: t.transform_point3(p.origin),
                    normal: (t.matrix3 * p.normal).normalize(),
                })
                .collect();
            let scope = match scope {
                SliceScope::Body(x) if *x == a => SliceScope::Body(b),
                SliceScope::Body(_) => return None,
                SliceScope::Patches(l) => {
                    let ps: Vec<PatchId> =
                        l.iter().filter(|p| p.body == a).filter_map(mp).collect();
                    if ps.is_empty() {
                        return None;
                    }
                    SliceScope::Patches(ps)
                }
            };
            Some(Rule::Slice {
                scope,
                count: *count,
                orient: SliceOrient::Auto,
                through: *through,
                frozen_planes: Some(planes),
            })
        }
        Rule::Iso { pieces, through } => {
            let ps: Vec<IsoPiece> = pieces
                .iter()
                .filter(|p| p.body == a)
                .filter_map(|p| {
                    let (fb, t) = map.get(&p.face)?;
                    let (dir, value, seed) =
                        map_iso(ca, p.face, p.dir, p.value, p.seed, cb, *fb, t)?;
                    Some(IsoPiece {
                        body: b,
                        face: *fb,
                        dir,
                        value,
                        seed,
                    })
                })
                .collect();
            if ps.is_empty() {
                None
            } else {
                Some(Rule::Iso {
                    pieces: ps,
                    through: *through,
                })
            }
        }
        Rule::Cut { points, closed } => {
            let mut out = Vec::new();
            for p in points {
                out.push(match p {
                    CutPoint::Face { body, face, uv } if *body == a => {
                        let (fb, t) = map.get(face)?;
                        let q = ca.point(*face, *uv);
                        CutPoint::Face {
                            body: b,
                            face: *fb,
                            uv: map_point_face(cb, *fb, t.transform_point3(q))?,
                        }
                    }
                    CutPoint::CadEdge { body, edge, t: te } if *body == a => {
                        let (e, tt) = map_edge_point(ca, *edge, *te, cb, map)?;
                        CutPoint::CadEdge {
                            body: b,
                            edge: e,
                            t: tt,
                        }
                    }
                    CutPoint::Vertex {
                        body,
                        key: crate::keys::VertexKey::Cad(v),
                    } if *body == a => {
                        let (_, t) = map.values().next()?;
                        let q = t.transform_point3(ca.topo.vertex(*v).point);
                        let (i, _) = cb.topo.vertices.iter().enumerate().min_by(|x, y| {
                            (x.1.point - q)
                                .length()
                                .total_cmp(&(y.1.point - q).length())
                        })?;
                        CutPoint::Vertex {
                            body: b,
                            key: crate::keys::VertexKey::Cad(VertexId(i as u32)),
                        }
                    }
                    _ => return None,
                });
            }
            Some(Rule::Cut {
                points: out,
                closed: *closed,
            })
        }
        Rule::Join { .. } => None,
    }
}

/// Map a point on a CAD edge (edge, parameter) through a face mapping: the
/// nearest edge of the mapped face to the transformed point.
pub fn map_edge_point(
    ca: &BodyCtx,
    edge: EdgeId,
    te: f64,
    cb: &BodyCtx,
    map: &FaceMap,
) -> Option<(EdgeId, f64)> {
    let f = ca
        .topo
        .edge(edge)
        .faces
        .iter()
        .find(|f| map.contains_key(f))?;
    let (fb, t) = map[f];
    let q = t.transform_point3(ca.edge_point(edge, te));
    let mut best: Option<(f64, EdgeId, f64)> = None;
    for w in &cb.topo.face(fb).wires {
        for c in &w.coedges {
            if let Ok(v) = cb.kernel.project_to_edge(cb.kbody, c.edge, &[q])
                && let Some(tt) = v[0]
            {
                let d = (cb.edge_point(c.edge, tt) - q).length();
                if best.is_none_or(|x| d < x.0) {
                    best = Some((d, c.edge, tt));
                }
            }
        }
    }
    best.map(|(_, e, tt)| (e, tt))
}

/// The phase reference of a face (override or default).
pub fn phase_ref(doc: &Document, ctx: &BodyCtx, body: BodyId, f: FaceId) -> DVec3 {
    if let Some(d) = doc.phase_overrides.get(&PhaseTarget::Face(body, f)) {
        return *d;
    }
    let axis = ctx.face(f).info.axis_dir.unwrap_or(DVec3::Z);
    default_phase_dir(axis)
}

/// Set the phase references of mapped faces (SPEC §4.1: a face mapping sets
/// the target's reference to the source's mapped through the transform).
pub fn map_phases(
    doc: &mut Document,
    a: BodyId,
    ca: &BodyCtx,
    b: BodyId,
    cb: &BodyCtx,
    map: &FaceMap,
) {
    for (fa, (fb, t)) in map {
        if !ca.face(*fa).is_periodic() && ca.face(*fa).ring.is_none() {
            continue;
        }
        let r = phase_ref(doc, ca, a, *fa);
        doc.phase_overrides
            .insert(PhaseTarget::Face(b, *fb), (t.matrix3 * r).normalize());
    }
    // Closed CAD edges (placeholder loops) of mapped faces carry their own
    // reference; without it the target's placeholders start at its own
    // default and land elsewhere.
    for (fa, (_, t)) in map {
        for w in &ca.topo.face(*fa).wires {
            for c in &w.coedges {
                let et = ca.topo.edge(c.edge);
                if !et.closed || et.seam || et.degenerate {
                    continue;
                }
                let (lo, hi) = et.t_range;
                let one: FaceMap = std::iter::once((*fa, map[fa])).collect();
                let Some((eb, _)) = map_edge_point(ca, c.edge, 0.5 * (lo + hi), cb, &one) else {
                    continue;
                };
                if !cb.topo.edge(eb).closed {
                    continue;
                }
                let r = edge_phase_ref(doc, ca, a, c.edge);
                doc.phase_overrides
                    .insert(PhaseTarget::Edge(b, eb), (t.matrix3 * r).normalize());
            }
        }
    }
}

/// The phase reference of a closed CAD edge (override or the evaluator's
/// default from the loop normal).
pub fn edge_phase_ref(doc: &Document, ctx: &BodyCtx, body: BodyId, e: EdgeId) -> DVec3 {
    if let Some(d) = doc.phase_overrides.get(&PhaseTarget::Edge(body, e)) {
        return *d;
    }
    let pts: Vec<DVec3> = ctx.edge_polys[e.idx()].iter().map(|(_, p)| *p).collect();
    let mut n = DVec3::ZERO;
    for i in 0..pts.len() {
        let (p, q) = (pts[i], pts[(i + 1) % pts.len()]);
        n += DVec3::new(
            (p.y - q.y) * (p.z + q.z),
            (p.z - q.z) * (p.x + q.x),
            (p.x - q.x) * (p.y + q.y),
        );
    }
    let n = n.normalize_or_zero();
    default_phase_dir(if n.length() > 0.5 { n } else { DVec3::Z })
}

/// Map a split line through a face mapping.
pub fn map_split(
    s: &SplitLine,
    ca: &BodyCtx,
    b: BodyId,
    cb: &BodyCtx,
    map: &FaceMap,
    id: crate::keys::SplitId,
    group: u32,
) -> Option<SplitLine> {
    let (fb, t) = map.get(&s.face)?;
    let SplitKind::Iso { dir, value, seed } = s.kind;
    let (dir, value, seed) = map_iso(ca, s.face, dir, value, seed, cb, *fb, t)?;
    Some(SplitLine {
        id,
        group,
        body: b,
        face: *fb,
        kind: SplitKind::Iso { dir, value, seed },
    })
}
