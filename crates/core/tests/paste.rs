//! Copy / paste of rules (SPEC §6.13), review round 3 findings 6–8:
//! takeover on paste, rules on split patches, and edits / Join rules.
mod common;
use common::*;
use glam::{DAffine3, DMat4, DQuat, DVec3};
use newtopo_core::doc::{IsoDir, PatchId, Rule, SliceScope, Spacing};
use newtopo_core::mesh::{OutEdgeKind, OutputMesh};
use newtopo_core::ops::PatchSel;
use newtopo_core::{Op, Scene};
use newtopo_kernel::mock::{MockBody, MockKernel};
use newtopo_kernel::{BodyAttributes, BodyId, FaceId, HealReport};
use std::sync::Arc;

/// Two identical cylinders; the second is moved by `m` (geometry, not the
/// body transform), so its parameterisation differs from the first's.
fn two(m: DAffine3) -> (Scene, BodyId, BodyId) {
    two_of(MockBody::cylinder(2.0, 5.0), m)
}

/// [`two`] for any mock shape.
fn two_of(shape: MockBody, m: DAffine3) -> (Scene, BodyId, BodyId) {
    let mut s = Scene::new(Arc::new(MockKernel::new()));
    let mut add = |body: MockBody, name: &str| {
        s.add_body(
            &body.to_brep_bytes(),
            BodyAttributes {
                name: name.into(),
                transform: DMat4::IDENTITY,
                ..Default::default()
            },
            HealReport::default(),
        )
        .unwrap()
    };
    let a = add(shape.clone(), "a");
    let b = add(shape.transformed(m), "b");
    (s, a, b)
}

fn moved() -> DAffine3 {
    DAffine3::from_rotation_translation(
        DQuat::from_axis_angle(DVec3::new(1.0, 2.0, 0.5).normalize(), 0.9),
        DVec3::new(10.0, -3.0, 4.0),
    )
}

fn on(body: BodyId, faces: &[u32]) -> PatchSel {
    PatchSel {
        body: Some(body),
        faces: faces.to_vec(),
        ..Default::default()
    }
}

fn grid(body: BodyId, f: u32, u: u32, v: u32) -> Op {
    Op::Grid {
        patches: on(body, &[f]),
        u,
        v,
        start_on_seam: false,
        spacing: Spacing::ArcLength,
    }
}

fn body_slice(body: BodyId, count: u32) -> Op {
    Op::Slice {
        body: Some(body),
        patches: None,
        count,
        through: false,
        guide_edge: None,
    }
}

fn paste(s: &mut Scene, from: BodyId, to: BodyId) -> String {
    let clip = s.copy_rules(from);
    s.apply(Op::PasteRules {
        clipboard: clip,
        targets: vec![to],
    })
    .unwrap()
    .message
    .unwrap()
}

/// Rounded output vertex positions, mapped by `t`.
fn positions(m: &OutputMesh, t: DAffine3) -> Vec<(i64, i64, i64)> {
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
}

fn assert_same_output(s: &mut Scene, a: BodyId, b: BodyId, m: DAffine3) {
    let ma = s.output_of(a).unwrap().clone();
    let mb = s.output_of(b).unwrap().clone();
    assert_eq!(ma.stats(), mb.stats());
    assert_eq!(
        positions(&ma, m),
        positions(&mb, DAffine3::IDENTITY),
        "identical up to the rigid transform"
    );
}

/// Finding 6: pasting a Grid onto one face of a body-wide Slice takes only
/// that face over; the Slice keeps the other faces (patch scope, frozen
/// planes). A pasted body Slice takes over every patch of the target.
#[test]
fn paste_takes_over_like_a_tool() {
    let (mut s, a, b) = two(DAffine3::IDENTITY);
    s.apply(body_slice(b, 4)).unwrap();
    s.apply(grid(a, 2, 2, 3)).unwrap();
    let msg = paste(&mut s, a, b);
    assert!(msg.contains("pasted 1 rule(s)"), "{msg}");
    let on_b: Vec<&Rule> = s
        .doc()
        .rules
        .iter()
        .map(|r| &r.rule)
        .filter(|r| match r {
            Rule::Slice {
                scope: SliceScope::Patches(l),
                ..
            } => l.iter().all(|p| p.body == b),
            Rule::Grid { patch, .. } => patch.body == b,
            _ => false,
        })
        .collect();
    assert_eq!(on_b.len(), 2, "{on_b:?}");
    let Rule::Slice {
        scope: SliceScope::Patches(l),
        frozen_planes,
        ..
    } = on_b[0]
    else {
        panic!("{on_b:?}")
    };
    assert!(frozen_planes.is_some());
    assert!(!l.contains(&PatchId::whole(b, FaceId(2))));
    assert!(l.contains(&PatchId::whole(b, FaceId(0))));
    assert_valid(&mut s, b, "grid pasted over slice");

    // A pasted body Slice replaces the target's spread rules.
    let (mut s, a, b) = two(DAffine3::IDENTITY);
    s.apply(grid(b, 2, 2, 3)).unwrap();
    s.apply(Op::Connect {
        patches: on(b, &[0]),
        a: 5,
        b: 0,
        start_on_seam: false,
    })
    .unwrap();
    s.apply(body_slice(a, 4)).unwrap();
    paste(&mut s, a, b);
    let on_b: Vec<&Rule> = s
        .doc()
        .rules
        .iter()
        .map(|r| &r.rule)
        .filter(|r| match r {
            Rule::Slice {
                scope: SliceScope::Body(x),
                ..
            } => *x == b,
            Rule::Grid { patch, .. } => patch.body == b,
            Rule::Connect { patches, .. } => patches[0].body == b,
            _ => false,
        })
        .collect();
    assert_eq!(on_b.len(), 1, "{on_b:?}");
    assert!(matches!(on_b[0], Rule::Slice { .. }));
    assert_valid(&mut s, b, "body slice pasted");
    assert_same_output(&mut s, a, b, DAffine3::IDENTITY);
}

