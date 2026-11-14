//! OCCT implementation of [`CadKernel`] via `occt-shim` (SPEC §3.4).
//!
//! All OCCT state lives on one dedicated thread owned by the shim crate
//! ([`newtopo_occt_shim::run`]); every call here is a closure sent to it, so
//! `OcctKernel` is `Send + Sync` and calls are serialised. Body ids are unique
//! per process: every `OcctKernel` value shares the same thread and store.
//!
//! Conventions (beyond the trait docs):
//! * Face, edge and vertex ids are OCCT `TopExp::MapShapes` traversal order.
//! * Wires are reported for the face in FORWARD orientation: the outer wire
//!   runs counter-clockwise in (u, v), holes clockwise. `FaceTopo::reversed`
//!   says the outward normal is −(du × dv); `face_eval` already flips it.
//! * `CoEdge::reversed` is that FORWARD-face wire's edge orientation, so it
//!   is exactly what [`CadKernel::edge_pcurve_eval`] expects for seams.

use crate::{
    BodyAttributes, BodyId, BodyTopology, CadKernel, CoEdge, CurveKind, EdgeId, EdgeTopo, FaceId, FaceMesh,
    FaceTopo, HealReport, KResult, KernelError, SurfEval, SurfaceInfo, SurfaceKind, UvClass, VertexId, VertexTopo,
    WireTopo,
};
use glam::{DMat4, DVec2, DVec3};
use newtopo_occt_shim::{self as shim, Ctx, ffi};
use std::path::Path;

/// CAD file formats the shim reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CadFormat {
    Step,
    Iges,
    Brep,
}

impl CadFormat {
    /// From the file extension (`.step .stp .iges .igs .brep .bbrep .brp`).
    pub fn from_path(path: &Path) -> Option<CadFormat> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "step" | "stp" | "p21" => Some(CadFormat::Step),
            "iges" | "igs" => Some(CadFormat::Iges),
            "brep" | "brp" | "bbrep" => Some(CadFormat::Brep),
            _ => None,
        }
    }
    fn code(self) -> u8 {
        match self {
            CadFormat::Step => 0,
            CadFormat::Iges => 1,
            CadFormat::Brep => 2,
        }
    }
}

/// Length unit in metres for a unit name ("mm", "cm", "m", "in", "ft").
pub fn unit_in_metres(name: &str) -> Option<f64> {
    Some(match name.to_ascii_lowercase().as_str() {
        "mm" => 0.001,
        "cm" => 0.01,
        "m" => 1.0,
        "in" | "inch" => 0.0254,
        "ft" => 0.3048,
        _ => return None,
    })
}

/// Healing settings (SPEC §4.1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HealSettings {
    /// Run `ShapeFix_Shape`.
    pub fix: bool,
    /// Sew faces when the body has free edges.
    pub sew: bool,
    pub sewing_tolerance: f64,
}

impl Default for HealSettings {
    fn default() -> Self {
        HealSettings { fix: true, sew: true, sewing_tolerance: 1e-3 }
    }
}

/// One body read from a CAD file, still in the kernel.
#[derive(Clone, Debug)]
pub struct LoadedBody {
    pub body: BodyId,
    pub attrs: BodyAttributes,
}

#[derive(Clone, Debug)]
pub struct LoadedFile {
    pub bodies: Vec<LoadedBody>,
    pub warnings: Vec<String>,
}

/// Open CASCADE kernel. Cheap to create; all instances share one thread.
#[derive(Clone, Debug, Default)]
pub struct OcctKernel {
    _private: (),
}

fn failure(msg: impl Into<String>) -> KernelError {
    KernelError::Failure(msg.into())
}

