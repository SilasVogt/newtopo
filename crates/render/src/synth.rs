//! Synthetic CAD-like test model for renderer tests, UI development and
//! screenshots while the engine is not available. Purely analytic: a
//! rounded base plate with a filleted, chamfered boss, plus a torus.
//!
//! Each face carries a fine display tessellation, its CAD boundary edges,
//! some "generated" lines (as Connect/Grid would make) and a coarse quad
//! "output mesh" for preview mode.

use crate::pick::{FaceRef, PickMesh};
use crate::scene::{MeshVertex, OverlayColors, RenderScene, with_alpha};
use glam::{Vec2, Vec3};
use std::f32::consts::{FRAC_PI_2, PI, TAU};

#[derive(Clone, Debug, Default)]
pub struct SynthFace {
    pub id: u32,
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub tris: Vec<[u32; 3]>,
    /// Coarse output polygons (quads or triangles).
    pub polys: Vec<Vec<Vec3>>,
    pub poly_normals: Vec<Vec3>,
    /// Generated lines on this face.
    pub lines: Vec<Vec<Vec3>>,
    pub surface: &'static str,
}

#[derive(Clone, Debug, Default)]
pub struct SynthBody {
    pub id: u32,
    pub name: String,
    pub layer: String,
    pub color: [f32; 4],
    pub faces: Vec<SynthFace>,
    pub cad_edges: Vec<Vec<Vec3>>,
}

impl SynthBody {
    pub fn bounds(&self) -> (Vec3, Vec3) {
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for f in &self.faces {
            for p in &f.positions {
                min = min.min(*p);
                max = max.max(*p);
            }
        }
        (min, max)
    }

    /// Unique output vertices (rounded to 1e-3) for vertex-mode display.
    pub fn output_vertices(&self) -> Vec<Vec3> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for f in &self.faces {
            for poly in &f.polys {
                for p in poly {
                    let k = (
                        (p.x * 1000.0).round() as i64,
                        (p.y * 1000.0).round() as i64,
                        (p.z * 1000.0).round() as i64,
                    );
                    if seen.insert(k) {
                        out.push(*p);
                    }
                }
            }
        }
        out
    }
}

type SurfFn = dyn Fn(f32, f32) -> (Vec3, Vec3);

struct ParamFace<'a> {
    f: &'a SurfFn,
    fine: (u32, u32),
    coarse: (u32, u32),
    /// CAD edges on the boundaries u=0, u=1, v=0, v=1.
    edges: [bool; 4],
    iso_u: Vec<f32>,
    iso_v: Vec<f32>,
    surface: &'static str,
}

fn build_face(id: u32, pf: &ParamFace, cad_edges: &mut Vec<Vec<Vec3>>) -> SynthFace {
    let (nu, nv) = pf.fine;
    let mut face = SynthFace {
        id,
        surface: pf.surface,
        ..Default::default()
    };
    for j in 0..=nv {
        for i in 0..=nu {
            let (p, n) = (pf.f)(i as f32 / nu as f32, j as f32 / nv as f32);
            face.positions.push(p);
            face.normals.push(n);
        }
    }
    for j in 0..nv {
        for i in 0..nu {
            let a = j * (nu + 1) + i;
            face.tris.push([a, a + 1, a + nu + 2]);
            face.tris.push([a, a + nu + 2, a + nu + 1]);
        }
    }
    let sample_u = |u: f32, n: u32| {
        (0..=n)
            .map(|j| (pf.f)(u, j as f32 / n as f32).0)
            .collect::<Vec<_>>()
    };
    let sample_v = |v: f32, n: u32| {
        (0..=n)
            .map(|i| (pf.f)(i as f32 / n as f32, v).0)
            .collect::<Vec<_>>()
    };
    if pf.edges[0] {
        cad_edges.push(sample_u(0.0, nv));
    }
    if pf.edges[1] {
        cad_edges.push(sample_u(1.0, nv));
    }
    if pf.edges[2] {
        cad_edges.push(sample_v(0.0, nu));
    }
    if pf.edges[3] {
        cad_edges.push(sample_v(1.0, nu));
    }
    for &u in &pf.iso_u {
        face.lines.push(sample_u(u, nv));
    }
    for &v in &pf.iso_v {
        face.lines.push(sample_v(v, nu));
    }
    let (cu, cv) = pf.coarse;
    for j in 0..cv {
        for i in 0..cu {
            let q = [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)]
                .map(|(a, b)| (pf.f)(a as f32 / cu as f32, b as f32 / cv as f32));
            let mut poly: Vec<Vec3> = Vec::new();
            for (p, _) in q {
                if poly.last().is_none_or(|l: &Vec3| l.distance(p) > 1e-4)
                    && poly.first().is_none_or(|f| f.distance(p) > 1e-4)
                {
                    poly.push(p);
                }
            }
            if poly.len() >= 3 {
                let n = q
                    .iter()
                    .fold(Vec3::ZERO, |acc, (_, n)| acc + *n)
                    .normalize_or_zero();
                face.polys.push(poly);
                face.poly_normals.push(n);
            }
        }
    }
    face
}

