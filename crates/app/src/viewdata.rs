//! Everything the viewport needs from the document, in world space: the
//! base render scene, the pick mesh, selectable edges and vertices, body
//! bounds and output topology. Rebuilt only when the document revision or
//! a display setting changes.

use crate::docstate::guard;
use crate::selection::{EdgeSel, TopoMesh, VertSel};
use glam::{DMat4, Vec3};
use newtopo_core::mesh::{OutEdgeKind, OutputMesh};
use newtopo_core::scene::Scene;
use newtopo_kernel::{BodyId, FaceId};
use newtopo_render::pick::{FaceRef, PickMesh};
use newtopo_render::scene::{MeshVertex, OverlayColors, RenderScene};
use newtopo_render::synth::{SynthBody, ground_grid};
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

pub const DEFAULT_BODY_COLOR: [f32; 4] = [0.62, 0.66, 0.72, 1.0];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeKind {
    Cad,
    Split,
    Line,
    Fill,
}

#[derive(Clone, Debug)]
pub struct EdgeItem {
    pub body: BodyId,
    pub sel: Option<EdgeSel>,
    pub kind: EdgeKind,
    pub path: Vec<Vec3>,
    /// Index into the body's [`TopoMesh`] edges.
    pub topo: Option<u32>,
    /// The CAD edge this lies on, if any.
    pub cad_edge: Option<newtopo_kernel::EdgeId>,
    /// Index into the body's `OutputMesh::edges` (real scenes).
    pub out: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct VertItem {
    pub body: BodyId,
    pub sel: VertSel,
    pub pos: Vec3,
}

#[derive(Clone, Debug)]
pub struct BodyInfo {
    pub id: BodyId,
    pub name: String,
    pub min: Vec3,
    pub max: Vec3,
}

#[derive(Clone, Debug, Default)]
pub struct BodyTopo {
    pub mesh: TopoMesh,
    /// Topo edge → index in [`ViewData::edges`].
    pub edge_item: HashMap<u32, usize>,
    /// Topo poly → CAD face.
    pub poly_face: Vec<FaceId>,
}

/// Where a patch's Connect directions point (world space).
#[derive(Clone, Copy, Debug)]
pub struct BendArrow {
    pub origin: Vec3,
    pub normal: Vec3,
    pub a: Option<Vec3>,
    pub b: Option<Vec3>,
    /// Size of the face (bounding-box diagonal) to scale the arrow.
    pub size: f32,
}

/// Display settings that change what the base scene contains.
#[derive(Clone, Debug, PartialEq)]
pub struct DisplayParams {
    pub preview: bool,
    pub wireframe: bool,
    pub show_verts: bool,
    pub show_grid: bool,
    pub up: newtopo_render::UpAxis,
    pub hidden: BTreeSet<BodyId>,
    pub colors: OverlayColors,
}

#[derive(Default)]
pub struct ViewData {
    pub base: Arc<RenderScene>,
    pub pick: PickMesh,
    pub face_tris: HashMap<FaceRef, Vec<u32>>,
    /// Area-weighted centre of each face's display triangles.
    pub face_centers: HashMap<FaceRef, Vec3>,
    pub edges: Vec<EdgeItem>,
    pub verts: Vec<VertItem>,
    pub bodies: Vec<BodyInfo>,
    pub topo: HashMap<BodyId, BodyTopo>,
    pub bounds: Option<(Vec3, Vec3)>,
    pub outputs: Vec<OutputMesh>,
    /// Connect direction arrows per selected-able face (real scenes).
    pub arrows: HashMap<(BodyId, FaceId), Vec<BendArrow>>,
    /// Patches per body (real scenes; SPEC §4.2).
    pub patches: HashMap<BodyId, Vec<newtopo_core::scene::PatchInfo>>,
    /// Non-fatal problems building the view (shown once as a toast).
    pub error: Option<String>,
}

impl ViewData {
    pub fn body(&self, id: BodyId) -> Option<&BodyInfo> {
        self.bodies.iter().find(|b| b.id == id)
    }

    pub fn edge_index(&self, sel: &EdgeSel) -> Option<usize> {
        self.edges.iter().position(|e| e.sel.as_ref() == Some(sel))
    }

    pub fn vert_index(&self, sel: &VertSel) -> Option<usize> {
        self.verts.iter().position(|v| &v.sel == sel)
    }

