//! Consistency checks for every mock body: pcurves match 3D curves, wires
//! are closed and oriented, vertices sit at edge ends, tessellation covers
//! the face area, and save/load round-trips.

use glam::{DVec2, DVec3};
use newtopo_kernel::mock::{MockBody, MockKernel};
use newtopo_kernel::{BodyId, CadKernel, FaceId, UvClass};
use std::f64::consts::PI;

fn all_bodies() -> Vec<MockBody> {
    vec![
        MockBody::make_box(DVec3::new(4.0, 3.0, 2.0)),
        MockBody::cylinder(2.0, 5.0),
        MockBody::plate_with_holes(2, 2),
        MockBody::cone(2.0, 4.0),
        MockBody::sphere(3.0),
        MockBody::torus(4.0, 1.0),
        MockBody::trimmed_torus(4.0, 1.0),
        MockBody::ring(3.0, 1.0),
        MockBody::symmetric_part(),
        MockBody::split_cylinder(2.0, 4.0),
    ]
}

fn signed_area(l: &[DVec2]) -> f64 {
    let n = l.len();
    (0..n).map(|i| l[i].perp_dot(l[(i + 1) % n])).sum::<f64>() * 0.5
}

#[test]
fn pcurves_match_edges_and_vertices() {
    for body in all_bodies() {
        for (ei, ed) in body.edges.iter().enumerate() {
            let e = newtopo_kernel::EdgeId(ei as u32);
            if let Some(v0) = ed.v0 {
                assert!(
                    (body.edge_point(e, ed.t0) - body.vertices[v0.idx()]).length() < 1e-9,
                    "{} edge {ei} v0",
                    body.name
                );
            }
            if let Some(v1) = ed.v1 {
                assert!(
                    (body.edge_point(e, ed.t1) - body.vertices[v1.idx()]).length() < 1e-9,
                    "{} edge {ei} v1",
                    body.name
                );
            }
            for pc in &ed.pcurves {
                let surf = &body.faces[pc.face.idx()].surface;
                for k in 0..=10 {
                    let t = ed.t0 + (ed.t1 - ed.t0) * k as f64 / 10.0;
                    let p3 = body.edge_point(e, t);
                    let ps = surf.point(pc.pc.eval(t));
                    assert!(
                        (p3 - ps).length() < 1e-9,
                        "{} edge {ei} pcurve on face {}",
                        body.name,
                        pc.face
                    );
                }
            }
        }
    }
}

#[test]
fn wires_are_closed_and_oriented() {
    for body in all_bodies() {
        for (fi, face) in body.faces.iter().enumerate() {
            let f = FaceId(fi as u32);
            for w in &face.wires {
                let n = w.coedges.len();
                for i in 0..n {
                    let a = w.coedges[i];
                    let b = w.coedges[(i + 1) % n];
                    let ea = &body.edges[a.edge.idx()];
                    let eb = &body.edges[b.edge.idx()];
                    let end_a = body
                        .pcurve(a.edge, f, a.reversed)
                        .unwrap()
                        .eval(if a.reversed { ea.t0 } else { ea.t1 });
                    let start_b = body
                        .pcurve(b.edge, f, b.reversed)
                        .unwrap()
                        .eval(if b.reversed { eb.t1 } else { eb.t0 });
                    assert!(
                        (end_a - start_b).length() < 1e-9,
                        "{} face {fi} wire gap {end_a} {start_b}",
                        body.name
                    );
                }
            }
            let loops = body.face_loops(f);
            for (w, l) in face.wires.iter().zip(&loops) {
                let a = signed_area(l);
                let want_ccw = w.outer != face.reversed;
                assert!(a.abs() > 1e-9, "{} face {fi} degenerate wire", body.name);
                assert_eq!(
                    a > 0.0,
                    want_ccw,
                    "{} face {fi} wire orientation (outer {})",
                    body.name,
                    w.outer
                );
            }
        }
    }
}

