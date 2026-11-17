//! App-facing Scene API (round 2) and Link / Symmetry / copy-paste ACs
//! (SPEC §6.11–§6.13) on mock shapes.
mod common;
use common::*;
use glam::{DMat4, DVec3};
use newtopo_core::doc::{Axis, Camera, IsoDir, LayerId, PatchId, Rule, Spacing};
use newtopo_core::eval::lines::Bend;
use newtopo_core::mesh::{OutEdgeKind, OutputMesh};
use newtopo_core::ops::PatchSel;
use newtopo_core::{Op, Scene};
use newtopo_kernel::mock::{MockBody, MockKernel};
use newtopo_kernel::{BodyAttributes, BodyId, FaceId, HealReport};
use std::collections::BTreeSet;
use std::sync::Arc;

fn faces(f: &[u32]) -> PatchSel {
    PatchSel {
        faces: f.to_vec(),
        ..Default::default()
    }
}
fn grid(f: u32, u: u32, v: u32) -> Op {
    Op::Grid {
        patches: faces(&[f]),
        u,
        v,
        start_on_seam: false,
        spacing: Spacing::ArcLength,
    }
}
fn connect(f: u32, a: u32) -> Op {
    Op::Connect {
        patches: faces(&[f]),
        a,
        b: 0,
        start_on_seam: false,
    }
}
/// Generated-line count per face (by the polygons using each line edge).
fn lines_on_face(m: &OutputMesh, face: u32) -> usize {
    m.edges
        .iter()
        .filter(|e| matches!(e.kind, OutEdgeKind::Line(_)))
        .filter(|e| {
            m.polygons
                .iter()
                .any(|p| p.face == FaceId(face) && p.verts.contains(&e.a) && p.verts.contains(&e.b))
        })
        .map(|e| {
            if let OutEdgeKind::Line(k) = e.kind {
                (k.rule, k.index, k.sub / 64)
            } else {
                unreachable!()
            }
        })
        .collect::<BTreeSet<_>>()
        .len()
}

#[test]
fn body_id_equals_kernel_id() {
    let (s, b) = scene_with(MockBody::cylinder(2.0, 5.0));
    assert_eq!(s.ctx(b).unwrap().kbody, b);
}

#[test]
fn apply_all_is_one_step_and_atomic() {
    let (mut s, b) = scene_with(MockBody::make_box(DVec3::splat(4.0)));
    let outs = s.apply_all(vec![grid(1, 2, 2), grid(2, 3, 1)]).unwrap();
    assert_eq!(outs.len(), 2);
    assert_eq!(s.doc().rules.len(), 2);
    assert!(s.undo());
    assert!(s.doc().rules.is_empty());
    let before = s.doc().clone();
    let r = s.apply_all(vec![
        grid(1, 2, 2),
        Op::DeleteRule {
            rule: newtopo_core::keys::RuleId(9999),
        },
    ]);
    assert!(r.is_err());
    assert_eq!(s.doc(), &before);
    assert_valid(&mut s, b, "after refused batch");
}

#[test]
fn layer_ops() {
    let (mut s, b) = scene_with(MockBody::make_box(DVec3::splat(4.0)));
    let id = LayerId(
        s.apply(Op::CreateLayer {
            name: "Parts".into(),
        })
        .unwrap()
        .created
        .unwrap(),
    );
    s.apply(Op::MoveBodyToLayer { body: b, layer: id }).unwrap();
    assert_eq!(s.doc().body(b).unwrap().layer, id);
    s.apply(Op::RenameLayer {
        layer: id,
        name: "Bolts".into(),
    })
    .unwrap();
    s.apply(Op::SetLayerVisible {
        layer: id,
        visible: false,
    })
    .unwrap();
    s.apply(Op::SetLayerLocked {
        layer: id,
        locked: true,
    })
    .unwrap();
    s.apply(Op::SetBodyLocked {
        body: b,
        locked: true,
    })
    .unwrap();
    s.apply(Op::SetActiveLayer { layer: id }).unwrap();
    let l = s.doc().layer(id).unwrap().clone();
    assert!(l.name == "Bolts" && !l.visible && l.locked);
    assert!(s.doc().body(b).unwrap().locked);
    s.apply(Op::DeleteLayer { layer: id }).unwrap();
    assert!(s.doc().layer(id).is_none());
    assert_eq!(s.doc().body(b).unwrap().layer, s.doc().active_layer);
    assert!(
        s.apply(Op::DeleteLayer {
            layer: s.doc().active_layer
        })
        .is_err(),
        "last layer stays"
    );
}

