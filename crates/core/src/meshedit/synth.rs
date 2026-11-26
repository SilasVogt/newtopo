//! Synthetic meshes with exact normals, and shape metrics, for tests and
//! benchmarks of the mesh tools.

use super::mesh::{EdgeFlags, EditMesh, EditPolygon, FlaggedEdge, ekey};
use super::work::point_triangle_distance;
use crate::fill::triangulate_3d;
use glam::{DVec2, DVec3};
use newtopo_kernel::{BodyId, FaceId};
use std::collections::BTreeMap;
use std::f64::consts::TAU;

struct Builder {
    pos: Vec<DVec3>,
    pinned: Vec<bool>,
    polys: Vec<EditPolygon>,
    flags: BTreeMap<(u32, u32), EdgeFlags>,
    index: BTreeMap<(i64, i64, i64), u32>,
}

impl Builder {
    fn new() -> Builder {
        Builder {
            pos: Vec::new(),
            pinned: Vec::new(),
            polys: Vec::new(),
            flags: BTreeMap::new(),
            index: BTreeMap::new(),
        }
    }
    /// Welded vertex (positions equal to 1e-9 share an index).
    fn v(&mut self, p: DVec3) -> u32 {
        let k = (
            (p.x * 1e9).round() as i64,
            (p.y * 1e9).round() as i64,
            (p.z * 1e9).round() as i64,
        );
        if let Some(&i) = self.index.get(&k) {
            return i;
        }
        self.pos.push(p);
        self.pinned.push(false);
        let i = (self.pos.len() - 1) as u32;
        self.index.insert(k, i);
        i
    }
    fn poly(
        &mut self,
        verts: Vec<u32>,
        group: u32,
        nrm: impl Fn(DVec3) -> DVec3,
        uvs: Option<Vec<DVec2>>,
    ) {
        let normals = verts.iter().map(|&v| nrm(self.pos[v as usize])).collect();
        self.polys.push(EditPolygon {
            verts,
            group: FaceId(group),
            normals,
            uvs,
        });
    }
    fn flag(&mut self, a: u32, b: u32, f: EdgeFlags) {
        let e = self.flags.entry(ekey(a, b)).or_default();
        *e = e.union(f);
    }
    fn finish(self) -> EditMesh {
        EditMesh {
            body: BodyId(0),
            positions: self.pos,
            pinned: self.pinned,
            polygons: self.polys,
            flagged: self
                .flags
                .into_iter()
                .map(|((a, b), flags)| FlaggedEdge { a, b, flags })
                .collect(),
        }
    }
}

const CAD_HARD: EdgeFlags = EdgeFlags {
    cad: true,
    hard: true,
    seam: false,
};

/// Height field `z = f(x, y)` over `[0, size]²` as an `nx × ny` quad grid
/// (open boundary), with normals from the numerical gradient.
pub fn heightfield(nx: usize, ny: usize, size: f64, f: impl Fn(f64, f64) -> f64) -> EditMesh {
    let mut b = Builder::new();
    let mut id = vec![vec![0u32; ny + 1]; nx + 1];
    for (i, row) in id.iter_mut().enumerate() {
        for (j, slot) in row.iter_mut().enumerate() {
            let (x, y) = (size * i as f64 / nx as f64, size * j as f64 / ny as f64);
            *slot = b.v(DVec3::new(x, y, f(x, y)));
        }
    }
    let h = 1e-6 * size;
    let nrm = |p: DVec3| {
        let dx = (f(p.x + h, p.y) - f(p.x - h, p.y)) / (2.0 * h);
        let dy = (f(p.x, p.y + h) - f(p.x, p.y - h)) / (2.0 * h);
        DVec3::new(-dx, -dy, 1.0).normalize()
    };
    for i in 0..nx {
        for j in 0..ny {
            let q = vec![id[i][j], id[i + 1][j], id[i + 1][j + 1], id[i][j + 1]];
            let uv = q
                .iter()
                .map(|&v| {
                    let p = b.pos[v as usize];
                    DVec2::new(p.x / size, p.y / size)
                })
                .collect();
            b.poly(q, 0, nrm, Some(uv));
        }
    }
    b.finish()
}

/// Flat square plate `[0, size]²` as an `n × n` quad grid.
pub fn plane(n: usize, size: f64) -> EditMesh {
    heightfield(n, n, size, |_, _| 0.0)
}