    /// Bounds of a set of bodies (or everything when empty).
    pub fn bounds_of(&self, ids: &BTreeSet<BodyId>) -> Option<(Vec3, Vec3)> {
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        let mut any = false;
        for b in &self.bodies {
            if ids.is_empty() || ids.contains(&b.id) {
                min = min.min(b.min);
                max = max.max(b.max);
                any = true;
            }
        }
        any.then_some((min, max))
    }

    fn finish(
        mut self,
        mut scene: RenderScene,
        p: &DisplayParams,
        pick_pos: Vec<Vec3>,
        pick_tris: Vec<[u32; 3]>,
        pick_ids: Vec<FaceRef>,
    ) -> Self {
        for (i, id) in pick_ids.iter().enumerate() {
            self.face_tris.entry(*id).or_default().push(i as u32);
        }
        for (id, tris) in &self.face_tris {
            let mut acc = Vec3::ZERO;
            let mut wsum = 0.0;
            for &t in tris {
                let [a, b, c] = pick_tris[t as usize].map(|i| pick_pos[i as usize]);
                let w = (b - a).cross(c - a).length() * 0.5 + 1e-9;
                acc += (a + b + c) / 3.0 * w;
                wsum += w;
            }
            self.face_centers.insert(*id, acc / wsum);
        }
        self.pick = PickMesh::new(pick_pos, pick_tris, pick_ids);
        self.bounds = self.bounds_of(&BTreeSet::new());
        if p.show_grid
            && let Some((min, max)) = self.bounds
        {
            let mut grid = RenderScene::default();
            let h = match p.up {
                newtopo_render::UpAxis::Z => min.z,
                newtopo_render::UpAxis::Y => min.y,
            };
            ground_grid(
                min,
                max,
                h - (max - min).length() * 0.001,
                p.up,
                p.colors.grid,
                &mut grid,
            );
            // Grid first so model lines draw over it.
            grid.lines.append(&mut scene.lines);
            scene.lines = grid.lines;
        }
        self.base = Arc::new(scene);
        self
    }
}

fn xf(m: &DMat4, p: glam::DVec3) -> Vec3 {
    m.transform_point3(p).as_vec3()
}

fn xn(m: &DMat4, n: glam::DVec3) -> Vec3 {
    m.transform_vector3(n).normalize_or_zero().as_vec3()
}

/// Triangulate a simple polygon (ear clipping in its best-fit plane).
pub fn triangulate(pts: &[Vec3]) -> Vec<[usize; 3]> {
    let n = pts.len();
    if n < 3 {
        return vec![];
    }
    if n == 3 {
        return vec![[0, 1, 2]];
    }
    // Newell normal.
    let mut nrm = Vec3::ZERO;
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        nrm += Vec3::new(
            (a.y - b.y) * (a.z + b.z),
            (a.z - b.z) * (a.x + b.x),
            (a.x - b.x) * (a.y + b.y),
        );
    }
    let nrm = nrm.normalize_or_zero();
    if nrm == Vec3::ZERO {
        return (1..n - 1).map(|k| [0, k, k + 1]).collect();
    }
    let u = nrm.any_orthonormal_vector();
    let v = nrm.cross(u);
    let p2: Vec<glam::Vec2> = pts
        .iter()
        .map(|p| glam::Vec2::new(p.dot(u), p.dot(v)))
        .collect();
    let cross = |o: glam::Vec2, a: glam::Vec2, b: glam::Vec2| (a - o).perp_dot(b - o);
    let mut idx: Vec<usize> = (0..n).collect();
    let mut out = Vec::with_capacity(n - 2);
    let mut guard_iter = 0;
    while idx.len() > 3 && guard_iter < n * n {
        guard_iter += 1;
        let m = idx.len();
        let mut clipped = false;
        for i in 0..m {
            let (ia, ib, ic) = (idx[(i + m - 1) % m], idx[i], idx[(i + 1) % m]);
            let (a, b, c) = (p2[ia], p2[ib], p2[ic]);
            if cross(a, b, c) <= 1e-12 {
                continue;
            }
            let inside = idx.iter().any(|&j| {
                j != ia && j != ib && j != ic && {
                    let p = p2[j];
                    cross(a, b, p) >= 0.0 && cross(b, c, p) >= 0.0 && cross(c, a, p) >= 0.0
                }
            });
            if !inside {
                out.push([ia, ib, ic]);
                idx.remove(i);
                clipped = true;
                break;
            }
        }
        if !clipped {
            break;
        }
    }
    if idx.len() == 3 {
        out.push([idx[0], idx[1], idx[2]]);
    } else if idx.len() > 3 {
        for k in 1..idx.len() - 1 {
            out.push([idx[0], idx[k], idx[k + 1]]);
        }
    }
    out
}