#[test]
fn tessellation_covers_faces_and_roundtrip() {
    let k = MockKernel::new();
    for body in all_bodies() {
        let name = body.name.clone();
        let id = k.add_body(body);
        let topo = k.topology(id).unwrap();
        for f in topo.face_ids() {
            let m = k.tessellate(id, f, 0.01).unwrap();
            assert!(!m.triangles.is_empty(), "{name} face {f} empty");
            // Triangles face outward (agree with normals).
            for t in &m.triangles {
                let [a, b, c] = t.map(|i| m.positions[i as usize]);
                let nn = (b - a).cross(c - a);
                if nn.length() < 1e-12 {
                    continue;
                }
                let avg =
                    m.normals[t[0] as usize] + m.normals[t[1] as usize] + m.normals[t[2] as usize];
                assert!(nn.dot(avg) > -1e-9, "{name} face {f} flipped tri");
            }
            assert!(topo.faces[f.idx()].area > 0.0);
        }
        let bytes = k.save_brep(id).unwrap();
        let id2 = k.load_brep(&bytes).unwrap();
        assert_eq!(k.topology(id2).unwrap(), topo, "{name} roundtrip");
        k.unload(id2);
    }
}

#[test]
fn areas_are_right() {
    let k = MockKernel::new();
    let cyl = k.add_cylinder(2.0, 5.0);
    let t = k.topology(cyl).unwrap();
    assert!((t.faces[0].area - 2.0 * PI * 2.0 * 5.0).abs() / (2.0 * PI * 10.0) < 0.01);
    assert!((t.faces[1].area - PI * 4.0).abs() / (PI * 4.0) < 0.01);
    let s = k.add_sphere(3.0);
    let t = k.topology(s).unwrap();
    assert!(
        (t.faces[0].area - 4.0 * PI * 9.0).abs() / (4.0 * PI * 9.0) < 0.02,
        "{}",
        t.faces[0].area
    );
    let tor = k.add_torus(4.0, 1.0);
    let t = k.topology(tor).unwrap();
    let want = 4.0 * PI * PI * 4.0;
    assert!(
        (t.faces[0].area - want).abs() / want < 0.02,
        "{}",
        t.faces[0].area
    );
    let plate = k.add_plate_with_holes(2, 2);
    let t = k.topology(plate).unwrap();
    let want = 400.0 - 4.0 * PI * 6.25;
    assert!(
        (t.faces[1].area - want).abs() / want < 0.01,
        "{}",
        t.faces[1].area
    );
}

#[test]
fn topology_flags() {
    let k = MockKernel::new();
    let cyl = k.add_cylinder(2.0, 5.0);
    let t = k.topology(cyl).unwrap();
    assert!(t.faces[0].surface.u_closed && !t.faces[0].surface.v_closed);
    let seam = t.edges.iter().find(|e| e.seam).unwrap();
    assert_eq!(seam.faces.len(), 1);
    let rim = &t.edges[0];
    assert!(rim.closed && !rim.seam);
    assert!((rim.dihedral.unwrap() - PI / 2.0).abs() < 1e-6);
    let sc = k.add_split_cylinder(2.0, 4.0);
    let t = k.topology(sc).unwrap();
    let g0 = t
        .edges
        .iter()
        .position(|e| e.curve == newtopo_kernel::CurveKind::Line)
        .unwrap();
    assert!(t.edges[g0].dihedral.unwrap() < 1e-9);
    let tor = k.add_torus(4.0, 1.0);
    let t = k.topology(tor).unwrap();
    assert!(t.faces[0].surface.u_closed && t.faces[0].surface.v_closed);
    let cone = k.add_cone(2.0, 4.0);
    let t = k.topology(cone).unwrap();
    assert!(t.edges.iter().any(|e| e.degenerate));
}

#[test]
fn classify_and_project() {
    let k = MockKernel::new();
    let plate = k.add_plate_with_holes(1, 1);
    let top = FaceId(1);
    let c = k
        .face_classify_uv(
            plate,
            top,
            &[
                DVec2::new(5.0, 5.0),
                DVec2::new(1.0, 1.0),
                DVec2::new(40.0, 0.0),
            ],
        )
        .unwrap();
    // The top plane starts at the min corner; (5,5) is the hole centre.
    assert_eq!(c, vec![UvClass::Out, UvClass::In, UvClass::Out]);
    let cyl = k.add_cylinder(2.0, 5.0);
    let r = k
        .project_to_face(cyl, FaceId(0), &[DVec3::new(0.0, 3.0, 2.0)])
        .unwrap();
    let (_, p) = r[0].unwrap();
    assert!((p - DVec3::new(0.0, 2.0, 2.0)).length() < 1e-9);
    let t = k
        .project_to_edge(cyl, newtopo_kernel::EdgeId(0), &[DVec3::new(0.0, 5.0, 0.0)])
        .unwrap();
    assert!((t[0].unwrap() - PI / 2.0).abs() < 1e-6);
    let _ = BodyId(0);
}
