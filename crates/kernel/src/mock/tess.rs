//! Display tessellation of mock faces: boundary curves subdivided adaptively
//! in 3D, plus an interior grid, triangulated with a constrained Delaunay
//! triangulation (spade) in metric-scaled UV, then trimmed by classifying
//! triangle centroids.

use super::geom::{Curve2, Surface};
use super::{MockBody, dist_point_seg};
use crate::{FaceId, FaceMesh, UvClass};
use glam::DVec2;
use spade::{ConstrainedDelaunayTriangulation, Point2, Triangulation};

fn subdivide(surf: &Surface, pc: &Curve2, t0: f64, t1: f64, tol: f64, out: &mut Vec<f64>) {
    // Initial uniform split, then refine where the 3D chord deviates.
    let init = 4;
    let mut stack: Vec<(f64, f64, u32)> = Vec::new();
    for i in (0..init).rev() {
        let a = t0 + (t1 - t0) * i as f64 / init as f64;
        let b = t0 + (t1 - t0) * (i + 1) as f64 / init as f64;
        stack.push((a, b, 0));
    }
    while let Some((a, b, depth)) = stack.pop() {
        let pa = surf.point(pc.eval(a));
        let pb = surf.point(pc.eval(b));
        let m = 0.5 * (a + b);
        let pm = surf.point(pc.eval(m));
        let dev = (pm - (pa + pb) * 0.5).length();
        if dev > tol && depth < 10 {
            stack.push((m, b, depth + 1));
            stack.push((a, m, depth + 1));
        } else {
            out.push(a);
        }
    }
}

fn curvature_radius(surf: &Surface) -> Option<f64> {
    match *surf {
        Surface::Plane { .. } => None,
        Surface::Cylinder { r, .. } | Surface::Sphere { r, .. } => Some(r),
        Surface::Cone { r0, .. } => Some(r0.abs() * 0.5),
        Surface::Torus { minor, .. } => Some(minor),
    }
}

pub fn tessellate(body: &MockBody, f: FaceId, tol: f64) -> FaceMesh {
    let face = &body.faces[f.idx()];
    let surf = &face.surface;
    let info = body.surface_info(f);
    let tol = tol.max(1e-9);

    // Boundary loops, adaptively sampled.
    let mut loops: Vec<Vec<DVec2>> = Vec::new();
    for w in &face.wires {
        let mut pts = Vec::new();
        for c in &w.coedges {
            let ed = &body.edges[c.edge.idx()];
            let Some(pc) = body.pcurve(c.edge, f, c.reversed) else {
                continue;
            };
            let mut ts = Vec::new();
            subdivide(surf, pc, ed.t0, ed.t1, tol, &mut ts);
            if c.reversed {
                // Start at t1 and walk down; drop the final t0 (next coedge starts there).
                let mut rev: Vec<f64> = ts.iter().skip(1).rev().copied().collect();
                rev.insert(0, ed.t1);
                ts = rev;
            }
            pts.extend(ts.into_iter().map(|t| pc.eval(t)));
        }
        if pts.len() >= 2 {
            loops.push(pts);
        }
    }

    // Metric scaling.
    let (u0, u1) = info.u_range;
    let (v0, v1) = info.v_range;
    let mut su = 0.0;
    let mut sv = 0.0;
    let k = 6;
    for i in 0..=k {
        for j in 0..=k {
            let uv = DVec2::new(
                u0 + (u1 - u0) * i as f64 / k as f64,
                v0 + (v1 - v0) * j as f64 / k as f64,
            );
            let (_, du, dv) = surf.eval(uv);
            su += du.length();
            sv += dv.length();
        }
    }
    let n = ((k + 1) * (k + 1)) as f64;
    let su = (su / n).max(1e-9);
    let sv = (sv / n).max(1e-9);

    let mut cdt: ConstrainedDelaunayTriangulation<Point2<f64>> =
        ConstrainedDelaunayTriangulation::new();
    let mut uvs: Vec<DVec2> = Vec::new();
    let insert = |cdt: &mut ConstrainedDelaunayTriangulation<Point2<f64>>,
                  uvs: &mut Vec<DVec2>,
                  p: DVec2| {
        let h = cdt.insert(Point2::new(p.x * su, p.y * sv)).ok()?;
        let i = h.index();
        if i >= uvs.len() {
            uvs.resize(i + 1, p);
        }
        Some(h)
    };
    for l in &loops {
        let hs: Vec<_> = l
            .iter()
            .filter_map(|&p| insert(&mut cdt, &mut uvs, p))
            .collect();
        for i in 0..hs.len() {
            let (a, b) = (hs[i], hs[(i + 1) % hs.len()]);
            if a != b {
                cdt.try_add_constraint(a, b);
            }
        }
    }
    let class_loops = body.face_loops(f);
    // Interior grid on curved faces.
    if let Some(r) = curvature_radius(surf) {
        let h = (8.0 * tol * r).sqrt().max(1e-9);
        let nu = (((u1 - u0) * su / h).ceil() as usize).clamp(1, 200);
        let nv = (((v1 - v0) * sv / h).ceil() as usize).clamp(1, 200);
        let min_d = 0.25 * h;
        for i in 1..nu {
            for j in 1..nv {
                let uv = DVec2::new(
                    u0 + (u1 - u0) * i as f64 / nu as f64,
                    v0 + (v1 - v0) * j as f64 / nv as f64,
                );
                if body.classify_one(&info, &class_loops, uv) != UvClass::In {
                    continue;
                }
                let sp = DVec2::new(uv.x * su, uv.y * sv);
                let near = loops.iter().any(|l| {
                    (0..l.len()).any(|q| {
                        let a = l[q];
                        let b = l[(q + 1) % l.len()];
                        dist_point_seg(
                            sp,
                            DVec2::new(a.x * su, a.y * sv),
                            DVec2::new(b.x * su, b.y * sv),
                        ) < min_d
                    })
                });
                if !near {
                    insert(&mut cdt, &mut uvs, uv);
                }
            }
        }
    }

    let mut mesh = FaceMesh::default();
    let mut remap = vec![u32::MAX; uvs.len()];
    for tri in cdt.inner_faces() {
        let vs = tri.vertices().map(|v| v.fix().index());
        let c = (uvs[vs[0]] + uvs[vs[1]] + uvs[vs[2]]) / 3.0;
        if body.classify_one(&info, &class_loops, c) != UvClass::In {
            continue;
        }
        let mut idx = [0u32; 3];
        for (k, &vi) in vs.iter().enumerate() {
            if remap[vi] == u32::MAX {
                remap[vi] = mesh.positions.len() as u32;
                let uv = uvs[vi];
                let (p, _, _) = surf.eval(uv);
                let mut nrm = surf.normal(uv);
                if face.reversed {
                    nrm = -nrm;
                }
                mesh.positions.push(p);
                mesh.normals.push(nrm);
                mesh.uvs.push(uv);
            }
            idx[k] = remap[vi];
        }
        if face.reversed {
            idx.swap(1, 2);
        }
        mesh.triangles.push(idx);
    }
    mesh
}