/// Finding 7: rules on patches bounded by split lines are pasted onto the
/// target's matching split patches.
#[test]
fn paste_rules_on_split_patches() {
    let m = moved();
    let (mut s, a, b) = two(m);
    s.apply(Op::Iso {
        body: Some(a),
        face: 0,
        dir: IsoDir::V,
        value: 0.3,
        seed: None,
        split: true,
        through: false,
    })
    .unwrap();
    let bands: Vec<PatchId> = s
        .patch_records(a)
        .iter()
        .filter(|p| p.face == FaceId(0))
        .map(|p| p.id)
        .collect();
    assert_eq!(bands.len(), 2);
    for (p, n) in bands.iter().zip([6, 3]) {
        s.apply(Op::Connect {
            patches: PatchSel {
                body: Some(a),
                patches: vec![*p],
                ..Default::default()
            },
            a: n,
            b: 0,
            start_on_seam: false,
        })
        .unwrap();
    }
    s.apply(grid(a, 2, 2, 2)).unwrap();
    assert_valid(&mut s, a, "source");
    let msg = paste(&mut s, a, b);
    assert!(
        msg.contains("pasted 3 rule(s)") && !msg.contains("could not"),
        "{msg}"
    );
    assert_valid(&mut s, b, "pasted");
    assert_same_output(&mut s, a, b, m);
}

/// Finding 8: edits and Join rules are pasted too, so an identical target
/// gets an identical output.
#[test]
fn paste_edits_and_joins() {
    let m = moved();
    let (mut s, a, b) = two(m);
    s.apply(Op::Connect {
        patches: on(a, &[0]),
        a: 6,
        b: 0,
        start_on_seam: false,
    })
    .unwrap();
    s.apply(grid(a, 2, 2, 2)).unwrap();
    let out = s.output_of(a).unwrap().clone();
    let quad = out
        .polygons
        .iter()
        .find(|p| p.face == FaceId(0) && p.verts.len() == 4)
        .unwrap()
        .clone();
    let key = |i: u32| out.vertices[i as usize].key.clone();
    s.apply(Op::Join {
        body: Some(a),
        a: key(quad.verts[0]),
        b: key(quad.verts[2]),
    })
    .unwrap();
    let out = s.output_of(a).unwrap().clone();
    let e = out
        .edges
        .iter()
        .find(|e| {
            matches!(e.kind, OutEdgeKind::Line(_))
                && !quad.verts.contains(&e.a)
                && !quad.verts.contains(&e.b)
                && out
                    .polygons
                    .iter()
                    .any(|p| p.face == FaceId(0) && p.verts.contains(&e.a))
        })
        .unwrap();
    s.apply(Op::RemoveEdge {
        body: Some(a),
        edge: e.key.clone().unwrap(),
    })
    .unwrap();
    assert_valid(&mut s, a, "source");
    let clip = s.copy_rules(a);
    assert_eq!(clip.edit_list.len(), 1);
    let msg = paste(&mut s, a, b);
    assert!(
        msg.contains("pasted 3 rule(s) and 1 edit(s)") && !msg.contains("could not"),
        "{msg}"
    );
    assert_valid(&mut s, b, "pasted");
    assert_same_output(&mut s, a, b, m);
}

/// Sol round 2, fix 2: a body with only a Join rule pastes onto an
/// identical, moved body.
#[test]
fn paste_join_only() {
    let m = moved();
    let (mut s, a, b) = two_of(MockBody::make_box(DVec3::new(4.0, 3.0, 2.0)), m);
    let out = s.output_of(a).unwrap().clone();
    // A diagonal across one face of the box (the first one a Join accepts).
    let key = |i: u32| out.vertices[i as usize].key.clone();
    let joined = out
        .polygons
        .iter()
        .filter(|p| p.verts.len() == 4)
        .flat_map(|p| [(p.verts[0], p.verts[2]), (p.verts[1], p.verts[3])])
        .any(|(x, y)| {
            s.apply(Op::Join {
                body: Some(a),
                a: key(x),
                b: key(y),
            })
            .is_ok()
        });
    assert!(joined);
    assert_valid(&mut s, a, "source");
    let msg = paste(&mut s, a, b);
    assert!(
        msg.contains("pasted 1 rule(s) and 0 edit(s)") && !msg.contains("could not"),
        "{msg}"
    );
    assert_valid(&mut s, b, "pasted");
    assert_same_output(&mut s, a, b, m);
}

/// Sol round 2, fix 2: a body with only an edit pastes onto an identical,
/// moved body.
#[test]
fn paste_edit_only() {
    let m = moved();
    let (mut s, a, b) = two(m);
    let out = s.output_of(a).unwrap().clone();
    let rim = out
        .edges
        .iter()
        .find(|e| {
            matches!(e.kind, OutEdgeKind::Cad(_))
                && out.polygons.iter().any(|p| {
                    p.face == FaceId(2) && p.verts.contains(&e.a) && p.verts.contains(&e.b)
                })
        })
        .unwrap();
    s.apply(Op::Collapse {
        body: Some(a),
        edge: rim.key.clone().unwrap(),
        confirmed: true,
    })
    .unwrap();
    assert_valid(&mut s, a, "source");
    assert!(s.doc().rules.is_empty());
    let msg = paste(&mut s, a, b);
    assert!(
        msg.contains("pasted 0 rule(s) and 1 edit(s)") && !msg.contains("could not"),
        "{msg}"
    );
    assert_valid(&mut s, b, "pasted");
    assert_same_output(&mut s, a, b, m);
}
