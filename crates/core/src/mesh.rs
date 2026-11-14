//! Evaluated output mesh (SPEC §4.6). This is what preview shows, statistics
//! count and export writes. Positions are in **body** coordinates; apply
//! `BodyEntry::transform` for world space.

use crate::keys::{EdgeKey, LineKey, SplitId, VertexKey};
use glam::{DVec2, DVec3};
use newtopo_kernel::{BodyId, EdgeId, FaceId};
use serde::{Deserialize, Serialize};

/// SPEC §4.4: every output vertex position is computed from an anchor.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Anchor {
    CadVertex { vertex: newtopo_kernel::VertexId },
    CadEdge { edge: EdgeId, t: f64 },
    Face { face: FaceId, uv: DVec2 },
}

impl Anchor {
    /// Collapse priority: CadVertex > CadEdge > Face.
    pub fn priority(&self) -> u8 {
        match self {
            Anchor::CadVertex { .. } => 2,
            Anchor::CadEdge { .. } => 1,
            Anchor::Face { .. } => 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutVertex {
    pub key: VertexKey,
    pub anchor: Anchor,
    pub position: DVec3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OutEdgeKind {
    /// Lies on a CAD edge.
    Cad(EdgeId),
    /// Lies on a split line.
    Split(SplitId),
    /// Lies on a generated line.
    Line(LineKey),
    /// Keyless edge added by the evaluator (bridge chord, kept seam, auto-fill
    /// diagonal). Not editable.
    Fill,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutEdge {
    /// Vertex indices, `a < b`.
    pub a: u32,
    pub b: u32,
    pub kind: OutEdgeKind,
    /// `None` for fill edges.
    pub key: Option<EdgeKey>,
    /// Hard on export (CAD sharpness or `SetHard` override).
    pub hard: bool,
    /// Polyline over the surface for drawing (body coordinates), including
    /// both end positions. Exported edges are straight segments.
    pub path: Vec<DVec3>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutPolygon {
    /// Vertex indices, counter-clockwise seen from outside (CAD normal side).
    pub verts: Vec<u32>,
    /// The CAD face (patch's face) this polygon lies on. Polygons merged
    /// across faces by RemoveCadEdge take the face of their first vertex run.
    pub face: FaceId,
    /// Per-corner normals (exact CAD normal of `face` at the corner,
    /// hard-edge splitting is implicit since normals come from each face).
    pub normals: Vec<DVec3>,
    /// True if only the fan fallback could fill it (SPEC §7).
    pub fan_fallback: bool,
}

/// Output for one body.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OutputMesh {
    pub body: BodyId,
    pub vertices: Vec<OutVertex>,
    pub edges: Vec<OutEdge>,
    pub polygons: Vec<OutPolygon>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeshStats {
    pub verts: usize,
    pub edges: usize,
    pub faces: usize,
    pub tris: usize,
    pub quads: usize,
    pub ngons: usize,
}

impl OutputMesh {
    pub fn stats(&self) -> MeshStats {
        let mut s = MeshStats { verts: self.vertices.len(), edges: self.edges.len(), faces: self.polygons.len(), ..Default::default() };
        for p in &self.polygons {
            match p.verts.len() {
                3 => s.tris += 1,
                4 => s.quads += 1,
                _ => s.ngons += 1,
            }
        }
        s
    }
}

impl std::ops::AddAssign for MeshStats {
    fn add_assign(&mut self, o: Self) {
        self.verts += o.verts;
        self.edges += o.edges;
        self.faces += o.faces;
        self.tris += o.tris;
        self.quads += o.quads;
        self.ngons += o.ngons;
    }
}