/// Maps the shim's "unknown body/face/edge N" messages to typed errors.
fn map_err(e: shim::cxx::Exception, body: BodyId, face: Option<FaceId>, edge: Option<EdgeId>) -> KernelError {
    let m = e.what();
    if m.starts_with("unknown body") {
        KernelError::UnknownBody(body)
    } else if let (true, Some(f)) = (m.starts_with("unknown face"), face) {
        KernelError::UnknownFace(f)
    } else if let (true, Some(ed)) = (m.starts_with("unknown edge"), edge) {
        KernelError::UnknownEdge(ed)
    } else {
        failure(m)
    }
}

fn v3(a: &[f64]) -> DVec3 {
    DVec3::new(a[0], a[1], a[2])
}

fn surface_kind(k: u8) -> SurfaceKind {
    match k {
        0 => SurfaceKind::Plane,
        1 => SurfaceKind::Cylinder,
        2 => SurfaceKind::Cone,
        3 => SurfaceKind::Sphere,
        4 => SurfaceKind::Torus,
        5 => SurfaceKind::Revolution,
        6 => SurfaceKind::Extrusion,
        7 => SurfaceKind::BSpline,
        _ => SurfaceKind::Other,
    }
}

fn curve_kind(k: u8) -> CurveKind {
    match k {
        0 => CurveKind::Line,
        1 => CurveKind::Circle,
        2 => CurveKind::Ellipse,
        3 => CurveKind::BSpline,
        _ => CurveKind::Other,
    }
}

fn convert_topology(t: ffi::TopologyFfi) -> BodyTopology {
    let faces = t
        .faces
        .into_iter()
        .map(|f| {
            let s = f.surface;
            FaceTopo {
                surface: SurfaceInfo {
                    kind: surface_kind(s.kind),
                    u_range: (s.u0, s.u1),
                    v_range: (s.v0, s.v1),
                    u_period: (s.u_period > 0.0).then_some(s.u_period),
                    v_period: (s.v_period > 0.0).then_some(s.v_period),
                    u_closed: s.u_closed,
                    v_closed: s.v_closed,
                    axis_origin: s.has_origin.then(|| v3(&s.axis_origin)),
                    axis_dir: s.has_axis.then(|| v3(&s.axis_dir)),
                    params: s.params,
                },
                reversed: f.reversed,
                wires: f
                    .wires
                    .into_iter()
                    .map(|w| WireTopo {
                        outer: w.outer,
                        coedges: w
                            .edges
                            .iter()
                            .zip(w.reversed.iter())
                            .map(|(&e, &r)| CoEdge { edge: EdgeId(e), reversed: r })
                            .collect(),
                    })
                    .collect(),
                area: f.area,
            }
        })
        .collect();
    let edges = t
        .edges
        .into_iter()
        .map(|e| EdgeTopo {
            v0: (e.v0 >= 0).then_some(VertexId(e.v0 as u32)),
            v1: (e.v1 >= 0).then_some(VertexId(e.v1 as u32)),
            t_range: (e.t0, e.t1),
            faces: e.faces.into_iter().map(FaceId).collect(),
            curve: curve_kind(e.curve),
            length: e.length,
            seam: e.seam,
            degenerate: e.degenerate,
            closed: e.closed,
            dihedral: (!e.dihedral.is_nan()).then_some(e.dihedral),
        })
        .collect();
    let vertices = t.vertices.chunks_exact(3).map(|c| VertexTopo { point: v3(c) }).collect();
    BodyTopology { faces, edges, vertices }
}

fn convert_attrs(b: &ffi::ImportedBodyFfi) -> BodyAttributes {
    // Shim matrices are row-major; glam is column-major.
    let transform = DMat4::from_cols_array(&b.transform).transpose();
    BodyAttributes {
        name: b.name.clone(),
        layer: b.has_layer.then(|| b.layer.clone()),
        color: b.has_color.then_some(b.color),
        transform,
        assembly_path: b.path.clone(),
    }
}

impl OcctKernel {
    pub fn new() -> Self {
        OcctKernel { _private: () }
    }

