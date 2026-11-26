use super::*;
use newtopo_core::ops::Op;

/// The home directory and user name the fixtures put into the report: a
/// controlled test identity (see [`scrub::use_test_identity`]), so the
/// privacy assertions hold whoever runs the tests (`USER=root` and the
/// prompt's "root cause", say).
fn home_and_user() -> (String, String) {
    scrub::use_test_identity();
    (scrub::TEST_HOME.into(), scrub::TEST_USER.into())
}

fn failure() -> Failure {
    let (home, user) = home_and_user();
    let mut f = Failure::from_message(&format!(
        "index out of bounds: the len is 117 but the index is 201 ({home}/parts/{user}_tube.step)"
    ));
    f.location = Some("crates/core/src/eval/assemble.rs:368:12".into());
    f.backtrace = scrub(&format!(
        "0: newtopo_core::eval::assemble\n   at {home}/src/x.rs:1"
    ));
    f.context = Some("Connect".into());
    f
}

const STEPS_MARK: &str = "steps-marker-7f3a";
const SYSTEM_MARK: &str = "system-marker-91c2";

fn contents(model: bool) -> Contents {
    let (home, user) = home_and_user();
    let mut s = Session::default();
    s.op(
        &Op::RenameBody {
            body: newtopo_kernel::BodyId(0),
            name: STEPS_MARK.into(),
        },
        Outcome::Applied,
    );
    s.event("undo");
    Contents {
        failure: Some(failure()),
        log: clean(&format!(
            "2026-11-25 10:00:00Z INFO  newtopo: opened {home}/a.step as {user}\n"
        )),
        steps_json: s.steps_json(),
        op_count: s.op_count(),
        mesh_edits: s.mesh_edit_count(),
        system: format!("OS: TestOS\nCPU: {SYSTEM_MARK}\n"),
        system_summary: "TestOS · TestGPU · 16 GB".into(),
        model: if model {
            vec![ModelFile {
                name: "model/tube.step".into(),
                size: 5,
                data: Data::Bytes(b"STEP;".to_vec().into()),
            }]
        } else {
            vec![]
        },
    }
}

fn no_private_data(text: &str) {
    let (home, user) = home_and_user();
    let text = text.to_lowercase();
    // The parts of the test identity, each distinctive on its own.
    for mark in [home.as_str(), "zqtester-home-5c7a", user.as_str()] {
        assert!(!text.contains(mark), "{mark:?} in:\n{text}");
    }
}

fn names(parts: &[Part]) -> Vec<String> {
    parts.iter().map(|p| p.name.clone()).collect()
}

#[test]
fn report_files_follow_the_toggles() {
    let c = contents(true);
    let mut ch = Choices::default();
    assert!(
        ch.steps && ch.pc && !ch.model,
        "defaults: steps and PC on, model off"
    );
    assert_eq!(
        names(&parts(&c, &ch)),
        ["error.txt", "log.txt", "steps.json", "system.txt"]
    );
    let off: Vec<String> = rows(&c, &ch)
        .into_iter()
        .filter(|r| !r.included)
        .map(|r| r.name)
        .collect();
    assert_eq!(
        off,
        ["model/tube.step"],
        "the model is listed as not included"
    );

    ch.steps = false;
    ch.pc = false;
    ch.comment = "  it broke  ".into();
    assert_eq!(
        names(&parts(&c, &ch)),
        ["error.txt", "log.txt", "comment.txt"]
    );
    ch.model = true;
    assert_eq!(
        names(&parts(&c, &ch)),
        ["error.txt", "log.txt", "comment.txt", "model/tube.step"]
    );
    // Every part is scrubbed text, except model files which are as-is.
    for p in parts(&c, &ch) {
        if let Data::Bytes(b) = &p.data
            && !p.name.starts_with("model/")
        {
            no_private_data(&String::from_utf8_lossy(b));
        }
        assert_eq!(
            p.size,
            match &p.data {
                Data::Bytes(b) => b.len() as u64,
                Data::File(_) => p.size,
            }
        );
    }
}

