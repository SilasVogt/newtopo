//! Engine acceptance criteria (SPEC §4.6) and Scene basics.
mod common;
use common::*;
use newtopo_core::Op;
use newtopo_core::doc::{IsoDir, PatchId, Rule, Spacing};
use newtopo_core::keys::{EdgeKey, EdgeOrSplit, VertexKey};
use newtopo_core::mesh::{Anchor, OutEdgeKind};
use newtopo_core::ops::PatchSel;
use newtopo_kernel::mock::MockBody;
use newtopo_kernel::{EdgeId, FaceId};

fn grid(face: u32, u: u32, v: u32) -> Op {
    Op::Grid {
        patches: PatchSel {
            faces: vec![face],
            ..Default::default()
        },
        u,
        v,
        start_on_seam: false,
        spacing: Spacing::ArcLength,
    }
}

#[test]
fn untouched_shapes_validate() {
    for body in [
        MockBody::make_box(glam::DVec3::new(4.0, 3.0, 2.0)),
        MockBody::cylinder(2.0, 5.0),
        MockBody::plate_with_holes(2, 2),
        MockBody::cone(2.0, 4.0),
        MockBody::sphere(3.0),
        MockBody::torus(4.0, 1.0),
        MockBody::trimmed_torus(4.0, 1.0),
        MockBody::ring(3.0, 1.0),
        MockBody::symmetric_part(),
        MockBody::split_cylinder(2.0, 4.0),
    ] {
        let name = body.name.clone();
        let (mut s, b) = scene_with(body);
        for on in [true, false] {
            set_autofill(&mut s, on);
            assert_valid(&mut s, b, &format!("{name} autofill {on}"));
        }
    }
}

/// Plate with a hole and a Grid around it; RemoveEdge on every generated
/// edge touching the hole rim: bridged and valid, auto-fill on and off.
#[test]
fn ac_plate_hole_remove_rim_edges() {
    for autofill in [true, false] {
        let (mut s, b) = scene_with(MockBody::plate_with_holes(1, 1));
        set_autofill(&mut s, autofill);
        s.apply(grid(1, 4, 4)).unwrap();
        assert_valid(&mut s, b, "grid");
        let rim = EdgeId(1); // top hole circle
        let m = s.output_of(b).unwrap().clone();
        let on_rim = |v: u32| matches!(m.vertices[v as usize].anchor, Anchor::CadEdge { edge, .. } if edge == rim);
        let keys: Vec<EdgeKey> = m
            .edges
            .iter()
            .filter(|e| matches!(e.kind, OutEdgeKind::Line(_)) && (on_rim(e.a) || on_rim(e.b)))
            .filter_map(|e| e.key.clone())
            .collect();
        assert!(keys.len() >= 4, "lines touching the rim: {}", keys.len());
        for k in keys {
            s.apply(Op::RemoveEdge {
                body: Some(b),
                edge: k,
            })
            .unwrap();
        }
        assert_valid(&mut s, b, &format!("after removals (autofill {autofill})"));
    }
}

/// A cylinder with exactly one generator line validates.
#[test]
fn ac_cylinder_one_generator() {
    for autofill in [true, false] {
        let (mut s, b) = scene_with(MockBody::cylinder(2.0, 5.0));
        set_autofill(&mut s, autofill);
        s.apply(Op::Iso {
            body: Some(b),
            face: 0,
            dir: IsoDir::U,
            value: 0.3,
            seed: None,
            split: false,
            through: false,
        })
        .unwrap();
        assert_valid(&mut s, b, "one generator");
        let m = s.output_of(b).unwrap();
        assert!(m.polygons.iter().all(|p| {
            let mut v = p.verts.clone();
            v.sort();
            v.windows(2).all(|w| w[0] != w[1])
        }));
    }
}

