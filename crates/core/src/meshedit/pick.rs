//! Selection helpers for mesh mode (SPEC §9.5.9): edge loops and shortest
//! paths on an [`EditMesh`]. Own design.
//!
//! - **Edge loop**: from an edge, walk straight on through every vertex with
//!   exactly four edges that is inside the surface (the next edge is the
//!   one that shares no polygon with the edge it came from); a boundary
//!   edge follows the open boundary instead. The walk stops at any other
//!   vertex, or when it comes back to the start (a closed loop).
//! - **Shortest path**: Dijkstra over edge lengths, ties broken by the
//!   lower vertex index, so the same input always gives the same path.

use super::check_input;
use super::mesh::{EditMesh, ekey};
use crate::ops::Refusal;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

/// Undirected adjacency of a mesh: polygons per edge, neighbours per vertex.
struct Adj {
    edge_polys: BTreeMap<(u32, u32), Vec<u32>>,
    nbrs: Vec<BTreeSet<u32>>,
}

impl Adj {
    fn new(m: &EditMesh) -> Adj {
        let mut edge_polys: BTreeMap<(u32, u32), Vec<u32>> = BTreeMap::new();
        let mut nbrs = vec![BTreeSet::new(); m.positions.len()];
        for (pi, p) in m.polygons.iter().enumerate() {
            let n = p.verts.len();
            for i in 0..n {
                let (a, b) = (p.verts[i], p.verts[(i + 1) % n]);
                edge_polys.entry(ekey(a, b)).or_default().push(pi as u32);
                nbrs[a as usize].insert(b);
                nbrs[b as usize].insert(a);
            }
        }
        Adj { edge_polys, nbrs }
    }