#[test]
fn zip_holds_exactly_the_listed_files() {
    let dir = tempfile::tempdir().unwrap();
    let c = contents(true);
    for model in [false, true] {
        let ch = Choices {
            model,
            ..Default::default()
        };
        let path = dir.path().join(format!("r-{model}.zip"));
        let ps = parts(&c, &ch);
        write_zip(&path, &ps).unwrap();
        let mut z = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        let mut inside: Vec<String> = (0..z.len())
            .map(|i| z.by_index(i).unwrap().name().unwrap().to_string())
            .collect();
        inside.sort();
        let mut listed: Vec<String> = rows(&c, &ch)
            .into_iter()
            .filter(|r| r.included)
            .map(|r| r.name)
            .collect();
        listed.sort();
        assert_eq!(inside, listed);
        assert_eq!(inside.iter().any(|n| n.starts_with("model/")), model);
        let mut steps = String::new();
        std::io::Read::read_to_string(&mut z.by_name("steps.json").unwrap(), &mut steps).unwrap();
        assert!(steps.contains(STEPS_MARK));
    }
    let left: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".part"))
        .collect();
    assert!(left.is_empty(), "temporary files left: {left:?}");
}

#[test]
fn listing_and_collecting_never_copy_the_model() {
    // The dialog lists the files every frame; a scene archive can be
    // hundreds of MB, so its bytes must be shared, not copied.
    let mut c = contents(false);
    let bytes: std::sync::Arc<[u8]> = vec![7u8; 1 << 20].into();
    c.model.push(ModelFile {
        name: "model/scene.ntopo".into(),
        size: bytes.len() as u64,
        data: Data::Bytes(bytes.clone()),
    });
    let ch = Choices {
        model: true,
        ..Default::default()
    };
    let ps = parts(&c, &ch);
    let p = ps.iter().find(|p| p.name == "model/scene.ntopo").unwrap();
    match &p.data {
        Data::Bytes(b) => assert!(std::sync::Arc::ptr_eq(b, &bytes), "copied"),
        Data::File(_) => panic!("bytes expected"),
    }
    let r = rows(&c, &ch);
    let row = r.iter().find(|r| r.name == "model/scene.ntopo").unwrap();
    assert_eq!(row.size, Some(1 << 20));
}

#[cfg(unix)]
#[test]
fn saving_never_writes_through_a_planted_link() {
    let dir = tempfile::tempdir().unwrap();
    let victim = dir.path().join("victim.txt");
    std::fs::write(&victim, "keep me").unwrap();
    let path = dir.path().join("report.zip");
    // The old fixed temporary name, and the final name, both planted.
    std::os::unix::fs::symlink(&victim, dir.path().join("report.zip.part")).unwrap();
    std::os::unix::fs::symlink(&victim, &path).unwrap();
    write_zip(&path, &parts(&contents(false), &Choices::default())).unwrap();
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep me");
    // The zip replaced the link at the chosen name.
    assert!(!std::fs::symlink_metadata(&path).unwrap().is_symlink());
    assert!(zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).is_ok());
}

#[test]
fn private_temp_dirs_are_fresh_and_private() {
    let a = private_temp_dir().unwrap();
    let b = private_temp_dir().unwrap();
    assert_ne!(a.path(), b.path());
    assert!(
        std::fs::read_dir(a.path()).unwrap().next().is_none(),
        "empty"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(a.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "readable by others: {mode:o}");
    }
    let p = a.path().to_path_buf();
    drop(a);
    assert!(!p.exists(), "removed when dropped");
}

#[test]
fn missing_model_files_are_noted_not_fatal() {
    let dir = tempfile::tempdir().unwrap();
    let ps = vec![
        Part::text("error.txt", "e".into()),
        Part {
            name: "model/gone.step".into(),
            size: 9,
            data: Data::File(dir.path().join("gone.step")),
        },
    ];
    let path = dir.path().join("r.zip");
    let not_saved = write_zip(&path, &ps).unwrap();
    assert_eq!(not_saved, ["model/gone.step"], "the caller is told");
    {
        let mut z = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        assert!(z.by_name("missing.txt").is_ok());
        assert!(z.by_name("model/gone.step").is_err());
        assert!(z.by_name("error.txt").is_ok(), "the rest is saved");
        // Closed here: Windows does not replace a file that is open.
    }
    // Nothing missing: nothing reported.
    let ok = write_zip(&path, &ps[..1]).unwrap();
    assert!(ok.is_empty());
}

