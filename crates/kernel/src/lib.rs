//! CAD kernel boundary (SPEC §3.4).
//!
//! Everything above this crate talks to [`CadKernel`]. No kernel-native type
//! crosses this boundary: faces, edges and vertices are `u32` ids into tables
//! owned per body. Two implementations exist:
//!
//! * [`mock::MockKernel`]: analytic bodies built in Rust (plane, cylinder,
//!   cone, sphere, torus). Used by most core tests; needs no C++.
//! * `occt::OcctKernel` (feature `occt`): Open CASCADE through `occt-shim`.

use glam::{DMat4, DVec2, DVec3};
use serde::{Deserialize, Serialize};
use std::fmt;

pub mod helper;
pub mod mock;
#[cfg(feature = "occt")]
pub mod occt;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(
            Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub u32);
        impl $name {
            #[inline]
            pub fn idx(self) -> usize {
                self.0 as usize
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

id_type!(BodyId);
id_type!(FaceId);
id_type!(EdgeId);
id_type!(VertexId);

/// Errors from the kernel. Kernel failures never panic across the boundary.
#[derive(Debug, thiserror::Error)]
pub enum KernelError {
    #[error("unknown body {0}")]
    UnknownBody(BodyId),
    #[error("unknown face {0}")]
    UnknownFace(FaceId),
    #[error("unknown edge {0}")]
    UnknownEdge(EdgeId),
    #[error("kernel failure: {0}")]
    Failure(String),
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("not supported by this kernel: {0}")]
    Unsupported(&'static str),
}

pub type KResult<T> = Result<T, KernelError>;

/// Surface type of a face (SPEC §3.4 `surface_info`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SurfaceKind {
    Plane,
    Cylinder,
    Cone,
    Sphere,
    Torus,
    Revolution,
    Extrusion,
    BSpline,
    Other,
}

impl SurfaceKind {
    pub fn name(self) -> &'static str {
        match self {
            SurfaceKind::Plane => "plane",
            SurfaceKind::Cylinder => "cylinder",
            SurfaceKind::Cone => "cone",
            SurfaceKind::Sphere => "sphere",
            SurfaceKind::Torus => "torus",
            SurfaceKind::Revolution => "revolution",
            SurfaceKind::Extrusion => "extrusion",
            SurfaceKind::BSpline => "bspline",
            SurfaceKind::Other => "other",
        }
    }
}

/// Parametric description of a face's underlying surface.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SurfaceInfo {
    pub kind: SurfaceKind,
    /// UV bounds of the *face* (trimmed), not the infinite surface.
    pub u_range: (f64, f64),
    pub v_range: (f64, f64),
    /// Period in u if the surface is u-periodic (cylinder, cone, sphere, torus,
    /// revolution: 2π). The face is "closed in u" when `u_closed` is true, i.e.
    /// the face itself wraps fully around (it has a seam edge).
    pub u_period: Option<f64>,
    pub v_period: Option<f64>,
    pub u_closed: bool,
    pub v_closed: bool,
    /// Axis for planes (normal), cylinders, cones, spheres, tori, revolutions.
    pub axis_origin: Option<DVec3>,
    pub axis_dir: Option<DVec3>,
    /// Radii where defined: cylinder [r], cone [ref radius, semi-angle],
    /// sphere [r], torus [major, minor].
    pub params: Vec<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CurveKind {
    Line,
    Circle,
    Ellipse,
    BSpline,
    Other,
}

/// One use of an edge inside a wire, in wire order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoEdge {
    pub edge: EdgeId,
    /// True if the wire traverses the edge from t1 to t0. Also selects which
    /// pcurve of a seam edge is meant ([`CadKernel::edge_pcurve_eval`]).
    pub reversed: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WireTopo {
    /// The outer wire of a face is listed first and has `outer == true`.
    /// Faces with no outer boundary in the seam-cut domain (full torus,
    /// full sphere) may have zero wires or only seam/degenerate wires.
    pub outer: bool,
    pub coedges: Vec<CoEdge>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FaceTopo {
    pub surface: SurfaceInfo,
    /// Face orientation is reversed relative to the surface normal.
    /// [`CadKernel::face_eval`] already returns the oriented (outward) normal.
    pub reversed: bool,
    pub wires: Vec<WireTopo>,
    pub area: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EdgeTopo {
    pub v0: Option<VertexId>,
    pub v1: Option<VertexId>,
    pub t_range: (f64, f64),
    /// Faces using this edge (1 = open boundary, 2 = manifold; a seam edge
    /// lists its single face once).
    pub faces: Vec<FaceId>,
    pub curve: CurveKind,
    pub length: f64,
    /// Exists only because a periodic surface was cut open.
    pub seam: bool,
    /// Zero-length edge at a pole or apex.
    pub degenerate: bool,
    /// Start and end vertex coincide (full circle).
    pub closed: bool,
    /// Dihedral angle between the two adjacent faces' normals in radians,
    /// averaged along the edge (0 for tangent-continuous). `None` for
    /// boundary/seam/degenerate edges.
    pub dihedral: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VertexTopo {
    pub point: DVec3,
}

/// Full B-rep topology of one body, ids assigned in deterministic traversal
/// order so they survive reload of the same file.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BodyTopology {
    pub faces: Vec<FaceTopo>,
    pub edges: Vec<EdgeTopo>,
    pub vertices: Vec<VertexTopo>,
}

impl BodyTopology {
    pub fn face(&self, f: FaceId) -> &FaceTopo {
        &self.faces[f.idx()]
    }
    pub fn edge(&self, e: EdgeId) -> &EdgeTopo {
        &self.edges[e.idx()]
    }
    pub fn vertex(&self, v: VertexId) -> &VertexTopo {
        &self.vertices[v.idx()]
    }
    pub fn face_ids(&self) -> impl Iterator<Item = FaceId> + '_ {
        (0..self.faces.len() as u32).map(FaceId)
    }
    pub fn edge_ids(&self) -> impl Iterator<Item = EdgeId> + '_ {
        (0..self.edges.len() as u32).map(EdgeId)
    }
}

