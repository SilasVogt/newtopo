//! Analytic mock kernel (SPEC §3.4). Bodies are built in Rust from planes,
//! cylinders, cones, spheres and tori with exact pcurves, so most core tests
//! run without OCCT.
//!
//! Conventions (shared with what the core expects from any kernel):
//! * Ids are assigned in construction order (deterministic).
//! * A seam edge carries two pcurves on its face; `CoEdge::reversed` selects
//!   which one ([`MockBody::pcurve`]).
//! * Degenerate edges (poles, apex) have a `Curve3::Point` curve and a
//!   straight pcurve along the singular parameter line.
//! * Wires are oriented with the material on the left when looking from the
//!   oriented (outward) normal; the core does not rely on it.
//! * `save_brep` writes the body as JSON behind the magic prefix
//!   [`MOCK_MAGIC`]; `load_brep` reads it back.

pub mod geom;
pub mod shapes;
mod tess;

use crate::{
    BodyId, BodyTopology, CadKernel, CoEdge, CurveKind, EdgeId, EdgeTopo, FaceId, FaceMesh,
    FaceTopo, KResult, KernelError, SurfEval, SurfaceInfo, SurfaceKind, UvClass, VertexId,
    VertexTopo, WireTopo,
};
use geom::{Curve2, Curve3, Surface};
use glam::{DVec2, DVec3};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::f64::consts::TAU;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, RwLock};