#[test]
fn steps_json_describes_the_open_scene() {
    let connect = |a: u32| Op::Connect {
        patches: newtopo_core::ops::PatchSel {
            faces: vec![1],
            ..Default::default()
        },
        a,
        b: 0,
        start_on_seam: false,
    };
    let mut s = Session::default();
    s.import(Path::new("/w/a.step"));
    s.op(&connect(4), Outcome::Applied);
    s.new_scene();
    s.import(Path::new("/w/b.step"));
    s.op(&connect(8), Outcome::Applied);
    let json = s.steps_json();
    let script: newtopo_core::Script = serde_json::from_str(&json).unwrap();
    assert_eq!(script.input.as_deref(), Some("b.step"));
    assert_eq!(script.ops, [connect(8)], "only B's ops");
    assert_eq!(s.op_count(), 1);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let steps: Vec<&str> = v["session"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["step"].as_str().unwrap())
        .collect();
    assert_eq!(steps, ["import", "op", "new_scene", "import", "op"]);
    assert_eq!(
        v["session"][1]["op"]["a"], 4,
        "A's op stays in the timeline"
    );
    assert_eq!(v["inputs"], serde_json::json!(["a.step", "b.step"]));
    // A second import into the same scene does not change its input.
    s.import(Path::new("/w/c.step"));
    let script: newtopo_core::Script = serde_json::from_str(&s.steps_json()).unwrap();
    assert_eq!(script.input.as_deref(), Some("b.step"));
    // Opening a scene file: that file is the input, ops start over.
    s.open(Path::new("/w/saved.ntopo"));
    s.op(&connect(2), Outcome::Applied);
    let script: newtopo_core::Script = serde_json::from_str(&s.steps_json()).unwrap();
    assert_eq!(script.input.as_deref(), Some("saved.ntopo"));
    assert_eq!(script.ops, [connect(2)]);
    // File > New before any import: no input until one is imported.
    s.new_scene();
    let v: serde_json::Value = serde_json::from_str(&s.steps_json()).unwrap();
    assert!(v["input"].is_null() && v["ops"].as_array().unwrap().is_empty());
}

#[test]
fn a_full_replay_list_keeps_the_timeline_going() {
    let mut s = Session::default();
    for _ in 0..MAX_OPS {
        s.op(&Op::PurgeDormant, Outcome::Applied);
    }
    let rename = Op::RenameBody {
        body: newtopo_kernel::BodyId(0),
        name: "after the cap".into(),
    };
    s.op(&rename, Outcome::Applied);
    s.event("undo");
    assert_eq!(s.op_count(), MAX_OPS, "the replay list stops");
    let v: serde_json::Value = serde_json::from_str(&s.steps_json()).unwrap();
    let session = v["session"].as_array().unwrap();
    assert_eq!(session.len(), MAX_OPS + 2);
    assert_eq!(session[MAX_OPS]["op"]["name"], "after the cap");
    assert_eq!(session[MAX_OPS + 1]["step"], "undo");
    assert_eq!(v["truncated"], true);
}