/// Axis-aligned cube `[-s, s]³`, each side an `n × n` quad grid, sides as
/// separate CAD faces, cube edges CAD + hard, corners pinned.
pub fn cube(n: usize, s: f64) -> EditMesh {
    let mut b = Builder::new();
    let axes = [
        (DVec3::X, DVec3::Y, DVec3::Z),
        (-DVec3::X, DVec3::Z, DVec3::Y),
        (DVec3::Y, DVec3::Z, DVec3::X),
        (-DVec3::Y, DVec3::X, DVec3::Z),
        (DVec3::Z, DVec3::X, DVec3::Y),
        (-DVec3::Z, DVec3::Y, DVec3::X),
    ];
    for (g, &(nrm, u, v)) in axes.iter().enumerate() {
        // Orient so u × v = nrm.
        let (u, v) = if u.cross(v).dot(nrm) > 0.0 {
            (u, v)
        } else {
            (v, u)
        };
        let mut id = vec![vec![0u32; n + 1]; n + 1];
        for (i, row) in id.iter_mut().enumerate() {
            for (j, slot) in row.iter_mut().enumerate() {
                let a = -s + 2.0 * s * i as f64 / n as f64;
                let c = -s + 2.0 * s * j as f64 / n as f64;
                *slot = b.v(nrm * s + u * a + v * c);
            }
        }
        for i in 0..n {
            for j in 0..n {
                let q = vec![id[i][j], id[i + 1][j], id[i + 1][j + 1], id[i][j + 1]];
                b.poly(q, g as u32, |_| nrm, None);
            }
        }
    }
    // Cube edges: both endpoints have two coordinates at ±s.
    let polys = b.polys.clone();
    for p in &polys {
        let m = p.verts.len();
        for k in 0..m {
            let (x, y) = (p.verts[k], p.verts[(k + 1) % m]);
            let on = |q: DVec3| {
                [q.x, q.y, q.z]
                    .iter()
                    .filter(|c| (c.abs() - s).abs() < 1e-9)
                    .count()
            };
            let (px, py) = (b.pos[x as usize], b.pos[y as usize]);
            if on(px) >= 2 && on(py) >= 2 {
                // Same edge line: they share the two extreme coordinates.
                let shared = [0, 1, 2]
                    .iter()
                    .filter(|&&c| (px[c].abs() - s).abs() < 1e-9 && (px[c] - py[c]).abs() < 1e-9)
                    .count();
                if shared >= 2 {
                    b.flag(x, y, CAD_HARD);
                }
            }
        }
    }
    for i in 0..b.pos.len() {
        let q = b.pos[i];
        if [q.x, q.y, q.z].iter().all(|c| (c.abs() - s).abs() < 1e-9) {
            b.pinned[i] = true;
        }
    }
    b.finish()
}

/// Closed cylinder of radius `r`, height `h` (z from 0 to h): `segs`
/// around, `rings` quad rows on the side, caps as triangle fans around a
/// centre vertex. Rims are CAD + hard edges. Side corners carry UVs
/// (u around, v up) and the cut at angle 0 is a UV seam; caps get planar
/// UVs in their own charts.
pub fn cylinder(segs: usize, rings: usize, r: f64, h: f64) -> EditMesh {
    let mut b = Builder::new();
    let ring = |b: &mut Builder, k: usize| -> Vec<u32> {
        let z = h * k as f64 / rings as f64;
        (0..segs)
            .map(|i| {
                let a = TAU * i as f64 / segs as f64;
                b.v(DVec3::new(r * a.cos(), r * a.sin(), z))
            })
            .collect()
    };
    let rows: Vec<Vec<u32>> = (0..=rings).map(|k| ring(&mut b, k)).collect();
    let side = |p: DVec3| DVec3::new(p.x, p.y, 0.0).normalize();
    for k in 0..rings {
        for i in 0..segs {
            let j = (i + 1) % segs;
            let q = vec![rows[k][i], rows[k][j], rows[k + 1][j], rows[k + 1][i]];
            let (u0, u1) = (i as f64 / segs as f64, (i + 1) as f64 / segs as f64);
            let (v0, v1) = (k as f64 / rings as f64, (k + 1) as f64 / rings as f64);
            let uv = vec![
                DVec2::new(u0, v0),
                DVec2::new(u1, v0),
                DVec2::new(u1, v1),
                DVec2::new(u0, v1),
            ];
            b.poly(q, 0, side, Some(uv));
        }
    }
    let cap_uv = |p: DVec3| DVec2::new(0.5 + p.x / (2.0 * r), 0.5 + p.y / (2.0 * r));
    let c0 = b.v(DVec3::new(0.0, 0.0, 0.0));
    let c1 = b.v(DVec3::new(0.0, 0.0, h));
    for i in 0..segs {
        let j = (i + 1) % segs;
        let t = vec![c0, rows[0][j], rows[0][i]];
        let uv = t.iter().map(|&v| cap_uv(b.pos[v as usize])).collect();
        b.poly(t, 1, |_| -DVec3::Z, Some(uv));
        let t = vec![c1, rows[rings][i], rows[rings][j]];
        let uv = t.iter().map(|&v| cap_uv(b.pos[v as usize])).collect();
        b.poly(t, 2, |_| DVec3::Z, Some(uv));
        b.flag(rows[0][i], rows[0][j], CAD_HARD);
        b.flag(rows[rings][i], rows[rings][j], CAD_HARD);
    }
    b.finish()
}