/// Point on a rounded rectangle outline, parameter t in [0, 1) by arc length.
fn rounded_rect(t: f32, hx: f32, hy: f32, r: f32) -> (Vec2, Vec2) {
    let sx = 2.0 * (hx - r);
    let sy = 2.0 * (hy - r);
    let arc = FRAC_PI_2 * r;
    let total = 2.0 * (sx + sy) + 4.0 * arc;
    let mut d = t.rem_euclid(1.0) * total;
    // Start at the middle of the +x side going counter-clockwise.
    type Seg = Box<dyn Fn(f32) -> (Vec2, Vec2)>;
    let segs: [(f32, Seg); 8] = [
        (sy * 0.5, Box::new(move |s| (Vec2::new(hx, s), Vec2::X))),
        (
            arc,
            Box::new(move |s| corner(Vec2::new(hx - r, hy - r), r, s / r)),
        ),
        (sx, Box::new(move |s| (Vec2::new(hx - r - s, hy), Vec2::Y))),
        (
            arc,
            Box::new(move |s| corner(Vec2::new(-hx + r, hy - r), r, FRAC_PI_2 + s / r)),
        ),
        (
            sy,
            Box::new(move |s| (Vec2::new(-hx, hy - r - s), Vec2::NEG_X)),
        ),
        (
            arc,
            Box::new(move |s| corner(Vec2::new(-hx + r, -hy + r), r, PI + s / r)),
        ),
        (
            sx,
            Box::new(move |s| (Vec2::new(-hx + r + s, -hy), Vec2::NEG_Y)),
        ),
        (
            arc + sy * 0.5,
            Box::new(move |s| {
                if s < arc {
                    corner(Vec2::new(hx - r, -hy + r), r, 1.5 * PI + s / r)
                } else {
                    (Vec2::new(hx, -hy + r + (s - arc)), Vec2::X)
                }
            }),
        ),
    ];
    for (len, f) in segs.iter() {
        if d <= *len {
            return f(d);
        }
        d -= len;
    }
    (Vec2::new(hx, 0.0), Vec2::X)
}

fn corner(c: Vec2, r: f32, a: f32) -> (Vec2, Vec2) {
    let n = Vec2::new(a.cos(), a.sin());
    (c + n * r, n)
}

