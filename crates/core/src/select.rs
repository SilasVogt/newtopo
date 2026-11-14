//! Loop, ring and strip selection on the output mesh (SPEC §6.14). Edge
//! loops pass through valence-4 vertices; rings and face strips cross quads
//! through opposite edges. Walks stop at poles, n-gons and boundaries and
//! close if they return. Indices are into `OutputMesh::edges` / `polygons`.

use crate::mesh::{OutEdgeKind, OutputMesh};
use std::collections::HashMap;

/// Edge / polygon adjacency of an output mesh.
pub struct Adjacency<'a> {
    pub mesh: &'a OutputMesh,
    /// (min, max) vertex pair → edge index.
    pub edge_of: HashMap<(u32, u32), usize>,
    /// Edge index → polygons using it.
    pub polys_of: Vec<Vec<usize>>,
    /// Vertex → incident edges.
    pub vert_edges: Vec<Vec<usize>>,
}

impl<'a> Adjacency<'a> {
    pub fn new(mesh: &'a OutputMesh) -> Self {
        let mut edge_of = HashMap::new();
        let mut vert_edges = vec![vec![]; mesh.vertices.len()];
        for (i, e) in mesh.edges.iter().enumerate() {
            edge_of.entry((e.a.min(e.b), e.a.max(e.b))).or_insert(i);
            vert_edges[e.a as usize].push(i);
            vert_edges[e.b as usize].push(i);
        }
        let mut polys_of = vec![vec![]; mesh.edges.len()];
        for (pi, p) in mesh.polygons.iter().enumerate() {
            let n = p.verts.len();
            for k in 0..n {
                let (a, b) = (p.verts[k], p.verts[(k + 1) % n]);
                if let Some(&e) = edge_of.get(&(a.min(b), a.max(b))) {
                    polys_of[e].push(pi);
                }
            }
        }
        Adjacency {
            mesh,
            edge_of,
            polys_of,
            vert_edges,
        }
    }

    fn ends(&self, e: usize) -> (u32, u32) {
        let x = &self.mesh.edges[e];
        (x.a, x.b)
    }

    /// Edge of polygon `p` at side `k` (vertex k → k+1).
    fn side(&self, p: usize, k: usize) -> Option<usize> {
        let v = &self.mesh.polygons[p].verts;
        let (a, b) = (v[k % v.len()], v[(k + 1) % v.len()]);
        self.edge_of.get(&(a.min(b), a.max(b))).copied()
    }

    fn side_index(&self, p: usize, e: usize) -> Option<usize> {
        let n = self.mesh.polygons[p].verts.len();
        (0..n).find(|&k| self.side(p, k) == Some(e))
    }

    /// The edge continuing `e` straight through vertex `v` (valence 4 with
    /// four polygons around), if any.
    fn straight_through(&self, e: usize, v: u32) -> Option<usize> {
        let inc = &self.vert_edges[v as usize];
        if inc.len() != 4 {
            return None;
        }
        // Edges sharing a polygon with `e` at `v`.
        let mut beside = Vec::new();
        for &p in &self.polys_of[e] {
            let n = self.mesh.polygons[p].verts.len();
            for k in 0..n {
                if let Some(s) = self.side(p, k)
                    && s != e
                {
                    let (a, b) = self.ends(s);
                    if a == v || b == v {
                        beside.push(s);
                    }
                }
            }
        }
        if self.polys_of[e].len() != 2 || beside.len() != 2 {
            return None;
        }
        inc.iter().copied().find(|x| *x != e && !beside.contains(x))
    }

    /// Edge loop through `e` (SPEC §6.14).
    pub fn edge_loop(&self, e: usize) -> Vec<usize> {
        let mut out = vec![e];
        let (a, b) = self.ends(e);
        // Forward from b.
        let mut cur = e;
        let mut v = b;
        let mut closed = false;
        while let Some(n) = self.straight_through(cur, v) {
            if n == e {
                closed = true;
                break;
            }
            if out.contains(&n) {
                break;
            }
            out.push(n);
            let (x, y) = self.ends(n);
            v = if x == v { y } else { x };
            cur = n;
        }
        if closed {
            return out;
        }
        let mut back = Vec::new();
        let mut cur = e;
        let mut v = a;
        while let Some(n) = self.straight_through(cur, v) {
            if out.contains(&n) || back.contains(&n) {
                break;
            }
            back.push(n);
            let (x, y) = self.ends(n);
            v = if x == v { y } else { x };
            cur = n;
        }
        back.reverse();
        back.extend(out);
        back
    }

    /// The opposite side of quad `p` from edge `e`.
    fn opposite_in_quad(&self, p: usize, e: usize) -> Option<usize> {
        if self.mesh.polygons[p].verts.len() != 4 {
            return None;
        }
        let k = self.side_index(p, e)?;
        self.side(p, k + 2)
    }

    /// Edge ring through `e`: edges across quad strips.
    pub fn edge_ring(&self, e: usize) -> Vec<usize> {
        let mut out = vec![e];
        let polys = self.polys_of[e].clone();
        let mut halves: Vec<Vec<usize>> = Vec::new();
        for &p0 in &polys {
            let mut half = Vec::new();
            let (mut p, mut cur) = (p0, e);
            while let Some(o) = self.opposite_in_quad(p, cur) {
                if o == e {
                    return out.into_iter().chain(half).collect();
                }
                if out.contains(&o) || half.contains(&o) {
                    break;
                }
                half.push(o);
                let Some(&np) = self.polys_of[o].iter().find(|&&q| q != p) else {
                    break;
                };
                p = np;
                cur = o;
            }
            halves.push(half);
        }
        let mut res: Vec<usize> = halves
            .get(1)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .rev()
            .collect();
        res.append(&mut out);
        if let Some(h) = halves.first() {
            res.extend(h.iter().copied());
        }
        res
    }

    /// Face loop/strip through polygon `p`, crossing through side `e`
    /// (an edge of `p`) and its opposite.
    pub fn face_loop(&self, p: usize, e: usize) -> Vec<usize> {
        let out = vec![p];
        if self.mesh.polygons[p].verts.len() != 4 {
            return out;
        }
        let Some(o) = self.opposite_in_quad(p, e) else {
            return out;
        };
        let mut halves = Vec::new();
        for start in [e, o] {
            let mut half = Vec::new();
            let (mut cur_p, mut cur_e) = (p, start);
            while let Some(&np) = self.polys_of[cur_e].iter().find(|&&q| q != cur_p) {
                if np == p {
                    // Closed loop.
                    return out.into_iter().chain(half).collect();
                }
                if out.contains(&np)
                    || half.contains(&np)
                    || self.mesh.polygons[np].verts.len() != 4
                {
                    break;
                }
                half.push(np);
                let Some(next_e) = self.opposite_in_quad(np, cur_e) else {
                    break;
                };
                cur_p = np;
                cur_e = next_e;
            }
            halves.push(half);
        }
        let mut res: Vec<usize> = halves[1].iter().rev().copied().collect();
        res.push(p);
        res.extend(halves[0].iter().copied());
        res
    }
}

/// Alternate elements along a loop/ring/strip result (SPEC §6.14).
pub fn every_other(items: &[usize]) -> Vec<usize> {
    items.iter().step_by(2).copied().collect()
}

/// Keep only edges that can be edited (generated, split or CAD edges), as
/// loops and rings walk through fill edges but don't select them (§6.7).
pub fn editable_only(mesh: &OutputMesh, edges: &[usize]) -> Vec<usize> {
    edges
        .iter()
        .copied()
        .filter(|&e| mesh.edges[e].kind != OutEdgeKind::Fill)
        .collect()
}
