//! Per-body static evaluation context: topology, sampled boundary curves in
//! each face's seam-cut UV domain, metric scales, display tessellation.
//! Built once per body (it doesn't depend on the document).

use super::Owner;
use glam::{DVec2, DVec3};
use newtopo_kernel::{
    Aabb, BodyId, BodyTopology, CadKernel, CurveKind, EdgeId, FaceId, FaceMesh, KResult,
    SurfaceInfo, SurfaceKind, VertexId,
};
use std::sync::Arc;

/// A boundary curve of a face in its seam-cut domain: one coedge's pcurve,
/// sampled with increasing edge parameter.
#[derive(Clone, Debug)]
pub struct BCurve {
    pub owner: Owner,
    /// `Some((dir, copy))` for seam copies: dir 0 = u-seam (constant u),
    /// 1 = v-seam; copy 0 = low side of the domain, 1 = high side.
    pub seam: Option<(u8, u8)>,
    pub pts: Vec<DVec2>,
    pub par: Vec<f64>,
    pub ends: [Option<VertexId>; 2],
}

#[derive(Clone, Debug)]
pub struct FaceGeom {
    pub id: FaceId,
    pub info: SurfaceInfo,
    pub reversed: bool,
    /// Period if the face is closed in that direction (has a seam).
    pub uper: Option<f64>,
    pub vper: Option<f64>,
    pub lo: DVec2,
    pub hi: DVec2,
    pub curves: Vec<BCurve>,
    /// The face has an outer wire with a real (non-seam, non-degenerate) edge.
    pub has_outer: bool,
    /// Average |∂S/∂u|, |∂S/∂v|.
    pub metric: DVec2,
    /// Planar ring (annulus between concentric circles): centre in UV.
    pub ring: Option<DVec2>,
    /// Distance tolerance in metric-scaled UV.
    pub eps: f64,
}

impl FaceGeom {
    pub fn periods(&self) -> DVec2 {
        DVec2::new(self.uper.unwrap_or(0.0), self.vper.unwrap_or(0.0))
    }
    /// Map into the seam-cut domain.
    pub fn wrap(&self, mut uv: DVec2) -> DVec2 {
        if let Some(p) = self.uper {
            uv.x = self.lo.x + (uv.x - self.lo.x).rem_euclid(p);
        }
        if let Some(p) = self.vper {
            uv.y = self.lo.y + (uv.y - self.lo.y).rem_euclid(p);
        }
        uv
    }
    /// Shift `uv` by whole periods to be nearest to `near`.
    pub fn lift_near(&self, mut uv: DVec2, near: DVec2) -> DVec2 {
        if let Some(p) = self.uper {
            uv.x += ((near.x - uv.x) / p).round() * p;
        }
        if let Some(p) = self.vper {
            uv.y += ((near.y - uv.y) / p).round() * p;
        }
        uv
    }
    /// Metric-scaled coordinates (≈ 3D lengths).
    pub fn m(&self, uv: DVec2) -> DVec2 {
        uv * self.metric
    }
    pub fn size_uv(&self) -> f64 {
        (self.hi - self.lo).max_element().max(1e-12)
    }
    pub fn is_periodic(&self) -> bool {
        self.uper.is_some() || self.vper.is_some()
    }
}

/// Static data for one body.
pub struct BodyCtx {
    pub kernel: Arc<dyn CadKernel>,
    /// Kernel-side body handle.
    pub kbody: BodyId,
    pub topo: BodyTopology,
    pub faces: Vec<FaceGeom>,
    pub size: f64,
    /// Dense 3D samples per edge: (t, point), increasing t.
    pub edge_polys: Vec<Vec<(f64, DVec3)>>,
    /// Cycles of non-seam, non-degenerate CAD edges (closed edges, circles
    /// made of arcs): placeholder loops (SPEC §4.1).
    pub cad_loops: Vec<Vec<EdgeId>>,
    pub face_meshes: Vec<FaceMesh>,
    pub bbox: Aabb,
}

fn sample_pcurve(
    k: &dyn CadKernel,
    body: BodyId,
    e: EdgeId,
    f: FaceId,
    rev: bool,
    t0: f64,
    t1: f64,
    tol: f64,
) -> KResult<(Vec<f64>, Vec<DVec2>)> {
    let mut ts: Vec<f64> = (0..=8).map(|i| t0 + (t1 - t0) * i as f64 / 8.0).collect();
    let mut pts = k.edge_pcurve_eval(body, e, f, rev, &ts)?;
    for _round in 0..7 {
        let mids: Vec<f64> = ts.windows(2).map(|w| 0.5 * (w[0] + w[1])).collect();
        let mp = k.edge_pcurve_eval(body, e, f, rev, &mids)?;
        let mut nts = vec![ts[0]];
        let mut npts = vec![pts[0]];
        let mut changed = false;
        for i in 0..mids.len() {
            let (d, _) = super::geom2::dist_point_seg(mp[i], pts[i], pts[i + 1]);
            if d > tol {
                nts.push(mids[i]);
                npts.push(mp[i]);
                changed = true;
            }
            nts.push(ts[i + 1]);
            npts.push(pts[i + 1]);
        }
        ts = nts;
        pts = npts;
        if !changed {
            break;
        }
    }
    // Drop collinear interior points (straight pcurves stay 2 points).
    let mut kt = vec![ts[0]];
    let mut kp = vec![pts[0]];
    for i in 1..pts.len() - 1 {
        let (d, _) = super::geom2::dist_point_seg(pts[i], *kp.last().unwrap(), pts[i + 1]);
        if d > tol * 1e-3 {
            kt.push(ts[i]);
            kp.push(pts[i]);
        }
    }
    kt.push(*ts.last().unwrap());
    kp.push(*pts.last().unwrap());
    Ok((kt, kp))
}

