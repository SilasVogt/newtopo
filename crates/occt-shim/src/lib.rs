//! cxx bridge to Open CASCADE (SPEC §3.4).
//!
//! The API is flat and id-based: bodies are `u32` ids into a [`ffi::Store`]
//! owned by C++; faces, edges and vertices are 0-based indices in OCCT's
//! deterministic `TopExp::MapShapes` traversal order. No OCCT type crosses.
//! Every fallible call returns `Result`; the C++ side converts
//! `Standard_Failure` and every other C++ exception into the error.
//!
//! OCCT has process-global state, so all calls go through one dedicated
//! thread: [`run`] sends a closure to it and waits for the result.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::OnceLock;
use std::sync::mpsc;
use std::thread;

pub use cxx;

#[cfg(feature = "testshapes")]
pub mod testshapes;

#[cxx::bridge(namespace = "ntshim")]
pub mod ffi {
    struct HealOptions {
        /// Run ShapeFix_Shape.
        fix: bool,
        /// Sew faces when the shape has free edges.
        sew: bool,
        sewing_tolerance: f64,
    }

    struct HealResult {
        fixes: Vec<String>,
        /// Face indices (after healing) that fail BRepCheck.
        failed_faces: Vec<u32>,
        faces_before: usize,
        faces_after: usize,
    }

    struct SurfaceInfoFfi {
        /// 0 plane, 1 cylinder, 2 cone, 3 sphere, 4 torus, 5 revolution,
        /// 6 extrusion, 7 bspline/bezier, 8 other.
        kind: u8,
        u0: f64,
        u1: f64,
        v0: f64,
        v1: f64,
        /// 0 when not periodic.
        u_period: f64,
        v_period: f64,
        u_closed: bool,
        v_closed: bool,
        has_origin: bool,
        axis_origin: [f64; 3],
        has_axis: bool,
        axis_dir: [f64; 3],
        params: Vec<f64>,
    }

    struct WireFfi {
        outer: bool,
        edges: Vec<u32>,
        reversed: Vec<bool>,
    }

    struct FaceFfi {
        surface: SurfaceInfoFfi,
        reversed: bool,
        area: f64,
        wires: Vec<WireFfi>,
    }

    struct EdgeFfi {
        /// -1 when absent.
        v0: i64,
        v1: i64,
        t0: f64,
        t1: f64,
        faces: Vec<u32>,
        /// 0 line, 1 circle, 2 ellipse, 3 bspline/bezier, 4 other.
        curve: u8,
        length: f64,
        seam: bool,
        degenerate: bool,
        closed: bool,
        /// NaN when undefined.
        dihedral: f64,
    }

    struct TopologyFfi {
        faces: Vec<FaceFfi>,
        edges: Vec<EdgeFfi>,
        /// xyz per vertex.
        vertices: Vec<f64>,
    }

    struct MeshFfi {
        positions: Vec<f64>,
        normals: Vec<f64>,
        uvs: Vec<f64>,
        triangles: Vec<u32>,
    }

    struct ImportedBodyFfi {
        body: u32,
        name: String,
        has_layer: bool,
        layer: String,
        has_color: bool,
        /// sRGB 0..1 as stored in the file.
        color: [f32; 3],
        /// Row-major 4×4, body → world.
        transform: [f64; 16],
        path: Vec<String>,
    }

    struct ImportResultFfi {
        bodies: Vec<ImportedBodyFfi>,
        warnings: Vec<String>,
    }