/// The demo scene: body 0 = bracket (plate + filleted boss), body 1 = ring.
pub fn demo_bodies() -> Vec<SynthBody> {
    let mut bodies = Vec::new();

    // ---- body 0: plate with boss
    let (hx, hy, rr, th) = (60.0f32, 40.0f32, 10.0f32, 10.0f32);
    let boss_c = Vec3::new(22.0, 0.0, 0.0);
    let (br, fr, top, ch) = (17.0f32, 6.0f32, 46.0f32, 3.0f32);
    let mut b0 = SynthBody {
        id: 0,
        name: "Bracket".into(),
        layer: "Parts".into(),
        color: [0.62, 0.66, 0.72, 1.0],
        ..Default::default()
    };
    let mut edges = Vec::new();
    let mut faces = Vec::new();
    let top_f = move |u: f32, v: f32| {
        let (p, _) = rounded_rect(u, hx, hy, rr);
        let q = p * v;
        (Vec3::new(q.x, q.y, th), Vec3::Z)
    };
    let bottom_f = move |u: f32, v: f32| {
        let (p, _) = rounded_rect(u, hx, hy, rr);
        let q = p * v;
        (Vec3::new(q.x, q.y, 0.0), Vec3::NEG_Z)
    };
    let side_f = move |u: f32, v: f32| {
        let (p, n) = rounded_rect(u, hx, hy, rr);
        (Vec3::new(p.x, p.y, v * th), n.extend(0.0))
    };
    let plate_u: Vec<f32> = (0..24).map(|i| i as f32 / 24.0).collect();
    faces.push(ParamFace {
        f: &top_f,
        fine: (160, 4),
        coarse: (24, 1),
        edges: [false, false, false, true],
        iso_u: vec![],
        iso_v: vec![],
        surface: "plane",
    });
    faces.push(ParamFace {
        f: &bottom_f,
        fine: (160, 4),
        coarse: (24, 1),
        edges: [false, false, false, true],
        iso_u: vec![],
        iso_v: vec![],
        surface: "plane",
    });
    faces.push(ParamFace {
        f: &side_f,
        fine: (240, 2),
        coarse: (24, 2),
        edges: [false; 4],
        iso_u: plate_u,
        iso_v: vec![0.5],
        surface: "extrusion",
    });
    let fillet_f = move |u: f32, v: f32| {
        let a = u * TAU;
        let phi = v * FRAC_PI_2;
        let radial = br + fr - fr * phi.sin();
        let z = th + fr - fr * phi.cos();
        let dir = Vec3::new(a.cos(), a.sin(), 0.0);
        let n = (dir * phi.sin() + Vec3::Z * phi.cos()).normalize();
        (boss_c + dir * radial + Vec3::Z * z, n)
    };
    let cyl_f = move |u: f32, v: f32| {
        let a = u * TAU;
        let dir = Vec3::new(a.cos(), a.sin(), 0.0);
        let z = th + fr + v * (top - ch - th - fr);
        (boss_c + dir * br + Vec3::Z * z, dir)
    };
    let cham_f = move |u: f32, v: f32| {
        let a = u * TAU;
        let dir = Vec3::new(a.cos(), a.sin(), 0.0);
        let r = br - v * ch;
        let z = top - ch + v * ch;
        (boss_c + dir * r + Vec3::Z * z, (dir + Vec3::Z).normalize())
    };
    let cap_f = move |u: f32, v: f32| {
        let a = u * TAU;
        let dir = Vec3::new(a.cos(), a.sin(), 0.0);
        (boss_c + dir * (br - ch) * v + Vec3::Z * top, Vec3::Z)
    };
    let around: Vec<f32> = (0..16).map(|i| i as f32 / 16.0).collect();
    faces.push(ParamFace {
        f: &fillet_f,
        fine: (96, 10),
        coarse: (16, 2),
        edges: [false, false, true, true],
        iso_u: around.clone(),
        iso_v: vec![0.5],
        surface: "torus",
    });
    faces.push(ParamFace {
        f: &cyl_f,
        fine: (96, 4),
        coarse: (16, 3),
        edges: [false, false, false, true],
        iso_u: around.clone(),
        iso_v: vec![1.0 / 3.0, 2.0 / 3.0],
        surface: "cylinder",
    });
    faces.push(ParamFace {
        f: &cham_f,
        fine: (96, 1),
        coarse: (16, 1),
        edges: [false, false, false, true],
        iso_u: around.clone(),
        iso_v: vec![],
        surface: "cone",
    });
    faces.push(ParamFace {
        f: &cap_f,
        fine: (96, 6),
        coarse: (16, 2),
        edges: [false; 4],
        iso_u: vec![],
        iso_v: vec![0.5],
        surface: "plane",
    });
    for (i, pf) in faces.iter().enumerate() {
        b0.faces.push(build_face(i as u32, pf, &mut edges));
    }
    // Vertical edges where the plate's flat sides meet the corner rounds.
    let (sx, sy) = (2.0 * (hx - rr), 2.0 * (hy - rr));
    let arc = FRAC_PI_2 * rr;
    let total = 2.0 * (sx + sy) + 4.0 * arc;
    let mut d = sy * 0.5;
    for len in [arc, sx, arc, sy, arc, sx, arc] {
        let (p, _) = rounded_rect(d / total, hx, hy, rr);
        edges.push(vec![Vec3::new(p.x, p.y, 0.0), Vec3::new(p.x, p.y, th)]);
        d += len;
    }
    let (p, _) = rounded_rect(d / total, hx, hy, rr);
    edges.push(vec![Vec3::new(p.x, p.y, 0.0), Vec3::new(p.x, p.y, th)]);
    b0.cad_edges = edges;
    bodies.push(b0);

    // ---- body 1: torus ring lying on the plate
    let tc = Vec3::new(-28.0, 4.0, th + 5.0);
    let (tr_major, tr_minor) = (14.0f32, 5.0f32);
    let mut b1 = SynthBody {
        id: 1,
        name: "Ring".into(),
        layer: "Details".into(),
        color: [0.50, 0.62, 0.70, 1.0],
        ..Default::default()
    };
    let torus_f = move |u: f32, v: f32| {
        let a = u * TAU;
        let b = v * TAU;
        let dir = Vec3::new(a.cos(), a.sin(), 0.0);
        let n = dir * b.cos() + Vec3::Z * b.sin();
        (tc + dir * tr_major + n * tr_minor, n)
    };
    let ring_u: Vec<f32> = (0..12).map(|i| i as f32 / 12.0).collect();
    let ring_v: Vec<f32> = (0..8).map(|i| i as f32 / 8.0).collect();
    let mut e1 = Vec::new();
    let pf = ParamFace {
        f: &torus_f,
        fine: (96, 32),
        coarse: (12, 8),
        edges: [false; 4],
        iso_u: ring_u,
        iso_v: ring_v,
        surface: "torus",
    };
    b1.faces.push(build_face(0, &pf, &mut e1));
    b1.cad_edges = e1;
    bodies.push(b1);
    bodies
}

