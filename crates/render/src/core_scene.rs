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

/// Triangulate an output polygon for shading, concave n-gons included (the
/// same triangulation export uses: ear clipping in the best-fit plane).
fn poly_triangles(pos: &[DVec3]) -> impl Iterator<Item = [usize; 3]> {
    newtopo_core::fill::triangulate_3d(pos)
        .into_iter()
        .map(|t| t.map(|i| i as usize))
}

/// Bodies to draw: visible themselves and on a visible layer (the app's
/// outliner rules; hidden layers also don't count for framing).
pub fn visible_bodies(doc: &newtopo_core::doc::Document) -> Vec<newtopo_core::doc::BodyEntry> {
    doc.bodies
        .iter()
        .filter(|b| b.visible && doc.layer(b.layer).is_none_or(|l| l.visible))
        .cloned()
        .collect()
}

/// Build the render scene for every visible body.
pub fn build(scene: &mut Scene, d: &CoreDisplay) -> RenderScene {
    let c = &d.colors;
    let mut rs = RenderScene::default();
    let entries = visible_bodies(scene.doc());
    let outputs = scene.output().to_vec();
    for entry in &entries {
        let m = entry.transform;
        let color = entry
            .color
            .map(|c| [c[0], c[1], c[2], 1.0])
            .unwrap_or([0.62, 0.66, 0.72, 1.0]);
        let out = outputs.iter().find(|o| o.body == entry.id);
        // A body with mesh edits is drawn from its edited polygons in both
        // modes: its CAD face meshes no longer show its shape (SPEC §9.5).
        let edited = scene.has_mesh_edits(entry.id);
        if let Some(out) = out.filter(|_| d.preview || edited) {
            let pos: Vec<Vec3> = out.vertices.iter().map(|v| xf(&m, v.position)).collect();
            let surface = if d.preview { c.preview_surface } else { color };
            for poly in &out.polygons {
                let local: Vec<DVec3> = poly
                    .verts
                    .iter()
                    .map(|&v| out.vertices[v as usize].position)
                    .collect();
                for t in poly_triangles(&local) {
                    let base = rs.mesh.vertices.len() as u32;
                    for k in t {
                        let n = poly.normals.get(k).map(|n| xn(&m, *n)).unwrap_or(Vec3::Z);
                        rs.mesh.vertices.push(MeshVertex {
                            pos: pos[poly.verts[k] as usize].to_array(),
                            normal: n.to_array(),
                            color: surface,
                        });
                    }
                    rs.mesh.indices.extend([base, base + 1, base + 2]);
                }
            }
            if d.preview && d.wireframe {
                for e in &out.edges {
                    rs.add_segment(pos[e.a as usize], pos[e.b as usize], c.preview_wire, 1.0);
                }
            }
        }
        if d.preview {
            continue;
        }
        if let Some(bv) = scene.body_view(entry.id) {
            if !edited {
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
                    OutEdgeKind::Cad(_)
                    | OutEdgeKind::Split(_)
                    | OutEdgeKind::Mesh { cad: true } => (c.cad_edge, 1.8),
                    OutEdgeKind::Line(_) | OutEdgeKind::Mesh { cad: false } => (c.line, 1.6),
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

#[cfg(test)]
mod tests {
    use super::*;
    use newtopo_core::doc::{BodyEntry, Document, Layer, LayerId};
    use newtopo_kernel::BodyId;

    fn body(id: u32, layer: u32, visible: bool) -> BodyEntry {
        BodyEntry {
            id: BodyId(id),
            name: format!("b{id}"),
            layer: LayerId(layer),
            color: None,
            transform: glam::DMat4::IDENTITY,
            visible,
            locked: false,
            heal: Default::default(),
            source_sha256: None,
        }
    }

    /// A concave n-gon (a U) is shaded without covering its notch, and its
    /// triangles add up to the polygon's area (a fan from vertex 0 gave 11).
    #[test]
    fn concave_polygon_is_not_fan_filled() {
        let uv = [
            (0.0, 0.0),
            (3.0, 0.0),
            (3.0, 3.0),
            (2.0, 3.0),
            (2.0, 1.0),
            (1.0, 1.0),
            (1.0, 3.0),
            (0.0, 3.0),
        ];
        let pos: Vec<DVec3> = uv.iter().map(|&(x, y)| DVec3::new(x, y, 0.0)).collect();
        let tris: Vec<[usize; 3]> = poly_triangles(&pos).collect();
        assert_eq!(tris.len(), 6);
        let notch = glam::DVec2::new(1.5, 1.75);
        let mut area = 0.0;
        for t in &tris {
            let [a, b, c] = t.map(|i| pos[i].truncate());
            let cr = |o: glam::DVec2, p: glam::DVec2, q: glam::DVec2| (p - o).perp_dot(q - o);
            let signed = cr(a, b, c);
            assert!(signed > 0.0, "triangle {t:?} keeps the polygon's winding");
            area += signed * 0.5;
            let inside = cr(a, b, notch) >= 0.0 && cr(b, c, notch) >= 0.0 && cr(c, a, notch) >= 0.0;
            assert!(!inside, "triangle {t:?} covers the notch");
        }
        assert!((area - 7.0).abs() < 1e-9, "area {area}");
    }

    /// Without `--preview`, a body with mesh edits is shaded from its edited
    /// polygons, not from the CAD face meshes under them.
    #[test]
    fn edited_body_is_shaded_from_its_mesh_without_preview() {
        use newtopo_core::Op;
        use newtopo_core::ops::PatchSel;
        use newtopo_kernel::mock::{MockBody, MockKernel};
        use newtopo_kernel::{BodyAttributes, HealReport};
        use std::sync::Arc;

        let body = MockBody::make_box(DVec3::new(4.0, 3.0, 2.0));
        let mut s = Scene::new(Arc::new(MockKernel::new()));
        let attrs = BodyAttributes {
            name: body.name.clone(),
            transform: DMat4::IDENTITY,
            ..Default::default()
        };
        let b = s
            .add_body(&body.to_brep_bytes(), attrs, HealReport::default())
            .unwrap();
        s.apply(Op::SetAutofill { on: true }).unwrap();
        for f in 0..6 {
            s.apply(Op::Grid {
                patches: PatchSel {
                    faces: vec![f],
                    ..Default::default()
                },
                u: 4,
                v: 4,
                start_on_seam: false,
                spacing: newtopo_core::doc::Spacing::ArcLength,
            })
            .unwrap();
        }
        // Move every free vertex off its face along the face normal.
        let mut m = (*s.editable_mesh(b).unwrap()).clone();
        let mut push = vec![DVec3::ZERO; m.positions.len()];
        for p in &m.polygons {
            for (k, &v) in p.verts.iter().enumerate() {
                push[v as usize] += p.normals[k];
            }
        }
        for (i, p) in m.positions.iter_mut().enumerate() {
            if !m.pinned[i] {
                *p += push[i].normalize_or_zero() * 0.25;
            }
        }
        s.set_meshes(vec![(b, Some(m.clone()))]).unwrap();

        let rs = build(&mut s, &CoreDisplay::default());
        let fan_verts: usize = m.polygons.iter().map(|p| 3 * (p.verts.len() - 2)).sum();
        assert_eq!(
            rs.mesh.vertices.len(),
            fan_verts,
            "only the edited polygons are shaded (no CAD face meshes)"
        );
        let moved = m.positions[m.pinned.iter().position(|p| !p).unwrap()].as_vec3();
        assert!(
            rs.mesh
                .vertices
                .iter()
                .any(|v| Vec3::from(v.pos).distance(moved) < 1e-5),
            "a moved vertex is drawn where the edit put it"
        );
    }

    #[test]
    fn hidden_layers_and_bodies_are_not_drawn() {
        let mut doc = Document::default();
        doc.layers.push(Layer {
            id: LayerId(1),
            name: "Hidden".into(),
            visible: false,
            locked: false,
        });
        doc.bodies = vec![body(1, 0, true), body(2, 1, true), body(3, 0, false)];
        let ids: Vec<u32> = visible_bodies(&doc).iter().map(|b| b.id.0).collect();
        assert_eq!(
            ids,
            vec![1],
            "body on a hidden layer and hidden body are skipped"
        );
    }
}
