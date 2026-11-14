//! newtopo renderer (SPEC §3.2, §5.2): a small wgpu renderer that draws the
//! viewport inside egui (feature `egui`, via `egui_wgpu` paint callbacks)
//! and headless for `newtopo-cli render` and visual tests. Both paths share
//! [`gpu::Renderer`]. OWNER: app.
//!
//! * [`scene`]: what gets drawn (world-space triangles, lines, dots).
//! * [`camera`]: orbit camera and framing math.
//! * [`pick`]: CPU picking (BVH ray cast, screen-space edge/vertex search).
//! * [`headless`]: headless rendering, `render_scene_to_png`.
//! * [`synth`]: a synthetic CAD-like model for tests and screenshots.

pub mod camera;
#[cfg(feature = "egui")]
pub mod egui_viewport;
pub mod gpu;
pub mod headless;
pub mod pick;
pub mod scene;
pub mod synth;

pub use camera::{CameraMatrices, NamedView, OrbitCamera, Ray, UpAxis};
pub use headless::{Headless, render_scene_to_png};
pub use scene::{
    LineSeg, MeshVertex, OverlayColors, PointMark, RenderOptions, RenderScene, ShadeMode, TriMesh,
};
