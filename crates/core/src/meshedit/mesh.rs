//! The baked, plain polygon mesh that mesh mode edits (SPEC §9.5).
//!
//! Unlike [`OutputMesh`] it has no anchors, keys or surface paths: once the
//! user presses Finish, vertices are just positions. What survives from the
//! rule stage is the information the mesh tools must respect: which edges
//! lie on CAD edges, which are hard, which are UV seams, which vertices are
//! CAD corners, and the exact per-corner CAD normals.

use crate::keys::VertexKey;
use crate::mesh::{Anchor, MeshStats, OutEdgeKind, OutputMesh};
use crate::ops::Refusal;
use glam::{DVec2, DVec3};
use newtopo_kernel::{BodyId, FaceId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// What an edge carries from the rule stage. Boundary is not a flag: it is
/// derived from topology (an edge with one polygon).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EdgeFlags {
    /// Lies on a CAD edge.
    pub cad: bool,
    /// Hard on export (CAD sharpness or a `SetHard` override).
    pub hard: bool,
    /// UV seam: the corners on the two sides carry different UVs.
    pub seam: bool,
}

impl EdgeFlags {
    pub const NONE: EdgeFlags = EdgeFlags {
        cad: false,
        hard: false,
        seam: false,
    };
    pub fn any(self) -> bool {
        self.cad || self.hard || self.seam
    }
    pub fn union(self, o: EdgeFlags) -> EdgeFlags {
        EdgeFlags {
            cad: self.cad || o.cad,
            hard: self.hard || o.hard,
            seam: self.seam || o.seam,
        }
    }
    pub(crate) fn bits(self) -> u8 {
        (self.cad as u8) | ((self.hard as u8) << 1) | ((self.seam as u8) << 2)
    }
    pub(crate) fn from_bits(b: u8) -> EdgeFlags {
        EdgeFlags {
            cad: b & 1 != 0,
            hard: b & 2 != 0,
            seam: b & 4 != 0,
        }
    }
}

/// An edge with at least one flag set; `a < b`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlaggedEdge {
    pub a: u32,
    pub b: u32,
    pub flags: EdgeFlags,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EditPolygon {
    /// Vertex indices, counter-clockwise seen from outside.
    pub verts: Vec<u32>,
    /// The CAD face the polygon came from (normal lookup, material).
    pub group: FaceId,
    /// Unit normal per corner.
    pub normals: Vec<DVec3>,
    /// UV per corner, if the mesh has UVs (all polygons or none).
    pub uvs: Option<Vec<DVec2>>,
}

/// A plain polygon mesh: the result of Finish (bake) and the input and
/// output of every mesh tool.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EditMesh {
    pub body: BodyId,
    pub positions: Vec<DVec3>,
    /// Pinned vertices are never removed or moved by any mesh tool. Bake
    /// pins CAD vertices and poles.
    pub pinned: Vec<bool>,
    pub polygons: Vec<EditPolygon>,
    /// Flagged edges, sorted by `(a, b)`, each at most once. Entries that are
    /// not edges of any polygon are ignored.
    pub flagged: Vec<FlaggedEdge>,
}

/// Sorted undirected edge key.
pub(crate) fn ekey(a: u32, b: u32) -> (u32, u32) {
    if a < b { (a, b) } else { (b, a) }
}

/// Area vector (twice the area times the unit normal) of a 3D polygon.
pub(crate) fn area_vec(p: &[DVec3]) -> DVec3 {
    let n = p.len();
    let mut s = DVec3::ZERO;
    if n < 3 {
        return s;
    }
    let c = p[0];
    for i in 1..n - 1 {
        s += (p[i] - c).cross(p[i + 1] - c);
    }
    s
}

impl EditMesh {
    /// Finish (bake): turn an evaluated rule-stage mesh into a plain mesh.
    /// Refused if the evaluated mesh is not a valid manifold polygon mesh
    /// (the evaluator's validation gate should have caught that already).
    pub fn bake(out: &OutputMesh) -> Result<EditMesh, Refusal> {
        let positions: Vec<DVec3> = out.vertices.iter().map(|v| v.position).collect();
        let pinned = out
            .vertices
            .iter()
            .map(|v| {
                matches!(v.anchor, Anchor::CadVertex { .. })
                    || matches!(v.key, VertexKey::Cad(_) | VertexKey::Pole(..))
            })
            .collect();
        let polygons = out
            .polygons
            .iter()
            .map(|p| EditPolygon {
                verts: p.verts.clone(),
                group: p.face,
                normals: p.normals.clone(),
                uvs: None,
            })
            .collect();
        let mut map: BTreeMap<(u32, u32), EdgeFlags> = BTreeMap::new();
        for e in &out.edges {
            let f = EdgeFlags {
                cad: matches!(e.kind, OutEdgeKind::Cad(_)),
                hard: e.hard,
                seam: false,
            };
            if f.any() {
                let k = ekey(e.a, e.b);
                let slot = map.entry(k).or_default();
                *slot = slot.union(f);
            }
        }
        let mesh = EditMesh {
            body: out.body,
            positions,
            pinned,
            polygons,
            flagged: map
                .into_iter()
                .map(|((a, b), flags)| FlaggedEdge { a, b, flags })
                .collect(),
        };
        let issues = mesh.issues();
        if let Some(first) = issues.first() {
            return Err(Refusal::new(format!(
                "Can't finish the mesh: {first} ({} problem(s) in total)",
                issues.len()
            )));
        }
        Ok(mesh)
    }