/// A planar face with one iso line from a hole to the outer edge validates.
#[test]
fn ac_iso_hole_to_outer_edge() {
    for autofill in [true, false] {
        let (mut s, b) = scene_with(MockBody::plate_with_holes(1, 1));
        set_autofill(&mut s, autofill);
        // Top face u range is the plate width; u = 0.5 passes the hole centre.
        s.apply(Op::Iso {
            body: Some(b),
            face: 1,
            dir: IsoDir::U,
            value: 0.5,
            seed: Some(0.1),
            split: false,
            through: false,
        })
        .unwrap();
        assert_valid(&mut s, b, "iso hole to edge");
        let m = s.output_of(b).unwrap();
        assert_eq!(
            m.edges
                .iter()
                .filter(|e| matches!(e.kind, OutEdgeKind::Line(_)))
                .count(),
            1
        );
    }
}

/// 10×4 grid split at u = 0.3 → 3 and 7; constant-v lines meet across the
/// split; removing the split → 10 × 4.
#[test]
fn ac_split_distributes_grid() {
    let (mut s, b) = scene_with(MockBody::make_box(glam::DVec3::new(10.0, 6.0, 2.0)));
    set_autofill(&mut s, false);
    s.apply(grid(1, 10, 4)).unwrap();
    s.apply(Op::Iso {
        body: Some(b),
        face: 1,
        dir: IsoDir::U,
        value: 0.3,
        seed: None,
        split: true,
        through: false,
    })
    .unwrap();
    let mut counts: Vec<(u32, u32)> = s
        .doc()
        .rules
        .iter()
        .filter_map(|r| match r.rule {
            Rule::Grid {
                count_u, count_v, ..
            } => Some((count_u, count_v)),
            _ => None,
        })
        .collect();
    counts.sort();
    assert_eq!(counts, vec![(3, 4), (7, 4)]);
    assert_valid(&mut s, b, "split grid");
    // Constant-v lines meet on the split: vertices on the split with line
    // edges on both sides.
    let m = s.output_of(b).unwrap().clone();
    let split_edges: Vec<&newtopo_core::mesh::OutEdge> = m
        .edges
        .iter()
        .filter(|e| matches!(e.kind, OutEdgeKind::Split(_)))
        .collect();
    let mut on_split: Vec<u32> = split_edges.iter().flat_map(|e| [e.a, e.b]).collect();
    on_split.sort();
    on_split.dedup();
    let meeting = on_split
        .iter()
        .filter(|&&v| {
            m.edges
                .iter()
                .filter(|e| matches!(e.kind, OutEdgeKind::Line(_)) && (e.a == v || e.b == v))
                .count()
                == 2
        })
        .count();
    assert_eq!(meeting, 4, "4 constant-v lines meet across the split");
    let sk = split_edges[0].key.clone().unwrap();
    s.apply(Op::RemoveEdge {
        body: Some(b),
        edge: sk,
    })
    .unwrap();
    let counts: Vec<(u32, u32)> = s
        .doc()
        .rules
        .iter()
        .filter_map(|r| match r.rule {
            Rule::Grid {
                count_u, count_v, ..
            } => Some((count_u, count_v)),
            _ => None,
        })
        .collect();
    assert_eq!(counts, vec![(10, 4)]);
    assert!(s.doc().split_lines.is_empty());
    assert_valid(&mut s, b, "unsplit");
}

