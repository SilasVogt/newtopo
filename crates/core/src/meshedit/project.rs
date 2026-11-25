//! "Stay on CAD surface" for the move tools (SPEC §9.5.2): after a tool
//! moved vertices, put each moved vertex back on the nearest point of the
//! CAD faces of its polygons. Own design.
//!
//! The projection is a caller-supplied function (the scene asks the CAD
//! kernel), so the tools stay pure. A vertex whose projection would break a
//! polygon around it (a shape-valid polygon becoming concave or degenerate,
//! or any polygon flipping) stays where the tool put it; the report says how
//! many vertices were put back and how many could not be.

use super::EditOutcome;
use super::mesh::EditMesh;
use super::smooth::polygon_acceptable;
use glam::DVec3;
use newtopo_kernel::FaceId;
use std::collections::{BTreeMap, BTreeSet};

/// Closest points on a CAD face: `(face, points) → point per input point`
/// (`None` where there is none).
pub type Projector<'a> = dyn FnMut(FaceId, &[DVec3]) -> Vec<Option<DVec3>> + 'a;

/// Project the vertices a tool moved (relative to `before`) onto the CAD
/// faces of their polygons. `project(face, points)` returns the closest
/// point on that face for each point (`None` if there is none).
pub fn stay_on_surface(
    before: &EditMesh,
    mut out: EditOutcome,
    project: &mut Projector,
) -> EditOutcome {
    let m = &mut out.mesh;
    let diag = m.diagonal().max(1e-300);
    let mut has_source = vec![false; m.positions.len()];
    let mut moved: BTreeSet<u32> = BTreeSet::new();
    for (i, o) in out.vertex_map.iter().enumerate() {
        if let Some(j) = *o {
            has_source[j as usize] = true;
            if m.positions[j as usize].distance(before.positions[i]) > 1e-12 * diag {
                moved.insert(j);
            }
        }
    }
    let mut vpolys: Vec<Vec<usize>> = vec![Vec::new(); m.positions.len()];
    for (pi, p) in m.polygons.iter().enumerate() {
        for &v in &p.verts {
            vpolys[v as usize].push(pi);
        }
    }
    // New vertices (none of the move tools make any, but be general).
    for (j, &s) in has_source.iter().enumerate() {
        if !s && !vpolys[j].is_empty() {
            moved.insert(j as u32);
        }
    }
    moved.retain(|&v| !m.pinned[v as usize]);
    if moved.is_empty() {
        return out;
    }
    // Candidate faces per vertex, batched per face.
    let mut by_face: BTreeMap<FaceId, Vec<u32>> = BTreeMap::new();
    for &v in &moved {
        let faces: BTreeSet<FaceId> = vpolys[v as usize]
            .iter()
            .map(|&p| m.polygons[p].group)
            .collect();
        for f in faces {
            by_face.entry(f).or_default().push(v);
        }
    }
    let mut best: BTreeMap<u32, (f64, DVec3)> = BTreeMap::new();
    for (f, vs) in &by_face {
        let pts: Vec<DVec3> = vs.iter().map(|&v| m.positions[v as usize]).collect();
        let res = project(*f, &pts);
        for (k, &v) in vs.iter().enumerate() {
            if let Some(Some(q)) = res.get(k)
                && q.is_finite()
            {
                let d = q.distance(pts[k]);
                if best.get(&v).is_none_or(|(bd, _)| d < *bd) {
                    best.insert(v, (d, *q));
                }
            }
        }
    }
    let (mut put, mut held) = (0usize, 0usize);
    for (&v, &(d, q)) in &best {
        if d <= 1e-12 * diag {
            continue;
        }
        let old = m.positions[v as usize];
        let ok = vpolys[v as usize].iter().all(|&p| {
            let b: Vec<DVec3> = m.polygons[p]
                .verts
                .iter()
                .map(|&x| m.positions[x as usize])
                .collect();
            let a: Vec<DVec3> = m.polygons[p]
                .verts
                .iter()
                .map(|&x| if x == v { q } else { m.positions[x as usize] })
                .collect();
            polygon_acceptable(&b, &a, diag)
        });
        if ok {
            m.positions[v as usize] = q;
            put += 1;
        } else {
            m.positions[v as usize] = old;
            held += 1;
        }
    }
    if put + held > 0 {
        out.report
            .message
            .push_str(&format!("; {put} vertices put back on the CAD surface"));
        if held > 0 {
            out.report.skipped.push(format!(
                "{held} vertices stay off the CAD surface: projecting them would fold or flip a polygon"
            ));
        }
    }
    out.report.after = out.mesh.stats();
    out
}
