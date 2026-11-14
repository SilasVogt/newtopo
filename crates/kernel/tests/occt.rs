//! OCCT kernel tests that need no generated models: text/binary B-rep
//! round trips, evaluation sanity and error handling. Reference-model
//! invariants live in `crates/testmodels/tests`.
#![cfg(feature = "occt")]

use glam::{DVec2, DVec3};
use newtopo_kernel::occt::OcctKernel;
use newtopo_kernel::occt::checks::{CheckOptions, check_body};
use newtopo_kernel::{BodyId, CadKernel, EdgeId, FaceId, KernelError, SurfaceKind, UvClass};

const CYL: &[u8] = include_bytes!("data/cylinder_chamfered.brep");
const SPHERE: &[u8] = include_bytes!("data/sphere.brep");

fn assert_ok(k: &dyn CadKernel, b: BodyId) {
    let errs = check_body(k, b, CheckOptions::default());
    assert!(errs.is_empty(), "invariants:\n{}", errs.join("\n"));
}

#[test]
fn text_and_binary_round_trip() {
    let k = OcctKernel::new();
    assert!(k.name().starts_with("occt "));
    let a = k.load_brep(CYL).unwrap();
    let ta = k.topology(a).unwrap();
    let bin = k.save_brep(a).unwrap();
    assert!(bin.trim_ascii_start().starts_with(b"Open CASCADE Topology"));
    let b = k.load_brep(&bin).unwrap();
    assert_ne!(a, b);
    let tb = k.topology(b).unwrap();
    assert_eq!(ta, tb, "binary round trip changed the topology");
    // Saving again gives identical bytes (deterministic writer).
    assert_eq!(k.save_brep(b).unwrap(), bin);
    assert_ok(&k, a);
    k.unload(a);
    k.unload(b);
    assert!(matches!(k.topology(a), Err(KernelError::UnknownBody(_))));
}

#[test]
fn chamfered_cylinder_topology() {
    let k = OcctKernel::new();
    let b = k.load_brep(CYL).unwrap();
    let t = k.topology(b).unwrap();
    let kinds: Vec<_> = t.faces.iter().map(|f| f.surface.kind).collect();
    assert_eq!(
        kinds
            .iter()
            .filter(|k| **k == SurfaceKind::Cylinder)
            .count(),
        1
    );
    assert_eq!(kinds.iter().filter(|k| **k == SurfaceKind::Cone).count(), 2);
    assert_eq!(
        kinds.iter().filter(|k| **k == SurfaceKind::Plane).count(),
        2
    );
    let cyl = t
        .faces
        .iter()
        .find(|f| f.surface.kind == SurfaceKind::Cylinder)
        .unwrap();
    assert!(cyl.surface.u_closed && !cyl.surface.v_closed);
    assert!((cyl.surface.u_period.unwrap() - std::f64::consts::TAU).abs() < 1e-12);
    assert!((cyl.surface.params[0] - 15.0).abs() < 1e-9);
    assert!(cyl.surface.axis_dir.unwrap().abs().z > 0.999);
    // Chamfer edges: dihedral 45° between the cone and its neighbours.
    let sharp: Vec<f64> = t
        .edges
        .iter()
        .filter_map(|e| e.dihedral)
        .filter(|d| *d > 0.1)
        .collect();
    assert!(!sharp.is_empty());
    for d in sharp {
        assert!(
            (d.to_degrees() - 45.0).abs() < 0.5,
            "dihedral {}",
            d.to_degrees()
        );
    }
    assert!(t.edges.iter().filter(|e| e.seam).count() >= 1);
    k.unload(b);
}

