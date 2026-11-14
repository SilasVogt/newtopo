//! Selection state (SPEC §5.3) and output-mesh topology walks for loops,
//! rings and every-other selections (SPEC §6.14).
//!
//! The topology walk is an app-side stand-in until the core exposes its
//! `select/` module through the facade (see docs/notes/app.md).

use newtopo_core::keys::{EdgeKey, VertexKey};
use newtopo_kernel::{BodyId, EdgeId, FaceId};
use std::collections::{BTreeSet, HashMap};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Mode {
    Vertex,
    Edge,
    Face,
    #[default]
    Object,
}

/// A selectable edge. Output edges with keys use `Key`; CAD edges from the
/// B-rep (when no output exists) use `Cad`; keyless or synthetic edges use
/// `Index` into the view's edge table.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EdgeSel {
    Key(BodyId, EdgeKey),
    Cad(BodyId, EdgeId),
    Index(BodyId, u32),
}

impl EdgeSel {
    pub fn body(&self) -> BodyId {
        match self {
            EdgeSel::Key(b, _) | EdgeSel::Cad(b, _) | EdgeSel::Index(b, _) => *b,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum VertSel {
    Key(BodyId, VertexKey),
    Index(BodyId, u32),
}

impl VertSel {
    pub fn body(&self) -> BodyId {
        match self {
            VertSel::Key(b, _) | VertSel::Index(b, _) => *b,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub objects: BTreeSet<BodyId>,
    pub faces: BTreeSet<(BodyId, FaceId)>,
    pub edges: BTreeSet<EdgeSel>,
    pub verts: BTreeSet<VertSel>,
}

/// How a click combines with the current selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Combine {
    Replace,
    Add,
    Remove,
}

impl Selection {
    pub fn clear_mode(&mut self, mode: Mode) {
        match mode {
            Mode::Object => self.objects.clear(),
            Mode::Face => self.faces.clear(),
            Mode::Edge => self.edges.clear(),
            Mode::Vertex => self.verts.clear(),
        }
    }

    pub fn count(&self, mode: Mode) -> usize {
        match mode {
            Mode::Object => self.objects.len(),
            Mode::Face => self.faces.len(),
            Mode::Edge => self.edges.len(),
            Mode::Vertex => self.verts.len(),
        }
    }

    /// Bodies touched by the selection in `mode` (component modes: the
    /// bodies being edited).
    pub fn bodies(&self, mode: Mode) -> BTreeSet<BodyId> {
        match mode {
            Mode::Object => self.objects.clone(),
            Mode::Face => self.faces.iter().map(|f| f.0).collect(),
            Mode::Edge => self.edges.iter().map(|e| e.body()).collect(),
            Mode::Vertex => self.verts.iter().map(|v| v.body()).collect(),
        }
    }
}

/// Combine a set into another per click semantics.
pub fn combine<T: Ord + Clone>(
    set: &mut BTreeSet<T>,
    items: impl IntoIterator<Item = T>,
    how: Combine,
) {
    match how {
        Combine::Replace => {
            set.clear();
            set.extend(items);
        }
        Combine::Add => set.extend(items),
        Combine::Remove => {
            for i in items {
                set.remove(&i);
            }
        }
    }
}

/// Polygon mesh connectivity for loop/ring walks.
#[derive(Clone, Debug, Default)]
pub struct TopoMesh {
    pub edges: Vec<(u32, u32)>,
    pub polys: Vec<Vec<u32>>,
    edge_index: HashMap<(u32, u32), u32>,
    vert_edges: HashMap<u32, Vec<u32>>,
    edge_polys: Vec<Vec<u32>>,
}

fn ek(a: u32, b: u32) -> (u32, u32) {
    if a < b { (a, b) } else { (b, a) }
}

impl TopoMesh {
    pub fn new(polys: Vec<Vec<u32>>) -> Self {
        let mut m = TopoMesh {
            polys,
            ..Default::default()
        };
        for (pi, p) in m.polys.iter().enumerate() {
            for k in 0..p.len() {
                let key = ek(p[k], p[(k + 1) % p.len()]);
                let next = m.edges.len() as u32;
                let e = *m.edge_index.entry(key).or_insert_with(|| {
                    m.edges.push(key);
                    m.edge_polys.push(Vec::new());
                    next
                });
                m.edge_polys[e as usize].push(pi as u32);
            }
        }
        for (i, &(a, b)) in m.edges.iter().enumerate() {
            m.vert_edges.entry(a).or_default().push(i as u32);
            m.vert_edges.entry(b).or_default().push(i as u32);
        }
        m
    }

    pub fn edge(&self, a: u32, b: u32) -> Option<u32> {
        self.edge_index.get(&ek(a, b)).copied()
    }

    fn shares_poly(&self, e1: u32, e2: u32) -> bool {
        self.edge_polys[e1 as usize]
            .iter()
            .any(|p| self.edge_polys[e2 as usize].contains(p))
    }

    /// Continue an edge loop through vertex `v` (valence 4 only).
    fn loop_next(&self, e: u32, v: u32) -> Option<u32> {
        let inc = self.vert_edges.get(&v)?;
        if inc.len() != 4 {
            return None;
        }
        let cands: Vec<u32> = inc
            .iter()
            .copied()
            .filter(|&o| o != e && !self.shares_poly(e, o))
            .collect();
        (cands.len() == 1).then(|| cands[0])
    }

    /// Edge loop through `start`, ordered from one end (or around).
    pub fn edge_loop(&self, start: u32) -> Vec<u32> {
        let (a, b) = self.edges[start as usize];
        let mut fwd = vec![start];
        let mut e = start;
        let mut v = b;
        let mut closed = false;
        while let Some(n) = self.loop_next(e, v) {
            if n == start {
                closed = true;
                break;
            }
            if fwd.contains(&n) {
                break;
            }
            let (na, nb) = self.edges[n as usize];
            v = if na == v { nb } else { na };
            e = n;
            fwd.push(n);
        }
        if closed {
            return fwd;
        }
        let mut back = Vec::new();
        let mut e = start;
        let mut v = a;
        while let Some(n) = self.loop_next(e, v) {
            if n == start || fwd.contains(&n) || back.contains(&n) {
                break;
            }
            let (na, nb) = self.edges[n as usize];
            v = if na == v { nb } else { na };
            e = n;
            back.push(n);
        }
        back.reverse();
        back.extend(fwd);
        back
    }

    /// Across quad `p` from edge `e`: the opposite edge.
    fn opposite_in_quad(&self, p: u32, e: u32) -> Option<u32> {
        let poly = &self.polys[p as usize];
        if poly.len() != 4 {
            return None;
        }
        let (a, b) = self.edges[e as usize];
        let i = (0..4).find(|&i| ek(poly[i], poly[(i + 1) % 4]) == ek(a, b))?;
        self.edge(poly[(i + 2) % 4], poly[(i + 3) % 4])
    }

    fn ring_walk(&self, start: u32, first_poly: u32) -> (Vec<u32>, bool) {
        let mut out = Vec::new();
        let mut e = start;
        let mut p = first_poly;
        while let Some(o) = self.opposite_in_quad(p, e) {
            if o == start {
                return (out, true);
            }
            if out.contains(&o) {
                break;
            }
            out.push(o);
            let Some(&np) = self.edge_polys[o as usize].iter().find(|&&q| q != p) else {
                break;
            };
            e = o;
            p = np;
        }
        (out, false)
    }

    /// Edge ring through `start` (across quad strips).
    pub fn edge_ring(&self, start: u32) -> Vec<u32> {
        let ps = self.edge_polys[start as usize].clone();
        let Some(&p0) = ps.first() else {
            return vec![start];
        };
        let (fwd, closed) = self.ring_walk(start, p0);
        let mut out = Vec::new();
        if !closed && let Some(&p1) = ps.get(1) {
            let (mut back, _) = self.ring_walk(start, p1);
            back.retain(|e| !fwd.contains(e));
            back.reverse();
            out.extend(back);
        }
        out.push(start);
        out.extend(fwd);
        out
    }

    /// Polygons across the ring of `start` (a face strip).
    pub fn face_strip(&self, start_poly: u32) -> Vec<u32> {
        let poly = &self.polys[start_poly as usize];
        if poly.len() != 4 {
            return vec![start_poly];
        }
        let Some(e) = self.edge(poly[1], poly[2]) else {
            return vec![start_poly];
        };
        let ring = self.edge_ring(e);
        let mut out: Vec<u32> = Vec::new();
        for w in ring.windows(2) {
            if let Some(p) = self.edge_polys[w[0] as usize]
                .iter()
                .find(|p| self.edge_polys[w[1] as usize].contains(p))
                && !out.contains(p)
            {
                out.push(*p);
            }
        }
        if out.is_empty() {
            vec![start_poly]
        } else {
            out
        }
    }

    /// Keep every other element, anchored so `anchor` stays selected.
    pub fn every_other(seq: &[u32], anchor: u32) -> Vec<u32> {
        let pos = seq.iter().position(|&e| e == anchor).unwrap_or(0);
        seq.iter()
            .enumerate()
            .filter(|(i, _)| (*i as isize - pos as isize).rem_euclid(2) == 0)
            .map(|(_, e)| *e)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An n×m quad grid; `wrap` closes it in x (a cylinder band).
    fn grid(n: u32, m: u32, wrap: bool) -> TopoMesh {
        let cols = if wrap { n } else { n + 1 };
        let idx = |i: u32, j: u32| j * cols + (i % cols);
        let mut polys = Vec::new();
        for j in 0..m {
            for i in 0..n {
                polys.push(vec![
                    idx(i, j),
                    idx(i + 1, j),
                    idx(i + 1, j + 1),
                    idx(i, j + 1),
                ]);
            }
        }
        TopoMesh::new(polys)
    }

    #[test]
    fn loop_on_band_closes() {
        let t = grid(8, 3, true);
        // A horizontal edge in the middle row: loop runs around the band.
        let e = t.edge(8, 9).unwrap();
        let l = t.edge_loop(e);
        assert_eq!(l.len(), 8);
        // A vertical edge in the middle: loop runs up the band; ends at boundaries.
        let e = t.edge(9, 17).unwrap();
        assert_eq!(t.edge_loop(e).len(), 3);
    }

    #[test]
    fn ring_and_every_other() {
        let t = grid(8, 3, true);
        let e = t.edge(9, 17).unwrap(); // vertical edge
        let r = t.edge_ring(e);
        assert_eq!(r.len(), 8, "ring around a closed band");
        let alt = TopoMesh::every_other(&r, e);
        assert_eq!(alt.len(), 4);
        assert!(alt.contains(&e));
        let t = grid(5, 2, false);
        let e = t.edge(0, 6).unwrap();
        assert_eq!(t.edge_ring(e).len(), 6);
    }

    #[test]
    fn strip() {
        let t = grid(6, 2, false);
        assert_eq!(t.face_strip(0).len(), 6);
    }
}
