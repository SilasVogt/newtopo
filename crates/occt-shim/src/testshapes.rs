//! Test-shape builder (SPEC §3.4, feature `testshapes`). Exists only to
//! generate the reference models of SPEC §11; outside the shim line budget.
//!
//! Bodies are ids in the same [`crate::ffi::Store`] as everything else; edge
//! ids for fillets and chamfers are the topology edge ids of that body.

#[cxx::bridge(namespace = "ntshim")]
pub mod ffi {
    /// One node of the XCAF tree written by [`ts_write_step`]. Parents come
    /// before children. `body < 0` makes an assembly node.
    struct XcafNode {
        name: String,
        /// Index of the parent node, -1 for a top-level shape.
        parent: i32,
        body: i32,
        /// Empty for none.
        layer: String,
        has_color: bool,
        /// sRGB 0..1.
        color: [f32; 3],
        /// Row-major 4×4 placement in the parent (rigid).
        transform: [f64; 16],
    }

    unsafe extern "C++" {
        include!("newtopo-occt-shim/cpp/testshapes.h");
        type Store = crate::ffi::Store;

        fn ts_box(store: Pin<&mut Store>, x: f64, y: f64, z: f64, dx: f64, dy: f64, dz: f64) -> Result<u32>;
        /// Axis +Z through the origin, base at z = 0.
        fn ts_cylinder(store: Pin<&mut Store>, r: f64, h: f64) -> Result<u32>;
        fn ts_cone(store: Pin<&mut Store>, r1: f64, r2: f64, h: f64) -> Result<u32>;
        fn ts_sphere(store: Pin<&mut Store>, r: f64) -> Result<u32>;
        fn ts_torus(store: Pin<&mut Store>, major: f64, minor: f64) -> Result<u32>;
        /// New body = `body` moved by a row-major 3×4 rigid matrix.
        fn ts_transform(store: Pin<&mut Store>, body: u32, m: &[f64]) -> Result<u32>;
        /// Empty `edges` = every non-seam, non-degenerate edge.
        fn ts_fillet(store: Pin<&mut Store>, body: u32, edges: &[u32], r: f64) -> Result<u32>;
        fn ts_chamfer(store: Pin<&mut Store>, body: u32, edges: &[u32], d: f64) -> Result<u32>;
        /// op: 0 fuse, 1 cut, 2 common. Tools are applied in one operation.
        fn ts_boolean(store: Pin<&mut Store>, op: u8, a: u32, tools: &[u32]) -> Result<u32>;
        /// Revolves a closed (r, z) profile 360° about +Z. Points with index in
        /// `[spline_from, spline_to]` are joined by one interpolated B-spline,
        /// the rest by lines; the last point connects back to the first.
        fn ts_revolve(store: Pin<&mut Store>, profile: &[f64], spline_from: i32, spline_to: i32) -> Result<u32>;
        /// Extrudes a closed (x, y) polygon along +Z.
        fn ts_prism(store: Pin<&mut Store>, polygon: &[f64], height: f64) -> Result<u32>;
        /// Writes a body as OCCT text B-rep (`BRepTools::Write`).
        fn ts_write_brep_text(store: Pin<&mut Store>, body: u32, path: &str) -> Result<()>;
        /// Writes an XCAF STEP file (AP214, mm) with names, layers and colours.
        fn ts_write_step(store: Pin<&mut Store>, path: &str, nodes: &[XcafNode]) -> Result<()>;
    }
}
