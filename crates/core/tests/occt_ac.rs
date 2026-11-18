//! Acceptance tests on the OCCT reference models (SPEC §11): untouched
//! validation, filleted-box Slice / Connect chain / through-iso, Connect on
//! the revolved bottle, Link on the plate with holes, and a property run
//! over every model.
mod common;
use common::assert_valid;
use newtopo_core::Scene;
use newtopo_kernel::occt::{CadFormat, HealSettings, OcctKernel};
use newtopo_kernel::{BodyId, SurfaceKind};
use std::sync::Arc;

/// Import a reference model like the import helper does (STEP, healed);
/// returns the scene and its first body.
pub fn occt_scene(name: &str) -> (Scene, BodyId) {
    let (s, ids) = occt_scene_all(name);
    (s, ids[0])
}

pub fn occt_scene_all(name: &str) -> (Scene, Vec<BodyId>) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!("{name}.step"));
    newtopo_testmodels::write_model(newtopo_testmodels::model(name).unwrap(), &path).unwrap();
    let k = Arc::new(OcctKernel::new());
    let file = k.import_file(&path, CadFormat::Step, 0.001).unwrap();
    let mut s = Scene::new(k.clone());
    let mut ids = Vec::new();
    for b in file.bodies {
        let heal = k.heal(b.body, HealSettings::default()).unwrap_or_default();
        ids.push(s.add_kernel_body(b.body, b.attrs, heal, None).unwrap());
    }
    (s, ids)
}

#[test]
fn occt_models_untouched_validate() {
    for m in newtopo_testmodels::MODELS {
        let (mut s, b) = occt_scene(m.name);
        let t = s.ctx(b).unwrap().topo.clone();
        let kinds: Vec<SurfaceKind> = t.faces.iter().map(|f| f.surface.kind).collect();
        eprintln!("{}: {} faces {:?}", m.name, t.faces.len(), kinds);
        for on in [true, false] {
            s.apply(newtopo_core::Op::SetAutofill { on }).unwrap();
            assert_valid(&mut s, b, &format!("{} autofill {on}", m.name));
        }
    }
}

use newtopo_core::Op;
use newtopo_core::doc::{IsoDir, PatchId, Rule, Spacing};
use newtopo_core::mesh::{OutEdgeKind, OutputMesh};
use newtopo_core::ops::PatchSel;
use newtopo_kernel::FaceId;
use std::collections::{BTreeMap, BTreeSet, HashMap};

fn faces_of(s: &Scene, b: BodyId, kind: SurfaceKind) -> Vec<FaceId> {
    let t = &s.ctx(b).unwrap().topo;
    t.face_ids()
        .filter(|f| t.face(*f).surface.kind == kind)
        .collect()
}
fn adjacent(s: &Scene, b: BodyId, f: FaceId, g: FaceId) -> bool {
    let t = &s.ctx(b).unwrap().topo;
    t.edges
        .iter()
        .any(|e| e.faces.contains(&f) && e.faces.contains(&g))
}
/// Connected components of the line edges selected by `keep`.
fn components(
    m: &OutputMesh,
    keep: &dyn Fn(&newtopo_core::mesh::OutEdge) -> bool,
) -> Vec<(BTreeSet<u32>, usize)> {
    let es: Vec<_> = m.edges.iter().filter(|e| keep(e)).collect();
    let mut uf: HashMap<u32, u32> = HashMap::new();
    fn find(uf: &mut HashMap<u32, u32>, x: u32) -> u32 {
        let p = *uf.entry(x).or_insert(x);
        if p == x {
            x
        } else {
            let r = find(uf, p);
            uf.insert(x, r);
            r
        }
    }
    for e in &es {
        let (a, b) = (find(&mut uf, e.a), find(&mut uf, e.b));
        uf.insert(a, b);
    }
    let mut comps: BTreeMap<u32, (BTreeSet<u32>, usize)> = BTreeMap::new();
    for e in &es {
        let r = find(&mut uf, e.a);
        let c = comps.entry(r).or_default();
        c.0.insert(e.a);
        c.0.insert(e.b);
        c.1 += 1;
    }
    comps.into_values().collect()
}
fn degrees_two(m: &OutputMesh, keep: &dyn Fn(&newtopo_core::mesh::OutEdge) -> bool) -> bool {
    let mut d: HashMap<u32, u32> = HashMap::new();
    for e in m.edges.iter().filter(|e| keep(e)) {
        *d.entry(e.a).or_default() += 1;
        *d.entry(e.b).or_default() += 1;
    }
    !d.is_empty() && d.values().all(|&x| x == 2)
}
fn poly_faces_of_vertex(m: &OutputMesh, v: u32) -> BTreeSet<FaceId> {
    m.polygons
        .iter()
        .filter(|p| p.verts.contains(&v))
        .map(|p| p.face)
        .collect()
}

