//! Small synthetic output meshes for export, bridge and DCC import tests.
//! Not part of the app's API.

use crate::keys::{LineKey, RuleId, VertexKey};
use crate::mesh::{Anchor, OutEdge, OutEdgeKind, OutPolygon, OutVertex, OutputMesh};
use glam::{DMat4, DVec3};
use newtopo_kernel::{BodyId, EdgeId, FaceId, VertexId};
use std::collections::BTreeMap;

fn vert(i: usize, p: DVec3) -> OutVertex {
    OutVertex {
        key: VertexKey::Cad(VertexId(i as u32)),
        anchor: Anchor::CadVertex {
            vertex: VertexId(i as u32),
        },
        position: p,
    }
}

/// Build a mesh from positions and polygons (with per-corner normals); the
/// edge list is derived from the polygons, with `hard(a, b)` deciding
/// hardness and `cad(a, b)` the edge kind.
pub fn build(
    body: u32,
    pos: &[DVec3],
    polys: Vec<(Vec<u32>, u32, Vec<DVec3>)>,
    hard: impl Fn(u32, u32) -> bool,
    cad: impl Fn(u32, u32) -> bool,
) -> OutputMesh {
    let vertices = pos.iter().enumerate().map(|(i, p)| vert(i, *p)).collect();
    let mut edges: BTreeMap<(u32, u32), ()> = BTreeMap::new();
    for (v, _, _) in &polys {
        for k in 0..v.len() {
            let (a, b) = (v[k], v[(k + 1) % v.len()]);
            edges.insert((a.min(b), a.max(b)), ());
        }
    }
    let edges = edges
        .keys()
        .enumerate()
        .map(|(i, &(a, b))| OutEdge {
            a,
            b,
            kind: if cad(a, b) {
                OutEdgeKind::Cad(EdgeId(i as u32))
            } else {
                OutEdgeKind::Line(LineKey {
                    rule: RuleId(0),
                    index: i as u32,
                    sub: 0,
                })
            },
            key: None,
            hard: hard(a, b),
            path: vec![pos[a as usize], pos[b as usize]],
        })
        .collect();
    let polygons = polys
        .into_iter()
        .map(|(verts, face, normals)| OutPolygon {
            verts,
            face: FaceId(face),
            normals,
            fan_fallback: false,
        })
        .collect();
    OutputMesh {
        body: BodyId(body),
        vertices,
        edges,
        polygons,
    }
}

/// 2 x 1 x 1 box, six quads, all 12 edges hard, exact face normals.
pub fn cube() -> OutputMesh {
    cuboid(DVec3::new(2.0, 1.0, 1.0))
}

/// Axis-aligned box from the origin to `size`, six quads, all edges hard.
pub fn cuboid(size: DVec3) -> OutputMesh {
    let pos: Vec<DVec3> = [
        (0., 0., 0.),
        (2., 0., 0.),
        (2., 1., 0.),
        (0., 1., 0.),
        (0., 0., 1.),
        (2., 0., 1.),
        (2., 1., 1.),
        (0., 1., 1.),
    ]
    .iter()
    .map(|&(x, y, z)| DVec3::new(x / 2.0, y, z) * size)
    .collect();
    let faces: [([u32; 4], DVec3); 6] = [
        ([0, 3, 2, 1], DVec3::NEG_Z),
        ([4, 5, 6, 7], DVec3::Z),
        ([0, 1, 5, 4], DVec3::NEG_Y),
        ([2, 3, 7, 6], DVec3::Y),
        ([0, 4, 7, 3], DVec3::NEG_X),
        ([1, 2, 6, 5], DVec3::X),
    ];
    let polys = faces
        .iter()
        .enumerate()
        .map(|(f, (v, n))| (v.to_vec(), f as u32, vec![*n; 4]))
        .collect();
    build(0, &pos, polys, |_, _| true, |_, _| true)
}