#[test]
fn flip_connect_and_bend_info() {
    let (mut s, b) = scene_with(MockBody::torus(4.0, 1.0));
    let p = PatchId::whole(b, FaceId(0));
    let bi = s.bend_info(&p).unwrap();
    assert_eq!(bi.bend, Bend::Two);
    let (a0, b0) = (bi.a.unwrap(), bi.b.unwrap());
    s.apply(Op::Connect {
        patches: faces(&[0]),
        a: 6,
        b: 4,
        start_on_seam: false,
    })
    .unwrap();
    s.apply(Op::FlipConnect { patch: p }).unwrap();
    let bi = s.bend_info(&p).unwrap();
    assert!(bi.flipped);
    assert!(
        (bi.a.unwrap().spread_dir - b0.spread_dir).length() < 1e-9
            && (bi.b.unwrap().spread_dir - a0.spread_dir).length() < 1e-9
    );
    assert_valid(&mut s, b, "flipped");
    // Cylinder: one direction, lines along the axis.
    let (mut s, b) = scene_with(MockBody::cylinder(2.0, 5.0));
    let bi = s.bend_info(&PatchId::whole(b, FaceId(0))).unwrap();
    assert_eq!(bi.bend, Bend::OneU);
    let a = bi.a.unwrap();
    assert!(
        a.line_dir.cross(DVec3::Z).length() < 1e-9
            && a.spread_dir.dot(DVec3::Z).abs() < 1e-9
            && bi.b.is_none()
    );
    assert_eq!(
        s.bend_info(&PatchId::whole(b, FaceId(1))).unwrap().bend,
        Bend::Flat
    );
}

#[test]
fn delete_edit() {
    let (mut s, b) = scene_with(MockBody::make_box(DVec3::new(6.0, 6.0, 2.0)));
    s.apply(grid(1, 2, 2)).unwrap();
    let m = s.output_of(b).unwrap().clone();
    let k = m
        .edges
        .iter()
        .find(|e| matches!(e.kind, OutEdgeKind::Line(_)))
        .unwrap()
        .key
        .clone()
        .unwrap();
    s.apply(Op::SetHard {
        body: Some(b),
        edge: k,
        hard: true,
    })
    .unwrap();
    let id = s.doc().edits[0].id;
    s.apply(Op::DeleteEdit { edit: id }).unwrap();
    assert!(s.doc().edits.is_empty());
    assert!(s.apply(Op::DeleteEdit { edit: id }).is_err());
}

#[test]
fn camera_and_save_copy() {
    let (mut s, _b) = scene_with(MockBody::cylinder(2.0, 5.0));
    let dir = tempfile::tempdir().unwrap();
    s.save(&dir.path().join("a.ntopo")).unwrap();
    assert!(!s.is_modified());
    let undo_before = s.can_undo();
    s.set_camera(Camera {
        distance: 42.0,
        ..Camera::default()
    });
    assert!(s.is_modified());
    assert_eq!(s.can_undo(), undo_before);
    s.save_copy(&dir.path().join("auto.ntopo")).unwrap();
    assert!(s.is_modified(), "autosave keeps the modified marker");
    let s2 = Scene::load(Arc::new(MockKernel::new()), &dir.path().join("auto.ntopo")).unwrap();
    assert_eq!(s2.doc().camera.distance, 42.0);
}

#[test]
fn iso_preview_matches_op() {
    let (mut s, b) = scene_with(MockBody::plate_with_holes(1, 1));
    let all = s.iso_preview(b, FaceId(1), IsoDir::U, 0.5, None, false);
    assert_eq!(all.len(), 2, "the hole cuts the line in two");
    let one = s.iso_preview(b, FaceId(1), IsoDir::U, 0.5, Some(0.1), false);
    assert_eq!(one.len(), 1);
    assert!(
        one[0]
            .iter()
            .all(|p| (p.z - 2.0).abs() < 1e-9 && p.x.abs() < 1e-9)
    );
    let (mut s, b) = scene_with(MockBody::split_cylinder(2.0, 4.0));
    let thr = s.iso_preview(b, FaceId(0), IsoDir::V, 0.5, None, true);
    assert_eq!(thr.len(), 2, "through mode crosses into the tangent half");
}