/// Point, oriented unit normal and first derivatives of a face's surface.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SurfEval {
    pub p: DVec3,
    pub n: DVec3,
    pub du: DVec3,
    pub dv: DVec3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum UvClass {
    In,
    On,
    Out,
}

/// Display tessellation of one face, in body coordinates.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FaceMesh {
    pub positions: Vec<DVec3>,
    pub normals: Vec<DVec3>,
    pub uvs: Vec<DVec2>,
    pub triangles: Vec<[u32; 3]>,
}

/// Non-geometric attributes of a body, from the import sidecar (SPEC §3.4).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BodyAttributes {
    pub name: String,
    pub layer: Option<String>,
    pub color: Option<[f32; 3]>,
    /// Assembly transform: body coordinates → world.
    pub transform: DMat4,
    /// Path of names from the assembly root, for hierarchy display.
    pub assembly_path: Vec<String>,
}

/// What healing changed (SPEC §4.1, §8.1).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HealReport {
    pub fixes: Vec<String>,
    pub failed_faces: Vec<FaceId>,
    pub faces_before: usize,
    pub faces_after: usize,
}

/// The kernel API. All methods are batched (slices in, vectors out) so a
/// live preview makes a handful of calls, not thousands.
///
/// Implementations must be `Send + Sync`. The OCCT implementation serialises
/// every call onto one dedicated kernel thread internally.
pub trait CadKernel: Send + Sync {
    /// Human-readable implementation name ("occt 8.0.1", "mock").
    fn name(&self) -> String;

    /// Load a body from OCCT binary B-rep bytes (`.bbrep`) or text `.brep`.
    /// Returns a fresh body id local to this kernel instance.
    fn load_brep(&self, bytes: &[u8]) -> KResult<BodyId>;
    /// Serialise a body to OCCT binary B-rep bytes.
    fn save_brep(&self, body: BodyId) -> KResult<Vec<u8>>;
    fn unload(&self, body: BodyId);

    fn topology(&self, body: BodyId) -> KResult<BodyTopology>;

    fn face_eval(&self, body: BodyId, face: FaceId, uv: &[DVec2]) -> KResult<Vec<SurfEval>>;
    fn face_classify_uv(&self, body: BodyId, face: FaceId, uv: &[DVec2]) -> KResult<Vec<UvClass>>;
    fn edge_eval(&self, body: BodyId, edge: EdgeId, t: &[f64]) -> KResult<Vec<DVec3>>;
    /// Pcurve of `edge` on `face`. For a seam edge, `reversed` picks which of
    /// its two pcurves (the one used by the coedge with that orientation).
    fn edge_pcurve_eval(
        &self,
        body: BodyId,
        edge: EdgeId,
        face: FaceId,
        reversed: bool,
        t: &[f64],
    ) -> KResult<Vec<DVec2>>;
    /// Closest point on the face (within its trims): (uv, point).
    fn project_to_face(&self, body: BodyId, face: FaceId, p: &[DVec3])
    -> KResult<Vec<Option<(DVec2, DVec3)>>>;
    /// Closest parameter on the edge curve within its range.
    fn project_to_edge(&self, body: BodyId, edge: EdgeId, p: &[DVec3]) -> KResult<Vec<Option<f64>>>;
    fn tessellate(&self, body: BodyId, face: FaceId, tol: f64) -> KResult<FaceMesh>;
}

/// Bounding box helper.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Aabb {
    pub min: DVec3,
    pub max: DVec3,
}

impl Aabb {
    pub const EMPTY: Aabb = Aabb { min: DVec3::splat(f64::INFINITY), max: DVec3::splat(f64::NEG_INFINITY) };
    pub fn add(&mut self, p: DVec3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }
    pub fn union(&self, o: &Aabb) -> Aabb {
        Aabb { min: self.min.min(o.min), max: self.max.max(o.max) }
    }
    pub fn diagonal(&self) -> f64 {
        if self.min.x > self.max.x { 0.0 } else { (self.max - self.min).length() }
    }
    pub fn center(&self) -> DVec3 {
        (self.min + self.max) * 0.5
    }
    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x
    }
}
