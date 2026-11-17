//! SPEC §9.3 AC: 50k faces sent over the bridge arrive in Blender in < 1 s
//! with layers, names, sharp edges and UVs intact. Headless Blender runs
//! the add-on's poll() in a loop (tests/blender/bridge_test.py).
//!
//! Opt-in: `NEWTOPO_BLENDER=1 cargo test --release -p newtopo-core --test blender_bridge -- --ignored --nocapture`.
//! The < 1 s budget is asserted in release builds (debug builds only report).

mod common;

use common::{blender_cmd, num};
use glam::{DMat4, DVec3};
use newtopo_core::bridge::{self, BridgeObject, SendOptions};
use newtopo_core::export::ExportObject;
use newtopo_core::export::samples::{SampleScene, grid};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[test]
#[ignore = "needs Blender; set NEWTOPO_BLENDER=1 and run with --ignored"]
fn fifty_k_faces_reach_blender_quickly() {
    if std::env::var("NEWTOPO_BLENDER").as_deref() != Ok("1") {
        eprintln!("skipped: NEWTOPO_BLENDER != 1");
        return;
    }
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let token = bridge::generate_token();
    let script = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/blender/bridge_test.py"
    );
    let mut child = Command::new(blender_cmd())
        .args([
            "-b",
            "--factory-startup",
            "-P",
            script,
            "--",
            &port.to_string(),
            &token,
            "2",
            "120",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("start Blender (set NEWTOPO_BLENDER_CMD)");
    let (tx, rx) = mpsc::channel::<String>();
    let stdout = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let wait_for = |prefix: &str, secs: u64| -> String {
        let end = Instant::now() + Duration::from_secs(secs);
        loop {
            let left = end.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(l) if l.starts_with(prefix) => return l,
                Ok(_) => continue,
                Err(e) => panic!("waiting for {prefix}: {e}"),
            }
        }
    };
    wait_for("NEWTOPO_READY", 60);
    assert!(matches!(
        bridge::probe(port, &token),
        bridge::BridgeStatus::Ready { .. }
    ));

    // 250 x 200 = 50,000 quads with per-corner UVs, plus the sample scene.
    let panel = grid(250, 200);
    let uvs: Vec<[f32; 2]> = panel
        .polygons
        .iter()
        .flat_map(|p| {
            p.verts.iter().map(|&v| {
                let q = panel.vertices[v as usize].position;
                [(q.x / 25.0) as f32, (q.y / 20.0) as f32]
            })
        })
        .collect();
    let scene = SampleScene::new();
    let mut objs: Vec<BridgeObject> = vec![BridgeObject {
        object: ExportObject {
            name: "Panel",
            layer: "Big",
            color: Some([0.5, 0.5, 0.5]),
            transform: DMat4::from_translation(DVec3::new(0.0, 0.0, 10.0)),
            mesh: &panel,
        },
        uvs: Some(&uvs),
    }];
    objs.extend(scene.objects().into_iter().map(BridgeObject::from));

    let t = Instant::now();
    let ack = bridge::send_with(port, &token, &objs, &SendOptions::default()).unwrap();
    let total_ms = t.elapsed().as_secs_f64() * 1e3;
    eprintln!(
        "push of 50,000 + 19 faces: {total_ms:.0} ms round trip (Blender reports {:.0} ms building)",
        ack.elapsed_ms
    );
    assert_eq!(
        ack.objects
            .iter()
            .map(|o| o.actual.as_str())
            .collect::<Vec<_>>(),
        ["Panel", "Cube", "Tube", "Hexagon"]
    );
    if !cfg!(debug_assertions) {
        assert!(total_ms < 1000.0, "50k-face push took {total_ms:.0} ms");
    }

    // Re-send the cube moved: it replaces the existing object by name.
    let moved = ExportObject {
        transform: DMat4::from_translation(DVec3::new(0.0, 7.0, 0.0)),
        ..scene.objects().remove(0)
    };
    let ack2 = bridge::send(port, &token, &[moved.into()]).unwrap();
    assert_eq!(ack2.objects[0].actual, "Cube");

    let json: serde_json::Value = serde_json::from_str(
        wait_for("NEWTOPO_JSON ", 60)
            .strip_prefix("NEWTOPO_JSON ")
            .unwrap(),
    )
    .unwrap();
    let _ = child.wait();
    eprintln!("{}", serde_json::to_string_pretty(&json).unwrap());
    assert_eq!(num(&json["handled"]), 2);
    let o = &json["objects"];
    let p = &o["Panel"];
    assert_eq!(num(&p["polygons"]), 50_000);
    assert_eq!(num(&p["vertices"]), 251 * 201);
    assert_eq!(
        num(&p["sharp_edges"]),
        panel.edges.iter().filter(|e| e.hard).count() as u64
    );
    assert_eq!(p["uv_layers"], serde_json::json!(["UVMap"]));
    assert_eq!(p["collections"], serde_json::json!(["Big"]));
    assert_eq!(p["custom_normals"], true);
    assert_eq!(p["materials"], serde_json::json!(["ntopo_808080"]));
    for (name, layer, polys, sharp) in [
        ("Cube", "Parts", 6, 12),
        ("Tube", "Parts", 12, 24),
        ("Hexagon", "Misc", 1, 6),
    ] {
        assert_eq!(o[name]["collections"], serde_json::json!([layer]), "{name}");
        assert_eq!(num(&o[name]["polygons"]), polys, "{name}");
        assert_eq!(num(&o[name]["sharp_edges"]), sharp, "{name}");
    }
    // Replaced, not duplicated: four objects, four meshes, cube moved.
    assert_eq!(o.as_object().unwrap().len(), 4);
    assert_eq!(num(&json["meshes"]), 4);
    assert_eq!(o["Cube"]["location"], serde_json::json!([0.0, 7.0, 0.0]));
    assert_eq!(
        json["collections"],
        serde_json::json!(["Big", "Misc", "Parts"])
    );
}
