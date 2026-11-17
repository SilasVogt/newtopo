//! Output validation (SPEC §4.6 step 8).

use super::ctx::BodyCtx;
use crate::mesh::{Anchor, OutEdgeKind, OutputMesh};
use glam::{DVec2, DVec3};
use newtopo_kernel::{BodyTopology, UvClass};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IssueKind {
    NonManifold,
    OpenBoundary,
    ZeroLengthEdge,
    Degenerate,
    Flipped,
    RepeatedVertex,
    TooFewVertices,
    MissingEdge,
    AnchorOff,
    FanFallback,
    /// The evaluator couldn't make a polygon simple (bridging failed).
    NotSimple,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Issue {
    pub kind: IssueKind,
    pub message: String,
    pub polygon: Option<u32>,
    pub vertices: Vec<u32>,
}

impl Issue {
    fn new(kind: IssueKind, message: impl Into<String>) -> Self {
        Issue {
            kind,
            message: message.into(),
            polygon: None,
            vertices: vec![],
        }
    }
}

fn newell(p: &[DVec3]) -> DVec3 {
    let mut n = DVec3::ZERO;
    for i in 0..p.len() {
        let a = p[i];
        let b = p[(i + 1) % p.len()];
        n.x += (a.y - b.y) * (a.z + b.z);
        n.y += (a.z - b.z) * (a.x + b.x);
        n.z += (a.x - b.x) * (a.y + b.y);
    }
    n
}

/// Topological and geometric checks on an output mesh. `tol` is a length
/// tolerance (e.g. 1e-6 × body size). Fan-fallback polygons are reported
/// as `FanFallback` (informational; export still proceeds).
pub fn validate(mesh: &OutputMesh, topo: &BodyTopology, tol: f64) -> Vec<Issue> {
    let mut issues = Vec::new();
    let nv = mesh.vertices.len() as u32;
    let mut directed: HashMap<(u32, u32), u32> = HashMap::new();
    let mut undirected: HashMap<(u32, u32), u32> = HashMap::new();
    for (pi, p) in mesh.polygons.iter().enumerate() {
        let pi = pi as u32;
        if p.verts.len() < 3 {
            issues.push(Issue {
                polygon: Some(pi),
                ..Issue::new(
                    IssueKind::TooFewVertices,
                    "polygon with fewer than 3 vertices",
                )
            });
            continue;
        }
        let mut sorted = p.verts.clone();
        sorted.sort();
        if sorted.windows(2).any(|w| w[0] == w[1]) {
            issues.push(Issue {
                polygon: Some(pi),
                vertices: p.verts.clone(),
                ..Issue::new(IssueKind::RepeatedVertex, "polygon repeats a vertex")
            });
        }
        if p.verts.iter().any(|&v| v >= nv) {
            issues.push(Issue {
                polygon: Some(pi),
                ..Issue::new(IssueKind::Degenerate, "vertex index out of range")
            });
            continue;
        }
        if p.fan_fallback {
            issues.push(Issue {
                polygon: Some(pi),
                ..Issue::new(IssueKind::FanFallback, "filled by the fan fallback")
            });
        }
        let n = p.verts.len();
        for k in 0..n {
            let (a, b) = (p.verts[k], p.verts[(k + 1) % n]);
            *directed.entry((a, b)).or_default() += 1;
            *undirected.entry((a.min(b), a.max(b))).or_default() += 1;
        }
        // Geometry.
        let pts: Vec<DVec3> = p
            .verts
            .iter()
            .map(|&v| mesh.vertices[v as usize].position)
            .collect();
        let nn = newell(&pts);
        let scale: f64 = pts
            .windows(2)
            .map(|w| (w[1] - w[0]).length())
            .fold(0.0, f64::max)
            .max(tol);
        if nn.length() <= tol * scale * 1e-3 {
            issues.push(Issue {
                polygon: Some(pi),
                vertices: p.verts.clone(),
                ..Issue::new(IssueKind::Degenerate, "polygon has no area")
            });
            continue;
        }
        let avg: DVec3 = p.normals.iter().copied().sum();
        if avg.length() > 1e-9 {
            let avg = avg.normalize();
            // The 3D test is only meaningful for polygons that don't wrap a
            // lot of curvature; the evaluator checks UV orientation itself.
            let flat = p.normals.iter().all(|m| m.dot(avg) > 0.8);
            if flat && nn.normalize().dot(avg) < 0.0 {
                issues.push(Issue {
                    polygon: Some(pi),
                    vertices: p.verts.clone(),
                    ..Issue::new(IssueKind::Flipped, "polygon faces against the CAD normal")
                });
            }
        }
    }
    for (&(a, b), &c) in &directed {
        if c > 1 {
            issues.push(Issue {
                vertices: vec![a, b],
                ..Issue::new(
                    IssueKind::NonManifold,
                    "directed edge used twice (inconsistent orientation)",
                )
            });
        }
    }
    let mut edge_kind: HashMap<(u32, u32), Vec<OutEdgeKind>> = HashMap::new();
    for e in &mesh.edges {
        edge_kind.entry((e.a, e.b)).or_default().push(e.kind);
        let pa = mesh.vertices[e.a as usize].position;
        let pb = mesh.vertices[e.b as usize].position;
        if (pa - pb).length() <= tol {
            issues.push(Issue {
                vertices: vec![e.a, e.b],
                ..Issue::new(IssueKind::ZeroLengthEdge, "zero-length edge")
            });
        }
    }
    for (&(a, b), &c) in &undirected {
        if c > 2 {
            issues.push(Issue {
                vertices: vec![a, b],
                ..Issue::new(
                    IssueKind::NonManifold,
                    "edge shared by more than two polygons",
                )
            });
        }
        let Some(kinds) = edge_kind.get(&(a, b)) else {
            issues.push(Issue {
                vertices: vec![a, b],
                ..Issue::new(IssueKind::MissingEdge, "polygon side without an edge")
            });
            continue;
        };
        if c == 1 {
            let open_ok = kinds.iter().any(|k| match k {
                OutEdgeKind::Cad(e) => topo.edge(*e).faces.len() < 2,
                _ => false,
            });
            if !open_ok {
                issues.push(Issue {
                    vertices: vec![a, b],
                    ..Issue::new(
                        IssueKind::OpenBoundary,
                        "hole in the mesh where the B-rep is closed",
                    )
                });
            }
        }
    }
    // Vertex fans: polygons around a vertex form one fan.
    let mut around: HashMap<u32, Vec<(u32, u32)>> = HashMap::new();
    for p in &mesh.polygons {
        let n = p.verts.len();
        for k in 0..n {
            let v = p.verts[k];
            around
                .entry(v)
                .or_default()
                .push((p.verts[(k + n - 1) % n], p.verts[(k + 1) % n]));
        }
    }
    for (v, links) in &around {
        // Each link is next→prev around v; chain them.
        let mut nxt: HashMap<u32, Vec<u32>> = HashMap::new();
        for &(prev, next) in links {
            nxt.entry(next).or_default().push(prev);
        }
        if nxt.values().any(|l| l.len() > 1) {
            issues.push(Issue {
                vertices: vec![*v],
                ..Issue::new(IssueKind::NonManifold, "non-manifold vertex")
            });
            continue;
        }
        // Count chains: starts are nodes with no predecessor.
        let preds: std::collections::HashSet<u32> = links.iter().map(|l| l.0).collect();
        let starts = nxt.keys().filter(|k| !preds.contains(k)).count();
        // Walk from any start (or any node if cyclic) and count visited links.
        let mut visited = 0usize;
        let start = nxt
            .keys()
            .find(|k| !preds.contains(k))
            .copied()
            .or_else(|| nxt.keys().next().copied());
        if let Some(mut cur) = start {
            let mut guard = 0;
            while let Some(l) = nxt.get(&cur) {
                visited += 1;
                cur = l[0];
                guard += 1;
                if guard > links.len() + 1 || Some(cur) == start {
                    break;
                }
            }
        }
        if starts > 1 || visited < links.len() {
            issues.push(Issue {
                vertices: vec![*v],
                ..Issue::new(IssueKind::NonManifold, "vertex fan is not a single disk")
            });
        }
    }
    issues
}

/// Anchor checks against the kernel: positions match their anchors and
/// face anchors lie inside their face's trims.
pub fn validate_anchors(mesh: &OutputMesh, ctx: &BodyCtx, tol: f64) -> Vec<Issue> {
    use std::collections::BTreeMap;
    let mut issues = Vec::new();
    let mut pos = vec![DVec3::ZERO; mesh.vertices.len()];
    let mut by_face: BTreeMap<newtopo_kernel::FaceId, Vec<usize>> = BTreeMap::new();
    let mut by_edge: BTreeMap<newtopo_kernel::EdgeId, Vec<usize>> = BTreeMap::new();
    for (i, v) in mesh.vertices.iter().enumerate() {
        match v.anchor {
            Anchor::CadVertex { vertex } => pos[i] = ctx.topo.vertex(vertex).point,
            Anchor::CadEdge { edge, .. } => by_edge.entry(edge).or_default().push(i),
            Anchor::Face { face, .. } => by_face.entry(face).or_default().push(i),
        }
    }
    for (e, list) in &by_edge {
        let ts: Vec<f64> = list
            .iter()
            .map(|&i| {
                if let Anchor::CadEdge { t, .. } = mesh.vertices[i].anchor {
                    t
                } else {
                    0.0
                }
            })
            .collect();
        if let Ok(ps) = ctx.kernel.edge_eval(ctx.kbody, *e, &ts) {
            for (k, &i) in list.iter().enumerate() {
                pos[i] = ps[k];
            }
        }
    }
    for (f, list) in &by_face {
        let uvs: Vec<DVec2> = list
            .iter()
            .map(|&i| {
                if let Anchor::Face { uv, .. } = mesh.vertices[i].anchor {
                    uv
                } else {
                    DVec2::ZERO
                }
            })
            .collect();
        let ev = ctx.face_eval(*f, &uvs);
        let cls = ctx
            .kernel
            .face_classify_uv(ctx.kbody, *f, &uvs)
            .unwrap_or_default();
        for (k, &i) in list.iter().enumerate() {
            if let Some(e) = ev.get(k) {
                pos[i] = e.p;
            }
            if cls.get(k) == Some(&UvClass::Out) {
                issues.push(Issue {
                    vertices: vec![i as u32],
                    ..Issue::new(IssueKind::AnchorOff, "face anchor outside its face")
                });
            }
        }
    }
    for (i, v) in mesh.vertices.iter().enumerate() {
        if (pos[i] - v.position).length() > tol {
            issues.push(Issue {
                vertices: vec![i as u32],
                ..Issue::new(IssueKind::AnchorOff, "vertex is off its anchor")
            });
        }
    }
    issues
}

/// Issues that block (everything except the informational fan fallback).
pub fn blocking(issues: &[Issue]) -> Vec<&Issue> {
    issues
        .iter()
        .filter(|i| i.kind != IssueKind::FanFallback)
        .collect()
}
