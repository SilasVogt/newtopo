//! Shared export preparation (SPEC §8.3): transforms, axis conversion,
//! normals split at hard edges, edge list with hard flags, smoothing groups,
//! optional triangulation, materials. Every writer consumes [`Prepared`].

use super::{ExportObject, ExportOptions, ExportUp};
use crate::mesh::OutputMesh;
use glam::{DMat3, DMat4, DQuat, DVec3};
use std::collections::HashMap;

/// How the body transform reaches the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransformMode {
    /// Bake the full transform into the positions (OBJ).
    Bake,
    /// Keep it as a node transform when it is a clean TRS with positive
    /// scale (glTF, FBX, bridge); bake otherwise.
    Node,
    /// Body space, no axis conversion or scale (bridge payload: the host
    /// applies `transform` itself).
    Body,
}

/// Node transform in export space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Trs {
    pub translation: DVec3,
    pub rotation: DQuat,
    pub scale: DVec3,
}

impl Trs {
    pub const IDENTITY: Trs = Trs {
        translation: DVec3::ZERO,
        rotation: DQuat::IDENTITY,
        scale: DVec3::ONE,
    };
    pub fn matrix(&self) -> DMat4 {
        DMat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
}

/// One undirected edge of the prepared polygons, in first-use order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PEdge {
    pub a: u32,
    pub b: u32,
    pub hard: bool,
    /// Polygon and corner where the edge first appears (corner -> next).
    pub poly: u32,
    pub corner: u32,
}

/// A material made from a CAD colour.
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    pub name: String,
    pub color: [f32; 3],
}

/// One object ready for a writer.
#[derive(Clone, Debug, PartialEq)]
pub struct Prepared {
    pub name: String,
    pub layer: String,
    /// Index into the material list.
    pub material: Option<usize>,
    pub node: Trs,
    /// Positions in node-local export space.
    pub positions: Vec<DVec3>,
    pub polys: Vec<Vec<u32>>,
    /// Unit normal per polygon corner, node-local export space.
    pub normals: Vec<Vec<DVec3>>,
    pub edges: Vec<PEdge>,
    /// Smoothing group per polygon (0 = off).
    pub smoothing: Vec<u32>,
}

impl Prepared {
    pub fn corner_count(&self) -> usize {
        self.polys.iter().map(Vec::len).sum()
    }
}

/// Axis conversion from the Z-up scene to the export up axis.
/// Y-up: (x, y, z) -> (x, z, -y), a -90 degree rotation about X.
pub fn axis_matrix(up: ExportUp) -> DMat3 {
    match up {
        ExportUp::Z => DMat3::IDENTITY,
        ExportUp::Y => DMat3::from_cols(
            DVec3::new(1.0, 0.0, 0.0),
            DVec3::new(0.0, 0.0, -1.0),
            DVec3::new(0.0, 1.0, 0.0),
        ),
    }
}

/// Material name for a colour: `ntopo_rrggbb`.
pub fn material_name(c: [f32; 3]) -> String {
    let b = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("ntopo_{:02x}{:02x}{:02x}", b(c[0]), b(c[1]), b(c[2]))
}

/// Materials for all objects (deduplicated by name, first-use order) and
/// each object's material index.
pub fn materials(
    objects: &[ExportObject],
    opts: &ExportOptions,
) -> (Vec<Material>, Vec<Option<usize>>) {
    let mut mats: Vec<Material> = Vec::new();
    let idx = objects
        .iter()
        .map(|o| {
            let c = o.color.filter(|_| opts.materials)?;
            let name = material_name(c);
            Some(match mats.iter().position(|m| m.name == name) {
                Some(i) => i,
                None => {
                    mats.push(Material { name, color: c });
                    mats.len() - 1
                }
            })
        })
        .collect();
    (mats, idx)
}

/// Layer names in first-use order.
pub fn layers(objects: &[Prepared]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for o in objects {
        if !out.contains(&o.layer) {
            out.push(o.layer.clone());
        }
    }
    out
}