/// §6.4: slicing the filleted box with 10 planes gives 10 closed loops that
/// pass through the fillets.
#[test]
fn occt_filleted_box_slice_ten_loops() {
    let (mut s, b) = occt_scene("filleted_box");
    s.apply(Op::Slice {
        body: Some(b),
        patches: None,
        count: 10,
        through: false,
        guide_edge: None,
    })
    .unwrap();
    assert_valid(&mut s, b, "slice 10");
    let m = s.output_of(b).unwrap().clone();
    let fillets: BTreeSet<FaceId> = faces_of(&s, b, SurfaceKind::Cylinder).into_iter().collect();
    for k in 0..10u32 {
        let keep = move |e: &newtopo_core::mesh::OutEdge| matches!(e.kind, OutEdgeKind::Line(l) if l.index == k);
        assert!(degrees_two(&m, &keep), "plane {k} is closed");
        let comps = components(&m, &keep);
        assert_eq!(comps.len(), 1, "plane {k} is one loop");
        let through_fillet = comps[0].0.iter().any(|&v| {
            poly_faces_of_vertex(&m, v)
                .iter()
                .any(|f| fillets.contains(f))
        });
        assert!(through_fillet, "plane {k} passes through a fillet");
    }
}

/// §6.2: three fillet patches in a chain (fillet, corner, fillet) with
/// Connect A = 4 give 4 continuous lines.
#[test]
fn occt_fillet_chain_connect() {
    let (mut s, b) = occt_scene("filleted_box");
    // Two horizontal edge fillets meeting at a corner patch whose axis is
    // vertical: their generators continue along the corner's latitudes.
    let t = s.ctx(b).unwrap().topo.clone();
    let axis = |f: FaceId| t.face(f).surface.axis_dir.unwrap();
    let cyl: Vec<FaceId> = faces_of(&s, b, SurfaceKind::Cylinder)
        .into_iter()
        .filter(|f| axis(*f).z.abs() < 1e-6)
        .collect();
    let sph: Vec<FaceId> = faces_of(&s, b, SurfaceKind::Sphere)
        .into_iter()
        .filter(|f| axis(*f).z.abs() > 0.9)
        .collect();
    let (f1, c, f2) = sph
        .iter()
        .find_map(|&c| {
            let n: Vec<FaceId> = cyl
                .iter()
                .copied()
                .filter(|&f| adjacent(&s, b, f, c))
                .collect();
            (n.len() >= 2).then(|| (n[0], c, n[1]))
        })
        .unwrap();
    s.apply(Op::Connect {
        patches: PatchSel {
            body: Some(b),
            faces: vec![f1.0, c.0, f2.0],
            ..Default::default()
        },
        a: 4,
        b: 0,
        start_on_seam: false,
    })
    .unwrap();
    assert_valid(&mut s, b, "fillet chain");
    let m = s.output_of(b).unwrap().clone();
    let rule = s
        .doc()
        .rules
        .iter()
        .find(|r| matches!(r.rule, Rule::Connect { .. }))
        .unwrap()
        .id;
    let keep = move |e: &newtopo_core::mesh::OutEdge| matches!(e.kind, OutEdgeKind::Line(l) if l.rule == rule && l.index < (1 << 16));
    let comps = components(&m, &keep);
    let spanning = comps
        .iter()
        .filter(|(vs, _)| {
            let fs: BTreeSet<FaceId> = vs
                .iter()
                .flat_map(|&v| poly_faces_of_vertex(&m, v))
                .collect();
            fs.contains(&f1) && fs.contains(&c) && fs.contains(&f2)
        })
        .count();
    assert_eq!(
        spanning,
        4,
        "4 continuous lines across fillet, corner, fillet ({} components)",
        comps.len()
    );
}