impl BodyCtx {
    pub fn build(kernel: Arc<dyn CadKernel>, kbody: BodyId) -> KResult<BodyCtx> {
        let topo = kernel.topology(kbody)?;
        let k = kernel.as_ref();
        // Edge 3D samples and bbox.
        let mut bbox = Aabb::EMPTY;
        for v in &topo.vertices {
            bbox.add(v.point);
        }
        let mut edge_polys = Vec::with_capacity(topo.edges.len());
        for e in topo.edge_ids() {
            let et = topo.edge(e);
            let n = if et.degenerate || et.curve == CurveKind::Line {
                1
            } else {
                96
            };
            let (t0, t1) = et.t_range;
            let ts: Vec<f64> = (0..=n)
                .map(|i| t0 + (t1 - t0) * i as f64 / n as f64)
                .collect();
            let ps = k.edge_eval(kbody, e, &ts)?;
            for p in &ps {
                bbox.add(*p);
            }
            edge_polys.push(ts.into_iter().zip(ps).collect());
        }
        let size = bbox.diagonal().max(1e-9);
        let mut faces = Vec::with_capacity(topo.faces.len());
        for f in topo.face_ids() {
            let ft = topo.face(f);
            let info = ft.surface.clone();
            let span = DVec2::new(
                info.u_range.1 - info.u_range.0,
                info.v_range.1 - info.v_range.0,
            )
            .abs()
            .max_element()
            .max(1e-12);
            let tol = 2e-4 * span;
            let mut curves = Vec::new();
            let mut has_outer = false;
            let mut seen: Vec<(EdgeId, bool)> = Vec::new();
            for w in &ft.wires {
                for c in &w.coedges {
                    let et = topo.edge(c.edge);
                    if w.outer && !et.seam && !et.degenerate {
                        has_outer = true;
                    }
                    let sel = if et.seam { c.reversed } else { false };
                    if seen.contains(&(c.edge, sel)) {
                        continue;
                    }
                    seen.push((c.edge, sel));
                    let (t0, t1) = et.t_range;
                    let (par, pts) = sample_pcurve(k, kbody, c.edge, f, sel, t0, t1, tol)?;
                    let owner = if et.degenerate {
                        Owner::Pole(c.edge)
                    } else if et.seam {
                        Owner::Seam(0)
                    } else {
                        Owner::Cad(c.edge)
                    };
                    let mut seam = None;
                    let mut owner = owner;
                    if et.seam {
                        let du = (pts[pts.len() - 1].x - pts[0].x).abs();
                        let dir: u8 = if du < 1e-9 * span { 0 } else { 1 };
                        let coord = if dir == 0 { pts[0].x } else { pts[0].y };
                        let lo = if dir == 0 {
                            info.u_range.0
                        } else {
                            info.v_range.0
                        };
                        let hi = if dir == 0 {
                            info.u_range.1
                        } else {
                            info.v_range.1
                        };
                        let copy: u8 = if (coord - lo).abs() <= (coord - hi).abs() {
                            0
                        } else {
                            1
                        };
                        seam = Some((dir, copy));
                        owner = Owner::Seam(dir);
                    }
                    curves.push(BCurve {
                        owner,
                        seam,
                        pts,
                        par,
                        ends: [et.v0, et.v1],
                    });
                }
            }
            let mut lo = DVec2::splat(f64::INFINITY);
            let mut hi = DVec2::splat(f64::NEG_INFINITY);
            for c in &curves {
                for p in &c.pts {
                    lo = lo.min(*p);
                    hi = hi.max(*p);
                }
            }
            if !lo.is_finite() {
                lo = DVec2::new(info.u_range.0, info.v_range.0);
                hi = DVec2::new(info.u_range.1, info.v_range.1);
            }
            // Metric.
            let mut samples = Vec::new();
            for i in 0..5 {
                for j in 0..5 {
                    samples.push(DVec2::new(
                        lo.x + (hi.x - lo.x) * (i as f64 + 0.5) / 5.0,
                        lo.y + (hi.y - lo.y) * (j as f64 + 0.5) / 5.0,
                    ));
                }
            }
            let ev = k.face_eval(kbody, f, &samples)?;
            let mut mu = 0.0;
            let mut mv = 0.0;
            for e in &ev {
                mu += e.du.length();
                mv += e.dv.length();
            }
            let metric = DVec2::new(
                (mu / ev.len() as f64).max(1e-9),
                (mv / ev.len() as f64).max(1e-9),
            );
            let uper = if info.u_closed {
                info.u_period.or(Some(hi.x - lo.x))
            } else {
                None
            };
            let vper = if info.v_closed {
                info.v_period.or(Some(hi.y - lo.y))
            } else {
                None
            };
            // Planar ring detection.
            let mut ring = None;
            if info.kind == SurfaceKind::Plane && ft.wires.len() == 2 {
                let circ = ft.wires.iter().all(|w| {
                    w.coedges
                        .iter()
                        .all(|c| topo.edge(c.edge).curve == CurveKind::Circle)
                });
                if circ {
                    let centre = |wi: usize| {
                        let mut lo = DVec2::splat(f64::INFINITY);
                        let mut hi = DVec2::splat(f64::NEG_INFINITY);
                        for c in &ft.wires[wi].coedges {
                            for cv in curves.iter().filter(|b| b.owner == Owner::Cad(c.edge)) {
                                for p in &cv.pts {
                                    lo = lo.min(*p);
                                    hi = hi.max(*p);
                                }
                            }
                        }
                        (lo + hi) * 0.5
                    };
                    let (c0, c1) = (centre(0), centre(1));
                    if (c0 - c1).length() < 1e-4 * span {
                        ring = Some((c0 + c1) * 0.5);
                    }
                }
            }
            faces.push(FaceGeom {
                id: f,
                info,
                reversed: ft.reversed,
                uper,
                vper,
                lo,
                hi,
                curves,
                has_outer,
                metric,
                ring,
                eps: 1e-7 * size,
            });
        }
        // CAD loops: maximal runs of non-seam, non-degenerate coedges in a
        // wire that close on themselves (closed edges, circles made of arcs).
        let mut cad_loops: Vec<Vec<EdgeId>> = Vec::new();
        for f in topo.face_ids() {
            for w in &topo.face(f).wires {
                let n = w.coedges.len();
                let real = |c: &newtopo_kernel::CoEdge| {
                    let et = topo.edge(c.edge);
                    !et.seam && !et.degenerate
                };
                // Start right after a non-real coedge (or at 0 if all real).
                let start = (0..n)
                    .find(|&i| !real(&w.coedges[i]))
                    .map(|i| (i + 1) % n)
                    .unwrap_or(0);
                let mut run: Vec<newtopo_kernel::CoEdge> = Vec::new();
                let mut runs: Vec<Vec<newtopo_kernel::CoEdge>> = Vec::new();
                for k in 0..n {
                    let c = w.coedges[(start + k) % n];
                    if real(&c) {
                        run.push(c);
                    } else if !run.is_empty() {
                        runs.push(std::mem::take(&mut run));
                    }
                }
                if !run.is_empty() {
                    runs.push(run);
                }
                for r in runs {
                    let first = topo.edge(r[0].edge);
                    let last = topo.edge(r[r.len() - 1].edge);
                    let s = if r[0].reversed { first.v1 } else { first.v0 };
                    let e = if r[r.len() - 1].reversed {
                        last.v0
                    } else {
                        last.v1
                    };
                    if s.is_some() && s == e {
                        let mut ids: Vec<EdgeId> = r.iter().map(|c| c.edge).collect();
                        ids.sort();
                        ids.dedup();
                        if !cad_loops.contains(&ids) {
                            cad_loops.push(ids);
                        }
                    }
                }
            }
        }
        let tol = 1e-3 * size;
        let face_meshes = topo
            .face_ids()
            .map(|f| k.tessellate(kbody, f, tol))
            .collect::<KResult<Vec<_>>>()?;
        Ok(BodyCtx {
            kernel,
            kbody,
            topo,
            faces,
            size,
            edge_polys,
            cad_loops,
            face_meshes,
            bbox,
        })
    }

    pub fn face(&self, f: FaceId) -> &FaceGeom {
        &self.faces[f.idx()]
    }

    /// 3D polyline of an edge (for drawing).
    pub fn edge_polyline(&self, e: EdgeId) -> Vec<DVec3> {
        self.edge_polys[e.idx()].iter().map(|(_, p)| *p).collect()
    }

    pub fn face_eval(&self, f: FaceId, uv: &[DVec2]) -> Vec<newtopo_kernel::SurfEval> {
        self.kernel.face_eval(self.kbody, f, uv).unwrap_or_default()
    }

    pub fn point(&self, f: FaceId, uv: DVec2) -> DVec3 {
        self.face_eval(f, &[uv])
            .first()
            .map(|e| e.p)
            .unwrap_or(DVec3::ZERO)
    }

    pub fn edge_point(&self, e: EdgeId, t: f64) -> DVec3 {
        self.kernel
            .edge_eval(self.kbody, e, &[t])
            .ok()
            .and_then(|v| v.first().copied())
            .unwrap_or(DVec3::ZERO)
    }
}