/// Prepare all objects.
pub fn prepare_all(
    objects: &[ExportObject],
    opts: &ExportOptions,
    mode: TransformMode,
    triangulate: bool,
) -> (Vec<Prepared>, Vec<Material>) {
    let (mats, idx) = materials(objects, opts);
    let out = objects
        .iter()
        .zip(idx)
        .map(|(o, m)| prepare(o, opts, mode, triangulate, m))
        .collect();
    (out, mats)
}

struct Dsu(Vec<u32>);
impl Dsu {
    fn find(&mut self, mut x: u32) -> u32 {
        while self.0[x as usize] != x {
            let p = self.0[self.0[x as usize] as usize];
            self.0[x as usize] = p;
            x = p;
        }
        x
    }
    fn union(&mut self, a: u32, b: u32) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            let (lo, hi) = if a < b { (a, b) } else { (b, a) };
            self.0[hi as usize] = lo;
        }
    }
}

/// Unnormalised Newell vector (length = 2 * area) of a polygon.
fn newell(pos: &[DVec3], poly: &[u32]) -> DVec3 {
    let mut s = DVec3::ZERO;
    for k in 0..poly.len() {
        let a = pos[poly[k] as usize];
        let b = pos[poly[(k + 1) % poly.len()] as usize];
        s.x += (a.y - b.y) * (a.z + b.z);
        s.y += (a.z - b.z) * (a.x + b.x);
        s.z += (a.x - b.x) * (a.y + b.y);
    }
    s
}

#[inline]
fn ekey(a: u32, b: u32) -> (u32, u32) {
    if a < b { (a, b) } else { (b, a) }
}

/// Smooth corner normals in body space: corners around a vertex that are
/// connected across non-hard edges form one smooth fan and share a normal.
/// Exact mode averages the distinct CAD-face normals in the fan (identical
/// across tangent-continuous faces); face-weighted mode sums area-weighted
/// polygon normals.
pub fn smooth_normals(
    mesh: &OutputMesh,
    polys: &[Vec<u32>],
    src_normals: &[Vec<DVec3>],
    faces: &[u32],
    hard: &HashMap<(u32, u32), bool>,
    face_weighted: bool,
) -> Vec<Vec<DVec3>> {
    let pos: Vec<DVec3> = mesh.vertices.iter().map(|v| v.position).collect();
    let mut offset = Vec::with_capacity(polys.len());
    let mut total = 0u32;
    for p in polys {
        offset.push(total);
        total += p.len() as u32;
    }
    let mut dsu = Dsu((0..total).collect());
    // edge -> corners (at min vertex, at max vertex) per use.
    let mut uses: HashMap<(u32, u32), Vec<(u32, u32)>> = HashMap::new();
    for (pi, p) in polys.iter().enumerate() {
        for k in 0..p.len() {
            let k2 = (k + 1) % p.len();
            let (a, b) = (p[k], p[k2]);
            let (ca, cb) = (offset[pi] + k as u32, offset[pi] + k2 as u32);
            let c = if a < b { (ca, cb) } else { (cb, ca) };
            uses.entry(ekey(a, b)).or_default().push(c);
        }
    }
    for (e, list) in &uses {
        if list.len() < 2 || hard.get(e).copied().unwrap_or(false) {
            continue;
        }
        for w in list.windows(2) {
            dsu.union(w[0].0, w[1].0);
            dsu.union(w[0].1, w[1].1);
        }
    }
    let poly_n: Vec<DVec3> = polys.iter().map(|p| newell(&pos, p)).collect();
    // Accumulate per group.
    let mut acc: HashMap<u32, DVec3> = HashMap::new();
    let mut seen_face: HashMap<(u32, u32), ()> = HashMap::new();
    let corner_normal = |pi: usize, k: usize| -> DVec3 {
        src_normals
            .get(pi)
            .and_then(|n| n.get(k))
            .copied()
            .filter(|n| n.is_finite() && n.length_squared() > 0.0)
            .map(|n| n.normalize())
            .unwrap_or_else(|| poly_n[pi].normalize_or_zero())
    };
    for (pi, p) in polys.iter().enumerate() {
        for k in 0..p.len() {
            let g = dsu.find(offset[pi] + k as u32);
            let add = if face_weighted {
                poly_n[pi]
            } else {
                // One contribution per CAD face per fan.
                if seen_face.insert((g, faces[pi]), ()).is_some() {
                    continue;
                }
                corner_normal(pi, k)
            };
            *acc.entry(g).or_insert(DVec3::ZERO) += add;
        }
    }
    polys
        .iter()
        .enumerate()
        .map(|(pi, p)| {
            (0..p.len())
                .map(|k| {
                    let g = dsu.find(offset[pi] + k as u32);
                    let n = acc[&g];
                    if n.length_squared() > 1e-30 {
                        n.normalize()
                    } else {
                        corner_normal(pi, k)
                    }
                })
                .collect()
        })
        .collect()
}

