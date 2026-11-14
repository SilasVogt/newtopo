//! Every reference model (SPEC §11): STEP write → XCAF read → B-rep round
//! trip → topology and geometry invariants, plus per-model expectations.
#![cfg(feature = "occt")]

use newtopo_kernel::occt::checks::{CheckOptions, check_body};
use newtopo_kernel::occt::{CadFormat, HealSettings, LoadedFile, OcctKernel};
use newtopo_kernel::{BodyTopology, CadKernel, SurfaceKind};
use newtopo_testmodels::{MODELS, model, write_model, write_model_brep};

fn import(name: &str) -> (OcctKernel, LoadedFile) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!("{name}.step"));
    write_model(model(name).unwrap(), &path).unwrap();
    let k = OcctKernel::new();
    let f = k.import_file(&path, CadFormat::Step, 0.001).unwrap();
    (k, f)
}

fn count(t: &BodyTopology, kind: SurfaceKind) -> usize {
    t.faces.iter().filter(|f| f.surface.kind == kind).count()
}

#[test]
fn every_model_passes_invariants() {
    let mut failures = Vec::new();
    for m in MODELS {
        let (k, f) = import(m.name);
        assert!(!f.bodies.is_empty(), "{}: no bodies", m.name);
        for b in &f.bodies {
            let report = k.heal(b.body, HealSettings::default()).unwrap();
            assert!(report.failed_faces.is_empty(), "{}: failed faces {:?}", m.name, report);
            assert_eq!(report.faces_before, report.faces_after, "{}", m.name);
            // Binary round trip keeps ids and topology.
            let t = k.topology(b.body).unwrap();
            let again = k.load_brep(&k.save_brep(b.body).unwrap()).unwrap();
            assert_eq!(t, k.topology(again).unwrap(), "{}: round trip changed topology", m.name);
            k.unload(again);
            for e in check_body(&k, b.body, CheckOptions::default()) {
                failures.push(format!("{} / {}: {e}", m.name, b.attrs.name));
            }
            k.unload(b.body);
        }
    }
    assert!(failures.is_empty(), "{} violations:\n{}", failures.len(), failures.join("\n"));
}

#[test]
fn text_brep_of_every_model_loads() {
    let dir = tempfile::tempdir().unwrap();
    let k = OcctKernel::new();
    for m in MODELS {
        let p = dir.path().join(format!("{}.brep", m.name));
        write_model_brep(m, &p).unwrap();
        let b = k.load_brep(&std::fs::read(&p).unwrap()).unwrap();
        assert!(!k.topology(b).unwrap().faces.is_empty(), "{}", m.name);
        k.unload(b);
    }
}

#[test]
fn seams_and_poles() {
    let cases: &[(&str, SurfaceKind, usize, usize)] = &[
        // model, periodic face kind, seam edges, degenerate edges
        ("sphere", SurfaceKind::Sphere, 1, 2),
        ("torus", SurfaceKind::Torus, 2, 0),
        ("cone_apex", SurfaceKind::Cone, 1, 1),
        ("cylinder_chamfered", SurfaceKind::Cylinder, 3, 0),
    ];
    for &(name, kind, seams, degen) in cases {
        let (k, f) = import(name);
        let t = k.topology(f.bodies[0].body).unwrap();
        assert!(count(&t, kind) >= 1, "{name}: no {kind:?} face");
        assert_eq!(t.edges.iter().filter(|e| e.seam).count(), seams, "{name}: seams");
        assert_eq!(t.edges.iter().filter(|e| e.degenerate).count(), degen, "{name}: degenerate");
        for face in t.faces.iter().filter(|f| f.surface.kind == kind) {
            assert!(face.surface.u_closed, "{name}: periodic face not closed in u");
            if kind == SurfaceKind::Torus {
                assert!(face.surface.v_closed, "{name}: torus not closed in v");
            }
        }
        k.unload(f.bodies[0].body);
    }
}

#[test]
fn torus_with_one_hole_has_a_hole_wire() {
    let (k, f) = import("torus_hole");
    let t = k.topology(f.bodies[0].body).unwrap();
    let torus = t.faces.iter().find(|f| f.surface.kind == SurfaceKind::Torus).unwrap();
    assert_eq!(torus.wires.len(), 2, "outer seam wire + one hole");
    assert!(torus.wires[0].outer && !torus.wires[1].outer);
    // The outer wire is made only of the two seams (each used twice).
    assert!(torus.wires[0].coedges.iter().all(|c| t.edge(c.edge).seam));
    assert!(torus.wires[1].coedges.iter().all(|c| !t.edge(c.edge).seam));
}