    /// Set the seam flag on every edge whose two sides carry different UVs
    /// (and clear it elsewhere), so `flagged` agrees with the UVs.
    pub(crate) fn flag_seams(&mut self) {
        if !self.has_uvs() {
            return;
        }
        let mut side: HashMap<(u32, u32), (DVec2, DVec2)> = HashMap::new();
        let mut seam: BTreeSet<(u32, u32)> = BTreeSet::new();
        for p in &self.polygons {
            let uv = p.uvs.as_ref().expect("has_uvs");
            let n = p.verts.len();
            for i in 0..n {
                let j = (i + 1) % n;
                let (a, b) = (p.verts[i], p.verts[j]);
                let val = if a < b {
                    (uv[i], uv[j])
                } else {
                    (uv[j], uv[i])
                };
                let k = ekey(a, b);
                match side.get(&k) {
                    Some(o) if o.0.distance(val.0) > 1e-9 || o.1.distance(val.1) > 1e-9 => {
                        seam.insert(k);
                    }
                    Some(_) => {}
                    None => {
                        side.insert(k, val);
                    }
                }
            }
        }
        let mut map: BTreeMap<(u32, u32), EdgeFlags> = self
            .flagged
            .iter()
            .map(|e| {
                (
                    ekey(e.a, e.b),
                    EdgeFlags {
                        seam: false,
                        ..e.flags
                    },
                )
            })
            .collect();
        for k in seam {
            map.entry(k).or_default().seam = true;
        }
        self.flagged = map
            .into_iter()
            .filter(|(_, f)| f.any())
            .map(|((a, b), flags)| FlaggedEdge { a, b, flags })
            .collect();
    }

    pub fn stats(&self) -> MeshStats {
        let mut s = MeshStats {
            verts: self.used_vertex_count(),
            edges: self.edges().len(),
            faces: self.polygons.len(),
            ..Default::default()
        };
        for p in &self.polygons {
            match p.verts.len() {
                3 => s.tris += 1,
                4 => s.quads += 1,
                _ => s.ngons += 1,
            }
        }
        s
    }

    /// Triangle count after triangulation (an n-gon counts n - 2).
    pub fn tri_count(&self) -> usize {
        self.polygons
            .iter()
            .map(|p| p.verts.len().saturating_sub(2))
            .sum()
    }

    fn used_vertex_count(&self) -> usize {
        let mut used = vec![false; self.positions.len()];
        for p in &self.polygons {
            for &v in &p.verts {
                if let Some(u) = used.get_mut(v as usize) {
                    *u = true;
                }
            }
        }
        used.iter().filter(|&&u| u).count()
    }

    pub fn has_uvs(&self) -> bool {
        self.polygons.first().is_some_and(|p| p.uvs.is_some())
    }

    /// All undirected edges, sorted.
    pub fn edges(&self) -> Vec<(u32, u32)> {
        let mut s = BTreeSet::new();
        for p in &self.polygons {
            let n = p.verts.len();
            for i in 0..n {
                s.insert(ekey(p.verts[i], p.verts[(i + 1) % n]));
            }
        }
        s.into_iter().collect()
    }

    /// Flags per edge (only edges with a flag).
    pub fn flag_map(&self) -> HashMap<(u32, u32), EdgeFlags> {
        let mut m = HashMap::new();
        for e in &self.flagged {
            let k = ekey(e.a, e.b);
            let f: &mut EdgeFlags = m.entry(k).or_default();
            *f = f.union(e.flags);
        }
        m
    }

    pub fn flags(&self, a: u32, b: u32) -> EdgeFlags {
        let k = ekey(a, b);
        self.flagged
            .binary_search_by(|e| (e.a, e.b).cmp(&k))
            .map(|i| self.flagged[i].flags)
            .unwrap_or_default()
    }

