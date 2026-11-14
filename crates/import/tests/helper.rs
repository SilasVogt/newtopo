//! The import helper process end to end through `kernel::helper::run_import`
//! (SPEC §3.4, §8.1, §11 robustness): good files, malformed files, timeouts
//! and cancellation. The test process must survive all of them.

use newtopo_kernel::helper::{ImportOptions, ImportedFile, run_import};
use newtopo_kernel::occt::OcctKernel;
use newtopo_kernel::{CadKernel, SurfaceKind};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn opts() -> ImportOptions {
    ImportOptions {
        helper: Some(PathBuf::from(env!("CARGO_BIN_EXE_newtopo-import"))),
        ..Default::default()
    }
}

fn import(path: &Path) -> Result<ImportedFile, String> {
    run_import(path, &opts(), &mut |_, _| true).map_err(|e| e.0)
}

fn write_models(dir: &Path) -> Vec<PathBuf> {
    newtopo_testmodels::write_all(dir).unwrap()
}

#[test]
fn every_reference_model_imports() {
    let dir = tempfile::tempdir().unwrap();
    let k = OcctKernel::new();
    for path in write_models(dir.path()) {
        let mut steps = Vec::new();
        let f = run_import(&path, &opts(), &mut |p, m| {
            steps.push((p, m.to_string()));
            true
        })
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert!(!f.bodies.is_empty());
        assert_eq!(f.source_sha256.len(), 64);
        assert!(
            steps.iter().any(|(p, m)| *p == 1.0 && m == "done"),
            "{steps:?}"
        );
        assert!(
            steps.windows(2).all(|w| w[0].0 <= w[1].0),
            "progress not monotonic: {steps:?}"
        );
        for b in &f.bodies {
            assert!(
                b.heal.failed_faces.is_empty(),
                "{}: {:?}",
                path.display(),
                b.heal
            );
            let id = k.load_brep(&b.brep).unwrap();
            let t = k.topology(id).unwrap();
            assert_eq!(t.faces.len(), b.heal.faces_after);
            k.unload(id);
        }
    }
}

#[test]
fn assembly_sidecar_attributes() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("assembly.step");
    newtopo_testmodels::write_model(newtopo_testmodels::model("assembly").unwrap(), &p).unwrap();
    let f = import(&p).unwrap();
    assert_eq!(f.bodies.len(), 5);
    let ring = f.bodies.iter().find(|b| b.attrs.name == "Ring").unwrap();
    assert_eq!(ring.attrs.layer.as_deref(), Some("Hardware"));
    assert_eq!(ring.attrs.assembly_path, ["Bench", "Cap"]);
    assert!(ring.attrs.color.is_some());
    let o = ring.attrs.transform.transform_point3(glam::DVec3::ZERO);
    assert!(o.distance(glam::DVec3::new(40.0, 10.0, 44.0)) < 1e-9);
    let k = OcctKernel::new();
    let id = k.load_brep(&ring.brep).unwrap();
    assert_eq!(
        k.topology(id).unwrap().faces[0].surface.kind,
        SurfaceKind::Torus
    );
    k.unload(id);
}

#[test]
fn brep_input_and_units() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("box.brep");
    newtopo_testmodels::write_model_brep(newtopo_testmodels::model("box").unwrap(), &p).unwrap();
    let f = import(&p).unwrap();
    assert_eq!(f.bodies.len(), 1);
    assert_eq!(f.bodies[0].attrs.name, "box");
    let s = dir.path().join("box.step");
    newtopo_testmodels::write_model(newtopo_testmodels::model("box").unwrap(), &s).unwrap();
    let o = ImportOptions {
        units: "cm".into(),
        ..opts()
    };
    let f = run_import(&s, &o, &mut |_, _| true).unwrap();
    let k = OcctKernel::new();
    let id = k.load_brep(&f.bodies[0].brep).unwrap();
    let max_x = k
        .topology(id)
        .unwrap()
        .vertices
        .iter()
        .map(|v| v.point.x)
        .fold(f64::MIN, f64::max);
    assert!((max_x - 4.0).abs() < 1e-9, "{max_x}");
    k.unload(id);
}

#[test]
fn malformed_step_fails_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let good = dir.path().join("box.step");
    newtopo_testmodels::write_model(newtopo_testmodels::model("filleted_box").unwrap(), &good)
        .unwrap();
    let text = std::fs::read_to_string(&good).unwrap();

    // Truncated in the middle of the DATA section.
    let truncated = dir.path().join("truncated.step");
    std::fs::write(&truncated, &text[..text.len() / 3]).unwrap();
    // Valid header, nonsense entities.
    let nonsense = dir.path().join("nonsense.step");
    let head = &text[..text.find("DATA;").unwrap() + 5];
    std::fs::write(&nonsense, format!("{head}\n#1 = FOO((#2,#3),'x');\n#2 = CARTESIAN_POINT('',(1.,2.));\n#3 = #1;\nENDSEC;\nEND-ISO-10303-21;\n")).unwrap();
    // Entities referring to missing entities.
    let dangling = dir.path().join("dangling.step");
    let mut lines: Vec<&str> = text.lines().collect();
    lines.retain(|l| !l.contains("ADVANCED_FACE"));
    std::fs::write(&dangling, lines.join("\n")).unwrap();
    // Garbage bytes with a STEP extension.
    let garbage = dir.path().join("garbage.step");
    let bytes: Vec<u8> = (0..20000u32)
        .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8)
        .collect();
    std::fs::write(&garbage, bytes).unwrap();
    // Garbage IGES and an empty file.
    let iges = dir.path().join("garbage.igs");
    std::fs::write(&iges, "this is not IGES\n".repeat(100)).unwrap();
    let empty = dir.path().join("empty.step");
    std::fs::write(&empty, "").unwrap();

    for p in [&truncated, &nonsense, &garbage, &iges, &empty] {
        let e = import(p).expect_err(&format!("{} should fail", p.display()));
        assert!(e.starts_with("import failed:"), "{}: {e}", p.display());
    }
    // A STEP file missing its faces may import partially; it must not crash
    // and must report what it found.
    match import(&dangling) {
        Ok(f) => assert!(f.bodies.iter().all(|b| b.heal.faces_after <= 26)),
        Err(e) => assert!(e.starts_with("import failed:"), "{e}"),
    }
}

#[test]
fn unsupported_and_missing_files() {
    let dir = tempfile::tempdir().unwrap();
    let txt = dir.path().join("model.txt");
    std::fs::write(&txt, "hello").unwrap();
    let e = import(&txt).unwrap_err();
    assert!(e.contains("unsupported file type"), "{e}");
    let e = import(&dir.path().join("missing.step")).unwrap_err();
    assert!(e.starts_with("import failed:"), "{e}");
}

#[test]
fn timeout_and_cancel_kill_the_helper() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("flange.step");
    newtopo_testmodels::write_model(newtopo_testmodels::model("flange").unwrap(), &p).unwrap();
    let o = ImportOptions {
        timeout: Duration::from_millis(1),
        ..opts()
    };
    let e = run_import(&p, &o, &mut |_, _| true).unwrap_err();
    assert!(e.0.contains("timed out"), "{e}");
    let e = run_import(&p, &opts(), &mut |_, _| false).unwrap_err();
    assert_eq!(e.0, "import cancelled");
}