    unsafe extern "C++" {
        include!("newtopo-occt-shim/cpp/shim.h");

        /// Owns every loaded body and its caches.
        type Store;

        fn new_store() -> UniquePtr<Store>;
        /// "8.0.1" etc.
        fn occt_version() -> String;
        /// Installs OCCT's signal handlers. Import helper process only.
        fn enable_signal_handlers() -> Result<()>;

        fn load_brep(store: Pin<&mut Store>, bytes: &[u8]) -> Result<u32>;
        fn save_brep(store: Pin<&mut Store>, body: u32) -> Result<Vec<u8>>;
        fn unload(store: Pin<&mut Store>, body: u32);
        fn body_count(store: &Store) -> usize;

        /// format: 0 STEP, 1 IGES, 2 BREP. `unit_m`: target length unit in
        /// metres (0.001 = mm).
        fn import_cad(
            store: Pin<&mut Store>,
            path: &str,
            format: u8,
            unit_m: f64,
        ) -> Result<ImportResultFfi>;
        fn heal(store: Pin<&mut Store>, body: u32, opts: &HealOptions) -> Result<HealResult>;

        fn topology(store: Pin<&mut Store>, body: u32) -> Result<TopologyFfi>;
        /// uv pairs in; 12 doubles out per point (p, n, du, dv).
        fn face_eval(store: Pin<&mut Store>, body: u32, face: u32, uv: &[f64]) -> Result<Vec<f64>>;
        /// 0 in, 1 on, 2 out.
        fn face_classify_uv(
            store: Pin<&mut Store>,
            body: u32,
            face: u32,
            uv: &[f64],
        ) -> Result<Vec<u8>>;
        /// 3 doubles out per parameter.
        fn edge_eval(store: Pin<&mut Store>, body: u32, edge: u32, t: &[f64]) -> Result<Vec<f64>>;
        /// 2 doubles out per parameter.
        fn edge_pcurve_eval(
            store: Pin<&mut Store>,
            body: u32,
            edge: u32,
            face: u32,
            reversed: bool,
            t: &[f64],
        ) -> Result<Vec<f64>>;
        /// xyz in; 6 doubles out per point: ok (0/1), u, v, x, y, z.
        fn project_to_face(
            store: Pin<&mut Store>,
            body: u32,
            face: u32,
            p: &[f64],
        ) -> Result<Vec<f64>>;
        /// xyz in; one parameter out per point (NaN = failed).
        fn project_to_edge(
            store: Pin<&mut Store>,
            body: u32,
            edge: u32,
            p: &[f64],
        ) -> Result<Vec<f64>>;
        fn tessellate(store: Pin<&mut Store>, body: u32, face: u32, tol: f64) -> Result<MeshFfi>;
    }
}

/// State owned by the OCCT thread.
pub struct Ctx {
    pub store: cxx::UniquePtr<ffi::Store>,
}

impl Ctx {
    pub fn store(&mut self) -> std::pin::Pin<&mut ffi::Store> {
        self.store.pin_mut()
    }
}

type Job = Box<dyn FnOnce(&mut Ctx) + Send>;

static OCCT_THREAD: OnceLock<(mpsc::Sender<Job>, thread::ThreadId)> = OnceLock::new();

fn occt_thread() -> &'static (mpsc::Sender<Job>, thread::ThreadId) {
    OCCT_THREAD.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        let handle = thread::Builder::new()
            .name("occt".into())
            // OCCT recurses deeply in booleans and STEP translation.
            .stack_size(64 << 20)
            .spawn(move || {
                let mut ctx = Ctx {
                    store: ffi::new_store(),
                };
                while let Ok(job) = rx.recv() {
                    // A panicking job only loses its own reply channel.
                    let _ = catch_unwind(AssertUnwindSafe(|| job(&mut ctx)));
                }
            })
            .expect("spawn OCCT thread");
        (tx, handle.thread().id())
    })
}

/// Runs `f` on the single OCCT thread and waits for its result.
///
/// Errors if the closure panicked or if called re-entrantly from the OCCT
/// thread itself (that would deadlock).
pub fn run<R: Send + 'static>(f: impl FnOnce(&mut Ctx) -> R + Send + 'static) -> Result<R, String> {
    let (tx, id) = occt_thread();
    if thread::current().id() == *id {
        return Err("re-entrant call on the OCCT thread".into());
    }
    let (rtx, rrx) = mpsc::sync_channel(1);
    tx.send(Box::new(move |ctx: &mut Ctx| {
        let _ = rtx.send(f(ctx));
    }))
    .map_err(|_| "OCCT thread is gone".to_string())?;
    rrx.recv().map_err(|_| "OCCT call panicked".to_string())
}

/// OCCT version string, e.g. "8.0.1".
pub fn version() -> String {
    ffi::occt_version()
}