/// Magic prefix of serialised mock bodies.
pub const MOCK_MAGIC: &[u8] = b"NTMOCKBREP1\n";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MFace {
    pub surface: Surface,
    pub reversed: bool,
    pub wires: Vec<WireTopo>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MPcurve {
    pub face: FaceId,
    /// For seam edges: which coedge orientation uses this pcurve.
    pub side: Option<bool>,
    pub pc: Curve2,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MEdge {
    pub curve: Curve3,
    pub t0: f64,
    pub t1: f64,
    pub v0: Option<VertexId>,
    pub v1: Option<VertexId>,
    pub pcurves: Vec<MPcurve>,
}

/// An analytic B-rep body.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MockBody {
    pub name: String,
    pub faces: Vec<MFace>,
    pub edges: Vec<MEdge>,
    pub vertices: Vec<DVec3>,
}

impl MockBody {
    pub fn to_brep_bytes(&self) -> Vec<u8> {
        let mut out = MOCK_MAGIC.to_vec();
        out.extend(serde_json::to_vec(self).expect("mock body serialises"));
        out
    }

    pub fn from_brep_bytes(bytes: &[u8]) -> KResult<MockBody> {
        let rest = bytes
            .strip_prefix(MOCK_MAGIC)
            .ok_or_else(|| KernelError::Failure("not a mock b-rep (bad magic)".into()))?;
        serde_json::from_slice(rest).map_err(|e| KernelError::Failure(format!("mock b-rep: {e}")))
    }

    pub fn edge_point(&self, e: EdgeId, t: f64) -> DVec3 {
        match &self.edges[e.idx()].curve {
            Curve3::Line { p0, d } => *p0 + *d * t,
            Curve3::Circle { c, x, y, r } => {
                let (s, co) = t.sin_cos();
                *c + (*x * co + *y * s) * *r
            }
            Curve3::Point { p } => *p,
            Curve3::OnFace { face, pc } => self.faces[*face as usize].surface.point(pc.eval(t)),
        }
    }

    pub fn is_seam(&self, e: EdgeId) -> bool {
        let ed = &self.edges[e.idx()];
        ed.pcurves.len() == 2 && ed.pcurves[0].face == ed.pcurves[1].face
    }

    pub fn is_degenerate(&self, e: EdgeId) -> bool {
        matches!(self.edges[e.idx()].curve, Curve3::Point { .. })
    }

    /// Pcurve of `e` on `f`; `reversed` picks the side of a seam edge.
    pub fn pcurve(&self, e: EdgeId, f: FaceId, reversed: bool) -> Option<&Curve2> {
        let ed = &self.edges[e.idx()];
        let mut first = None;
        for pc in &ed.pcurves {
            if pc.face != f {
                continue;
            }
            match pc.side {
                Some(s) if s == reversed => return Some(&pc.pc),
                Some(_) => {}
                None => return Some(&pc.pc),
            }
            if first.is_none() {
                first = Some(&pc.pc);
            }
        }
        first
    }

    /// Sampled UV loops of a face's wires, in wire order.
    pub fn face_loops(&self, f: FaceId) -> Vec<Vec<DVec2>> {
        let face = &self.faces[f.idx()];
        let mut loops = Vec::new();
        for w in &face.wires {
            let mut pts: Vec<DVec2> = Vec::new();
            for ce in &w.coedges {
                let ed = &self.edges[ce.edge.idx()];
                let Some(pc) = self.pcurve(ce.edge, f, ce.reversed) else {
                    continue;
                };
                let n = match pc {
                    Curve2::Line { .. } => 1,
                    Curve2::Ellipse { .. } => 96,
                };
                for i in 0..n {
                    let s = i as f64 / n as f64;
                    let s = if ce.reversed { 1.0 - s } else { s };
                    let t = ed.t0 + (ed.t1 - ed.t0) * s;
                    pts.push(pc.eval(t));
                }
            }
            loops.push(pts);
        }
        loops
    }

    pub fn surface_info(&self, f: FaceId) -> SurfaceInfo {
        let face = &self.faces[f.idx()];
        let loops = self.face_loops(f);
        let mut lo = DVec2::splat(f64::INFINITY);
        let mut hi = DVec2::splat(f64::NEG_INFINITY);
        for l in &loops {
            for p in l {
                lo = lo.min(*p);
                hi = hi.max(*p);
            }
        }
        let (mut u_closed, mut v_closed) = (false, false);
        for w in &face.wires {
            for ce in &w.coedges {
                if self.is_seam(ce.edge)
                    && let Some(Curve2::Line { d, .. }) = self.pcurve(ce.edge, f, ce.reversed)
                {
                    if d.x.abs() < 1e-12 {
                        u_closed = true;
                    } else if d.y.abs() < 1e-12 {
                        v_closed = true;
                    }
                }
            }
        }
        let s = &face.surface;
        let (kind, params) = match *s {
            Surface::Plane { .. } => (SurfaceKind::Plane, vec![]),
            Surface::Cylinder { r, .. } => (SurfaceKind::Cylinder, vec![r]),
            Surface::Cone { r0, k, .. } => (SurfaceKind::Cone, vec![r0, k.atan()]),
            Surface::Sphere { r, .. } => (SurfaceKind::Sphere, vec![r]),
            Surface::Torus { major, minor, .. } => (SurfaceKind::Torus, vec![major, minor]),
        };
        SurfaceInfo {
            kind,
            u_range: (lo.x, hi.x),
            v_range: (lo.y, hi.y),
            u_period: s.u_periodic().then_some(TAU),
            v_period: s.v_periodic().then_some(TAU),
            u_closed,
            v_closed,
            axis_origin: Some(s.origin()),
            axis_dir: s.axis(),
            params,
        }
    }

    /// Map a UV point into the face's seam-cut domain.
    fn wrap(&self, info: &SurfaceInfo, mut uv: DVec2) -> DVec2 {
        if info.u_closed {
            let p = info.u_period.unwrap_or(TAU);
            uv.x = info.u_range.0 + (uv.x - info.u_range.0).rem_euclid(p);
        }
        if info.v_closed {
            let p = info.v_period.unwrap_or(TAU);
            uv.y = info.v_range.0 + (uv.y - info.v_range.0).rem_euclid(p);
        }
        uv
    }

    fn classify_one(&self, info: &SurfaceInfo, loops: &[Vec<DVec2>], uv: DVec2) -> UvClass {
        let q = self.wrap(info, uv);
        let size = (info.u_range.1 - info.u_range.0)
            .abs()
            .max((info.v_range.1 - info.v_range.0).abs())
            .max(1e-12);
        let tol = 1e-9 * size;
        let mut inside = false;
        for l in loops {
            let n = l.len();
            for i in 0..n {
                let a = l[i];
                let b = l[(i + 1) % n];
                if dist_point_seg(q, a, b) <= tol {
                    return UvClass::On;
                }
                if (a.y > q.y) != (b.y > q.y) {
                    let x = a.x + (q.y - a.y) * (b.x - a.x) / (b.y - a.y);
                    if q.x < x {
                        inside = !inside;
                    }
                }
            }
        }
        if inside { UvClass::In } else { UvClass::Out }
    }

    fn eval_face(&self, f: FaceId, uv: DVec2) -> SurfEval {
        let face = &self.faces[f.idx()];
        let (p, du, dv) = face.surface.eval(uv);
        let mut n = face.surface.normal(uv);
        if face.reversed {
            n = -n;
        }
        SurfEval { p, n, du, dv }
    }

    fn edge_len(&self, e: EdgeId) -> f64 {
        let ed = &self.edges[e.idx()];
        let n = 256;
        let mut len = 0.0;
        let mut prev = self.edge_point(e, ed.t0);
        for i in 1..=n {
            let t = ed.t0 + (ed.t1 - ed.t0) * i as f64 / n as f64;
            let p = self.edge_point(e, t);
            len += (p - prev).length();
            prev = p;
        }
        len
    }

    fn dihedral(&self, e: EdgeId, faces: &[FaceId]) -> Option<f64> {
        if faces.len() != 2 || self.is_seam(e) || self.is_degenerate(e) {
            return None;
        }
        let ed = &self.edges[e.idx()];
        let mut sum = 0.0;
        let k = 5;
        for i in 0..k {
            let t = ed.t0 + (ed.t1 - ed.t0) * (i as f64 + 0.5) / k as f64;
            let na = self
                .eval_face(faces[0], self.pcurve(e, faces[0], false)?.eval(t))
                .n;
            let nb = self
                .eval_face(faces[1], self.pcurve(e, faces[1], false)?.eval(t))
                .n;
            sum += na.dot(nb).clamp(-1.0, 1.0).acos();
        }
        Some(sum / k as f64)
    }

    /// Full topology with derived flags (seam, degenerate, closed, dihedral).
    pub fn topology(&self) -> BodyTopology {
        let mut edges = Vec::with_capacity(self.edges.len());
        for (i, ed) in self.edges.iter().enumerate() {
            let e = EdgeId(i as u32);
            let mut faces: Vec<FaceId> = Vec::new();
            for pc in &ed.pcurves {
                if !faces.contains(&pc.face) {
                    faces.push(pc.face);
                }
            }
            let degenerate = self.is_degenerate(e);
            let curve = match ed.curve {
                Curve3::Line { .. } => CurveKind::Line,
                Curve3::Circle { .. } => CurveKind::Circle,
                Curve3::Point { .. } | Curve3::OnFace { .. } => CurveKind::Other,
            };
            edges.push(EdgeTopo {
                v0: ed.v0,
                v1: ed.v1,
                t_range: (ed.t0, ed.t1),
                curve,
                length: if degenerate { 0.0 } else { self.edge_len(e) },
                seam: self.is_seam(e),
                degenerate,
                closed: !degenerate && ed.v0.is_some() && ed.v0 == ed.v1,
                dihedral: self.dihedral(e, &faces),
                faces,
            });
        }
        let mut faces = Vec::with_capacity(self.faces.len());
        for (i, face) in self.faces.iter().enumerate() {
            let f = FaceId(i as u32);
            let mesh = tess::tessellate(self, f, 2e-4 * self.size().max(1e-9));
            let area = mesh
                .triangles
                .iter()
                .map(|t| {
                    let [a, b, c] = t.map(|k| mesh.positions[k as usize]);
                    (b - a).cross(c - a).length() * 0.5
                })
                .sum();
            faces.push(FaceTopo {
                surface: self.surface_info(f),
                reversed: face.reversed,
                wires: face.wires.clone(),
                area,
            });
        }
        BodyTopology {
            faces,
            edges,
            vertices: self
                .vertices
                .iter()
                .map(|&point| VertexTopo { point })
                .collect(),
        }
    }

    /// Rough body size (bounding-box diagonal of vertices and edge samples).
    pub fn size(&self) -> f64 {
        let mut b = crate::Aabb::EMPTY;
        for p in &self.vertices {
            b.add(*p);
        }
        for (i, ed) in self.edges.iter().enumerate() {
            for k in 0..=8 {
                b.add(self.edge_point(EdgeId(i as u32), ed.t0 + (ed.t1 - ed.t0) * k as f64 / 8.0));
            }
        }
        b.diagonal()
    }
}

pub(crate) fn dist_point_seg(p: DVec2, a: DVec2, b: DVec2) -> f64 {
    let ab = b - a;
    let l2 = ab.length_squared();
    let t = if l2 > 0.0 {
        ((p - a).dot(ab) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (a + ab * t - p).length()
}

struct Entry {
    body: MockBody,
    topo: BodyTopology,
    infos: Vec<SurfaceInfo>,
    loops: Vec<Vec<Vec<DVec2>>>,
}

/// The mock kernel. Thread-safe; bodies are immutable once added.
#[derive(Default)]
pub struct MockKernel {
    bodies: RwLock<BTreeMap<u32, Arc<Entry>>>,
    next: AtomicU32,
}

impl MockKernel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a body and return its id.
    pub fn add_body(&self, body: MockBody) -> BodyId {
        let topo = body.topology();
        let infos = topo.faces.iter().map(|f| f.surface.clone()).collect();
        let loops = (0..body.faces.len())
            .map(|i| body.face_loops(FaceId(i as u32)))
            .collect();
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.bodies.write().unwrap().insert(
            id,
            Arc::new(Entry {
                body,
                topo,
                infos,
                loops,
            }),
        );
        BodyId(id)
    }

    /// The analytic body behind an id (tests, debugging).
    pub fn body(&self, id: BodyId) -> Option<MockBody> {
        self.get(id).ok().map(|e| e.body.clone())
    }

    fn get(&self, id: BodyId) -> KResult<Arc<Entry>> {
        self.bodies
            .read()
            .unwrap()
            .get(&id.0)
            .cloned()
            .ok_or(KernelError::UnknownBody(id))
    }

    pub fn add_box(&self, size: DVec3) -> BodyId {
        self.add_body(MockBody::make_box(size))
    }
    pub fn add_cylinder(&self, r: f64, h: f64) -> BodyId {
        self.add_body(MockBody::cylinder(r, h))
    }
    pub fn add_plate_with_holes(&self, n: u32, m: u32) -> BodyId {
        self.add_body(MockBody::plate_with_holes(n, m))
    }
    pub fn add_cone(&self, r: f64, h: f64) -> BodyId {
        self.add_body(MockBody::cone(r, h))
    }
    pub fn add_sphere(&self, r: f64) -> BodyId {
        self.add_body(MockBody::sphere(r))
    }
    pub fn add_torus(&self, major: f64, minor: f64) -> BodyId {
        self.add_body(MockBody::torus(major, minor))
    }
    pub fn add_trimmed_torus(&self, major: f64, minor: f64) -> BodyId {
        self.add_body(MockBody::trimmed_torus(major, minor))
    }
    pub fn add_ring(&self, outer: f64, inner: f64) -> BodyId {
        self.add_body(MockBody::ring(outer, inner))
    }
    pub fn add_symmetric_part(&self) -> BodyId {
        self.add_body(MockBody::symmetric_part())
    }
    pub fn add_split_cylinder(&self, r: f64, h: f64) -> BodyId {
        self.add_body(MockBody::split_cylinder(r, h))
    }
}

impl CadKernel for MockKernel {
    fn name(&self) -> String {
        "mock".into()
    }

    fn load_brep(&self, bytes: &[u8]) -> KResult<BodyId> {
        Ok(self.add_body(MockBody::from_brep_bytes(bytes)?))
    }

    fn save_brep(&self, body: BodyId) -> KResult<Vec<u8>> {
        Ok(self.get(body)?.body.to_brep_bytes())
    }

    fn unload(&self, body: BodyId) {
        self.bodies.write().unwrap().remove(&body.0);
    }

    fn topology(&self, body: BodyId) -> KResult<BodyTopology> {
        Ok(self.get(body)?.topo.clone())
    }

    fn face_eval(&self, body: BodyId, face: FaceId, uv: &[DVec2]) -> KResult<Vec<SurfEval>> {
        let e = self.get(body)?;
        if face.idx() >= e.body.faces.len() {
            return Err(KernelError::UnknownFace(face));
        }
        Ok(uv.iter().map(|&q| e.body.eval_face(face, q)).collect())
    }

    fn face_classify_uv(&self, body: BodyId, face: FaceId, uv: &[DVec2]) -> KResult<Vec<UvClass>> {
        let e = self.get(body)?;
        if face.idx() >= e.body.faces.len() {
            return Err(KernelError::UnknownFace(face));
        }
        let info = &e.infos[face.idx()];
        let loops = &e.loops[face.idx()];
        Ok(uv
            .iter()
            .map(|&q| e.body.classify_one(info, loops, q))
            .collect())
    }

    fn edge_eval(&self, body: BodyId, edge: EdgeId, t: &[f64]) -> KResult<Vec<DVec3>> {
        let e = self.get(body)?;
        if edge.idx() >= e.body.edges.len() {
            return Err(KernelError::UnknownEdge(edge));
        }
        Ok(t.iter().map(|&t| e.body.edge_point(edge, t)).collect())
    }

    fn edge_pcurve_eval(
        &self,
        body: BodyId,
        edge: EdgeId,
        face: FaceId,
        reversed: bool,
        t: &[f64],
    ) -> KResult<Vec<DVec2>> {
        let e = self.get(body)?;
        if edge.idx() >= e.body.edges.len() {
            return Err(KernelError::UnknownEdge(edge));
        }
        let pc = e.body.pcurve(edge, face, reversed).ok_or_else(|| {
            KernelError::Failure(format!("edge {edge} has no pcurve on face {face}"))
        })?;
        Ok(t.iter().map(|&t| pc.eval(t)).collect())
    }

    fn project_to_face(
        &self,
        body: BodyId,
        face: FaceId,
        p: &[DVec3],
    ) -> KResult<Vec<Option<(DVec2, DVec3)>>> {
        let e = self.get(body)?;
        if face.idx() >= e.body.faces.len() {
            return Err(KernelError::UnknownFace(face));
        }
        let b = &e.body;
        let surf = &b.faces[face.idx()].surface;
        let info = &e.infos[face.idx()];
        let loops = &e.loops[face.idx()];
        let mut out = Vec::with_capacity(p.len());
        for &q in p {
            let guess = b.wrap(info, surf.invert_guess(q));
            let uv = b.wrap(info, surf.refine_closest(q, guess));
            if b.classify_one(info, loops, uv) != UvClass::Out {
                out.push(Some((uv, surf.point(uv))));
                continue;
            }
            // Outside the trims: nearest point on the boundary.
            let mut best: Option<(f64, DVec2)> = None;
            for l in loops {
                let n = l.len();
                for i in 0..n {
                    let (a, c) = (l[i], l[(i + 1) % n]);
                    for k in 0..8 {
                        let s = a + (c - a) * (k as f64 / 8.0);
                        let d = (surf.point(s) - q).length();
                        if best.is_none_or(|(bd, _)| d < bd) {
                            best = Some((d, s));
                        }
                    }
                }
            }
            out.push(best.map(|(_, s)| (s, surf.point(s))));
        }
        Ok(out)
    }

    fn project_to_edge(
        &self,
        body: BodyId,
        edge: EdgeId,
        p: &[DVec3],
    ) -> KResult<Vec<Option<f64>>> {
        let e = self.get(body)?;
        if edge.idx() >= e.body.edges.len() {
            return Err(KernelError::UnknownEdge(edge));
        }
        let b = &e.body;
        let ed = &b.edges[edge.idx()];
        let n = 128;
        Ok(p.iter()
            .map(|&q| {
                let dist = |t: f64| (b.edge_point(edge, t) - q).length_squared();
                let mut best = (f64::INFINITY, ed.t0);
                for i in 0..=n {
                    let t = ed.t0 + (ed.t1 - ed.t0) * i as f64 / n as f64;
                    let d = dist(t);
                    if d < best.0 {
                        best = (d, t);
                    }
                }
                let step = (ed.t1 - ed.t0) / n as f64;
                let (lo0, hi0) = (ed.t0.min(ed.t1), ed.t0.max(ed.t1));
                let (mut lo, mut hi) = (
                    (best.1 - step.abs()).max(lo0),
                    (best.1 + step.abs()).min(hi0),
                );
                for _ in 0..80 {
                    let m1 = lo + (hi - lo) / 3.0;
                    let m2 = hi - (hi - lo) / 3.0;
                    if dist(m1) < dist(m2) {
                        hi = m2
                    } else {
                        lo = m1
                    }
                }
                Some(0.5 * (lo + hi))
            })
            .collect())
    }

    fn tessellate(&self, body: BodyId, face: FaceId, tol: f64) -> KResult<FaceMesh> {
        let e = self.get(body)?;
        if face.idx() >= e.body.faces.len() {
            return Err(KernelError::UnknownFace(face));
        }
        Ok(tess::tessellate(&e.body, face, tol))
    }
}

/// Coedge helper.
pub fn ce(edge: EdgeId, reversed: bool) -> CoEdge {
    CoEdge { edge, reversed }
}