/// §6.5: a through-iso along a fillet ring makes one closed loop.
#[test]
fn occt_fillet_ring_through_iso() {
    let (mut s, b) = occt_scene("filleted_box");
    let f = faces_of(&s, b, SurfaceKind::Cylinder)[0];
    // The line along the fillet: the iso direction whose 3D tangent follows
    // the fillet's straight direction.
    let c = s.ctx(b).unwrap();
    let fg = c.face(f);
    let mid = (fg.lo + fg.hi) * 0.5;
    let e = c.face_eval(f, &[mid])[0];
    let axis = fg.info.axis_dir.unwrap();
    // Around the fillet's arc (not along its length).
    let dir = if e.dv.normalize().dot(axis).abs() > 0.9 {
        IsoDir::V
    } else {
        IsoDir::U
    };
    s.apply(Op::Iso {
        body: Some(b),
        face: f.0,
        dir,
        value: 0.5,
        seed: None,
        split: false,
        through: true,
    })
    .unwrap();
    assert_valid(&mut s, b, "through iso");
    let Rule::Iso { pieces, .. } = &s.doc().rules[0].rule else {
        panic!()
    };
    assert!(
        pieces.len() >= 8,
        "the walk goes round the ring: {} pieces",
        pieces.len()
    );
    let m = s.output_of(b).unwrap().clone();
    let keep = |e: &newtopo_core::mesh::OutEdge| matches!(e.kind, OutEdgeKind::Line(_));
    assert!(degrees_two(&m, &keep), "closed");
    assert_eq!(components(&m, &keep).len(), 1, "one loop");
}

/// §6.2: Connect on the revolved bottle (surface of revolution, two directions).
#[test]
fn occt_bottle_connect() {
    let (mut s, b) = occt_scene("bottle");
    let rev = faces_of(&s, b, SurfaceKind::Revolution)[0];
    let bi = s.bend_info(&PatchId::whole(b, rev)).unwrap();
    assert!(bi.a.is_some());
    s.apply(Op::Connect {
        patches: PatchSel {
            body: Some(b),
            faces: vec![rev.0],
            ..Default::default()
        },
        a: 12,
        b: 6,
        start_on_seam: false,
    })
    .unwrap();
    assert_valid(&mut s, b, "bottle connect");
    s.apply(Op::Slice {
        body: Some(b),
        patches: None,
        count: 6,
        through: false,
        guide_edge: None,
    })
    .unwrap();
    assert_valid(&mut s, b, "bottle slice (radial)");
}

/// §6.11: plate with a 4 × 4 grid of holes, Connect A = 8 on one hole wall
/// with Link on gives 8 lines in all 16.
#[test]
fn occt_plate_link() {
    let (mut s, b) = occt_scene("plate_holes");
    let walls = faces_of(&s, b, SurfaceKind::Cylinder);
    assert_eq!(walls.len(), 16);
    assert_eq!(s.link_set(b, walls[0]).len(), 16);
    s.apply(Op::SetLink { on: true }).unwrap();
    s.apply(Op::Connect {
        patches: PatchSel {
            body: Some(b),
            faces: vec![walls[0].0],
            ..Default::default()
        },
        a: 8,
        b: 0,
        start_on_seam: false,
    })
    .unwrap();
    assert_valid(&mut s, b, "linked");
    let Rule::Connect { patches, .. } = &s.doc().rules[0].rule else {
        panic!()
    };
    assert_eq!(patches.len(), 16);
    let m = s.output_of(b).unwrap().clone();
    for w in &walls {
        let n: BTreeSet<(u32, u32)> = m
            .edges
            .iter()
            .filter_map(|e| {
                if let OutEdgeKind::Line(k) = e.kind {
                    Some((e, k))
                } else {
                    None
                }
            })
            .filter(|(e, _)| {
                m.polygons
                    .iter()
                    .any(|p| p.face == *w && p.verts.contains(&e.a) && p.verts.contains(&e.b))
            })
            .map(|(_, k)| (k.index, k.sub))
            .collect();
        assert_eq!(n.len(), 8, "wall {w}");
    }
}