    /// Edges used by exactly one polygon, sorted.
    pub fn boundary_edges(&self) -> Vec<(u32, u32)> {
        let mut uses: BTreeMap<(u32, u32), u32> = BTreeMap::new();
        for p in &self.polygons {
            let n = p.verts.len();
            for i in 0..n {
                *uses
                    .entry(ekey(p.verts[i], p.verts[(i + 1) % n]))
                    .or_default() += 1;
            }
        }
        uses.into_iter()
            .filter(|&(_, c)| c == 1)
            .map(|(k, _)| k)
            .collect()
    }

    /// Topology signature: (Euler characteristic V - E + F over used
    /// vertices, number of open boundary loops, number of connected parts).
    /// Every mesh tool keeps it unchanged.
    pub fn topology(&self) -> (i64, usize, usize) {
        let edges = self.edges();
        let v = self.used_vertex_count() as i64;
        let chi = v - edges.len() as i64 + self.polygons.len() as i64;
        let n = self.positions.len();
        let mut parent: Vec<usize> = (0..n).collect();
        fn find(p: &mut [usize], x: usize) -> usize {
            let mut r = x;
            while p[r] != r {
                r = p[r];
            }
            let mut y = x;
            while p[y] != r {
                let nx = p[y];
                p[y] = r;
                y = nx;
            }
            r
        }
        let union = |p: &mut Vec<usize>, a: u32, b: u32| {
            let (x, y) = (find(p, a as usize), find(p, b as usize));
            if x != y {
                p[x.max(y)] = x.min(y);
            }
        };
        for &(a, b) in &edges {
            union(&mut parent, a, b);
        }
        let mut used = vec![false; n];
        for p in &self.polygons {
            for &x in &p.verts {
                used[x as usize] = true;
            }
        }
        let parts = (0..n)
            .filter(|&x| used[x] && find(&mut parent, x) == x)
            .count();
        // Boundary loops: components of the boundary-edge graph.
        let mut bp: Vec<usize> = (0..n).collect();
        let be = self.boundary_edges();
        let mut on = vec![false; n];
        for &(a, b) in &be {
            union(&mut bp, a, b);
            on[a as usize] = true;
            on[b as usize] = true;
        }
        let loops = (0..n).filter(|&x| on[x] && find(&mut bp, x) == x).count();
        (chi, loops, parts)
    }

    /// Length of the bounding-box diagonal of the used vertices.
    pub fn diagonal(&self) -> f64 {
        let mut lo = DVec3::splat(f64::INFINITY);
        let mut hi = DVec3::splat(f64::NEG_INFINITY);
        for p in &self.polygons {
            for &v in &p.verts {
                if let Some(q) = self.positions.get(v as usize) {
                    lo = lo.min(*q);
                    hi = hi.max(*q);
                }
            }
        }
        if lo.x > hi.x {
            return 0.0;
        }
        (hi - lo).length()
    }

    pub fn polygon_positions(&self, i: usize) -> Vec<DVec3> {
        self.polygons[i]
            .verts
            .iter()
            .map(|&v| self.positions[v as usize])
            .collect()
    }

    /// Unit geometric normal of polygon `i` (zero if degenerate).
    pub fn polygon_normal(&self, i: usize) -> DVec3 {
        area_vec(&self.polygon_positions(i)).normalize_or_zero()
    }

    /// Polygons whose geometric normal points away from their corner
    /// normals (the exact CAD normals after bake): flipped faces.
    pub fn flipped_polygons(&self) -> Vec<usize> {
        (0..self.polygons.len())
            .filter(|&i| {
                let r: DVec3 = self.polygons[i].normals.iter().copied().sum();
                self.polygon_normal(i).dot(r) <= 0.0
            })
            .collect()
    }

    /// Polygons that break a §9.5.2 shape guarantee the structural check
    /// ([`EditMesh::issues`]) doesn't cover: concave or degenerate
    /// triangles and quads, polygons on the same vertex set as another
    /// one, and polygons whose normal is more than `max_normal_change_deg`
    /// from the mean CAD normal at their corners. A tool may leave such
    /// polygons of its input as they are but never make new ones; baked
    /// meshes can contain a few (slivers on curved CAD faces).
    pub fn polygon_problems(&self, max_normal_change_deg: f64) -> BTreeSet<usize> {
        let diag = self.diagonal().max(1e-300);
        let cos = max_normal_change_deg.to_radians().cos();
        let mut out = BTreeSet::new();
        let mut sets: HashMap<Vec<u32>, usize> = HashMap::new();
        for (i, p) in self.polygons.iter().enumerate() {
            let pos = self.polygon_positions(i);
            let shape = if pos.len() <= 4 {
                super::work::polygon_shape_ok(&pos, diag)
            } else {
                area_vec(&pos).length() > 1e-12 * diag * diag
            };
            let r = p.normals.iter().copied().sum::<DVec3>().normalize_or_zero();
            if !shape || self.polygon_normal(i).dot(r) < cos {
                out.insert(i);
            }
            let mut k = p.verts.clone();
            k.sort_unstable();
            if let Some(&j) = sets.get(&k) {
                out.insert(i);
                out.insert(j);
            } else {
                sets.insert(k, i);
            }
        }
        out
    }