/// Smoothing groups: connected components of polygons across non-hard
/// edges; isolated polygons with no smooth neighbour get 0 ("off").
pub fn smoothing_groups(polys: &[Vec<u32>], edges: &[PEdge]) -> Vec<u32> {
    let mut dsu = Dsu((0..polys.len() as u32).collect());
    let hard: HashMap<(u32, u32), bool> = edges.iter().map(|e| (ekey(e.a, e.b), e.hard)).collect();
    let mut first: HashMap<(u32, u32), u32> = HashMap::new();
    let mut has_neighbour = vec![false; polys.len()];
    for (pi, p) in polys.iter().enumerate() {
        for k in 0..p.len() {
            let e = ekey(p[k], p[(k + 1) % p.len()]);
            if hard.get(&e).copied().unwrap_or(false) {
                continue;
            }
            match first.get(&e) {
                Some(&q) => {
                    dsu.union(q, pi as u32);
                    has_neighbour[q as usize] = true;
                    has_neighbour[pi] = true;
                }
                None => {
                    first.insert(e, pi as u32);
                }
            }
        }
    }
    let mut ids: HashMap<u32, u32> = HashMap::new();
    (0..polys.len())
        .map(|pi| {
            if !has_neighbour[pi] {
                return 0;
            }
            let r = dsu.find(pi as u32);
            let next = ids.len() as u32 + 1;
            *ids.entry(r).or_insert(next)
        })
        .collect()
}

/// Unique edges of `polys` in first-use order with hard flags.
pub fn edge_list(polys: &[Vec<u32>], hard: &HashMap<(u32, u32), bool>) -> Vec<PEdge> {
    let mut seen: HashMap<(u32, u32), ()> = HashMap::new();
    let mut out = Vec::new();
    for (pi, p) in polys.iter().enumerate() {
        for k in 0..p.len() {
            let (a, b) = (p[k], p[(k + 1) % p.len()]);
            let e = ekey(a, b);
            if seen.insert(e, ()).is_none() {
                out.push(PEdge {
                    a: e.0,
                    b: e.1,
                    hard: hard.get(&e).copied().unwrap_or(false),
                    poly: pi as u32,
                    corner: k as u32,
                });
            }
        }
    }
    out
}

/// Decompose `m` into a TRS with strictly positive scale, if exact.
fn clean_trs(m: DMat4) -> Option<(DVec3, DQuat, DVec3)> {
    if !m.is_finite() || m.row(3) != glam::DVec4::new(0.0, 0.0, 0.0, 1.0) || m.determinant() <= 0.0
    {
        return None;
    }
    let (s, r, t) = m.to_scale_rotation_translation();
    let back = DMat4::from_scale_rotation_translation(s, r, t);
    let tol = 1e-9
        * (1.0
            + m.to_cols_array()
                .iter()
                .fold(0.0_f64, |a, x| a.max(x.abs())));
    let ok = back
        .to_cols_array()
        .iter()
        .zip(m.to_cols_array())
        .all(|(a, b)| (a - b).abs() <= tol);
    // Snap last-ulp noise (platform libm differences) to an exact unit scale.
    let s = s.map(|x| if (x - 1.0).abs() < 1e-12 { 1.0 } else { x });
    (ok && s.min_element() > 0.0).then_some((s, r, t))
}