#[test]
fn model_shapes() {
    let (k, f) = import("plate_holes");
    let t = k.topology(f.bodies[0].body).unwrap();
    assert_eq!(count(&t, SurfaceKind::Cylinder), 16);
    assert_eq!(count(&t, SurfaceKind::Plane), 6);
    let (k2, f) = import("flange");
    let t = k2.topology(f.bodies[0].body).unwrap();
    assert_eq!(count(&t, SurfaceKind::Torus), 1, "hub fillet");
    assert!(count(&t, SurfaceKind::Cylinder) >= 6 + 1 + 2);
    let (k3, f) = import("bottle");
    let t = k3.topology(f.bodies[0].body).unwrap();
    assert!(count(&t, SurfaceKind::Revolution) + count(&t, SurfaceKind::BSpline) >= 1);
    let (k4, f) = import("filleted_box");
    let t = k4.topology(f.bodies[0].body).unwrap();
    assert_eq!(t.faces.len(), 6 + 12 + 8);
    assert_eq!(t.edges.iter().filter(|e| e.dihedral.is_some_and(|d| d > 30f64.to_radians())).count(), 0);
    assert_eq!(f.bodies[0].attrs.layer.as_deref(), Some("Parts"));
    let c = f.bodies[0].attrs.color.unwrap();
    assert!((c[0] - 0.2).abs() < 0.01 && (c[2] - 0.8).abs() < 0.01, "{c:?}");
}

#[test]
fn bracket_is_x_symmetric() {
    let (k, f) = import("bracket");
    let t = k.topology(f.bodies[0].body).unwrap();
    // Every vertex of an open, non-seam edge has a mirror image under x → −x
    // (closed circles start wherever their parametrisation does).
    let real: std::collections::BTreeSet<u32> = t
        .edges
        .iter()
        .filter(|e| !e.closed && !e.seam && !e.degenerate)
        .flat_map(|e| [e.v0, e.v1])
        .flatten()
        .map(|v| v.0)
        .collect();
    for v in real.iter().map(|&i| &t.vertices[i as usize]) {
        let m = glam::DVec3::new(-v.point.x, v.point.y, v.point.z);
        assert!(t.vertices.iter().any(|w| w.point.distance(m) < 1e-6), "no mirror for {:?}", v.point);
    }
    assert_eq!(f.bodies[0].attrs.layer.as_deref(), Some("Brackets"));
}

#[test]
fn assembly_attributes() {
    let (k, f) = import("assembly");
    let names: Vec<&str> = f.bodies.iter().map(|b| b.attrs.name.as_str()).collect();
    assert_eq!(names, ["Base", "Post left", "Post right", "Knob", "Ring"]);
    let by = |n: &str| f.bodies.iter().find(|b| b.attrs.name == n).unwrap();
    assert_eq!(by("Base").attrs.layer.as_deref(), Some("Structure"));
    assert_eq!(by("Knob").attrs.layer.as_deref(), Some("Hardware"));
    assert_eq!(by("Knob").attrs.assembly_path, ["Bench", "Cap"]);
    assert_eq!(by("Base").attrs.assembly_path, ["Bench"]);
    let red = by("Knob").attrs.color.unwrap();
    assert!((red[0] - 0.85).abs() < 0.01 && red[1] < 0.15, "{red:?}");
    // Shared prototype placed twice: same geometry, different transforms.
    let l = by("Post left");
    let r = by("Post right");
    assert_eq!(k.topology(l.body).unwrap(), k.topology(r.body).unwrap());
    assert!((l.attrs.transform.w_axis.x - 10.0).abs() < 1e-9);
    assert!((r.attrs.transform.w_axis.x - 70.0).abs() < 1e-9);
    // Nested transform: Cap at (40,10,50) then Ring at z −6, rotated about X.
    let ring = by("Ring").attrs.transform;
    let o = ring.transform_point3(glam::DVec3::ZERO);
    assert!(o.distance(glam::DVec3::new(40.0, 10.0, 44.0)) < 1e-9, "{o:?}");
    let z = ring.transform_vector3(glam::DVec3::Z);
    assert!(z.distance(glam::DVec3::new(0.0, -1.0, 0.0)) < 1e-9, "{z:?}");
}

#[test]
fn units_scale_geometry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("box.step");
    write_model(model("box").unwrap(), &path).unwrap();
    let k = OcctKernel::new();
    let mm = k.import_file(&path, CadFormat::Step, 0.001).unwrap();
    let cm = k.import_file(&path, CadFormat::Step, 0.01).unwrap();
    let ext = |f: &LoadedFile| {
        let t = k.topology(f.bodies[0].body).unwrap();
        t.vertices.iter().map(|v| v.point.x).fold(f64::MIN, f64::max)
    };
    assert!((ext(&mm) - 40.0).abs() < 1e-9);
    assert!((ext(&cm) - 4.0).abs() < 1e-9);
}