    /// OCCT version, e.g. "8.0.1".
    pub fn occt_version() -> String {
        shim::version()
    }

    /// Runs `f` on the OCCT thread.
    fn call<R: Send + 'static>(&self, f: impl FnOnce(&mut Ctx) -> KResult<R> + Send + 'static) -> KResult<R> {
        shim::run(f).map_err(failure)?
    }

    /// Installs OCCT's process-wide signal handlers so crashes inside the CAD
    /// readers become errors. **Only the import helper process may call
    /// this** (SPEC §3.4); never the app.
    pub fn enable_signal_handlers() -> KResult<()> {
        shim::run(|_| ffi::enable_signal_handlers()).map_err(failure)?.map_err(|e| failure(e.what()))
    }

    /// Reads a STEP/IGES/BREP file through XCAF: one body per leaf shape
    /// (assemblies flattened, multi-solid shapes split), with names, layers,
    /// colours and assembly transforms. `unit_m` is the target length unit in
    /// metres (0.001 = mm). Does not heal.
    pub fn import_file(&self, path: &Path, format: CadFormat, unit_m: f64) -> KResult<LoadedFile> {
        let path = path.to_str().ok_or_else(|| failure("path is not valid UTF-8"))?.to_owned();
        self.call(move |ctx| {
            let r = ffi::import_cad(ctx.store(), &path, format.code(), unit_m).map_err(|e| failure(e.what()))?;
            Ok(LoadedFile {
                bodies: r
                    .bodies
                    .iter()
                    .map(|b| LoadedBody { body: BodyId(b.body), attrs: convert_attrs(b) })
                    .collect(),
                warnings: r.warnings,
            })
        })
    }

    /// Heals a body in place (ShapeFix + sewing). Face ids change.
    pub fn heal(&self, body: BodyId, settings: HealSettings) -> KResult<HealReport> {
        self.call(move |ctx| {
            let opts =
                ffi::HealOptions { fix: settings.fix, sew: settings.sew, sewing_tolerance: settings.sewing_tolerance };
            let r = ffi::heal(ctx.store(), body.0, &opts).map_err(|e| map_err(e, body, None, None))?;
            Ok(HealReport {
                fixes: r.fixes,
                failed_faces: r.failed_faces.into_iter().map(FaceId).collect(),
                faces_before: r.faces_before,
                faces_after: r.faces_after,
            })
        })
    }

    /// Number of bodies currently loaded (all `OcctKernel` values).
    pub fn body_count(&self) -> usize {
        self.call(|ctx| Ok(ffi::body_count(&ctx.store))).unwrap_or(0)
    }
}

impl CadKernel for OcctKernel {
    fn name(&self) -> String {
        format!("occt {}", shim::version())
    }

    fn load_brep(&self, bytes: &[u8]) -> KResult<BodyId> {
        let bytes = bytes.to_vec();
        self.call(move |ctx| ffi::load_brep(ctx.store(), &bytes).map(BodyId).map_err(|e| failure(e.what())))
    }

    fn save_brep(&self, body: BodyId) -> KResult<Vec<u8>> {
        self.call(move |ctx| ffi::save_brep(ctx.store(), body.0).map_err(|e| map_err(e, body, None, None)))
    }

    fn unload(&self, body: BodyId) {
        let _ = self.call(move |ctx| {
            ffi::unload(ctx.store(), body.0);
            Ok(())
        });
    }

    fn topology(&self, body: BodyId) -> KResult<BodyTopology> {
        self.call(move |ctx| {
            ffi::topology(ctx.store(), body.0).map(convert_topology).map_err(|e| map_err(e, body, None, None))
        })
    }