/// SPEC §11 property run on every OCCT reference model (seeded, small):
/// each step validates or is refused; undo-all restores the document;
/// save → load → evaluate gives the same output.
#[test]
fn occt_property_all_models() {
    let mut seed: u64 = 0x5eed_0f0c;
    let mut rnd = move |n: u64| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) % n.max(1)
    };
    let (mut acc, mut refused) = (0, 0);
    for m in newtopo_testmodels::MODELS {
        let (mut s, b) = occt_scene(m.name);
        let initial = s.doc().clone();
        let nf = s.ctx(b).unwrap().topo.faces.len() as u64;
        for _ in 0..6 {
            let f = rnd(nf) as u32;
            let out = s.output_of(b).unwrap().clone();
            let lines: Vec<_> = out
                .edges
                .iter()
                .filter(|e| matches!(e.kind, OutEdgeKind::Line(_)))
                .collect();
            let op = match rnd(7) {
                0 => Op::Grid {
                    patches: PatchSel {
                        body: Some(b),
                        faces: vec![f],
                        ..Default::default()
                    },
                    u: rnd(5) as u32,
                    v: rnd(4) as u32,
                    start_on_seam: false,
                    spacing: Spacing::ArcLength,
                },
                1 => Op::Connect {
                    patches: PatchSel {
                        body: Some(b),
                        faces: vec![f],
                        ..Default::default()
                    },
                    a: 1 + rnd(8) as u32,
                    b: rnd(5) as u32,
                    start_on_seam: false,
                },
                2 => Op::Iso {
                    body: Some(b),
                    face: f,
                    dir: if rnd(2) == 0 { IsoDir::U } else { IsoDir::V },
                    value: 0.1 + 0.8 * rnd(100) as f64 / 100.0,
                    seed: None,
                    split: rnd(2) == 0,
                    through: rnd(2) == 0,
                },
                3 => Op::Slice {
                    body: Some(b),
                    patches: None,
                    count: 1 + rnd(6) as u32,
                    through: false,
                    guide_edge: None,
                },
                4 if !lines.is_empty() => Op::RemoveEdge {
                    body: Some(b),
                    edge: lines[rnd(lines.len() as u64) as usize].key.clone().unwrap(),
                },
                5 if !lines.is_empty() => Op::Collapse {
                    body: Some(b),
                    edge: lines[rnd(lines.len() as u64) as usize].key.clone().unwrap(),
                    confirmed: true,
                },
                _ => Op::SetAutofill {
                    on: !s.doc().settings.autofill,
                },
            };
            match s.apply(op.clone()) {
                Ok(_) => {
                    acc += 1;
                    assert_valid(&mut s, b, &format!("{} after {:?}", m.name, op));
                }
                Err(r) => {
                    refused += 1;
                    assert!(!r.reason.is_empty());
                    eprintln!("{}: refused {:?}: {}", m.name, op, r.reason);
                }
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.ntopo");
        s.save(&path).unwrap();
        let mut s2 = Scene::load(Arc::new(OcctKernel::new()), &path).unwrap();
        let b2 = s2.body_ids()[0];
        let (o1, o2) = (
            s.output_of(b).unwrap().clone(),
            s2.output_of(b2).unwrap().clone(),
        );
        assert_eq!(
            o1.polygons.iter().map(|p| &p.verts).collect::<Vec<_>>(),
            o2.polygons.iter().map(|p| &p.verts).collect::<Vec<_>>(),
            "{}: save/load",
            m.name
        );
        let mut guard = 0;
        while s.doc() != &initial && guard < 50 {
            assert!(s.undo());
            guard += 1;
        }
        assert_eq!(s.doc(), &initial, "{}: undo all", m.name);
    }
    eprintln!("occt property: accepted {acc}, refused {refused}");
}

/// Line edges lying on face `w` (by the polygons using them).
fn line_edges_on(m: &OutputMesh, w: FaceId) -> Vec<usize> {
    m.edges
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e.kind, OutEdgeKind::Line(_)))
        .filter(|(_, e)| {
            m.polygons
                .iter()
                .any(|p| p.face == w && p.verts.contains(&e.a) && p.verts.contains(&e.b))
        })
        .map(|(i, _)| i)
        .collect()
}