/// Open tube (radius 1, height 1) of `n` quads around Z with smooth radial
/// normals. Two CAD faces (half each) meet tangentially: the edges between
/// them are CAD edges but not hard. Rims are hard CAD edges.
pub fn strip(n: u32) -> OutputMesh {
    let ring = |i: u32| {
        DVec3::from((
            glam::DVec2::from_angle(i as f64 * std::f64::consts::TAU / n as f64),
            0.0,
        ))
    };
    let mut pos: Vec<DVec3> = (0..n).map(ring).collect();
    pos.extend((0..n).map(|i| ring(i) + DVec3::Z));
    let polys = (0..n)
        .map(|i| {
            let j = (i + 1) % n;
            let v = vec![i, j, n + j, n + i];
            let nr = v
                .iter()
                .map(|&k| DVec3::new(pos[k as usize].x, pos[k as usize].y, 0.0).normalize())
                .collect();
            (v, 10 + u32::from(i >= n / 2), nr)
        })
        .collect();
    let rim = move |a: u32, b: u32| (a < n) == (b < n);
    build(1, &pos, polys, rim, move |a, b| {
        rim(a, b) || a % n == 0 || a % n == n / 2 || b % n == 0
    })
}

/// Planar regular n-gon in z = 0 facing +Z (an n-gon export with auto-fill off).
pub fn ngon(n: u32) -> OutputMesh {
    let pos: Vec<DVec3> = (0..n)
        .map(|i| {
            DVec3::from((
                glam::DVec2::from_angle(i as f64 * std::f64::consts::TAU / n as f64),
                0.0,
            ))
        })
        .collect();
    build(
        2,
        &pos,
        vec![((0..n).collect(), 20, vec![DVec3::Z; n as usize])],
        |_, _| true,
        |_, _| true,
    )
}

/// `nx * ny` quad grid on a gentle wave, every 8th grid line hard. Used for
/// the 50k-face bridge test.
pub fn grid(nx: u32, ny: u32) -> OutputMesh {
    let h = |x: f64, y: f64| 0.05 * (x * 0.3).sin() * (y * 0.2).cos();
    let mut pos = Vec::with_capacity(((nx + 1) * (ny + 1)) as usize);
    for j in 0..=ny {
        for i in 0..=nx {
            pos.push(DVec3::new(
                i as f64 * 0.1,
                j as f64 * 0.1,
                h(i as f64, j as f64),
            ));
        }
    }
    let id = |i: u32, j: u32| j * (nx + 1) + i;
    let mut polys = Vec::with_capacity((nx * ny) as usize);
    for j in 0..ny {
        for i in 0..nx {
            let v = vec![id(i, j), id(i + 1, j), id(i + 1, j + 1), id(i, j + 1)];
            polys.push((v, 0, vec![DVec3::Z; 4]));
        }
    }
    let w = nx + 1;
    let hard = move |a: u32, b: u32| {
        let (ia, ja, ib, jb) = (a % w, a / w, b % w, b / w);
        (ia == ib && ia % 8 == 0) || (ja == jb && ja % 8 == 0)
    };
    build(3, &pos, polys, hard, |_, _| false)
}

/// The three sample objects used by golden and DCC tests.
pub struct SampleScene {
    pub cube: OutputMesh,
    pub strip: OutputMesh,
    pub ngon: OutputMesh,
}

impl SampleScene {
    pub fn new() -> Self {
        SampleScene {
            cube: cube(),
            strip: strip(12),
            ngon: ngon(6),
        }
    }
    /// Cube on layer "Parts" (red, moved and turned 90 degrees about Z),
    /// tube on "Parts" (blue), hexagon on "Misc" (no colour, lifted).
    pub fn objects(&self) -> Vec<super::ExportObject<'_>> {
        vec![
            super::ExportObject {
                name: "Cube",
                layer: "Parts",
                color: Some([0.8, 0.2, 0.2]),
                transform: DMat4::from_rotation_translation(
                    glam::DQuat::from_rotation_z(std::f64::consts::FRAC_PI_2),
                    DVec3::new(5.0, 0.0, 0.0),
                ),
                mesh: &self.cube,
            },
            super::ExportObject {
                name: "Tube",
                layer: "Parts",
                color: Some([0.2, 0.4, 0.8]),
                transform: DMat4::IDENTITY,
                mesh: &self.strip,
            },
            super::ExportObject {
                name: "Hexagon",
                layer: "Misc",
                color: None,
                transform: DMat4::from_translation(DVec3::new(0.0, 0.0, 3.0)),
                mesh: &self.ngon,
            },
        ]
    }
}

impl Default for SampleScene {
    fn default() -> Self {
        Self::new()
    }
}
