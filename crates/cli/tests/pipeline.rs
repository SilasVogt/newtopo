//! End to end through the CLI on a real OCCT model: run an op script, save,
//! validate, export all three formats (SPEC §10, §8.3).
#![cfg(feature = "occt")]

use std::path::Path;
use std::process::Command;

fn cli(args: &[&std::ffi::OsStr]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_newtopo-cli"))
        .args(args)
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

fn helper_present() -> bool {
    let cli = Path::new(env!("CARGO_BIN_EXE_newtopo-cli"));
    cli.with_file_name(format!("newtopo-import{}", std::env::consts::EXE_SUFFIX))
        .is_file()
}

#[test]
fn script_save_validate_export() {
    if !helper_present() {
        eprintln!("SKIPPED: newtopo-import not built next to newtopo-cli");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let models = newtopo_testmodels::write_all(dir.path()).unwrap();
    let fbox = models
        .iter()
        .find(|p| p.file_name().unwrap() == "filleted_box.step")
        .expect("filleted box model");
    let script = dir.path().join("slice.json");
    std::fs::write(
        &script,
        r#"{ "version": 1, "input": "filleted_box.step", "ops": [ { "op": "slice", "body": 1, "count": 10 } ] }"#,
    )
    .unwrap();
    assert!(fbox.is_file());
    let scene = dir.path().join("out.ntopo");
    let (ok, text) = cli(&[
        "run".as_ref(),
        script.as_os_str(),
        "-o".as_ref(),
        scene.as_os_str(),
    ]);
    assert!(ok, "run failed: {text}");
    assert!(
        text.contains("0 n-gons"),
        "auto-fill should leave no n-gons: {text}"
    );

    let (ok, text) = cli(&["validate".as_ref(), scene.as_os_str()]);
    assert!(ok, "validate failed: {text}");
    assert!(text.contains("valid"), "{text}");

    for ext in ["obj", "fbx", "glb"] {
        let out = dir.path().join(format!("out.{ext}"));
        let (ok, text) = cli(&[
            "export".as_ref(),
            scene.as_os_str(),
            "-o".as_ref(),
            out.as_os_str(),
        ]);
        assert!(ok, "export {ext} failed: {text}");
        assert!(
            std::fs::metadata(&out).unwrap().len() > 1000,
            "{ext} too small"
        );
    }
    assert!(
        dir.path().join("out.mtl").is_file(),
        "OBJ export writes its .mtl"
    );
    let obj = std::fs::read_to_string(dir.path().join("out.obj")).unwrap();
    assert!(obj.lines().any(|l| l.starts_with("f ")), "OBJ has faces");
}

#[test]
fn refused_op_fails_the_run() {
    if !helper_present() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    newtopo_testmodels::write_all(dir.path()).unwrap();
    let script = dir.path().join("bad.json");
    // Connect on a box: every face is flat, so the op is refused.
    std::fs::write(
        &script,
        r#"{ "version": 1, "input": "box.step", "ops": [ { "op": "connect", "patches": { "faces": [0] }, "a": 4 } ] }"#,
    )
    .unwrap();
    let (ok, text) = cli(&["run".as_ref(), script.as_os_str()]);
    assert!(!ok, "a refused op must fail the run: {text}");
    assert!(text.contains("refused"), "{text}");
}