#[test]
fn sphere_poles_and_seam() {
    let k = OcctKernel::new();
    let b = k.load_brep(SPHERE).unwrap();
    let t = k.topology(b).unwrap();
    assert_eq!(t.faces.len(), 1);
    assert_eq!(t.edges.iter().filter(|e| e.degenerate).count(), 2);
    assert_eq!(t.edges.iter().filter(|e| e.seam).count(), 1);
    let f = &t.faces[0];
    assert_eq!(f.surface.kind, SurfaceKind::Sphere);
    assert!(f.surface.u_closed);
    // Normals point outward, also at the poles (limit off the singularity).
    let (v0, v1) = f.surface.v_range;
    let uv = [
        DVec2::new(0.3, 0.0),
        DVec2::new(0.0, v1),
        DVec2::new(1.0, v0),
    ];
    let ev = k.face_eval(b, FaceId(0), &uv).unwrap();
    for e in &ev {
        assert!((e.n.length() - 1.0).abs() < 1e-9);
        assert!(
            e.n.dot(e.p.normalize()) > 0.999,
            "normal {:?} at {:?}",
            e.n,
            e.p
        );
    }
    assert_ok(&k, b);
    k.unload(b);
}

#[test]
fn classify_and_project() {
    let k = OcctKernel::new();
    let b = k.load_brep(CYL).unwrap();
    let t = k.topology(b).unwrap();
    let (fi, f) = t
        .faces
        .iter()
        .enumerate()
        .find(|(_, f)| f.surface.kind == SurfaceKind::Cylinder)
        .unwrap();
    let face = FaceId(fi as u32);
    let (u0, u1) = f.surface.u_range;
    let (v0, v1) = f.surface.v_range;
    let mid = DVec2::new(0.5 * (u0 + u1), 0.5 * (v0 + v1));
    let cls = k
        .face_classify_uv(
            b,
            face,
            &[mid, DVec2::new(mid.x, v1 + 10.0), DVec2::new(mid.x, v0)],
        )
        .unwrap();
    assert_eq!(cls[0], UvClass::In);
    assert_eq!(cls[1], UvClass::Out);
    assert_ne!(cls[2], UvClass::Out);
    // A point outside the radius projects radially onto the cylinder.
    let p = k.face_eval(b, face, &[mid]).unwrap()[0];
    let r = k.project_to_face(b, face, &[p.p + p.n * 5.0]).unwrap()[0].unwrap();
    assert!(r.1.distance(p.p) < 1e-7);
    // Beyond the trimmed height the nearest point is on the boundary edge.
    let top = DVec3::new(p.p.x, p.p.y, 1000.0);
    let r = k.project_to_face(b, face, &[top]).unwrap()[0].unwrap();
    assert!((r.1.z - 38.0).abs() < 1e-6, "{:?}", r.1);
    k.unload(b);
}

#[test]
fn errors_do_not_crash() {
    let k = OcctKernel::new();
    assert!(k.load_brep(b"definitely not a brep").is_err());
    assert!(k.load_brep(b"").is_err());
    assert!(
        k.load_brep(b"Open CASCADE Topology V3 (c)\n\x00\x01\x02garbage")
            .is_err()
    );
    let b = k.load_brep(CYL).unwrap();
    assert!(matches!(
        k.face_eval(b, FaceId(999), &[DVec2::ZERO]),
        Err(KernelError::UnknownFace(_))
    ));
    assert!(matches!(
        k.edge_eval(b, EdgeId(999), &[0.0]),
        Err(KernelError::UnknownEdge(_))
    ));
    assert!(k.tessellate(b, FaceId(0), 0.0).is_err());
    // Edge that does not bound the face.
    let t = k.topology(b).unwrap();
    let e = t
        .edges
        .iter()
        .position(|e| !e.faces.contains(&FaceId(0)))
        .unwrap();
    assert!(
        k.edge_pcurve_eval(b, EdgeId(e as u32), FaceId(0), false, &[0.0])
            .is_err()
    );
    assert!(k.face_eval(b, FaceId(0), &[]).unwrap().is_empty());
    k.unload(b);
}

#[test]
fn kernel_is_shared_across_threads() {
    let k = std::sync::Arc::new(OcctKernel::new());
    let hs: Vec<_> = (0..4)
        .map(|_| {
            let k = k.clone();
            std::thread::spawn(move || {
                let b = k.load_brep(SPHERE).unwrap();
                let n = k.topology(b).unwrap().faces.len();
                k.unload(b);
                n
            })
        })
        .collect();
    for h in hs {
        assert_eq!(h.join().unwrap(), 1);
    }
}
