//! Tool acceptance criteria (SPEC §4.3, §6.2–§6.10, §6.14) on mock shapes.
mod common;
use common::*;
use glam::{DVec2, DVec3};
use newtopo_core::Op;
use newtopo_core::doc::{CutPoint, IsoDir, Rule, Spacing};
use newtopo_core::eval::lines::Bend;
use newtopo_core::keys::{EdgeKey, LineKey, RuleId, VertexKey};
use newtopo_core::mesh::{Anchor, OutEdgeKind, OutputMesh};
use newtopo_core::ops::PatchSel;
use newtopo_core::select::{Adjacency, every_other};
use newtopo_kernel::mock::MockBody;
use newtopo_kernel::{BodyId, EdgeId, FaceId};
use std::collections::BTreeSet;

fn faces(f: &[u32]) -> PatchSel {
    PatchSel {
        faces: f.to_vec(),
        ..Default::default()
    }
}
fn connect(f: &[u32], a: u32, b: u32, seam: bool) -> Op {
    Op::Connect {
        patches: faces(f),
        a,
        b,
        start_on_seam: seam,
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
fn line_indices(m: &OutputMesh, rule: Option<RuleId>) -> BTreeSet<u32> {
    m.edges
        .iter()
        .filter_map(|e| match e.kind {
            OutEdgeKind::Line(k) if rule.is_none_or(|r| r == k.rule) => Some(k.index),
            _ => None,
        })
        .collect()
}
fn rule_ids(s: &newtopo_core::Scene) -> Vec<RuleId> {
    s.doc().rules.iter().map(|r| r.id).collect()
}

/// §6.2: a full cylinder with Connect A = 16 gives 16 generator lines, every
/// vertex exactly on the surface.
#[test]
fn connect_cylinder_16() {
    let (mut s, b) = scene_with(MockBody::cylinder(2.0, 5.0));
    s.apply(connect(&[0], 16, 0, false)).unwrap();
    assert_valid(&mut s, b, "connect 16");
    let m = s.output_of(b).unwrap().clone();
    assert_eq!(line_indices(&m, None).len(), 16);
    let diag = s.ctx(b).unwrap().size;
    for v in &m.vertices {
        let r = (v.position.x * v.position.x + v.position.y * v.position.y).sqrt();
        let on_side = (r - 2.0).abs() < 1e-6 * diag;
        let on_cap = v.position.z.abs() < 1e-9 || (v.position.z - 5.0).abs() < 1e-9;
        assert!(
            on_side || on_cap,
            "vertex off the surface: {:?}",
            v.position
        );
    }
    // Generators: both ends at the rims, same angle.
    for e in m
        .edges
        .iter()
        .filter(|e| matches!(e.kind, OutEdgeKind::Line(_)))
    {
        let (a, c) = (
            m.vertices[e.a as usize].position,
            m.vertices[e.b as usize].position,
        );
        assert!(
            (a.truncate() - c.truncate()).length() < 1e-9,
            "not a generator"
        );
    }
}

/// §6.2: changing 16 → 8 → 16 gives an identical output mesh.
#[test]
fn connect_16_8_16_identical() {
    let (mut s, b) = scene_with(MockBody::cylinder(2.0, 5.0));
    s.apply(connect(&[0], 16, 0, false)).unwrap();
    let m16 = s.output_of(b).unwrap().clone();
    s.apply(connect(&[0], 8, 0, false)).unwrap();
    s.apply(connect(&[0], 16, 0, false)).unwrap();
    let again = s.output_of(b).unwrap().clone();
    // Rule ids differ (re-running replaces the rule); compare geometry and topology.
    assert_eq!(m16.polygons.len(), again.polygons.len());
    assert_eq!(m16.vertices.len(), again.vertices.len());
    for (x, y) in m16.vertices.iter().zip(&again.vertices) {
        assert!((x.position - y.position).length() < 1e-12);
    }
    let strip = |m: &OutputMesh| {
        m.polygons
            .iter()
            .map(|p| p.verts.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(strip(&m16), strip(&again));
}

fn rim_points(m: &OutputMesh) -> Vec<DVec3> {
    let mut v: Vec<DVec3> = m
        .edges
        .iter()
        .filter(|e| matches!(e.kind, OutEdgeKind::Line(_)))
        .flat_map(|e| {
            [
                m.vertices[e.a as usize].position,
                m.vertices[e.b as usize].position,
            ]
        })
        .collect();
    let key = |p: &DVec3| {
        (
            (p.x * 1e6).round() as i64,
            (p.y * 1e6).round() as i64,
            (p.z * 1e6).round() as i64,
        )
    };
    v.sort_by_key(key);
    v
}

/// §6.2: start-on-seam off gives the same world-space lines on a cylinder
/// whose seam is rotated 40°; start-on-seam on moves them with the seam.
#[test]
fn connect_canonical_angle_ignores_seam() {
    let (mut s1, b1) = scene_with(MockBody::cylinder(2.0, 5.0));
    let (mut s2, b2) = scene_with(MockBody::cylinder_with_seam(2.0, 5.0, 40f64.to_radians()));
    for (s, b) in [(&mut s1, b1), (&mut s2, b2)] {
        s.apply(connect(&[0], 6, 0, false)).unwrap();
        assert_valid(s, b, "connect 6");
    }
    let (p1, p2) = (
        rim_points(s1.output_of(b1).unwrap()),
        rim_points(s2.output_of(b2).unwrap()),
    );
    assert_eq!(p1.len(), p2.len());
    for (a, c) in p1.iter().zip(&p2) {
        assert!((*a - *c).length() < 1e-9, "{a} vs {c}");
    }
    for (s, b) in [(&mut s1, b1), (&mut s2, b2)] {
        s.apply(connect(&[0], 6, 0, true)).unwrap();
        assert_valid(s, b, "connect 6 on seam");
    }
    let (p1, p2) = (
        rim_points(s1.output_of(b1).unwrap()),
        rim_points(s2.output_of(b2).unwrap()),
    );
    assert!(
        p1.iter().zip(&p2).any(|(a, c)| (*a - *c).length() > 1e-3),
        "start on seam follows the seam"
    );
}

/// §6.2: a torus face has two directions; A = 12, B = 8 gives a 12 × 8 quad grid.
#[test]
fn connect_torus_12_by_8() {
    let (mut s, b) = scene_with(MockBody::torus(4.0, 1.0));
    set_autofill(&mut s, false);
    let p = s.patches(b);
    assert_eq!(p[0].bend, Bend::Two);
    s.apply(connect(&[0], 12, 8, false)).unwrap();
    assert_valid(&mut s, b, "torus 12x8");
    let st = s.output_of(b).unwrap().stats();
    assert_eq!((st.quads, st.tris, st.ngons, st.verts), (96, 0, 0, 96));
}

/// §6.2: flat patches can't be connected (toast-worthy refusal); a planar
/// ring connects radially.
#[test]
fn connect_flat_refused_ring_radial() {
    let (mut s, _) = scene_with(MockBody::make_box(DVec3::splat(2.0)));
    let r = s.apply(connect(&[1], 4, 0, false)).unwrap_err();
    assert!(r.reason.contains("Surface Grid"), "{}", r.reason);
    let (mut s, b) = scene_with(MockBody::ring(3.0, 1.0));
    s.apply(connect(&[0], 8, 0, false)).unwrap();
    assert_valid(&mut s, b, "ring radial");
    let m = s.output_of(b).unwrap();
    for e in m
        .edges
        .iter()
        .filter(|e| matches!(e.kind, OutEdgeKind::Line(_)))
    {
        let (a, c) = (
            m.vertices[e.a as usize].position,
            m.vertices[e.b as usize].position,
        );
        assert!(
            a.truncate().perp_dot(c.truncate()).abs() < 1e-9,
            "radial line through the centre"
        );
    }
}

/// §6.5: a split at mid-height of a cylinder lets the two halves carry
/// different Connect counts; Connect over both halves joins lines end to end.
#[test]
fn split_cylinder_halves_counts_and_chain() {
    let (mut s, b) = scene_with(MockBody::cylinder(2.0, 5.0));
    s.apply(Op::Iso {
        body: Some(b),
        face: 0,
        dir: IsoDir::V,
        value: 0.5,
        seed: None,
        split: true,
        through: false,
    })
    .unwrap();
    let patches: Vec<_> = s
        .patches(b)
        .into_iter()
        .filter(|p| p.face == FaceId(0))
        .collect();
    assert_eq!(patches.len(), 2);
    // Chain: both halves, same count → lines meet on the split.
    s.apply(Op::Connect {
        patches: PatchSel {
            patches: patches.iter().map(|p| p.id).collect(),
            ..Default::default()
        },
        a: 4,
        b: 0,
        start_on_seam: false,
    })
    .unwrap();
    assert_valid(&mut s, b, "chain");
    let m = s.output_of(b).unwrap().clone();
    let on_split: Vec<u32> = m
        .edges
        .iter()
        .filter(|e| matches!(e.kind, OutEdgeKind::Split(_)))
        .flat_map(|e| [e.a, e.b])
        .collect();
    let joined = on_split
        .iter()
        .collect::<BTreeSet<_>>()
        .iter()
        .filter(|&&&v| {
            m.edges
                .iter()
                .filter(|e| matches!(e.kind, OutEdgeKind::Line(_)) && (e.a == v || e.b == v))
                .count()
                == 2
        })
        .count();
    assert_eq!(joined, 4, "4 continuous lines across the split");
    // Different counts per half.
    s.apply(Op::Connect {
        patches: PatchSel {
            patches: vec![patches[0].id],
            ..Default::default()
        },
        a: 8,
        b: 0,
        start_on_seam: false,
    })
    .unwrap();
    s.apply(Op::Connect {
        patches: PatchSel {
            patches: vec![patches[1].id],
            ..Default::default()
        },
        a: 3,
        b: 0,
        start_on_seam: false,
    })
    .unwrap();
    assert_valid(&mut s, b, "different counts");
    let counts: Vec<u32> = s
        .doc()
        .rules
        .iter()
        .filter_map(|r| {
            if let Rule::Connect { count_a, .. } = r.rule {
                Some(count_a)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(counts.len(), 2);
    assert!(counts.contains(&8) && counts.contains(&3));
}

/// §6.3: a trimmed planar face with a circular hole gets a grid that stops
/// at the hole, with no polygon crossing the hole.
#[test]
fn grid_stops_at_hole() {
    for af in [true, false] {
        let (mut s, b) = scene_with(MockBody::plate_with_holes(1, 1));
        set_autofill(&mut s, af);
        s.apply(grid(1, 5, 5)).unwrap();
        assert_valid(&mut s, b, "grid with hole");
        let m = s.output_of(b).unwrap();
        for p in m.polygons.iter().filter(|p| p.face == FaceId(1)) {
            let c: DVec3 = p
                .verts
                .iter()
                .map(|&v| m.vertices[v as usize].position)
                .sum::<DVec3>()
                / p.verts.len() as f64;
            assert!(
                c.truncate().length() > 2.5 - 1e-6,
                "polygon centroid inside the hole"
            );
        }
    }
}

/// §6.4: slicing a box body with 10 planes gives 10 closed loops.
#[test]
fn slice_box_ten_loops() {
    let (mut s, b) = scene_with(MockBody::make_box(DVec3::new(10.0, 4.0, 3.0)));
    s.apply(Op::Slice {
        body: Some(b),
        patches: None,
        count: 10,
        through: false,
        guide_edge: None,
    })
    .unwrap();
    assert_valid(&mut s, b, "slice box");
    let m = s.output_of(b).unwrap();
    // Each plane cuts the four faces around the long edges: 4 line pieces.
    let pieces: BTreeSet<(u32, u32)> = m
        .edges
        .iter()
        .filter_map(|e| {
            if let OutEdgeKind::Line(k) = e.kind {
                Some((k.index, k.sub))
            } else {
                None
            }
        })
        .collect();
    let planes: BTreeSet<u32> = pieces.iter().map(|p| p.0).collect();
    assert_eq!(planes.len(), 10);
    // Each loop: its vertices all have exactly two line edges of that plane.
    for k in planes {
        let es: Vec<_> = m
            .edges
            .iter()
            .filter(|e| matches!(e.kind, OutEdgeKind::Line(l) if l.index == k))
            .collect();
        let mut deg = std::collections::HashMap::new();
        for e in &es {
            *deg.entry(e.a).or_insert(0) += 1;
            *deg.entry(e.b).or_insert(0) += 1;
        }
        assert!(deg.values().all(|&d| d == 2), "plane {k} is a closed loop");
    }
}

/// §6.4: slicing a revolved part gives radial lines through its axis.
#[test]
fn slice_revolved_radial() {
    let (mut s, b) = scene_with(MockBody::cylinder(2.0, 5.0));
    s.apply(Op::Slice {
        body: Some(b),
        patches: None,
        count: 3,
        through: false,
        guide_edge: None,
    })
    .unwrap();
    assert_valid(&mut s, b, "slice cylinder");
    let m = s.output_of(b).unwrap();
    let mut n = 0;
    for e in m
        .edges
        .iter()
        .filter(|e| matches!(e.kind, OutEdgeKind::Line(_)))
    {
        let (a, c) = (
            m.vertices[e.a as usize].position,
            m.vertices[e.b as usize].position,
        );
        if (a.z - c.z).abs() < 1e-9 {
            // On a cap: a radial chord through the axis.
            assert!(
                a.truncate().perp_dot(c.truncate()).abs() < 1e-6,
                "cap line passes through the axis"
            );
            n += 1;
        } else {
            assert!(
                (a.truncate() - c.truncate()).length() < 1e-6,
                "side line is a generator"
            );
        }
    }
    assert!(n >= 6);
}

/// §4.3: body Slice with 10 planes, then Connect on one patch: that patch
/// has only Connect lines; every other patch keeps its Slice lines.
#[test]
fn slice_body_then_connect_takeover() {
    let (mut s, b) = scene_with(MockBody::symmetric_part());
    s.apply(Op::Slice {
        body: Some(b),
        patches: None,
        count: 10,
        through: false,
        guide_edge: None,
    })
    .unwrap();
    assert_valid(&mut s, b, "slice symmetric part");
    let before = s.output_of(b).unwrap().clone();
    let boss = 6u32; // first boss side face
    s.apply(connect(&[boss], 6, 0, false)).unwrap();
    assert_valid(&mut s, b, "connect after slice");
    let after = s.output_of(b).unwrap().clone();
    let slice_id = s
        .doc()
        .rules
        .iter()
        .find(|r| matches!(r.rule, Rule::Slice { .. }))
        .unwrap()
        .id;
    let connect_id = s
        .doc()
        .rules
        .iter()
        .find(|r| matches!(r.rule, Rule::Connect { .. }))
        .unwrap()
        .id;
    // Face of a line edge, by the closest polygon centroid containing both ends.
    let edge_face = |m: &OutputMesh, e: &newtopo_core::mesh::OutEdge| -> u32 {
        m.polygons
            .iter()
            .find(|p| p.verts.contains(&e.a) && p.verts.contains(&e.b))
            .map(|p| p.face.0)
            .unwrap_or(u32::MAX)
    };
    let pts = |m: &OutputMesh, face: u32, rule: RuleId| -> Vec<(i64, i64, i64)> {
        let mut v: Vec<(i64, i64, i64)> = m
            .edges
            .iter()
            .filter(|e| {
                matches!(e.kind, OutEdgeKind::Line(k) if k.rule == rule) && edge_face(m, e) == face
            })
            .flat_map(|e| [e.a, e.b])
            .map(|v| {
                let p = m.vertices[v as usize].position;
                (
                    (p.x * 1e6).round() as i64,
                    (p.y * 1e6).round() as i64,
                    (p.z * 1e6).round() as i64,
                )
            })
            .collect();
        v.sort();
        v.dedup();
        v
    };
    let old_slice = before
        .edges
        .iter()
        .find_map(|e| {
            if let OutEdgeKind::Line(k) = e.kind {
                Some(k.rule)
            } else {
                None
            }
        })
        .unwrap();
    assert!(
        pts(&after, boss, slice_id).is_empty(),
        "boss patch has no slice lines"
    );
    assert!(
        !pts(&after, boss, connect_id).is_empty(),
        "boss patch has connect lines"
    );
    for f in 0..s.ctx(b).unwrap().topo.faces.len() as u32 {
        if f == boss {
            continue;
        }
        assert_eq!(
            pts(&before, f, old_slice),
            pts(&after, f, slice_id),
            "face {f} keeps its slice lines"
        );
    }
}

/// §6.5: a through iso on the two tangent halves of a split cylinder makes
/// one closed loop.
#[test]
fn iso_through_closed_loop() {
    let (mut s, b) = scene_with(MockBody::split_cylinder(2.0, 4.0));
    s.apply(Op::Iso {
        body: Some(b),
        face: 0,
        dir: IsoDir::V,
        value: 0.5,
        seed: None,
        split: false,
        through: true,
    })
    .unwrap();
    assert_valid(&mut s, b, "through iso");
    let Rule::Iso { pieces, .. } = &s.doc().rules[0].rule else {
        panic!()
    };
    assert_eq!(pieces.len(), 2, "continues into the tangent neighbour");
    let m = s.output_of(b).unwrap();
    let es: Vec<_> = m
        .edges
        .iter()
        .filter(|e| matches!(e.kind, OutEdgeKind::Line(_)))
        .collect();
    let mut deg = std::collections::HashMap::new();
    for e in &es {
        *deg.entry(e.a).or_insert(0) += 1;
        *deg.entry(e.b).or_insert(0) += 1;
    }
    assert!(deg.values().all(|&d| d == 2), "closed loop");
}

fn edge_between(s: &newtopo_core::Scene, b: BodyId, f1: u32, f2: u32) -> EdgeId {
    let t = &s.ctx(b).unwrap().topo;
    t.edge_ids()
        .find(|&e| t.edge(e).faces.contains(&FaceId(f1)) && t.edge(e).faces.contains(&FaceId(f2)))
        .unwrap()
}

/// §6.6: a cut across three faces of a box puts every vertex on the exact
/// surface and splits each crossed polygon in two.
#[test]
fn cut_across_three_faces() {
    let (mut s, b) = scene_with(MockBody::make_box(DVec3::new(4.0, 4.0, 4.0)));
    set_autofill(&mut s, false);
    // Faces: 0 −z, 1 +z, 2 −x, 3 +x. Start on the −x/−z edge, cross −x, the
    // top and +x, end on the +x/−z edge.
    let e1 = edge_between(&s, b, 2, 0);
    let e2 = edge_between(&s, b, 3, 0);
    let t1 = s.ctx(b).unwrap().topo.edge(e1).t_range;
    let t2 = s.ctx(b).unwrap().topo.edge(e2).t_range;
    let points = vec![
        CutPoint::CadEdge {
            body: b,
            edge: e1,
            t: 0.5 * (t1.0 + t1.1),
        },
        CutPoint::Face {
            body: b,
            face: FaceId(1),
            uv: DVec2::new(2.0, 2.0),
        },
        CutPoint::CadEdge {
            body: b,
            edge: e2,
            t: 0.5 * (t2.0 + t2.1),
        },
    ];
    s.apply(Op::Cut {
        points,
        closed: false,
    })
    .unwrap();
    assert_valid(&mut s, b, "cut");
    let st = s.output_of(b).unwrap().stats();
    assert_eq!(st.faces, 9, "three faces split in two");
}

/// §6.6: face-anchored cut points don't move when a Connect count changes.
#[test]
fn cut_points_stay_when_counts_change() {
    let (mut s, b) = scene_with(MockBody::cylinder(2.0, 5.0));
    let points = vec![
        CutPoint::Face {
            body: b,
            face: FaceId(0),
            uv: DVec2::new(0.3, 1.0),
        },
        CutPoint::Face {
            body: b,
            face: FaceId(0),
            uv: DVec2::new(0.9, 3.5),
        },
    ];
    s.apply(Op::Cut {
        points,
        closed: false,
    })
    .unwrap();
    s.apply(connect(&[0], 5, 0, false)).unwrap();
    assert_valid(&mut s, b, "cut + connect");
    let pos = |s: &mut newtopo_core::Scene| -> Vec<DVec3> {
        let m = s.output_of(b).unwrap();
        m.vertices
            .iter()
            .filter(|v| v.keys_cut())
            .map(|v| v.position)
            .collect()
    };
    let p1 = pos(&mut s);
    s.apply(connect(&[0], 9, 0, false)).unwrap();
    assert_valid(&mut s, b, "cut + connect 9");
    let p2 = pos(&mut s);
    assert_eq!(p1.len(), 2);
    assert_eq!(p1, p2);
}

trait CutKey {
    fn keys_cut(&self) -> bool;
}
impl CutKey for newtopo_core::mesh::OutVertex {
    fn keys_cut(&self) -> bool {
        matches!(self.key, VertexKey::CutPoint { .. })
    }
}

/// §6.8: collapsing an edge with one end on a CAD edge leaves the vertex on
/// that CAD edge.
#[test]
fn collapse_keeps_cad_side() {
    let (mut s, b) = scene_with(MockBody::make_box(DVec3::new(6.0, 6.0, 2.0)));
    s.apply(grid(1, 3, 3)).unwrap();
    let m = s.output_of(b).unwrap().clone();
    let e = m
        .edges
        .iter()
        .find(|e| {
            matches!(e.kind, OutEdgeKind::Line(_))
                && matches!(m.vertices[e.a as usize].anchor, Anchor::CadEdge { .. })
                    != matches!(m.vertices[e.b as usize].anchor, Anchor::CadEdge { .. })
        })
        .unwrap();
    let (cad_v, cad_edge) = if let Anchor::CadEdge { edge, .. } = m.vertices[e.a as usize].anchor {
        (e.a, edge)
    } else {
        let Anchor::CadEdge { edge, .. } = m.vertices[e.b as usize].anchor else {
            panic!()
        };
        (e.b, edge)
    };
    let key = m.vertices[cad_v as usize].key.clone();
    s.apply(Op::Collapse {
        body: Some(b),
        edge: e.key.clone().unwrap(),
        confirmed: false,
    })
    .unwrap();
    assert_valid(&mut s, b, "collapse");
    let m = s.output_of(b).unwrap();
    let v = m
        .vertices
        .iter()
        .find(|v| v.key == key)
        .expect("CAD-side vertex kept");
    assert!(matches!(v.anchor, Anchor::CadEdge { edge, .. } if edge == cad_edge));
}

/// §6.8: collapsing an edge whose both ends are CAD-anchored needs a
/// confirmation.
#[test]
fn collapse_cad_cad_needs_confirmation() {
    let (mut s, b) = scene_with(MockBody::make_box(DVec3::new(6.0, 6.0, 2.0)));
    s.apply(grid(1, 2, 0)).unwrap();
    let m = s.output_of(b).unwrap().clone();
    let e = m
        .edges
        .iter()
        .find(|e| matches!(e.kind, OutEdgeKind::Line(_)))
        .unwrap();
    let r = s
        .apply(Op::Collapse {
            body: Some(b),
            edge: e.key.clone().unwrap(),
            confirmed: false,
        })
        .unwrap_err();
    assert!(r.needs_confirmation);
}

/// §6.9: welding two vertices of a quad strip turns one quad into a
/// triangle and the mesh still validates.
#[test]
fn weld_quad_strip() {
    let (mut s, b) = scene_with(MockBody::make_box(DVec3::new(8.0, 4.0, 2.0)));
    set_autofill(&mut s, false);
    s.apply(grid(1, 3, 1)).unwrap();
    let m = s.output_of(b).unwrap().clone();
    // Two adjacent crossings on the constant-v line.
    let xs: Vec<&newtopo_core::mesh::OutVertex> = m
        .vertices
        .iter()
        .filter(|v| matches!(v.key, VertexKey::Crossing { .. }))
        .collect();
    assert_eq!(xs.len(), 3);
    let mut xs = xs;
    xs.sort_by(|a, c| a.position.x.total_cmp(&c.position.x));
    let tris0 = m.stats().tris;
    s.apply(Op::Weld {
        body: Some(b),
        from: xs[0].key.clone(),
        to: xs[1].key.clone(),
        confirmed: false,
    })
    .unwrap();
    assert_valid(&mut s, b, "weld");
    let st = s.output_of(b).unwrap().stats();
    assert_eq!(
        st.tris,
        tris0 + 2,
        "the two quads on either side of the welded edge become triangles"
    );
}

/// §6.9: a weld that pulls a CAD vertex onto a face point needs confirmation.
#[test]
fn weld_cad_into_face_needs_confirmation() {
    let (mut s, b) = scene_with(MockBody::make_box(DVec3::new(8.0, 4.0, 2.0)));
    s.apply(grid(1, 3, 1)).unwrap();
    let m = s.output_of(b).unwrap().clone();
    let cad = m
        .vertices
        .iter()
        .find(|v| matches!(v.anchor, Anchor::CadVertex { .. }))
        .unwrap()
        .key
        .clone();
    let x = m
        .vertices
        .iter()
        .find(|v| matches!(v.key, VertexKey::Crossing { .. }))
        .unwrap()
        .key
        .clone();
    let r = s
        .apply(Op::Weld {
            body: Some(b),
            from: cad,
            to: x,
            confirmed: false,
        })
        .unwrap_err();
    assert!(r.needs_confirmation);
}

/// §6.10: Remove CAD edge only between tangent or coplanar faces, after
/// confirmation.
#[test]
fn remove_cad_edge_rules() {
    let (mut s, b) = scene_with(MockBody::make_box(DVec3::splat(2.0)));
    let r = s
        .apply(Op::RemoveCadEdge {
            body: Some(b),
            edge: EdgeId(0),
            confirmed: true,
        })
        .unwrap_err();
    assert!(r.reason.contains("tangent"), "{}", r.reason);
    let (mut s, b) = scene_with(MockBody::split_cylinder(2.0, 4.0));
    let g = edge_between(&s, b, 0, 1);
    let r = s
        .apply(Op::RemoveCadEdge {
            body: Some(b),
            edge: g,
            confirmed: false,
        })
        .unwrap_err();
    assert!(r.needs_confirmation);
    s.apply(Op::RemoveCadEdge {
        body: Some(b),
        edge: g,
        confirmed: true,
    })
    .unwrap();
    assert_valid(&mut s, b, "remove tangent CAD edge");
}

/// §6.14: loops, rings and face strips on a torus quad grid.
#[test]
fn loops_rings_strips() {
    let (mut s, b) = scene_with(MockBody::torus(4.0, 1.0));
    set_autofill(&mut s, false);
    s.apply(connect(&[0], 8, 6, false)).unwrap();
    let m = s.output_of(b).unwrap().clone();
    let adj = Adjacency::new(&m);
    let e = m
        .edges
        .iter()
        .position(|e| matches!(e.kind, OutEdgeKind::Line(k) if k.index == 0))
        .unwrap();
    let lp = adj.edge_loop(e);
    assert_eq!(lp.len(), 6, "loop around the tube");
    let ring = adj.edge_ring(e);
    assert_eq!(ring.len(), 8, "ring around the axis");
    assert_eq!(every_other(&ring).len(), 4);
    let p = adj.polys_of[e][0];
    let strip = adj.face_loop(p, e);
    assert_eq!(strip.len(), 8);
    let _ = LineKey {
        rule: RuleId(0),
        index: 0,
        sub: 0,
    };
    let _ = EdgeKey::line(
        LineKey {
            rule: RuleId(0),
            index: 0,
            sub: 0,
        },
        VertexKey::Pole(FaceId(0), 0),
        VertexKey::Pole(FaceId(0), 1),
    );
    let _ = rule_ids(&s);
}
