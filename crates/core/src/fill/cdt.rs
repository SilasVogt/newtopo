//! Large polygons (SPEC §7 step 4): constrained Delaunay triangulation of the
//! boundary vertices only (spade, no Steiner points), then greedy pairing of
//! adjacent triangles into admissible quads by descending quality.

use super::dp::admissible;
use super::quality::Scorer;
use glam::DVec2;
use spade::{ConstrainedDelaunayTriangulation, Point2, Triangulation};
use std::collections::HashMap;

/// Triangles (counter-clockwise, polygon indices) of the inside of the simple
/// counter-clockwise polygon `uv`, or `None` if the CDT can't represent it
/// (duplicate points, crossing or touching edges).
pub fn cdt_triangles(uv: &[DVec2]) -> Option<Vec<[usize; 3]>> {
    let n = uv.len();
    let mut cdt: ConstrainedDelaunayTriangulation<Point2<f64>> =
        ConstrainedDelaunayTriangulation::new();
    let mut handles = Vec::with_capacity(n);
    for p in uv {
        if !p.is_finite() {
            return None;
        }
        let h = cdt.insert(Point2::new(p.x, p.y)).ok()?;
        if cdt.num_vertices() != handles.len() + 1 {
            return None; // duplicate position merged by spade
        }
        handles.push(h);
    }
    let mut poly_index = vec![usize::MAX; cdt.num_vertices()];
    for (i, h) in handles.iter().enumerate() {
        poly_index[h.index()] = i;
    }
    for i in 0..n {
        // One edge back means no conflict and no vertex on the segment.
        if cdt
            .try_add_constraint(handles[i], handles[(i + 1) % n])
            .len()
            != 1
        {
            return None;
        }
    }
    // Flood the inside from the left side of every boundary edge, never
    // crossing a constraint.
    let mut seen = vec![false; cdt.num_all_faces()];
    let mut stack = Vec::new();
    for i in 0..n {
        let e = cdt.get_edge_from_neighbors(handles[i], handles[(i + 1) % n])?;
        let f = e.face().as_inner()?;
        if !seen[f.fix().index()] {
            seen[f.fix().index()] = true;
            stack.push(f.fix());
        }
    }
    let mut tris = Vec::with_capacity(n - 2);
    while let Some(fh) = stack.pop() {
        let f = cdt.face(fh);
        let vs = f.vertices();
        tris.push([
            poly_index[vs[0].fix().index()],
            poly_index[vs[1].fix().index()],
            poly_index[vs[2].fix().index()],
        ]);
        for e in f.adjacent_edges() {
            if cdt.is_constraint_edge(e.as_undirected().fix()) {
                continue;
            }
            let g = e.rev().face().as_inner()?; // leaked to the outside
            if !seen[g.fix().index()] {
                seen[g.fix().index()] = true;
                stack.push(g.fix());
            }
        }
    }
    if tris.len() != n - 2 || tris.iter().any(|t| t.contains(&usize::MAX)) {
        return None;
    }
    tris.sort_unstable();
    Some(tris)
}

/// CDT then greedy maximum-quality pairing into admissible quads.
pub fn cdt_fill(uv: &[DVec2], sc: &Scorer, area_eps: f64) -> Option<Vec<Vec<u32>>> {
    let tris = cdt_triangles(uv)?;
    let mut by_edge: HashMap<(usize, usize), usize> = HashMap::with_capacity(tris.len() * 3);
    for (t, tri) in tris.iter().enumerate() {
        for k in 0..3 {
            by_edge.insert((tri[k], tri[(k + 1) % 3]), t);
        }
    }
    // Candidate quads across every interior edge u->v of triangle t1 = (u, v, w1)
    // with t2 = (v, u, w2): quad (u, w2, v, w1) is counter-clockwise.
    let mut cands: Vec<(f64, [usize; 4], usize, usize)> = Vec::new();
    for (t1, tri) in tris.iter().enumerate() {
        for k in 0..3 {
            let (u, v, w1) = (tri[k], tri[(k + 1) % 3], tri[(k + 2) % 3]);
            let Some(&t2) = by_edge.get(&(v, u)) else {
                continue;
            };
            if t2 <= t1 {
                continue;
            }
            let t2v = tris[t2];
            let w2 = t2v.iter().copied().find(|&x| x != u && x != v)?;
            let q = [u, w2, v, w1];
            if admissible(uv, q[0], q[1], q[2], q[3], area_eps) {
                cands.push((sc.quad(q[0], q[1], q[2], q[3]), q, t1, t2));
            }
        }
    }
    // Highest quality first; ties by the diagonal's (canonical) indices.
    cands.sort_by(|a, b| {
        b.0.total_cmp(&a.0).then_with(|| {
            let ka = (a.1[0].min(a.1[2]), a.1[0].max(a.1[2]));
            let kb = (b.1[0].min(b.1[2]), b.1[0].max(b.1[2]));
            ka.cmp(&kb)
        })
    });
    let mut used = vec![false; tris.len()];
    let mut out = Vec::with_capacity(tris.len());
    for (_, q, t1, t2) in cands {
        if used[t1] || used[t2] {
            continue;
        }
        used[t1] = true;
        used[t2] = true;
        out.push(q.iter().map(|&x| x as u32).collect());
    }
    for (t, tri) in tris.iter().enumerate() {
        if !used[t] {
            out.push(tri.iter().map(|&x| x as u32).collect());
        }
    }
    Some(out)
}
