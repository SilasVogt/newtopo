mod common;
use common::*;
use newtopo_core::doc::Spacing;
use newtopo_core::keys::EdgeKey;
use newtopo_core::mesh::OutEdgeKind;
use newtopo_core::ops::PatchSel;
use newtopo_core::Op;
use newtopo_kernel::mock::MockBody;

#[test]
fn diag_tt_remove() {
    let Ok(af) = std::env::var("TTR") else { return };
    let autofill = af == "1";
    let (mut s, b) = scene_with(MockBody::trimmed_torus(4.0, 1.0));
    set_autofill(&mut s, autofill);
    s.apply(Op::Grid { patches: PatchSel { faces: vec![0], ..Default::default() }, u: 6, v: 6, start_on_seam: false, spacing: Spacing::ArcLength }).unwrap();
    let m = s.output_of(b).unwrap().clone();
    let keys: Vec<EdgeKey> = m.edges.iter().filter(|e| matches!(e.kind, OutEdgeKind::Line(_))).filter_map(|e| e.key.clone()).collect();
    let n = keys.len();
    for (i, k) in keys.into_iter().enumerate() {
        match s.apply(Op::RemoveEdge { body: Some(b), edge: k.clone() }) {
            Ok(_) => {}
            Err(e) => {
                println!("step {i}/{n}: refused: {}", e.reason);
                if e.reason.contains("validate") || e.reason.contains("simple") || e.reason.contains("flip") {
                    let mut doc = s.doc().clone();
                    let id = doc.fresh_id();
                    doc.edits.push(newtopo_core::doc::EditEntry { id, edit: newtopo_core::doc::Edit::RemoveEdge { body: b, edge: k.clone() } });
                    let saved = s.doc().clone();
                    s.set_document(doc);
                    for is in s.validate(b) {
                        println!("   would: {:?} {} {:?}", is.kind, is.message, is.vertices);
                    }
                    s.set_document(saved);
                }
            }
        }
    }
    let issues = s.validate(b);
    let m = s.output_of(b).unwrap().clone();
    println!("final stats {:?}", m.stats());
    for i in &issues {
        println!("ISSUE {:?} {} {:?}", i.kind, i.message, i.vertices);
    }
    for p in &m.polygons {
        println!("poly {:?}", p.verts);
    }
}

#[test]
fn diag_tt_remove_all() {
    let Ok(af) = std::env::var("TTA") else { return };
    let autofill = af == "1";
    let (mut s, b) = scene_with(MockBody::trimmed_torus(4.0, 1.0));
    set_autofill(&mut s, autofill);
    s.apply(Op::Grid { patches: PatchSel { faces: vec![0], ..Default::default() }, u: 6, v: 6, start_on_seam: false, spacing: Spacing::ArcLength }).unwrap();
    let m = s.output_of(b).unwrap().clone();
    let keys: Vec<EdgeKey> = m.edges.iter().filter(|e| matches!(e.kind, OutEdgeKind::Line(_))).filter_map(|e| e.key.clone()).collect();
    let limit: usize = std::env::var("LIMIT").ok().and_then(|x| x.parse().ok()).unwrap_or(keys.len());
    let mut doc = s.doc().clone();
    for k in keys.into_iter().take(limit) {
        let id = doc.fresh_id();
        doc.edits.push(newtopo_core::doc::EditEntry { id, edit: newtopo_core::doc::Edit::RemoveEdge { body: b, edge: k } });
    }
    s.set_document(doc);
    let issues = s.validate(b);
    let m = s.output_of(b).unwrap().clone();
    println!("final stats {:?} dormant {:?}", m.stats(), s.dormant_reasons());
    for i in &issues {
        println!("ISSUE {:?} {} {:?} {:?}", i.kind, i.message, i.vertices, i.polygon.map(|p| &m.polygons[p as usize].verts));
    }
    for (i, v) in m.vertices.iter().enumerate() {
        println!("v{i} {:?} {:.3?}", v.key, v.position);
    }
    for p in &m.polygons {
        println!("poly {:?}", p.verts);
    }
    for e in &m.edges {
        println!("edge {} {} {:?}", e.a, e.b, e.kind);
    }
}

#[test]
fn diag_tt_loop() {
    let Ok(af) = std::env::var("TTL") else { return };
    let autofill = af == "1";
    let (mut s, b) = scene_with(MockBody::trimmed_torus(4.0, 1.0));
    set_autofill(&mut s, autofill);
    s.apply(Op::Grid { patches: PatchSel { faces: vec![0], ..Default::default() }, u: 6, v: 6, start_on_seam: false, spacing: Spacing::ArcLength }).unwrap();
    for step in 0..500 {
        let m = s.output_of(b).unwrap().clone();
        let Some(k) = m.edges.iter().find(|e| matches!(e.kind, OutEdgeKind::Line(_))).and_then(|e| e.key.clone()) else { println!("done at {step}"); break };
        if let Err(e) = s.apply(Op::RemoveEdge { body: Some(b), edge: k.clone() }) {
            println!("step {step}: refused {} for {:?}", e.reason, k);
            let mut doc = s.doc().clone();
            let id = doc.fresh_id();
            doc.edits.push(newtopo_core::doc::EditEntry { id, edit: newtopo_core::doc::Edit::RemoveEdge { body: b, edge: k.clone() } });
            s.set_document(doc);
            let m = s.output_of(b).unwrap().clone();
            for is in s.validate(b) {
                println!("   would: {:?} {} {:?} {:?}", is.kind, is.message, is.vertices, is.polygon.map(|p| m.polygons[p as usize].verts.iter().map(|&v| (v, m.vertices[v as usize].position)).collect::<Vec<_>>()));
            }
            break;
        }
    }
}