/// §6.11 (review round 3, finding 5): a Cut on one hole wall of
/// plate_holes with Link on puts a cut on all 16 walls, as one undo step.
#[test]
fn occt_plate_link_cut() {
    use newtopo_core::doc::CutPoint;
    let (mut s, b) = occt_scene("plate_holes");
    let walls = faces_of(&s, b, SurfaceKind::Cylinder);
    s.apply(Op::SetLink { on: true }).unwrap();
    let t = s.ctx(b).unwrap().topo.clone();
    // The wall's two rim circles (non-seam edges).
    let rims: Vec<_> = t
        .face(walls[0])
        .wires
        .iter()
        .flat_map(|w| w.coedges.iter().map(|c| c.edge))
        .filter(|e| !t.edge(*e).seam && !t.edge(*e).degenerate)
        .collect();
    assert_eq!(rims.len(), 2, "{rims:?}");
    let mid = |e: newtopo_kernel::EdgeId| {
        let (a, z) = t.edge(e).t_range;
        0.5 * (a + z)
    };
    let out = s
        .apply(Op::Cut {
            points: vec![
                CutPoint::CadEdge {
                    body: b,
                    edge: rims[0],
                    t: mid(rims[0]),
                },
                CutPoint::CadEdge {
                    body: b,
                    edge: rims[1],
                    t: mid(rims[1]),
                },
            ],
            closed: false,
        })
        .unwrap();
    assert!(
        out.message.as_deref().unwrap_or("").contains("15 linked"),
        "{:?}",
        out.message
    );
    assert_eq!(
        s.doc()
            .rules
            .iter()
            .filter(|r| matches!(r.rule, Rule::Cut { .. }))
            .count(),
        16
    );
    assert_valid(&mut s, b, "linked cut");
    let m = s.output_of(b).unwrap().clone();
    for w in &walls {
        assert_eq!(line_edges_on(&m, *w).len(), 1, "wall {w}");
    }
    assert!(s.undo());
    assert!(s.doc().rules.is_empty(), "one undo step removes all 16");
}

/// §6.11 (review round 3, finding 5): RemoveEdge on one linked wall removes
/// the matching edge on every wall.
#[test]
fn occt_plate_link_remove_edge() {
    let (mut s, b) = occt_scene("plate_holes");
    let walls = faces_of(&s, b, SurfaceKind::Cylinder);
    s.apply(Op::SetLink { on: true }).unwrap();
    s.apply(Op::Connect {
        patches: PatchSel {
            body: Some(b),
            faces: vec![walls[0].0],
            ..Default::default()
        },
        a: 8,
        b: 0,
        start_on_seam: false,
    })
    .unwrap();
    let m = s.output_of(b).unwrap().clone();
    let e = line_edges_on(&m, walls[0])[0];
    let out = s
        .apply(Op::RemoveEdge {
            body: Some(b),
            edge: m.edges[e].key.clone().unwrap(),
        })
        .unwrap();
    assert!(
        out.message.as_deref().unwrap_or("").contains("15 linked"),
        "{:?}",
        out.message
    );
    assert_eq!(s.doc().edits.len(), 16);
    assert_valid(&mut s, b, "linked remove");
    let m = s.output_of(b).unwrap().clone();
    for w in &walls {
        assert_eq!(line_edges_on(&m, *w).len(), 7, "wall {w}");
    }
}

/// Body-wide Slices on the periodic OCCT models: pins the seam-clipping
/// output (a loop-variable bug once changed these silently while every
/// result still validated).
#[test]
fn occt_torus_slices_are_stable() {
    let mut got = Vec::new();
    for name in ["torus_hole", "torus"] {
        for count in [1u32, 3, 5] {
            let (mut s, b) = occt_scene(name);
            // The pinned outcomes below were recorded with auto-fill on.
            s.apply(Op::SetAutofill { on: true }).unwrap();
            let r = s.apply(Op::Slice {
                body: Some(b),
                patches: None,
                count,
                through: false,
                guide_edge: None,
            });
            let st = s.output_of(b).unwrap().stats();
            // Vertices come from the arrangement (what seam clipping
            // decides); face counts also depend on auto-fill tie-breaks,
            // which vary with OCCT's floating point across platforms.
            got.push(format!(
                "{name} slice {count}: {} -> {} verts",
                if r.is_ok() { "ok" } else { "refused" },
                st.verts
            ));
        }
    }
    // Recorded at 1a0b2a5 (before the bounded seam loop) and unchanged since.
    let expected = [
        "torus_hole slice 1: ok -> 32 verts",
        "torus_hole slice 3: refused -> 75 verts",
        "torus_hole slice 5: ok -> 80 verts",
        "torus slice 1: refused -> 64 verts",
        "torus slice 3: ok -> 48 verts",
        "torus slice 5: ok -> 80 verts",
    ];
    assert_eq!(got, expected);
}