#[test]
fn archive_names_extract_on_every_os() {
    // Names Windows (the strictest) refuses, and what they become.
    for (name, safe) in [
        ("tube.step", "tube.step"),
        ("<user>_tube.step", "[user]_tube.step"),
        ("a:b|c?d*e\"f.step", "a_b_c_d_e_f.step"),
        ("tab\there.step", "tab_here.step"),
        ("part. ", "part"),
        ("CON", "_CON"),
        ("nul.step", "_nul.step"),
        ("com1.ntopo", "_com1.ntopo"),
        ("console.step", "console.step"),
        ("", "file"),
    ] {
        assert_eq!(safe_file_name(name), safe, "{name:?}");
    }
    let windows_ok = |n: &str| {
        !n.chars()
            .any(|c| "<>:\"\\|?*".contains(c) || c.is_control())
            && !n.ends_with(['.', ' '])
    };
    // A user name that is in a model file's name.
    let (_, user) = home_and_user();
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join(format!("{user}_tube.step"));
    let b = dir
        .path()
        .join(format!("{}_TUBE.step", user.to_uppercase()));
    std::fs::write(&a, "STEP;").unwrap();
    std::fs::write(&b, "STEP;").unwrap();
    let mut model = ui::source_files(&[a, b]);
    model.push(ModelFile {
        name: ui::model_entry(&model, &format!("{}.ntopo", scrub(&user))),
        size: 1,
        data: Data::Bytes(b"x".to_vec().into()),
    });
    let names: Vec<&str> = model.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "model/[user]_tube.step",
            "model/2-[user]_TUBE.step",
            "model/[user].ntopo"
        ],
        "safe, readable, and unique ignoring case"
    );
    let c = Contents {
        model,
        ..contents(false)
    };
    let ch = Choices {
        model: true,
        ..Default::default()
    };
    let path = dir.path().join(zip_name(now_secs()));
    write_zip(&path, &parts(&c, &ch)).unwrap();
    let mut z = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
    for i in 0..z.len() {
        let e = z.by_index(i).unwrap();
        let name = e.name().unwrap().to_string();
        for seg in name.split('/') {
            assert!(windows_ok(seg), "{name:?}");
        }
        no_private_data(&name);
    }
    assert!(windows_ok(&zip_name(now_secs())), "the zip's own name");
    // Text keeps the readable placeholder.
    assert!(error_text(c.failure.as_ref()).contains("<user>_tube.step"));
}

#[test]
fn prompt_checks_out_the_build_that_failed() {
    // A crash of an older build that did not record its commit, reported
    // from a newer one.
    let mut c = contents(false);
    let f = c.failure.as_mut().unwrap();
    f.version = "0.0.9".into();
    f.commit = None;
    let p = agent_prompt(&c, &Choices::default(), "r.zip");
    assert!(p.contains("the tag `v0.0.9` (newtopo 0.0.9"), "{p}");
    if version() != "0.0.9" {
        assert!(!p.contains(&format!("v{}", version())), "{p}");
    }
    if let Some(sha) = commit() {
        assert!(!p.contains(sha), "not this build's commit: {p}");
    }
    // With its commit: that commit, and its version.
    c.failure.as_mut().unwrap().commit = Some("abc123def".into());
    let p = agent_prompt(&c, &Choices::default(), "r.zip");
    assert!(p.contains("commit `abc123def` (newtopo 0.0.9)"), "{p}");
    // No failure attached: the running build.
    c.failure = None;
    let p = agent_prompt(&c, &Choices::default(), "r.zip");
    match commit() {
        Some(sha) => assert!(p.contains(&format!("commit `{sha}` (newtopo {})", version()))),
        None => assert!(p.contains(&format!("the tag `v{}`", version()))),
    }
}

#[test]
fn steps_json_is_an_op_script() {
    let c = contents(false);
    let script: newtopo_core::Script = serde_json::from_str(&c.steps_json).unwrap();
    assert_eq!(script.version, 1);
    assert_eq!(script.ops.len(), 1);
    let v: serde_json::Value = serde_json::from_str(&c.steps_json).unwrap();
    assert_eq!(v["session"][1]["step"], "undo");
    // Refused ops are in the timeline only.
    let mut s = Session::default();
    s.op(&Op::PurgeDormant, Outcome::Refused);
    s.op(&Op::PurgeDormant, Outcome::Failed);
    assert_eq!(s.op_count(), 1);
    s.import(std::path::Path::new("/some/where/bracket.step"));
    let v: serde_json::Value = serde_json::from_str(&s.steps_json()).unwrap();
    assert_eq!(v["input"], "bracket.step");
    assert_eq!(v["session"][0]["outcome"], "refused");
    assert!(!s.steps_json().contains("/some/where"), "no folders");
}

