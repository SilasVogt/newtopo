//! Build a [`RenderScene`] from the core `Scene` (world space). This is what
//! `newtopo-cli render` uses; the app has a richer builder with picking data.

use crate::camera::{NamedView, OrbitCamera, UpAxis};
use crate::headless::Headless;
use crate::scene::{MeshVertex, OverlayColors, RenderOptions, RenderScene, ShadeMode};
use glam::{DMat4, DVec3, Vec3};
use newtopo_core::mesh::OutEdgeKind;
use newtopo_core::scene::Scene;
use std::path::Path;

/// What to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct CoreDisplay {
    /// Preview mode: output mesh only (SPEC §5.2).
    pub preview: bool,
    /// Wireframe on top of the preview.
    pub wireframe: bool,
    pub colors: OverlayColors,
}

impl Default for CoreDisplay {
    fn default() -> Self {
        CoreDisplay {
            preview: false,
            wireframe: true,
            colors: OverlayColors::default(),
        }
    }
}

fn xf(m: &DMat4, p: DVec3) -> Vec3 {
    m.transform_point3(p).as_vec3()
}

fn xn(m: &DMat4, n: DVec3) -> Vec3 {
    m.transform_vector3(n).normalize_or_zero().as_vec3()
}

/// Fan-triangulate a polygon (good enough for display of convex output
/// polygons; the app uses ear clipping).
fn fan(n: usize) -> impl Iterator<Item = [usize; 3]> {
    (1..n.saturating_sub(1)).map(|k| [0, k, k + 1])
}

/// Build the render scene for every visible body.
pub fn build(scene: &mut Scene, d: &CoreDisplay) -> RenderScene {
    let c = &d.colors;
    let mut rs = RenderScene::default();
    let entries: Vec<_> = scene
        .doc()
        .bodies
        .iter()
        .filter(|b| b.visible)
        .cloned()
        .collect();
    let outputs = scene.output().to_vec();
    for entry in &entries {
        let m = entry.transform;
        let color = entry
            .color
            .map(|c| [c[0], c[1], c[2], 1.0])
            .unwrap_or([0.62, 0.66, 0.72, 1.0]);
        let out = outputs.iter().find(|o| o.body == entry.id);
        if d.preview {
            let Some(out) = out else { continue };
            let pos: Vec<Vec3> = out.vertices.iter().map(|v| xf(&m, v.position)).collect();
            for poly in &out.polygons {
                for t in fan(poly.verts.len()) {
                    let base = rs.mesh.vertices.len() as u32;
                    for k in t {
                        let n = poly.normals.get(k).map(|n| xn(&m, *n)).unwrap_or(Vec3::Z);
                        rs.mesh.vertices.push(MeshVertex {
                            pos: pos[poly.verts[k] as usize].to_array(),
                            normal: n.to_array(),
                            color: c.preview_surface,
                        });
                    }
                    rs.mesh.indices.extend([base, base + 1, base + 2]);
                }
            }
            if d.wireframe {
                for e in &out.edges {
                    rs.add_segment(pos[e.a as usize], pos[e.b as usize], c.preview_wire, 1.0);
                }
            }
            continue;
        }
        if let Some(bv) = scene.body_view(entry.id) {
            for fm in bv.face_meshes {
                let base = rs.mesh.vertices.len() as u32;
                for (k, p) in fm.positions.iter().enumerate() {
                    let n = fm.normals.get(k).map(|n| xn(&m, *n)).unwrap_or(Vec3::Z);
                    rs.mesh.vertices.push(MeshVertex {
                        pos: xf(&m, *p).to_array(),
                        normal: n.to_array(),
                        color,
                    });
                }
                rs.mesh
                    .indices
                    .extend(fm.triangles.iter().flat_map(|t| t.map(|i| i + base)));
            }
            if out.is_none() {
                for (i, pl) in bv.edge_polylines.iter().enumerate() {
                    if bv
                        .topology
                        .edges
                        .get(i)
                        .is_some_and(|e| e.seam || e.degenerate)
                    {
                        continue;
                    }
                    let pts: Vec<Vec3> = pl.iter().map(|p| xf(&m, *p)).collect();
                    rs.add_polyline(&pts, c.cad_edge, 1.8);
                }
            }
        }
        if let Some(out) = out {
            for e in &out.edges {
                let (col, w) = match e.kind {
                    OutEdgeKind::Cad(_) | OutEdgeKind::Split(_) => (c.cad_edge, 1.8),
                    OutEdgeKind::Line(_) => (c.line, 1.6),
                    OutEdgeKind::Fill => (c.fill_edge, 1.1),
                };
                let pts: Vec<Vec3> = if e.path.len() >= 2 {
                    e.path.iter().map(|p| xf(&m, *p)).collect()
                } else {
                    vec![
                        xf(&m, out.vertices[e.a as usize].position),
                        xf(&m, out.vertices[e.b as usize].position),
                    ]
                };
                rs.add_polyline(&pts, col, w);
            }
        }
    }
    rs
}

/// One-call helper for `newtopo-cli render in.ntopo --view iso [--preview] -o shot.png`.
pub fn render_core_scene_to_png(
    scene: &mut Scene,
    view: NamedView,
    display: &CoreDisplay,
    up: UpAxis,
    size: [u32; 2],
    path: &Path,
) -> anyhow::Result<()> {
    let rs = build(scene, display);
    let mut cam = OrbitCamera {
        up,
        ..Default::default()
    };
    cam.set_view(view);
    if let Some((min, max)) = rs.bounds() {
        cam.frame_aabb(
            min.as_dvec3(),
            max.as_dvec3(),
            size[0] as f64 / size[1].max(1) as f64,
        );
    }
    let opts = RenderOptions {
        shade: ShadeMode::Smooth,
        background_top: display.colors.background_top,
        background_bottom: display.colors.background_bottom,
        pixels_per_point: (size[1] as f32 / 800.0).max(1.0),
        ..Default::default()
    };
    let mut h = Headless::new()?;
    let img = h.render(&[&rs], &cam, size, &opts)?;
    img.save(path)?;
    Ok(())
}