#[test]
fn selection_wrappers() {
    let (mut s, b) = scene_with(MockBody::torus(4.0, 1.0));
    set_autofill(&mut s, false);
    s.apply(Op::Connect {
        patches: faces(&[0]),
        a: 8,
        b: 6,
        start_on_seam: false,
    })
    .unwrap();
    let m = s.output_of(b).unwrap().clone();
    let e = m
        .edges
        .iter()
        .position(|e| matches!(e.kind, OutEdgeKind::Line(k) if k.index == 0))
        .unwrap();
    assert_eq!(s.edge_loop(b, e).len(), 6);
    assert_eq!(s.edge_ring(b, e).len(), 8);
    let p = m
        .polygons
        .iter()
        .position(|p| p.verts.contains(&m.edges[e].a) && p.verts.contains(&m.edges[e].b))
        .unwrap();
    assert_eq!(s.face_strip(b, p, e).len(), 8);
}

#[test]
fn replace_body_keeps_rules() {
    let (mut s, b) = scene_with(MockBody::cylinder(2.0, 5.0));
    s.apply(connect(0, 6)).unwrap();
    s.apply(grid(2, 3, 3)).unwrap();
    let before = s.output_of(b).unwrap().stats();
    let rep = s
        .replace_body_with_report(
            b,
            &MockBody::cylinder(2.0, 5.0).to_brep_bytes(),
            BodyAttributes {
                name: "cyl v2".into(),
                transform: DMat4::IDENTITY,
                ..Default::default()
            },
            HealReport::default(),
        )
        .unwrap();
    let nb = rep.new_body.unwrap();
    assert_ne!(nb, b);
    assert_eq!(rep.matched, 3);
    assert_eq!(rep.rules_kept, 2);
    assert!(s.doc().body(b).is_none() && s.doc().body(nb).unwrap().name == "cyl v2");
    assert_eq!(s.output_of(nb).unwrap().stats(), before);
    assert_valid(&mut s, nb, "replaced");
    assert!(s.undo());
    assert!(s.doc().body(b).is_some());
}

/// §6.11: plate with a 4 × 4 grid of holes, Connect A = 8 on one hole wall
/// with Link on gives 8 lines in all 16 walls.
#[test]
fn link_plate_holes() {
    let (mut s, b) = scene_with(MockBody::plate_with_holes(4, 4));
    let wall = 6u32;
    let set = s.link_set(b, FaceId(wall));
    assert_eq!(set.len(), 16);
    s.apply(Op::SetLink { on: true }).unwrap();
    s.apply(connect(wall, 8)).unwrap();
    assert_valid(&mut s, b, "linked connect");
    let m = s.output_of(b).unwrap().clone();
    for f in 6..22 {
        assert_eq!(lines_on_face(&m, f), 8, "wall {f}");
    }
    // Link off: editing one wall affects only it.
    s.apply(Op::SetLink { on: false }).unwrap();
    s.apply(connect(wall, 4)).unwrap();
    let m = s.output_of(b).unwrap().clone();
    assert_eq!(lines_on_face(&m, wall), 4);
    assert_eq!(lines_on_face(&m, 7), 8);
}

/// §6.12: on a part symmetric about X, Connect on the left boss appears on
/// the right one; with symmetry off, editing one side affects only it.
#[test]
fn symmetry_bosses() {
    let (mut s, b) = scene_with(MockBody::symmetric_part());
    s.apply(Op::SetSymmetry {
        on: true,
        axes: vec![Axis::X],
    })
    .unwrap();
    assert_eq!(s.symmetry_partners(b, FaceId(8)), vec![FaceId(6)]);
    s.apply(connect(8, 6)).unwrap();
    assert_valid(&mut s, b, "mirrored connect");
    let m = s.output_of(b).unwrap().clone();
    assert_eq!(lines_on_face(&m, 6), 6);
    assert_eq!(lines_on_face(&m, 8), 6);
    // Mirrored lines are mirror images.
    let pts = |m: &OutputMesh, f: u32, sx: f64| -> Vec<(i64, i64, i64)> {
        let mut v: Vec<_> = m
            .polygons
            .iter()
            .filter(|p| p.face == FaceId(f))
            .flat_map(|p| p.verts.iter().map(|&i| m.vertices[i as usize].position))
            .map(|p| {
                (
                    (p.x * sx * 1e5).round() as i64,
                    (p.y * 1e5).round() as i64,
                    (p.z * 1e5).round() as i64,
                )
            })
            .collect();
        v.sort();
        v.dedup();
        v
    };
    assert_eq!(pts(&m, 6, 1.0), pts(&m, 8, -1.0));
    s.apply(Op::SetSymmetry {
        on: false,
        axes: vec![Axis::X],
    })
    .unwrap();
    s.apply(connect(8, 3)).unwrap();
    let m = s.output_of(b).unwrap().clone();
    assert_eq!(lines_on_face(&m, 8), 3);
    assert_eq!(lines_on_face(&m, 6), 6);
}