/// Prepare one object.
pub fn prepare(
    obj: &ExportObject,
    opts: &ExportOptions,
    mode: TransformMode,
    triangulate: bool,
    material: Option<usize>,
) -> Prepared {
    let mesh = obj.mesh;
    let hard: HashMap<(u32, u32), bool> = mesh
        .edges
        .iter()
        .map(|e| (ekey(e.a, e.b), e.hard))
        .collect();
    let body_pos: Vec<DVec3> = mesh.vertices.iter().map(|v| v.position).collect();

    // Polygons (validated) and their source normals and faces.
    let mut polys: Vec<Vec<u32>> = Vec::with_capacity(mesh.polygons.len());
    let mut src_n: Vec<Vec<DVec3>> = Vec::with_capacity(mesh.polygons.len());
    let mut faces: Vec<u32> = Vec::with_capacity(mesh.polygons.len());
    for p in &mesh.polygons {
        if p.verts.len() < 3 || p.verts.iter().any(|&v| v as usize >= body_pos.len()) {
            continue;
        }
        let ns = if p.normals.len() == p.verts.len() {
            p.normals.clone()
        } else {
            Vec::new()
        };
        if triangulate && p.verts.len() > 3 {
            let pts: Vec<DVec3> = p.verts.iter().map(|&v| body_pos[v as usize]).collect();
            for t in crate::fill::triangulate_3d(&pts) {
                polys.push(t.iter().map(|&k| p.verts[k as usize]).collect());
                src_n.push(if ns.is_empty() {
                    Vec::new()
                } else {
                    t.iter().map(|&k| ns[k as usize]).collect()
                });
                faces.push(p.face.0);
            }
        } else {
            polys.push(p.verts.clone());
            src_n.push(ns);
            faces.push(p.face.0);
        }
    }
    let mut normals = smooth_normals(
        mesh,
        &polys,
        &src_n,
        &faces,
        &hard,
        opts.face_weighted_normals,
    );

    // Transforms.
    let c = axis_matrix(opts.up);
    let cq = DQuat::from_mat3(&c);
    let s = opts.scale;
    let (node, lin, trans) = match mode {
        TransformMode::Body => (Trs::IDENTITY, DMat3::IDENTITY, DVec3::ZERO),
        TransformMode::Node => match clean_trs(obj.transform) {
            Some((sc, r, t)) => {
                let node = Trs {
                    translation: c * (t * s),
                    rotation: (cq * r * cq.inverse()).normalize(),
                    scale: (c * sc).abs(),
                };
                (node, c * s, DVec3::ZERO)
            }
            None => (
                Trs::IDENTITY,
                c * DMat3::from_mat4(obj.transform) * s,
                c * (obj.transform.w_axis.truncate() * s),
            ),
        },
        TransformMode::Bake => (
            Trs::IDENTITY,
            c * DMat3::from_mat4(obj.transform) * s,
            c * (obj.transform.w_axis.truncate() * s),
        ),
    };
    let positions: Vec<DVec3> = body_pos.iter().map(|p| lin * *p + trans).collect();
    let nmat = if lin.determinant().abs() > 0.0 {
        lin.inverse().transpose()
    } else {
        DMat3::IDENTITY
    };
    for ns in &mut normals {
        for n in ns.iter_mut() {
            *n = (nmat * *n).normalize_or_zero();
        }
    }
    if lin.determinant() < 0.0 {
        // Mirrored: reverse winding so faces still point outwards.
        for (p, n) in polys.iter_mut().zip(normals.iter_mut()) {
            p.reverse();
            n.reverse();
        }
    }
    let edges = edge_list(&polys, &hard);
    let smoothing = smoothing_groups(&polys, &edges);
    Prepared {
        name: obj.name.to_string(),
        layer: obj.layer.to_string(),
        material,
        node,
        positions,
        polys,
        normals,
        edges,
        smoothing,
    }
}