#[test]
fn steps_json_replays_for_any_user_name() {
    use newtopo_core::doc::Axis;
    let ops = [
        Op::Connect {
            patches: newtopo_core::ops::PatchSel {
                faces: vec![1, 2],
                ..Default::default()
            },
            a: 4,
            b: 0,
            start_on_seam: false,
        },
        Op::SetSymmetry {
            on: true,
            axes: vec![Axis::X],
        },
        Op::SetUnits { units: "mm".into() },
        Op::RenameBody {
            body: newtopo_kernel::BodyId(0),
            name: "pat's part".into(),
        },
    ];
    let mut s = Session::default();
    for op in &ops {
        s.op(op, Outcome::Applied);
    }
    // User names that match field names, op names and enum values.
    for user in ["pat", "op", "ops", "connect", "version", "X", "mm", "a"] {
        let sc = scrub::Scrubber::new(Some(&format!("/home/{user}")), Some(user));
        let json = s.steps_json_with(&sc);
        let script: newtopo_core::Script =
            serde_json::from_str(&json).unwrap_or_else(|e| panic!("user {user:?}: {e}\n{json}"));
        assert_eq!(script.ops[..3], ops[..3], "user {user:?}");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["session"][0]["step"], "op", "user {user:?}");
        assert_eq!(v["session"][0]["op"]["op"], "connect", "user {user:?}");
    }
    // What the user typed is still scrubbed.
    let sc = scrub::Scrubber::new(Some("/home/pat"), Some("pat"));
    let script: newtopo_core::Script = serde_json::from_str(&s.steps_json_with(&sc)).unwrap();
    match &script.ops[3] {
        Op::RenameBody { name, .. } => assert_eq!(name, "<user>'s part"),
        o => panic!("{o:?}"),
    }
    assert!(!s.steps_json_with(&sc).contains("pat's"));
}

#[test]
fn capture_runs_never_touch_the_session_files() {
    use crate::launch::parse_args;
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    // A crash waiting from the last session (a process that has ended).
    let gone = crash::Owner {
        pid: u32::MAX - 3,
        start: 1,
    };
    let (prev, none) = crash::begin_session_as(d, gone, &|_: &crash::Owner| false);
    assert!(none.is_none());
    prev.write_crash(&Failure::from_message("waiting crash"));
    for args in [
        vec!["--window-shot", "/tmp/x.png"],
        vec!["--shot", "/tmp/shots"],
    ] {
        let args: Vec<String> = args.into_iter().map(String::from).collect();
        let st = start(&parse_args(&args), Some(d));
        assert!(st.data.is_none(), "{args:?}: no log or crash file");
        assert!(st.session.is_none(), "{args:?}: no session marker");
        assert!(st.previous_crash.is_none(), "{args:?}");
        assert!(prev.crash_file().exists(), "{args:?} consumed the report");
    }
    // The next interactive start offers it, and the session is marked as
    // running right away, before any window or graphics start-up.
    let st = start(&parse_args(&[]), Some(d));
    assert_eq!(st.data.as_deref(), Some(d));
    let found = st.previous_crash.unwrap();
    assert_eq!(found.failure.message, "waiting crash");
    let session = st.session.expect("marked");
    assert!(session.marker().exists());
    assert!(prev.crash_file().exists(), "kept until offered");
    found.offered();
    assert!(!prev.crash_file().exists(), "consumed");
    // So a panic during start-up (before the app exists) is offered next
    // time (this test's process stands in for the one that crashed).
    session.write_crash(&Failure::from_message("startup panic"));
    let st = start(&parse_args(&[]), Some(d));
    assert_eq!(st.previous_crash.unwrap().failure.message, "startup panic");
}

#[test]
fn home_and_user_never_reach_the_report() {
    let c = contents(false);
    let ch = Choices {
        comment: "it crashed".into(),
        ..Default::default()
    };
    for p in parts(&c, &ch) {
        if let Data::Bytes(b) = &p.data {
            no_private_data(&String::from_utf8_lossy(b));
        }
    }
    no_private_data(&issue_url(&c, &ch, "newtopo-report-2026-11-25.zip"));
    no_private_data(&agent_prompt(&c, &ch, "newtopo-report-2026-11-25.zip"));
    let e = error_text(c.failure.as_ref());
    assert!(
        e.contains("(…/<user>_tube.step)") || e.contains("(…/"),
        "{e}"
    );
    assert!(!e.contains("parts/"), "folders are removed: {e}");
}