/// Build from the core scene. Engine panics are caught; whatever could be
/// built is returned with `error` set.
pub fn from_scene(scene: &mut Scene, p: &DisplayParams) -> ViewData {
    let mut vd = ViewData::default();
    let entries = match guard(|| scene.doc().bodies.clone()) {
        Ok(b) => b,
        Err(e) => {
            vd.error = Some(e);
            return vd.finish(RenderScene::default(), p, vec![], vec![], vec![]);
        }
    };
    match guard(|| scene.output().to_vec()) {
        Ok(o) => vd.outputs = o,
        Err(e) => vd.error = Some(e),
    }
    for e in &entries {
        if let Ok(p) = guard(|| scene.patches(e.id)) {
            // Arrows: evaluate each patch's interior sample on the surface;
            // the A family spreads along u (OneU, Two) or v (OneV).
            use newtopo_core::eval::lines::Bend;
            // Sample point per face: the display-mesh vertex nearest the
            // mesh centroid (on the surface, with its exact uv).
            let samples: HashMap<FaceId, (glam::DVec3, f64)> = guard(|| {
                scene
                    .body_view(e.id)
                    .map(|bv| {
                        bv.face_meshes
                            .iter()
                            .enumerate()
                            .filter(|(_, fm)| !fm.positions.is_empty())
                            .map(|(fi, fm)| {
                                let c = fm.positions.iter().copied().sum::<glam::DVec3>()
                                    / fm.positions.len() as f64;
                                let k = (0..fm.positions.len())
                                    .min_by(|&x, &y| {
                                        (fm.positions[x] - c)
                                            .length()
                                            .total_cmp(&(fm.positions[y] - c).length())
                                    })
                                    .unwrap_or(0);
                                let mut bb = newtopo_kernel::Aabb::EMPTY;
                                fm.positions.iter().for_each(|p| bb.add(*p));
                                (FaceId(fi as u32), (fm.positions[k], bb.diagonal()))
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default();
            let k = scene.kernel.clone();
            // Kernel calls take the kernel's body handle, not the document's.
            let Some(kb) = guard(|| scene.ctx(e.id).map(|c| c.kbody)).ok().flatten() else {
                vd.patches.insert(e.id, p);
                continue;
            };
            let mut done: BTreeSet<FaceId> = BTreeSet::new();
            for info in &p {
                let (da, db) = match info.bend {
                    Bend::OneU => (Some(0), None),
                    Bend::OneV => (Some(1), None),
                    Bend::Two => (Some(0), Some(1)),
                    Bend::Radial => (Some(0), None),
                    Bend::Flat => (None, None),
                };
                if da.is_none() || !done.insert(info.face) {
                    continue;
                }
                let Some((sp, size)) = samples.get(&info.face) else {
                    continue;
                };
                // Exact uv of the sample through the kernel.
                let Ok(Ok(proj)) = guard(|| k.project_to_face(kb, info.face, &[*sp])) else {
                    continue;
                };
                let Some(Some((uv, _))) = proj.first() else {
                    continue;
                };
                let Ok(Ok(ev)) = guard(|| k.face_eval(kb, info.face, &[*uv])) else {
                    continue;
                };
                let Some(ev) = ev.first() else { continue };
                let m = e.transform;
                let pick = |d: Option<usize>| {
                    d.map(|d| xn(&m, if d == 0 { ev.du } else { ev.dv }))
                        .filter(|v| *v != Vec3::ZERO)
                };
                vd.arrows
                    .entry((e.id, info.face))
                    .or_default()
                    .push(BendArrow {
                        origin: xf(&m, ev.p),
                        normal: xn(&m, ev.n),
                        a: pick(da),
                        b: pick(db),
                        size: *size as f32,
                    });
            }
            vd.patches.insert(e.id, p);
        }
    }
    let c = &p.colors;
    let mut rs = RenderScene::default();
    let (mut ppos, mut ptris, mut pids) = (Vec::new(), Vec::new(), Vec::new());

    for entry in entries
        .iter()
        .filter(|e| e.visible && !p.hidden.contains(&e.id))
    {
        let m = entry.transform;
        let body_color = entry
            .color
            .map(|c| [c[0], c[1], c[2], 1.0])
            .unwrap_or(DEFAULT_BODY_COLOR);
        let mut bmin = Vec3::splat(f32::INFINITY);
        let mut bmax = Vec3::splat(f32::NEG_INFINITY);
        let output = vd.outputs.iter().find(|o| o.body == entry.id).cloned();

        // Surfaces.
        if p.preview {
            if let Some(out) = &output {
                let wpos: Vec<Vec3> = out.vertices.iter().map(|v| xf(&m, v.position)).collect();
                for poly in &out.polygons {
                    let pts: Vec<Vec3> = poly.verts.iter().map(|&i| wpos[i as usize]).collect();
                    let nrm: Vec<Vec3> = poly.normals.iter().map(|n| xn(&m, *n)).collect();
                    for t in triangulate(&pts) {
                        let base = rs.mesh.vertices.len() as u32;
                        for k in t {
                            let n = nrm.get(k).copied().unwrap_or(Vec3::Z);
                            rs.mesh.vertices.push(MeshVertex {
                                pos: pts[k].to_array(),
                                normal: n.to_array(),
                                color: c.preview_surface,
                            });
                        }
                        rs.mesh.indices.extend([base, base + 1, base + 2]);
                        let pb = ppos.len() as u32;
                        ppos.extend(t.map(|k| pts[k]));
                        ptris.push([pb, pb + 1, pb + 2]);
                        pids.push(FaceRef {
                            body: entry.id.0,
                            face: poly.face.0,
                        });
                    }
                    for p3 in &pts {
                        bmin = bmin.min(*p3);
                        bmax = bmax.max(*p3);
                    }
                }
            }
        } else {
            match guard(|| {
                scene.body_view(entry.id).map(|bv| {
                    let faces: Vec<_> = bv.face_meshes.to_vec();
                    let edges: Vec<(usize, Vec<glam::DVec3>)> = bv
                        .edge_polylines
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| {
                            bv.topology
                                .edges
                                .get(*i)
                                .is_none_or(|e| !e.seam && !e.degenerate)
                        })
                        .map(|(i, pl)| (i, pl.clone()))
                        .collect();
                    (faces, edges)
                })
            }) {
                Ok(Some((faces, cad_edges))) => {
                    for (fi, fm) in faces.iter().enumerate() {
                        let base = rs.mesh.vertices.len() as u32;
                        let pbase = ppos.len() as u32;
                        for (k, pos) in fm.positions.iter().enumerate() {
                            let w = xf(&m, *pos);
                            let n = fm.normals.get(k).map(|n| xn(&m, *n)).unwrap_or(Vec3::Z);
                            rs.mesh.vertices.push(MeshVertex {
                                pos: w.to_array(),
                                normal: n.to_array(),
                                color: body_color,
                            });
                            ppos.push(w);
                            bmin = bmin.min(w);
                            bmax = bmax.max(w);
                        }
                        for t in &fm.triangles {
                            rs.mesh.indices.extend(t.map(|i| i + base));
                            ptris.push(t.map(|i| i + pbase));
                            pids.push(FaceRef {
                                body: entry.id.0,
                                face: fi as u32,
                            });
                        }
                    }
                    if output.is_none() {
                        for (ei, pl) in cad_edges {
                            let path: Vec<Vec3> = pl.iter().map(|q| xf(&m, *q)).collect();
                            if path.len() < 2 {
                                continue;
                            }
                            rs.add_polyline(&path, c.cad_edge, 1.8);
                            vd.edges.push(EdgeItem {
                                body: entry.id,
                                sel: Some(EdgeSel::Cad(
                                    entry.id,
                                    newtopo_kernel::EdgeId(ei as u32),
                                )),
                                kind: EdgeKind::Cad,
                                path,
                                topo: None,
                                cad_edge: Some(newtopo_kernel::EdgeId(ei as u32)),
                                out: None,
                            });
                        }
                    }
                }
                Ok(None) => {}
                Err(e) => vd.error = Some(e),
            }
        }

        // Edges and vertices from the output mesh.
        if let Some(out) = &output {
            let wpos: Vec<Vec3> = out.vertices.iter().map(|v| xf(&m, v.position)).collect();
            let polys: Vec<Vec<u32>> = out.polygons.iter().map(|p| p.verts.clone()).collect();
            let mut bt = BodyTopo {
                mesh: TopoMesh::new(polys),
                poly_face: out.polygons.iter().map(|p| p.face).collect(),
                ..Default::default()
            };
            for (i, e) in out.edges.iter().enumerate() {
                let path: Vec<Vec3> = if e.path.len() >= 2 {
                    e.path.iter().map(|q| xf(&m, *q)).collect()
                } else {
                    vec![wpos[e.a as usize], wpos[e.b as usize]]
                };
                let kind = match e.kind {
                    OutEdgeKind::Cad(_) => EdgeKind::Cad,
                    OutEdgeKind::Split(_) => EdgeKind::Split,
                    OutEdgeKind::Line(_) => EdgeKind::Line,
                    OutEdgeKind::Fill => EdgeKind::Fill,
                };
                if p.preview {
                    if p.wireframe {
                        rs.add_segment(wpos[e.a as usize], wpos[e.b as usize], c.preview_wire, 1.0);
                    }
                } else {
                    let (col, w) = match kind {
                        EdgeKind::Cad | EdgeKind::Split => (c.cad_edge, 1.8),
                        EdgeKind::Line => (c.line, 1.6),
                        EdgeKind::Fill => (c.fill_edge, 1.1),
                    };
                    rs.add_polyline(&path, col, w);
                }
                let sel = match (&e.key, kind) {
                    (Some(k), _) => Some(EdgeSel::Key(entry.id, k.clone())),
                    (None, _) => Some(EdgeSel::Index(entry.id, i as u32)),
                };
                let topo = bt.mesh.edge(e.a, e.b);
                if let Some(t) = topo {
                    bt.edge_item.insert(t, vd.edges.len());
                }
                let cad_edge = match e.kind {
                    OutEdgeKind::Cad(id) => Some(id),
                    _ => None,
                };
                vd.edges.push(EdgeItem {
                    body: entry.id,
                    sel,
                    kind,
                    path,
                    topo,
                    cad_edge,
                    out: Some(i as u32),
                });
            }
            for (i, v) in out.vertices.iter().enumerate() {
                let pos = wpos[i];
                if p.show_verts {
                    rs.add_point(pos, c.vertex, 5.0);
                }
                vd.verts.push(VertItem {
                    body: entry.id,
                    sel: VertSel::Key(entry.id, v.key.clone()),
                    pos,
                });
            }
            vd.topo.insert(entry.id, bt);
        }

        if bmin.x <= bmax.x {
            vd.bodies.push(BodyInfo {
                id: entry.id,
                name: entry.name.clone(),
                min: bmin,
                max: bmax,
            });
        }
    }
    vd.finish(rs, p, ppos, ptris, pids)
}

/// Build from the synthetic sample model (no engine needed).
pub fn from_demo(bodies: &[SynthBody], p: &DisplayParams) -> ViewData {
    let mut vd = ViewData::default();
    let c = &p.colors;
    let mut rs = RenderScene::default();
    let (mut ppos, mut ptris, mut pids) = (Vec::new(), Vec::new(), Vec::new());
    for b in bodies {
        let id = BodyId(b.id);
        if p.hidden.contains(&id) {
            continue;
        }
        let (min, max) = b.bounds();
        vd.bodies.push(BodyInfo {
            id,
            name: b.name.clone(),
            min,
            max,
        });

        // Weld the coarse polygons into an output-like mesh.
        let mut vmap: HashMap<(i64, i64, i64), u32> = HashMap::new();
        let mut vpos: Vec<Vec3> = Vec::new();
        let mut polys: Vec<Vec<u32>> = Vec::new();
        let mut poly_face = Vec::new();
        let mut poly_n = Vec::new();
        for f in &b.faces {
            for (poly, n) in f.polys.iter().zip(&f.poly_normals) {
                let ids: Vec<u32> = poly
                    .iter()
                    .map(|q| {
                        let k = (
                            (q.x * 1000.0).round() as i64,
                            (q.y * 1000.0).round() as i64,
                            (q.z * 1000.0).round() as i64,
                        );
                        *vmap.entry(k).or_insert_with(|| {
                            vpos.push(*q);
                            (vpos.len() - 1) as u32
                        })
                    })
                    .collect();
                polys.push(ids);
                poly_face.push(FaceId(f.id));
                poly_n.push(*n);
            }
        }
        let mut bt = BodyTopo {
            mesh: TopoMesh::new(polys.clone()),
            poly_face: poly_face.clone(),
            ..Default::default()
        };

        if p.preview {
            for ((poly, face), n) in polys.iter().zip(&poly_face).zip(&poly_n) {
                let pts: Vec<Vec3> = poly.iter().map(|&i| vpos[i as usize]).collect();
                for t in triangulate(&pts) {
                    rs.mesh
                        .push_triangle(t.map(|k| pts[k]), [*n; 3], c.preview_surface);
                    let pb = ppos.len() as u32;
                    ppos.extend(t.map(|k| pts[k]));
                    ptris.push([pb, pb + 1, pb + 2]);
                    pids.push(FaceRef {
                        body: b.id,
                        face: face.0,
                    });
                }
            }
        } else {
            for f in &b.faces {
                let base = rs.mesh.vertices.len() as u32;
                let pbase = ppos.len() as u32;
                for (q, n) in f.positions.iter().zip(&f.normals) {
                    rs.mesh.vertices.push(MeshVertex {
                        pos: q.to_array(),
                        normal: n.to_array(),
                        color: b.color,
                    });
                }
                ppos.extend_from_slice(&f.positions);
                for t in &f.tris {
                    rs.mesh.indices.extend(t.map(|i| i + base));
                    ptris.push(t.map(|i| i + pbase));
                    pids.push(FaceRef {
                        body: b.id,
                        face: f.id,
                    });
                }
                for l in &f.lines {
                    rs.add_polyline(l, c.line, 1.6);
                }
            }
            for e in &b.cad_edges {
                rs.add_polyline(e, c.cad_edge, 1.8);
            }
        }
        // Selectable edges: polygon edges; on a face boundary → CAD-like.
        for (ti, &(a, bb)) in bt.mesh.edges.clone().iter().enumerate() {
            let faces: BTreeSet<FaceId> = bt
                .mesh
                .polys
                .iter()
                .enumerate()
                .filter(|(_, pl)| pl.contains(&a) && pl.contains(&bb))
                .map(|(pi, _)| poly_face[pi])
                .collect();
            let kind = if faces.len() == 1 {
                EdgeKind::Line
            } else {
                EdgeKind::Cad
            };
            let path = vec![vpos[a as usize], vpos[bb as usize]];
            if p.preview && p.wireframe {
                rs.add_segment(path[0], path[1], c.preview_wire, 1.0);
            }
            bt.edge_item.insert(ti as u32, vd.edges.len());
            vd.edges.push(EdgeItem {
                body: id,
                sel: Some(EdgeSel::Index(id, ti as u32)),
                kind,
                path,
                topo: Some(ti as u32),
                cad_edge: None,
                out: None,
            });
        }
        for (i, q) in vpos.iter().enumerate() {
            if p.show_verts {
                rs.add_point(*q, c.vertex, 5.0);
            }
            vd.verts.push(VertItem {
                body: id,
                sel: VertSel::Index(id, i as u32),
                pos: *q,
            });
        }
        vd.topo.insert(id, bt);
    }
    vd.finish(rs, p, ppos, ptris, pids)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> DisplayParams {
        DisplayParams {
            preview: false,
            wireframe: false,
            show_verts: false,
            show_grid: true,
            up: newtopo_render::UpAxis::Z,
            hidden: BTreeSet::new(),
            colors: OverlayColors::default(),
        }
    }

    #[test]
    fn triangulates_concave() {
        // An L shape.
        let pts = [
            (0.0, 0.0),
            (2.0, 0.0),
            (2.0, 1.0),
            (1.0, 1.0),
            (1.0, 2.0),
            (0.0, 2.0),
        ]
        .map(|(x, y)| Vec3::new(x, y, 0.0));
        let t = triangulate(&pts);
        assert_eq!(t.len(), 4);
        let area: f32 = t
            .iter()
            .map(|t| {
                (pts[t[1]] - pts[t[0]])
                    .cross(pts[t[2]] - pts[t[0]])
                    .length()
                    * 0.5
            })
            .sum();
        assert!((area - 3.0).abs() < 1e-5);
    }

    #[test]
    fn demo_view_has_everything() {
        let bodies = newtopo_render::synth::demo_bodies();
        let vd = from_demo(&bodies, &params());
        assert_eq!(vd.bodies.len(), 2);
        assert!(!vd.pick.is_empty());
        assert!(!vd.edges.is_empty() && !vd.verts.is_empty());
        assert!(
            vd.edges.iter().any(|e| e.kind == EdgeKind::Cad)
                && vd.edges.iter().any(|e| e.kind == EdgeKind::Line)
        );
        // Hiding a body removes it.
        let mut p = params();
        p.hidden.insert(BodyId(1));
        let vd2 = from_demo(&bodies, &p);
        assert_eq!(vd2.bodies.len(), 1);
        // Preview mode builds surfaces from the output polygons.
        p.preview = true;
        p.wireframe = true;
        let vd3 = from_demo(&bodies, &p);
        assert!(!vd3.base.mesh.is_empty());
    }
}