    fn face_eval(&self, body: BodyId, face: FaceId, uv: &[DVec2]) -> KResult<Vec<SurfEval>> {
        let flat: Vec<f64> = uv.iter().flat_map(|p| [p.x, p.y]).collect();
        self.call(move |ctx| {
            let out = ffi::face_eval(ctx.store(), body.0, face.0, &flat).map_err(|e| map_err(e, body, Some(face), None))?;
            Ok(out
                .chunks_exact(12)
                .map(|c| SurfEval { p: v3(&c[0..3]), n: v3(&c[3..6]), du: v3(&c[6..9]), dv: v3(&c[9..12]) })
                .collect())
        })
    }

    fn face_classify_uv(&self, body: BodyId, face: FaceId, uv: &[DVec2]) -> KResult<Vec<UvClass>> {
        let flat: Vec<f64> = uv.iter().flat_map(|p| [p.x, p.y]).collect();
        self.call(move |ctx| {
            let out = ffi::face_classify_uv(ctx.store(), body.0, face.0, &flat)
                .map_err(|e| map_err(e, body, Some(face), None))?;
            Ok(out
                .into_iter()
                .map(|c| match c {
                    0 => UvClass::In,
                    1 => UvClass::On,
                    _ => UvClass::Out,
                })
                .collect())
        })
    }

    fn edge_eval(&self, body: BodyId, edge: EdgeId, t: &[f64]) -> KResult<Vec<DVec3>> {
        let t = t.to_vec();
        self.call(move |ctx| {
            let out = ffi::edge_eval(ctx.store(), body.0, edge.0, &t).map_err(|e| map_err(e, body, None, Some(edge)))?;
            Ok(out.chunks_exact(3).map(v3).collect())
        })
    }

    fn edge_pcurve_eval(
        &self,
        body: BodyId,
        edge: EdgeId,
        face: FaceId,
        reversed: bool,
        t: &[f64],
    ) -> KResult<Vec<DVec2>> {
        let t = t.to_vec();
        self.call(move |ctx| {
            let out = ffi::edge_pcurve_eval(ctx.store(), body.0, edge.0, face.0, reversed, &t)
                .map_err(|e| map_err(e, body, Some(face), Some(edge)))?;
            Ok(out.chunks_exact(2).map(|c| DVec2::new(c[0], c[1])).collect())
        })
    }

    fn project_to_face(&self, body: BodyId, face: FaceId, p: &[DVec3]) -> KResult<Vec<Option<(DVec2, DVec3)>>> {
        let flat: Vec<f64> = p.iter().flat_map(|q| q.to_array()).collect();
        self.call(move |ctx| {
            let out = ffi::project_to_face(ctx.store(), body.0, face.0, &flat)
                .map_err(|e| map_err(e, body, Some(face), None))?;
            Ok(out
                .chunks_exact(6)
                .map(|c| (c[0] > 0.5).then(|| (DVec2::new(c[1], c[2]), v3(&c[3..6]))))
                .collect())
        })
    }

    fn project_to_edge(&self, body: BodyId, edge: EdgeId, p: &[DVec3]) -> KResult<Vec<Option<f64>>> {
        let flat: Vec<f64> = p.iter().flat_map(|q| q.to_array()).collect();
        self.call(move |ctx| {
            let out = ffi::project_to_edge(ctx.store(), body.0, edge.0, &flat)
                .map_err(|e| map_err(e, body, None, Some(edge)))?;
            Ok(out.into_iter().map(|t| (!t.is_nan()).then_some(t)).collect())
        })
    }

    fn tessellate(&self, body: BodyId, face: FaceId, tol: f64) -> KResult<FaceMesh> {
        self.call(move |ctx| {
            let m = ffi::tessellate(ctx.store(), body.0, face.0, tol).map_err(|e| map_err(e, body, Some(face), None))?;
            Ok(FaceMesh {
                positions: m.positions.chunks_exact(3).map(v3).collect(),
                normals: m.normals.chunks_exact(3).map(v3).collect(),
                uvs: m.uvs.chunks_exact(2).map(|c| DVec2::new(c[0], c[1])).collect(),
                triangles: m.triangles.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect(),
            })
        })
    }
}