/// Display options for [`build_scene`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SynthDisplay {
    pub preview: bool,
    pub wireframe: bool,
    pub show_lines: bool,
    pub show_vertices: bool,
}

/// Build a render scene from synthetic bodies.
pub fn build_scene(bodies: &[SynthBody], colors: &OverlayColors, d: SynthDisplay) -> RenderScene {
    let mut s = RenderScene::default();
    for b in bodies {
        if d.preview {
            for f in &b.faces {
                for (poly, n) in f.polys.iter().zip(&f.poly_normals) {
                    for k in 1..poly.len() - 1 {
                        s.mesh.push_triangle(
                            [poly[0], poly[k], poly[k + 1]],
                            [*n; 3],
                            colors.preview_surface,
                        );
                    }
                    if d.wireframe {
                        for k in 0..poly.len() {
                            s.add_segment(
                                poly[k],
                                poly[(k + 1) % poly.len()],
                                colors.preview_wire,
                                1.0,
                            );
                        }
                    }
                }
            }
            continue;
        }
        for f in &b.faces {
            let base = s.mesh.vertices.len() as u32;
            for (p, n) in f.positions.iter().zip(&f.normals) {
                s.mesh.vertices.push(MeshVertex {
                    pos: p.to_array(),
                    normal: n.to_array(),
                    color: b.color,
                });
            }
            s.mesh
                .indices
                .extend(f.tris.iter().flat_map(|t| t.map(|i| i + base)));
            if d.show_lines {
                for l in &f.lines {
                    s.add_polyline(l, colors.line, 1.6);
                }
            }
        }
        for e in &b.cad_edges {
            s.add_polyline(e, colors.cad_edge, 1.8);
        }
        if d.show_vertices {
            for p in b.output_vertices() {
                s.add_point(p, colors.vertex, 5.0);
            }
        }
    }
    s
}