    fn polys(&self, a: u32, b: u32) -> &[u32] {
        self.edge_polys
            .get(&ekey(a, b))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    fn is_boundary(&self, a: u32, b: u32) -> bool {
        self.polys(a, b).len() == 1
    }

    /// The edge after `prev`→`v` on a loop, if the loop continues.
    fn next(&self, m: &EditMesh, prev: u32, v: u32) -> Option<u32> {
        let nb = &self.nbrs[v as usize];
        if self.is_boundary(prev, v) {
            // Follow the open boundary.
            let others: Vec<u32> = nb
                .iter()
                .copied()
                .filter(|&x| x != prev && self.is_boundary(v, x))
                .collect();
            return (others.len() == 1).then(|| others[0]);
        }
        if nb.len() != 4 || nb.iter().any(|&x| self.is_boundary(v, x)) {
            return None;
        }
        let beside: BTreeSet<u32> = self
            .polys(prev, v)
            .iter()
            .flat_map(|&p| m.polygons[p as usize].verts.iter().copied())
            .collect();
        let straight: Vec<u32> = nb.iter().copied().filter(|x| !beside.contains(x)).collect();
        (straight.len() == 1).then(|| straight[0])
    }
}

fn check_edge(m: &EditMesh, adj: &Adj, a: u32, b: u32) -> Result<(), Refusal> {
    let n = m.positions.len() as u32;
    if a >= n || b >= n || a == b || adj.polys(a, b).is_empty() {
        return Err(Refusal::new(format!("{a}-{b} is not an edge of this mesh")));
    }
    Ok(())
}

/// The edge loop through the edge `a`-`b`, in walk order (each edge as a
/// sorted pair, the given edge first among them in loop order). A closed
/// loop lists every edge once.
pub fn edge_loop(mesh: &EditMesh, a: u32, b: u32) -> Result<Vec<[u32; 2]>, Refusal> {
    check_input(mesh)?;
    let adj = Adj::new(mesh);
    check_edge(mesh, &adj, a, b)?;
    let start = ekey(a, b);
    let mut seen: BTreeSet<(u32, u32)> = BTreeSet::from([start]);
    // Walk forward from b, then backward from a.
    let walk = |from: u32, to: u32, seen: &mut BTreeSet<(u32, u32)>| -> (Vec<(u32, u32)>, bool) {
        let (mut prev, mut v) = (from, to);
        let mut out = Vec::new();
        loop {
            let Some(x) = adj.next(mesh, prev, v) else {
                return (out, false);
            };
            let k = ekey(v, x);
            if k == start {
                return (out, true);
            }
            if !seen.insert(k) {
                return (out, false);
            }
            out.push(k);
            prev = v;
            v = x;
        }
    };
    let (fwd, closed) = walk(a, b, &mut seen);
    let back = if closed {
        Vec::new()
    } else {
        walk(b, a, &mut seen).0
    };
    let mut out: Vec<[u32; 2]> = back.iter().rev().map(|&(x, y)| [x, y]).collect();
    out.push([start.0, start.1]);
    out.extend(fwd.iter().map(|&(x, y)| [x, y]));
    Ok(out)
}

#[derive(PartialEq)]
struct Node(f64, u32);
impl Eq for Node {}
impl PartialOrd for Node {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Node {
    /// Min-heap on distance, then on the vertex index.
    fn cmp(&self, o: &Self) -> Ordering {
        o.0.total_cmp(&self.0).then_with(|| o.1.cmp(&self.1))
    }
}

/// Shortest vertex path from `from` to `to` along edges (by length),
/// including both ends. Refused if they are not connected.
pub fn shortest_path(mesh: &EditMesh, from: u32, to: u32) -> Result<Vec<u32>, Refusal> {
    check_input(mesh)?;
    let adj = Adj::new(mesh);
    let n = mesh.positions.len();
    for v in [from, to] {
        if v as usize >= n || adj.nbrs[v as usize].is_empty() {
            return Err(Refusal::new(format!("Vertex {v} is not in this mesh")));
        }
    }
    let (dist, prev) = dijkstra(mesh, &adj, &[from], Some(to));
    if !dist[to as usize].is_finite() {
        return Err(Refusal::new(
            "There is no path between them: they are on separate parts",
        ));
    }
    let mut path = vec![to];
    let mut x = to;
    while x != from {
        x = prev[x as usize];
        path.push(x);
    }
    path.reverse();
    Ok(path)
}

/// Dijkstra from several sources; stops early once `target` is settled.
fn dijkstra(
    mesh: &EditMesh,
    adj: &Adj,
    sources: &[u32],
    target: Option<u32>,
) -> (Vec<f64>, Vec<u32>) {
    let n = mesh.positions.len();
    let mut dist = vec![f64::INFINITY; n];
    let mut prev: Vec<u32> = (0..n as u32).collect();
    let mut heap = BinaryHeap::new();
    for &s in sources {
        dist[s as usize] = 0.0;
        heap.push(Node(0.0, s));
    }
    while let Some(Node(d, v)) = heap.pop() {
        if d > dist[v as usize] {
            continue;
        }
        if Some(v) == target {
            break;
        }
        for &x in &adj.nbrs[v as usize] {
            let nd = d + mesh.positions[v as usize].distance(mesh.positions[x as usize]);
            if nd < dist[x as usize] {
                dist[x as usize] = nd;
                prev[x as usize] = v;
                heap.push(Node(nd, x));
            }
        }
    }
    (dist, prev)
}

/// Shortest edge path between two edges: both edges plus the shortest
/// vertex path between their nearest ends. Sorted pairs, in path order.
pub fn shortest_edge_path(
    mesh: &EditMesh,
    from: [u32; 2],
    to: [u32; 2],
) -> Result<Vec<[u32; 2]>, Refusal> {
    check_input(mesh)?;
    let adj = Adj::new(mesh);
    check_edge(mesh, &adj, from[0], from[1])?;
    check_edge(mesh, &adj, to[0], to[1])?;
    let norm = |e: [u32; 2]| {
        let k = ekey(e[0], e[1]);
        [k.0, k.1]
    };
    if norm(from) == norm(to) {
        return Ok(vec![norm(from)]);
    }
    let mut best: Option<(f64, Vec<u32>)> = None;
    for &s in &from {
        let (dist, prev) = dijkstra(mesh, &adj, &[s], None);
        for &t in &to {
            let d = dist[t as usize];
            if d.is_finite() && best.as_ref().is_none_or(|(bd, _)| d < *bd) {
                let mut path = vec![t];
                let mut x = t;
                while x != s {
                    x = prev[x as usize];
                    path.push(x);
                }
                path.reverse();
                best = Some((d, path));
            }
        }
    }
    let Some((_, path)) = best else {
        return Err(Refusal::new(
            "There is no path between them: they are on separate parts",
        ));
    };
    let mut out = vec![norm(from)];
    for w in path.windows(2) {
        let e = norm([w[0], w[1]]);
        if !out.contains(&e) {
            out.push(e);
        }
    }
    let t = norm(to);
    if !out.contains(&t) {
        out.push(t);
    }
    // A path that ran along `from` (or `to`) doesn't list it twice.
    Ok(out)
}