/// §6.13: copy/paste between two identical bodies placed differently (and
/// parameterised differently) gives identical outputs up to the transform.
#[test]
fn copy_paste_rules() {
    let k = Arc::new(MockKernel::new());
    let mut s = Scene::new(k);
    let a = s
        .add_body(
            &MockBody::cylinder(2.0, 5.0).to_brep_bytes(),
            BodyAttributes {
                name: "a".into(),
                transform: DMat4::IDENTITY,
                ..Default::default()
            },
            HealReport::default(),
        )
        .unwrap();
    let m = glam::DAffine3::from_rotation_translation(
        glam::DQuat::from_axis_angle(DVec3::new(1.0, 2.0, 0.5).normalize(), 0.9),
        DVec3::new(10.0, -3.0, 4.0),
    );
    let b = s
        .add_body(
            &MockBody::cylinder(2.0, 5.0).transformed(m).to_brep_bytes(),
            BodyAttributes {
                name: "b".into(),
                transform: DMat4::IDENTITY,
                ..Default::default()
            },
            HealReport::default(),
        )
        .unwrap();
    s.apply(Op::Connect {
        patches: PatchSel {
            body: Some(a),
            faces: vec![0],
            ..Default::default()
        },
        a: 6,
        b: 0,
        start_on_seam: false,
    })
    .unwrap();
    s.apply(Op::Grid {
        patches: PatchSel {
            body: Some(a),
            faces: vec![2],
            ..Default::default()
        },
        u: 2,
        v: 3,
        start_on_seam: false,
        spacing: Spacing::ArcLength,
    })
    .unwrap();
    s.apply(Op::Iso {
        body: Some(a),
        face: 0,
        dir: IsoDir::V,
        value: 0.3,
        seed: None,
        split: false,
        through: false,
    })
    .unwrap();
    let clip = s.copy_rules(a);
    assert_eq!(clip.rules.len(), 3);
    let out = s
        .apply(Op::PasteRules {
            clipboard: clip,
            targets: vec![b],
        })
        .unwrap();
    let msg = out.message.unwrap();
    assert!(msg.contains("pasted 3"), "{msg}");
    assert_valid(&mut s, b, "pasted");
    let ma = s.output_of(a).unwrap().clone();
    let mb = s.output_of(b).unwrap().clone();
    assert_eq!(ma.stats(), mb.stats());
    let set = |m: &OutputMesh, t: glam::DAffine3| -> Vec<(i64, i64, i64)> {
        let mut v: Vec<_> = m
            .vertices
            .iter()
            .map(|v| t.transform_point3(v.position))
            .map(|p| {
                (
                    (p.x * 1e4).round() as i64,
                    (p.y * 1e4).round() as i64,
                    (p.z * 1e4).round() as i64,
                )
            })
            .collect();
        v.sort();
        v
    };
    assert_eq!(
        set(&ma, m),
        set(&mb, glam::DAffine3::IDENTITY),
        "identical up to the rigid transform"
    );
    let _ = (Rule::Join {
        body: BodyId(0),
        a: newtopo_core::keys::VertexKey::Pole(FaceId(0), 0),
        b: newtopo_core::keys::VertexKey::Pole(FaceId(0), 1),
    },);
}

