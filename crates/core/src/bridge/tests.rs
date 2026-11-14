//! Bridge client tests against a tiny loopback server.

use super::payload::{Layout, read_f32, read_u32};
use super::*;
use crate::export::samples::SampleScene;
use std::net::TcpListener;
use std::thread;

/// Minimal host: serves `n` connections; checks the token, answers hello
/// and push (renaming a duplicate the way Maya would).
fn serve(n: usize, token: &'static str) -> (u16, thread::JoinHandle<Vec<(Layout, Vec<u8>)>>) {
    let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = l.local_addr().unwrap().port();
    let h = thread::spawn(move || {
        let mut got = Vec::new();
        for _ in 0..n {
            let (mut s, peer) = l.accept().unwrap();
            assert!(peer.ip().is_loopback());
            let (h, payload) = codec::read_frame(&mut s).unwrap();
            if h.token != token {
                let mut e = Header::new(MsgType::Error, "");
                e.info = Some(serde_json::json!({"code": "bad_token", "message": "bad token"}));
                codec::write_frame(&mut s, &e, &[]).unwrap();
                continue;
            }
            match h.kind {
                MsgType::Hello => {
                    let mut r = Header::new(MsgType::Hello, "");
                    r.info =
                        Some(serde_json::json!({"host": "test 1.0", "addon_version": "0.1.0"}));
                    codec::write_frame(&mut s, &r, &[]).unwrap();
                }
                MsgType::Push => {
                    let layout: Layout = serde_json::from_value(h.layout.unwrap()).unwrap();
                    let mut seen = std::collections::HashSet::new();
                    let objects: Vec<serde_json::Value> = layout
                        .objects
                        .iter()
                        .map(|o| {
                            let mut actual = o.name.clone();
                            while !seen.insert(actual.clone()) {
                                actual.push('1');
                            }
                            serde_json::json!({"name": o.name, "actual": actual})
                        })
                        .collect();
                    let mut r = Header::new(MsgType::Ack, "");
                    r.info = Some(
                        serde_json::json!({"objects": objects, "elapsed_ms": 1.5, "host": "test"}),
                    );
                    codec::write_frame(&mut s, &r, &[]).unwrap();
                    got.push((layout, payload));
                }
                _ => panic!("unexpected"),
            }
        }
        got
    });
    (port, h)
}

#[test]
fn token_is_random_hex() {
    let (a, b) = (generate_token(), generate_token());
    assert_eq!(a.len(), 32);
    assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(a, b);
}

#[test]
fn push_round_trip_over_loopback() {
    let scene = SampleScene::new();
    let mut objs: Vec<BridgeObject> = scene
        .objects()
        .into_iter()
        .map(BridgeObject::from)
        .collect();
    // Per-corner UVs on the cube; a duplicate name to see the host rename it.
    let uvs: Vec<[f32; 2]> = (0..24).map(|i| [i as f32 / 24.0, 0.5]).collect();
    objs[0].uvs = Some(&uvs);
    let mut dup = scene.objects().remove(2);
    dup.name = "Tube";
    objs.push(dup.into());

    let (port, server) = serve(2, "secret");
    assert_eq!(
        probe(port, "secret"),
        BridgeStatus::Ready {
            host: "test 1.0".into(),
            addon_version: "0.1.0".into()
        }
    );
    let ack = send_with(
        port,
        "secret",
        &objs,
        &SendOptions {
            scale: 0.001,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(ack.objects.len(), 4);
    assert_eq!(
        ack.renamed().map(|o| o.actual.as_str()).collect::<Vec<_>>(),
        ["Tube1"]
    );
    let mut got = server.join().unwrap();
    let (layout, payload) = got.remove(0);
    assert_eq!(layout.units.scale, 0.001);
    assert_eq!(layout.objects[0].name, "Cube");
    assert_eq!(layout.objects[0].layer, "Parts");
    assert_eq!(
        layout.objects[0].material.as_ref().unwrap().name,
        "ntopo_cc3333"
    );
    assert_eq!(
        layout.objects[0].transform,
        scene.objects()[0].transform.to_cols_array()
    );
    let a = &layout.objects[0].arrays;
    let pos = read_f32(&payload, &a["positions"]);
    assert_eq!(pos.len(), 24);
    assert_eq!(&pos[3..6], &[2.0, 0.0, 0.0]);
    assert_eq!(read_u32(&payload, &a["poly_sizes"]), vec![4; 6]);
    assert_eq!(read_u32(&payload, &a["poly_verts"])[..4], [0, 3, 2, 1]);
    assert_eq!(read_f32(&payload, &a["normals"])[..3], [0.0, 0.0, -1.0]);
    assert_eq!(a["hard_edges"].count, 12);
    let uv = read_f32(&payload, &a["uvs"]);
    assert_eq!(uv.len(), 48);
    assert_eq!(uv[2], 1.0 / 24.0);
    // The tube has no UVs and 24 hard rim edges.
    let t = &layout.objects[1].arrays;
    assert!(!t.contains_key("uvs"));
    assert_eq!(t["hard_edges"].count, 24);
    for spec in t.values() {
        assert_eq!(spec.offset % 4, 0);
    }
}

#[test]
fn bad_token_and_offline() {
    let (port, server) = serve(2, "right");
    assert_eq!(probe(port, "wrong"), BridgeStatus::BadToken);
    let scene = SampleScene::new();
    let objs: Vec<BridgeObject> = scene
        .objects()
        .into_iter()
        .map(BridgeObject::from)
        .collect();
    assert!(matches!(
        send(port, "wrong", &objs),
        Err(BridgeError::BadToken)
    ));
    server.join().unwrap();
    // A port with nothing listening.
    let free = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    assert_eq!(probe(free, "x"), BridgeStatus::Offline);
    assert!(matches!(
        send(free, "x", &objs),
        Err(BridgeError::NotListening(..))
    ));
}