#[test]
fn issue_url_is_capped_and_never_has_the_log() {
    let mut c = contents(false);
    c.log = "LOGLINE ".repeat(10_000);
    if let Some(f) = c.failure.as_mut() {
        f.message = "x".repeat(5000);
    }
    let ch = Choices {
        comment: "a long story ".repeat(2000),
        ..Default::default()
    };
    let url = issue_url(&c, &ch, "newtopo-report-2026-11-25.zip");
    assert!(url.len() <= MAX_ISSUE_URL, "{} bytes", url.len());
    assert!(url.starts_with(&format!("{ISSUE_URL}?title=")));
    assert!(!url.contains("LOGLINE"));
    assert!(url.contains("newtopo-report-2026-11-25.zip"));
    assert!(
        url.contains(&encode("a long story")),
        "the comment is kept, shortened"
    );
    // A short report keeps everything.
    let c = contents(false);
    let ch = Choices {
        comment: "Clicked Connect on the tube".into(),
        ..Default::default()
    };
    let url = issue_url(&c, &ch, "r.zip");
    assert!(url.contains(&encode("Clicked Connect on the tube")));
    assert!(url.contains(&encode("TestOS · TestGPU · 16 GB")));
    assert!(url.contains(&encode("Connect failed: index out of bounds")));
    // PC details off: no platform in the issue either.
    let ch = Choices { pc: false, ..ch };
    let url = issue_url(&c, &ch, "r.zip");
    assert!(!url.contains(&encode("TestOS")));
    assert!(url.contains(&encode("not shared")));
}

#[test]
fn prompt_has_only_what_is_switched_on() {
    let c = contents(true);
    let all = Choices {
        steps: true,
        model: true,
        pc: true,
        comment: "after the second slice".into(),
    };
    let p = agent_prompt(&c, &all, "newtopo-report-2026-11-25.zip");
    for want in [
        "Fork `SilasVogt/newtopo`",
        "newtopo-report-2026-11-25.zip",
        "index out of bounds",
        STEPS_MARK,
        SYSTEM_MARK,
        "model/",
        "after the second slice",
        "cargo fmt --all --check",
        "cargo clippy --workspace --all-targets -- -D warnings",
        "cargo test --workspace",
        "pull request against `main`",
        "regression test",
    ] {
        assert!(p.contains(want), "missing {want:?} in:\n{p}");
    }
    let none = Choices {
        steps: false,
        model: false,
        pc: false,
        comment: String::new(),
    };
    let p = agent_prompt(&c, &none, "r.zip");
    for gone in [
        STEPS_MARK,
        SYSTEM_MARK,
        "steps.json",
        "system.txt",
        "model/",
        "TestOS",
    ] {
        assert!(!p.contains(gone), "{gone:?} is switched off but in:\n{p}");
    }
    assert!(
        !p.contains("2026-11-25 10:00:00Z"),
        "the log is never pasted"
    );
    no_private_data(&p);
}

#[test]
fn big_steps_are_referenced_not_pasted() {
    let mut c = contents(false);
    c.steps_json = format!("{{\"ops\": \"{}\"}}", "y".repeat(PROMPT_STEPS_MAX + 10));
    let p = agent_prompt(&c, &Choices::default(), "r.zip");
    assert!(!p.contains(&"y".repeat(100)));
    assert!(p.contains("too large to paste"));
}

#[test]
fn dates_sizes_and_labels() {
    assert_eq!(utc_stamp(0), "1970-01-01 00:00:00Z");
    assert_eq!(utc_stamp(1_700_000_000), "2023-11-14 22:13:20Z");
    assert_eq!(utc_date(951_868_800), "2000-03-01");
    assert_eq!(zip_name(1_700_000_000), "newtopo-report-2023-11-14.zip");
    assert_eq!(human_size(512), "512 B");
    assert_eq!(human_size(31 * 1024 + 1), "32 KB");
    assert_eq!(human_size(1_258_291), "1.2 MB");
    assert_eq!(op_label("connect"), "Connect");
    assert_eq!(op_label("remove_cad_edge"), "Remove CAD edge");
    assert_eq!(op_label("grid"), "Surface Grid");
    assert_eq!(op_name(&Op::PurgeDormant), "purge_dormant");
    assert_eq!(encode("a b/ü"), "a%20b%2F%C3%BC");
}