/// Review round 3, finding 5: Collapse and Weld on one linked hole wall are
/// applied to every wall.
#[test]
fn link_collapse_and_weld() {
    use newtopo_core::doc::Edit;
    use newtopo_core::keys::VertexKey;
    let (mut s, b) = scene_with(MockBody::plate_with_holes(2, 2));
    let walls: Vec<u32> = s.link_set(b, FaceId(6)).iter().map(|f| f.0).collect();
    assert_eq!(walls.len(), 4);
    s.apply(Op::SetLink { on: true }).unwrap();
    s.apply(grid(6, 8, 3)).unwrap();
    assert_valid(&mut s, b, "linked grid");
    // Line segments inside a wall, between two line crossings.
    let inner = |m: &OutputMesh, w: u32| -> Vec<usize> {
        m.edges
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e.kind, OutEdgeKind::Line(_)))
            .filter(|(_, e)| {
                [e.a, e.b]
                    .iter()
                    .all(|v| matches!(m.vertices[*v as usize].key, VertexKey::Crossing { .. }))
            })
            .filter(|(_, e)| {
                m.polygons.iter().any(|p| {
                    p.face == FaceId(w) && p.verts.contains(&e.a) && p.verts.contains(&e.b)
                })
            })
            .map(|(i, _)| i)
            .collect()
    };
    let m = s.output_of(b).unwrap().clone();
    let counts: Vec<usize> = walls.iter().map(|w| inner(&m, *w).len()).collect();
    assert!(
        counts.iter().all(|c| *c == counts[0] && *c >= 4),
        "{counts:?}"
    );
    let n =
        |s: &Scene, f: &dyn Fn(&Edit) -> bool| s.doc().edits.iter().filter(|e| f(&e.edit)).count();
    let e = inner(&m, 6)[0];
    let out = s
        .apply(Op::Collapse {
            body: Some(b),
            edge: m.edges[e].key.clone().unwrap(),
            confirmed: false,
        })
        .unwrap();
    assert!(
        out.message.as_deref().unwrap_or("").contains("3 linked"),
        "{:?}",
        out.message
    );
    assert_eq!(n(&s, &|e| matches!(e, Edit::Collapse { .. })), 4);
    assert_valid(&mut s, b, "linked collapse");
    let m2 = s.output_of(b).unwrap().clone();
    let after: Vec<usize> = walls.iter().map(|w| inner(&m2, *w).len()).collect();
    assert!(
        after.iter().all(|c| *c == after[0] && *c < counts[0]),
        "{after:?}"
    );
    // Weld the two ends of another inner segment, away from the collapse.
    let e = *inner(&m2, 6).last().unwrap();
    let (from, to) = (
        m2.vertices[m2.edges[e].a as usize].key.clone(),
        m2.vertices[m2.edges[e].b as usize].key.clone(),
    );
    let out = s
        .apply(Op::Weld {
            body: Some(b),
            from,
            to,
            confirmed: false,
        })
        .unwrap();
    assert!(
        out.message.as_deref().unwrap_or("").contains("3 linked"),
        "{:?}",
        out.message
    );
    assert_eq!(n(&s, &|e| matches!(e, Edit::Weld { .. })), 4);
    assert_valid(&mut s, b, "linked weld");
    let m3 = s.output_of(b).unwrap().clone();
    let welded: Vec<usize> = walls.iter().map(|w| inner(&m3, *w).len()).collect();
    assert!(
        welded.iter().all(|c| *c == welded[0] && *c < after[0]),
        "{welded:?}"
    );
    // Link off: one wall only.
    s.apply(Op::SetLink { on: false }).unwrap();
    let e = inner(&m3, 6)[0];
    let out = s
        .apply(Op::Collapse {
            body: Some(b),
            edge: m3.edges[e].key.clone().unwrap(),
            confirmed: false,
        })
        .unwrap();
    assert!(out.message.is_none(), "{:?}", out.message);
    assert_eq!(n(&s, &|e| matches!(e, Edit::Collapse { .. })), 5);
}

/// Review round 3, finding 5: a Cut on the left boss is mirrored onto the
/// right one with symmetry on.
#[test]
fn symmetry_cut() {
    use newtopo_core::doc::CutPoint;
    let (mut s, b) = scene_with(MockBody::symmetric_part());
    s.apply(Op::SetSymmetry {
        on: true,
        axes: vec![Axis::X],
    })
    .unwrap();
    let t = s.ctx(b).unwrap().topo.clone();
    let rims: Vec<_> = t
        .face(FaceId(8))
        .wires
        .iter()
        .flat_map(|w| w.coedges.iter().map(|c| c.edge))
        .filter(|e| !t.edge(*e).seam && !t.edge(*e).degenerate)
        .collect();
    assert!(rims.len() >= 2, "{rims:?}");
    let pt = |e: newtopo_kernel::EdgeId| {
        let (a, z) = t.edge(e).t_range;
        CutPoint::CadEdge {
            body: b,
            edge: e,
            t: 0.5 * (a + z),
        }
    };
    let out = s
        .apply(Op::Cut {
            points: vec![pt(rims[0]), pt(rims[1])],
            closed: false,
        })
        .unwrap();
    assert!(
        out.message
            .as_deref()
            .unwrap_or("")
            .contains("1 linked or mirrored"),
        "{:?}",
        out.message
    );
    assert_valid(&mut s, b, "mirrored cut");
    let m = s.output_of(b).unwrap().clone();
    assert_eq!(lines_on_face(&m, 8), 1);
    assert_eq!(lines_on_face(&m, 6), 1);
}
