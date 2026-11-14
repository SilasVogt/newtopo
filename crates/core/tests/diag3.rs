mod common;
use common::*;
use glam::DVec3;
use newtopo_core::doc::{IsoDir, Spacing};
use newtopo_core::mesh::{Anchor, OutEdgeKind};
use newtopo_core::ops::PatchSel;
use newtopo_core::Op;
use newtopo_kernel::mock::MockBody;

fn show(s: &mut newtopo_core::Scene, b: newtopo_kernel::BodyId) {
    let m = s.output_of(b).unwrap().clone();
    for i in s.validate(b) {
        println!("ISSUE {:?} {} {:?} poly {:?}", i.kind, i.message, i.vertices.iter().map(|&v| (v, &m.vertices[v as usize].key, m.vertices[v as usize].position)).collect::<Vec<_>>(), i.polygon.map(|p| m.polygons[p as usize].verts.clone()));
    }
    if std::env::var("ALL").is_ok() {
        for (i, v) in m.vertices.iter().enumerate() { println!("v{i} {:?} {:.3?}", v.key, v.position); }
        for p in &m.polygons { println!("poly f{} {:?}", p.face, p.verts); }
        for e in &m.edges { println!("edge {} {} {:?}", e.a, e.b, e.kind); }
    }
}

#[test]
fn d_case() {
    let Ok(c) = std::env::var("CASE") else { return };
    match c.as_str() {
        "collapse" => {
            let (mut s, b) = scene_with(MockBody::make_box(DVec3::new(6.0, 6.0, 2.0)));
            s.apply(Op::Grid { patches: PatchSel { faces: vec![1], ..Default::default() }, u: 3, v: 3, start_on_seam: false, spacing: Spacing::ArcLength }).unwrap();
            let m = s.output_of(b).unwrap().clone();
            let e = m.edges.iter().find(|e| matches!(e.kind, OutEdgeKind::Line(_)) && matches!(m.vertices[e.a as usize].anchor, Anchor::CadEdge { .. }) != matches!(m.vertices[e.b as usize].anchor, Anchor::CadEdge { .. })).unwrap();
            println!("collapse {:?}", e.key);
            s.apply_unchecked(Op::Collapse { body: Some(b), edge: e.key.clone().unwrap(), confirmed: false }).unwrap();
            println!("dormant {:?}", s.dormant_reasons());
            show(&mut s, b);
        }
        "iso" => {
            let (mut s, b) = scene_with(MockBody::split_cylinder(2.0, 4.0));
            s.apply_unchecked(Op::Iso { body: Some(b), face: 0, dir: IsoDir::V, value: 0.5, seed: None, split: false, through: true }).unwrap();
            println!("{:?}", s.doc().rules);
            show(&mut s, b);
        }
        "slicecyl" => {
            let (mut s, b) = scene_with(MockBody::cylinder(2.0, 5.0));
            s.apply_unchecked(Op::Slice { body: Some(b), patches: None, count: 3, through: false, guide_edge: None }).unwrap();
            show(&mut s, b);
        }
        "slicesym" => {
            let (mut s, b) = scene_with(MockBody::symmetric_part());
            s.apply_unchecked(Op::Slice { body: Some(b), patches: None, count: 10, through: false, guide_edge: None }).unwrap();
            show(&mut s, b);
        }
        _ => {}
    }
}

#[test]
fn d_ring() {
    let Ok(_) = std::env::var("RING") else { return };
    let (mut s, b) = scene_with(MockBody::ring(3.0, 1.0));
    s.apply_unchecked(Op::Grid { patches: PatchSel { faces: vec![0], ..Default::default() }, u: 0, v: 3, start_on_seam: false, spacing: Spacing::ArcLength }).unwrap();
    show(&mut s, b);
}