/// A pick mesh over the synthetic bodies' display triangles.
pub fn build_pick(bodies: &[SynthBody]) -> PickMesh {
    let mut pos = Vec::new();
    let mut tris = Vec::new();
    let mut ids = Vec::new();
    for b in bodies {
        for f in &b.faces {
            let base = pos.len() as u32;
            pos.extend_from_slice(&f.positions);
            for t in &f.tris {
                tris.push(t.map(|i| i + base));
                ids.push(FaceRef {
                    body: b.id,
                    face: f.id,
                });
            }
        }
    }
    PickMesh::new(pos, tris, ids)
}

/// Selection-style tint mesh for one synthetic face.
pub fn face_tint(b: &SynthBody, face: u32, color: [f32; 4], out: &mut RenderScene) {
    let Some(f) = b.faces.iter().find(|f| f.id == face) else {
        return;
    };
    for t in &f.tris {
        let p = t.map(|i| f.positions[i as usize]);
        let n = t.map(|i| f.normals[i as usize]);
        out.translucent.push_triangle(p, n, color);
    }
}

/// Ground grid lines under a bounding box, in the z = `z` plane.
pub fn ground_grid(min: Vec3, max: Vec3, z: f32, color: [f32; 4], out: &mut RenderScene) {
    let size = (max - min).truncate().max_element().max(1.0);
    let step = 10f32.powf((size / 4.0).log10().floor()).max(1e-3);
    let c = ((min + max) * 0.5).truncate();
    let half = (size * 0.8 / step).ceil() * step;
    let n = (half / step) as i32;
    for i in -n..=n {
        let k = i as f32 * step;
        let major = i % 5 == 0;
        let col = if major {
            with_alpha(color, color[3] * 1.8)
        } else {
            color
        };
        let gx = (c.x / step).round() * step;
        let gy = (c.y / step).round() * step;
        out.add_segment(
            Vec3::new(gx + k, gy - half, z),
            Vec3::new(gx + k, gy + half, z),
            col,
            1.0,
        );
        out.add_segment(
            Vec3::new(gx - half, gy + k, z),
            Vec3::new(gx + half, gy + k, z),
            col,
            1.0,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_is_well_formed() {
        let b = demo_bodies();
        assert_eq!(b.len(), 2);
        for body in &b {
            assert!(!body.faces.is_empty());
            for f in &body.faces {
                assert_eq!(f.positions.len(), f.normals.len());
                assert!(
                    f.tris
                        .iter()
                        .all(|t| t.iter().all(|&i| (i as usize) < f.positions.len()))
                );
                assert!(f.normals.iter().all(|n| (n.length() - 1.0).abs() < 1e-3));
            }
        }
        let s = build_scene(
            &b,
            &OverlayColors::default(),
            SynthDisplay {
                show_lines: true,
                ..Default::default()
            },
        );
        assert!(!s.mesh.is_empty() && !s.lines.is_empty());
        let p = build_pick(&b);
        assert_eq!(
            p.tris.len(),
            b.iter()
                .flat_map(|b| &b.faces)
                .map(|f| f.tris.len())
                .sum::<usize>()
        );
    }

    #[test]
    fn rounded_rect_is_closed_and_continuous() {
        let mut prev = rounded_rect(0.0, 60.0, 40.0, 10.0).0;
        for i in 1..=2000 {
            let p = rounded_rect(i as f32 / 2000.0, 60.0, 40.0, 10.0).0;
            assert!(p.distance(prev) < 0.5, "jump at {i}: {prev:?} -> {p:?}");
            prev = p;
        }
    }
}