    /// Structural problems that make the mesh unusable for the tools:
    /// bad indices, short polygons, repeated vertices, inconsistent
    /// attributes, non-manifold edges or vertices, inconsistent winding,
    /// zero-length edges and zero-area polygons. Empty when valid.
    pub fn issues(&self) -> Vec<String> {
        let mut out = Vec::new();
        let nv = self.positions.len();
        if self.pinned.len() != nv {
            out.push(format!(
                "pinned has {} entries for {} vertices",
                self.pinned.len(),
                nv
            ));
        }
        if let Some(i) = self.positions.iter().position(|p| !p.is_finite()) {
            out.push(format!("vertex {i} has a non-finite position"));
        }
        let uv = self.has_uvs();
        let mut directed: HashMap<(u32, u32), u32> = HashMap::new();
        let mut structural_ok = true;
        let diag = self.diagonal().max(1e-300);
        for (pi, p) in self.polygons.iter().enumerate() {
            let n = p.verts.len();
            if n < 3 {
                out.push(format!("polygon {pi} has {n} vertices"));
                structural_ok = false;
                continue;
            }
            if p.verts.iter().any(|&v| v as usize >= nv) {
                out.push(format!("polygon {pi} refers to a missing vertex"));
                structural_ok = false;
                continue;
            }
            let distinct: BTreeSet<u32> = p.verts.iter().copied().collect();
            if distinct.len() != n {
                out.push(format!("polygon {pi} repeats a vertex"));
                structural_ok = false;
                continue;
            }
            if p.normals.len() != n {
                out.push(format!("polygon {pi} has {} normals", p.normals.len()));
            }
            match &p.uvs {
                Some(u) if u.len() != n => out.push(format!("polygon {pi} has {} uvs", u.len())),
                Some(_) if !uv => out.push(format!("polygon {pi} has uvs but others don't")),
                None if uv => out.push(format!("polygon {pi} has no uvs but others do")),
                _ => {}
            }
            for i in 0..n {
                let (a, b) = (p.verts[i], p.verts[(i + 1) % n]);
                *directed.entry((a, b)).or_default() += 1;
            }
            let pos = self.polygon_positions(pi);
            if area_vec(&pos).length() <= 1e-14 * diag * diag {
                out.push(format!("polygon {pi} has zero area"));
            }
            for i in 0..n {
                if pos[i].distance(pos[(i + 1) % n]) <= 1e-12 * diag {
                    out.push(format!(
                        "edge {}-{} has zero length",
                        p.verts[i],
                        p.verts[(i + 1) % n]
                    ));
                }
            }
        }
        if !structural_ok {
            return out;
        }
        let mut keys: Vec<_> = directed.iter().collect();
        keys.sort();
        for (&(a, b), &c) in keys {
            if c > 1 {
                out.push(format!(
                    "edge {a}-{b} is used {c} times in the same direction (non-manifold or flipped neighbour)"
                ));
            }
        }
        // Vertex fans.
        let mut corners: Vec<Vec<(u32, u32)>> = vec![Vec::new(); nv];
        for p in &self.polygons {
            let n = p.verts.len();
            for i in 0..n {
                corners[p.verts[i] as usize].push((p.verts[(i + n - 1) % n], p.verts[(i + 1) % n]));
            }
        }
        for (v, links) in corners.iter().enumerate() {
            if !links.is_empty() && !single_fan(links) {
                out.push(format!("vertex {v} is non-manifold"));
            }
        }
        out
    }
}

/// True if the corner links `(prev, next)` around one vertex form a single
/// fan: one open chain or one closed cycle.
pub(crate) fn single_fan(links: &[(u32, u32)]) -> bool {
    let mut next_of: HashMap<u32, u32> = HashMap::with_capacity(links.len());
    let mut is_next: HashMap<u32, ()> = HashMap::with_capacity(links.len());
    for &(p, n) in links {
        if next_of.insert(p, n).is_some() || is_next.insert(n, ()).is_some() {
            return false;
        }
    }
    // Walk from a chain start (a `prev` that is nobody's `next`), else any.
    let mut starts = links.iter().filter(|(p, _)| !is_next.contains_key(p));
    let start = match starts.next() {
        Some(&(p, _)) => {
            if starts.next().is_some() {
                return false;
            }
            p
        }
        None => links.iter().map(|l| l.0).min().unwrap_or(0),
    };
    let mut x = start;
    let mut steps = 0;
    while let Some(&n) = next_of.get(&x) {
        steps += 1;
        if steps > links.len() {
            break;
        }
        x = n;
        if x == start {
            break;
        }
    }
    steps == links.len()
}