/// UV sphere: `segs` around, `rings` bands, quads with triangle fans at the
/// poles (poles pinned), one CAD face, no feature edges.
pub fn sphere(segs: usize, rings: usize, r: f64) -> EditMesh {
    let mut b = Builder::new();
    let south = b.v(DVec3::new(0.0, 0.0, -r));
    let mut rows = Vec::new();
    for k in 1..rings {
        let phi = std::f64::consts::PI * k as f64 / rings as f64;
        let row: Vec<u32> = (0..segs)
            .map(|i| {
                let a = TAU * i as f64 / segs as f64;
                b.v(DVec3::new(
                    r * phi.sin() * a.cos(),
                    r * phi.sin() * a.sin(),
                    -r * phi.cos(),
                ))
            })
            .collect();
        rows.push(row);
    }
    let north = b.v(DVec3::new(0.0, 0.0, r));
    b.pinned[south as usize] = true;
    b.pinned[north as usize] = true;
    let nrm = |p: DVec3| p.normalize();
    for i in 0..segs {
        let j = (i + 1) % segs;
        b.poly(vec![south, rows[0][j], rows[0][i]], 0, nrm, None);
        let l = rows.len() - 1;
        b.poly(vec![north, rows[l][i], rows[l][j]], 0, nrm, None);
        for k in 0..l {
            b.poly(
                vec![rows[k][i], rows[k][j], rows[k + 1][j], rows[k + 1][i]],
                0,
                nrm,
                None,
            );
        }
    }
    b.finish()
}

/// Torus with major radius `big`, minor radius `small`, all quads.
pub fn torus(nu: usize, nv: usize, big: f64, small: f64) -> EditMesh {
    let mut b = Builder::new();
    let p = |i: usize, j: usize| {
        let (u, v) = (TAU * i as f64 / nu as f64, TAU * j as f64 / nv as f64);
        DVec3::new(
            (big + small * v.cos()) * u.cos(),
            (big + small * v.cos()) * u.sin(),
            small * v.sin(),
        )
    };
    let id: Vec<Vec<u32>> = (0..nu)
        .map(|i| (0..nv).map(|j| b.v(p(i, j))).collect())
        .collect();
    let nrm = move |q: DVec3| {
        let c = DVec3::new(q.x, q.y, 0.0).normalize() * big;
        (q - c).normalize()
    };
    for i in 0..nu {
        for j in 0..nv {
            let (i1, j1) = ((i + 1) % nu, (j + 1) % nv);
            b.poly(
                vec![id[i][j], id[i1][j], id[i1][j1], id[i][j1]],
                0,
                nrm,
                None,
            );
        }
    }
    b.finish()
}

// ------------------------------------------------------------------ metrics

/// Triangles of every polygon (export triangulation).
pub fn triangles(m: &EditMesh) -> Vec<[DVec3; 3]> {
    let mut out = Vec::new();
    for i in 0..m.polygons.len() {
        let pos = m.polygon_positions(i);
        for t in triangulate_3d(&pos) {
            out.push([pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]]);
        }
    }
    out
}

/// Sample points: vertices, edge midpoints and triangle centroids plus
/// points at a third of the way to each corner.
fn samples(m: &EditMesh) -> Vec<DVec3> {
    let mut out: Vec<DVec3> = Vec::new();
    for p in &m.polygons {
        for &v in &p.verts {
            out.push(m.positions[v as usize]);
        }
    }
    for t in triangles(m) {
        let c = (t[0] + t[1] + t[2]) / 3.0;
        out.push(c);
        for k in 0..3 {
            out.push((t[k] + t[(k + 1) % 3]) * 0.5);
            out.push(c.lerp(t[k], 0.5));
        }
    }
    out
}

/// Largest distance from a sample of `a` to the surface of `b`.
pub fn one_sided_distance(a: &EditMesh, b: &EditMesh) -> f64 {
    let tb = triangles(b);
    let mut worst: f64 = 0.0;
    for p in samples(a) {
        let d = tb
            .iter()
            .map(|t| point_triangle_distance(p, t[0], t[1], t[2]))
            .fold(f64::INFINITY, f64::min);
        worst = worst.max(d);
    }
    worst
}

/// Symmetric sampled Hausdorff distance.
pub fn hausdorff(a: &EditMesh, b: &EditMesh) -> f64 {
    one_sided_distance(a, b).max(one_sided_distance(b, a))
}

/// Largest distance from a baked vertex of `a` to the surface of `b`.
pub fn vertex_deviation(a: &EditMesh, b: &EditMesh) -> f64 {
    let tb = triangles(b);
    let mut used = vec![false; a.positions.len()];
    for p in &a.polygons {
        for &v in &p.verts {
            used[v as usize] = true;
        }
    }
    let mut worst: f64 = 0.0;
    for (i, p) in a.positions.iter().enumerate() {
        if !used[i] {
            continue;
        }
        let d = tb
            .iter()
            .map(|t| point_triangle_distance(*p, t[0], t[1], t[2]))
            .fold(f64::INFINITY, f64::min);
        worst = worst.max(d);
    }
    worst
}