/// Torus trimmed by one hole: implicit grid validates; Grid 6×6 validates
/// with every seam dissolved; removing every grid edge brings the seams
/// back as fill edges and still validates.
#[test]
fn ac_trimmed_torus() {
    for autofill in [true, false] {
        let (mut s, b) = scene_with(MockBody::trimmed_torus(4.0, 1.0));
        set_autofill(&mut s, autofill);
        assert_valid(&mut s, b, "implicit grid");
        s.apply(grid(0, 6, 6)).unwrap();
        assert_valid(&mut s, b, "grid 6x6");
        let m = s.output_of(b).unwrap().clone();
        let seam_vertex = m.vertices.iter().any(|v| v.keys_on_seam());
        assert!(!seam_vertex, "seams dissolved");
        // Delete every generated edge as one selection.
        let keys: Vec<EdgeKey> = m
            .edges
            .iter()
            .filter(|e| matches!(e.kind, OutEdgeKind::Line(_)))
            .filter_map(|e| e.key.clone())
            .collect();
        s.apply(Op::RemoveEdges {
            body: Some(b),
            edges: keys,
        })
        .unwrap_or_else(|e| panic!("removal refused: {}", e.reason));
        assert_valid(
            &mut s,
            b,
            &format!("all grid edges removed (autofill {autofill})"),
        );
        let m = s.output_of(b).unwrap();
        assert!(
            m.edges.iter().any(|e| e.kind == OutEdgeKind::Fill),
            "seams came back as fill edges"
        );
    }
}

trait SeamKey {
    fn keys_on_seam(&self) -> bool;
}
impl SeamKey for newtopo_core::mesh::OutVertex {
    fn keys_on_seam(&self) -> bool {
        matches!(
            self.key,
            VertexKey::OnBoundary {
                on: EdgeOrSplit::Seam(..),
                ..
            }
        )
    }
}

#[test]
fn undo_redo_and_modified() {
    let (mut s, b) = scene_with(MockBody::make_box(glam::DVec3::splat(2.0)));
    let d0 = s.doc().clone();
    s.apply(grid(1, 2, 2)).unwrap();
    s.apply(grid(1, 3, 3)).unwrap();
    assert!(s.can_undo());
    assert!(s.undo());
    assert!(s.undo());
    assert_eq!(s.doc().rules, d0.rules);
    assert!(s.redo());
    assert_eq!(s.doc().rules.len(), 1);
    // Coalescing: three wheel steps → one undo step.
    s.begin_coalesce();
    for n in 4..7 {
        s.apply(grid(1, n, n)).unwrap();
    }
    s.end_coalesce();
    assert!(s.undo());
    assert!(matches!(
        s.doc().rules[0].rule,
        Rule::Grid { count_u: 2, .. }
    ));
    let _ = (b, PatchId::whole(b, FaceId(1)));
}

/// SPEC §8.2: `.ntopo` round trip; newer formats are refused.
#[test]
fn save_load_and_newer_format() {
    use std::io::Write;
    let (mut s, b) = scene_with(MockBody::cylinder(2.0, 5.0));
    s.apply(grid(0, 4, 2)).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.ntopo");
    s.save(&path).unwrap();
    assert!(!s.is_modified());
    let mut s2 = newtopo_core::Scene::load(
        std::sync::Arc::new(newtopo_kernel::mock::MockKernel::new()),
        &path,
    )
    .unwrap();
    assert_eq!(s2.doc(), s.doc());
    assert_eq!(
        s2.output_of(b).unwrap().polygons,
        s.output_of(b).unwrap().polygons
    );
    assert!(
        s2.body_view(b)
            .is_some_and(|v| v.face_meshes.len() == 3 && v.edge_polylines.len() == 3)
    );
    // Bump the format version and expect a clear refusal.
    let mut doc = s.doc().clone();
    doc.format_version = 99;
    let newer = dir.path().join("b.ntopo");
    let f = std::fs::File::create(&newer).unwrap();
    let mut z = zip::ZipWriter::new(f);
    z.start_file("scene.json", zip::write::SimpleFileOptions::default())
        .unwrap();
    z.write_all(serde_json::to_string(&doc).unwrap().as_bytes())
        .unwrap();
    z.finish().unwrap();
    let err = newtopo_core::Scene::load(
        std::sync::Arc::new(newtopo_kernel::mock::MockKernel::new()),
        &newer,
    )
    .err()
    .unwrap();
    assert!(err.to_string().contains("newer version"), "{err}");
}
