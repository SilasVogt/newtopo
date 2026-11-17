//! P0 exit criterion (SPEC §12): `newtopo-cli info` prints stats for every
//! reference model. Needs the `newtopo-import` helper next to the CLI binary
//! (`cargo build --workspace` or `cargo test --workspace` builds it); the
//! test is skipped with a message when it is missing.
#![cfg(feature = "occt")]

use std::process::Command;

fn helper_present() -> bool {
    let cli = std::path::Path::new(env!("CARGO_BIN_EXE_newtopo-cli"));
    let name = format!("newtopo-import{}", std::env::consts::EXE_SUFFIX);
    cli.with_file_name(name).is_file()
}

#[test]
fn info_prints_stats_for_every_reference_model() {
    if !helper_present() {
        eprintln!(
            "SKIPPED: newtopo-import not built next to newtopo-cli (run `cargo build -p newtopo-import`)"
        );
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    for path in newtopo_testmodels::write_all(dir.path()).unwrap() {
        let out = Command::new(env!("CARGO_BIN_EXE_newtopo-cli"))
            .arg("info")
            .arg(&path)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{}: {}",
            path.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        for key in [
            "bodies:",
            "faces:",
            "edges:",
            "layers:",
            "colours:",
            "link sets: n/a",
            "symmetry: n/a",
        ] {
            assert!(
                text.contains(key),
                "{}: missing {key:?} in\n{text}",
                path.display()
            );
        }

        let out = Command::new(env!("CARGO_BIN_EXE_newtopo-cli"))
            .args(["info", "--json"])
            .arg(&path)
            .output()
            .unwrap();
        assert!(out.status.success());
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let bodies = v["bodies"].as_array().unwrap();
        assert!(!bodies.is_empty());
        assert!(v["faces"].as_u64().unwrap() > 0);
        assert!(v["link_sets"].is_null());
        if path.file_stem().unwrap() == "assembly" {
            assert_eq!(bodies.len(), 5);
            assert_eq!(v["layers"]["Hardware"], 2);
            assert_eq!(v["layers"]["Structure"], 3);
        }
        if path.file_stem().unwrap() == "sphere" {
            assert_eq!(v["edges"]["degenerate"], 2);
            assert_eq!(v["edges"]["seam"], 1);
        }
    }
}

#[test]
fn info_reports_import_failure() {
    if !helper_present() {
        eprintln!("SKIPPED: newtopo-import not built next to newtopo-cli");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("bad.step");
    std::fs::write(&p, "ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\n#1=GARBAGE(;\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_newtopo-cli"))
        .arg("info")
        .arg(&p)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("import failed"));
}
