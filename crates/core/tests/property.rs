//! Property tests (SPEC §11): seeded random tool sequences on every mock
//! model. After each step either the output validates or the tool refused
//! with a reason; undoing everything returns the initial document exactly;
//! save → load → evaluate gives the same output.
mod common;
use common::*;
use glam::DVec2;
use newtopo_core::doc::{CutPoint, IsoDir, Spacing};
use newtopo_core::eval::IssueKind;
use newtopo_core::mesh::{OutEdgeKind, OutputMesh};
use newtopo_core::ops::PatchSel;
use newtopo_core::{Op, Scene};
use newtopo_kernel::mock::{MockBody, MockKernel};
use newtopo_kernel::{BodyId, FaceId};
use proptest::prelude::*;
use proptest::test_runner::{Config, RngSeed};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Debug)]
enum A {
    Grid(u8, u8, u8),
    Connect(u8, u8, u8),
    Iso(u8, bool, f64, bool, bool),
    Slice(u8),
    Remove(u16),
    Collapse(u16),
    Weld(u16),
    Autofill,
    DeleteRule(u8),
    Cut(u8, (f64, f64), (f64, f64)),
}

fn arb_a() -> impl Strategy<Value = A> {
    prop_oneof![
        3 => (any::<u8>(), 0u8..6, 0u8..6).prop_map(|(f, u, v)| A::Grid(f, u, v)),
        3 => (any::<u8>(), 0u8..10, 0u8..6).prop_map(|(f, a, b)| A::Connect(f, a, b)),
        2 => (any::<u8>(), any::<bool>(), 0.05f64..0.95, any::<bool>(), any::<bool>()).prop_map(|(f, d, v, s, t)| A::Iso(f, d, v, s, t)),
        1 => (0u8..6).prop_map(A::Slice),
        3 => any::<u16>().prop_map(A::Remove),
        2 => any::<u16>().prop_map(A::Collapse),
        2 => any::<u16>().prop_map(A::Weld),
        1 => Just(A::Autofill),
        1 => any::<u8>().prop_map(A::DeleteRule),
        1 => (any::<u8>(), (0.1f64..0.9, 0.1f64..0.9), (0.1f64..0.9, 0.1f64..0.9)).prop_map(|(f, a, b)| A::Cut(f, a, b)),
    ]
}

fn models() -> Vec<MockBody> {
    vec![
        MockBody::make_box(glam::DVec3::new(4.0, 3.0, 2.0)),
        MockBody::cylinder(2.0, 5.0),
        MockBody::plate_with_holes(1, 1),
        MockBody::cone(2.0, 4.0),
        MockBody::sphere(3.0),
        MockBody::torus(4.0, 1.0),
        MockBody::trimmed_torus(4.0, 1.0),
        MockBody::ring(3.0, 1.0),
        MockBody::symmetric_part(),
        MockBody::split_cylinder(2.0, 4.0),
    ]
}

static GATE_REFUSALS: AtomicUsize = AtomicUsize::new(0);
static ACCEPTED: AtomicUsize = AtomicUsize::new(0);
static REFUSED: AtomicUsize = AtomicUsize::new(0);

fn resolve(s: &mut Scene, b: BodyId, a: &A) -> Option<Op> {
    let m: OutputMesh = s.output_of(b).unwrap().clone();
    let ctx = s.ctx(b).unwrap();
    let nf = ctx.topo.faces.len() as u32;
    let infos: Vec<_> = ctx.faces.iter().map(|f| f.info.clone()).collect();
    let face_info = |f: u32| infos[f as usize].clone();
    let line_edges: Vec<usize> = m
        .edges
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e.kind, OutEdgeKind::Line(_) | OutEdgeKind::Split(_)))
        .map(|(i, _)| i)
        .collect();
    Some(match a {
        A::Grid(f, u, v) => Op::Grid {
            patches: PatchSel {
                faces: vec![*f as u32 % nf],
                ..Default::default()
            },
            u: *u as u32,
            v: *v as u32,
            start_on_seam: false,
            spacing: Spacing::ArcLength,
        },
        A::Connect(f, ca, cb) => Op::Connect {
            patches: PatchSel {
                faces: vec![*f as u32 % nf],
                ..Default::default()
            },
            a: *ca as u32,
            b: *cb as u32,
            start_on_seam: false,
        },
        A::Iso(f, d, v, split, through) => Op::Iso {
            body: Some(b),
            face: *f as u32 % nf,
            dir: if *d { IsoDir::U } else { IsoDir::V },
            value: *v,
            seed: None,
            split: *split,
            through: *through,
        },
        A::Slice(c) => Op::Slice {
            body: Some(b),
            patches: None,
            count: *c as u32,
            through: false,
            guide_edge: None,
        },
        A::Remove(p) => {
            let e = &m.edges[*line_edges.get(*p as usize % line_edges.len().max(1))?];
            Op::RemoveEdge {
                body: Some(b),
                edge: e.key.clone()?,
            }
        }
        A::Collapse(p) => {
            let e = &m.edges[*line_edges.get(*p as usize % line_edges.len().max(1))?];
            Op::Collapse {
                body: Some(b),
                edge: e.key.clone()?,
                confirmed: true,
            }
        }
        A::Weld(p) => {
            let e = &m.edges[*line_edges.get(*p as usize % line_edges.len().max(1))?];
            Op::Weld {
                body: Some(b),
                from: m.vertices[e.a as usize].key.clone(),
                to: m.vertices[e.b as usize].key.clone(),
                confirmed: true,
            }
        }
        A::Autofill => Op::SetAutofill {
            on: !s.doc().settings.autofill,
        },
        A::DeleteRule(p) => {
            let rules = &s.doc().rules;
            if rules.is_empty() {
                return None;
            }
            Op::DeleteRule {
                rule: rules[*p as usize % rules.len()].id,
            }
        }
        A::Cut(f, pa, pb) => {
            let f = *f as u32 % nf;
            let info = face_info(f);
            let at = |p: (f64, f64)| {
                DVec2::new(
                    info.u_range.0 + (info.u_range.1 - info.u_range.0) * p.0,
                    info.v_range.0 + (info.v_range.1 - info.v_range.0) * p.1,
                )
            };
            let (ua, ub) = (at(*pa), at(*pb));
            let k = s.ctx(b).unwrap().kernel.clone();
            let kb = s.ctx(b).unwrap().kbody;
            let cls = k.face_classify_uv(kb, FaceId(f), &[ua, ub]).ok()?;
            if cls.iter().any(|c| *c != newtopo_kernel::UvClass::In) {
                return None;
            }
            Op::Cut {
                points: vec![
                    CutPoint::Face {
                        body: b,
                        face: FaceId(f),
                        uv: ua,
                    },
                    CutPoint::Face {
                        body: b,
                        face: FaceId(f),
                        uv: ub,
                    },
                ],
                closed: false,
            }
        }
    })
}

fn same_output(a: &OutputMesh, b: &OutputMesh) -> bool {
    a.vertices.len() == b.vertices.len()
        && a.polygons
            .iter()
            .map(|p| &p.verts)
            .eq(b.polygons.iter().map(|p| &p.verts))
        && a.vertices
            .iter()
            .zip(&b.vertices)
            .all(|(x, y)| x.key == y.key && (x.position - y.position).length() < 1e-9)
}

fn run_case(model: usize, steps: &[A]) -> Result<(), TestCaseError> {
    let body = models().swap_remove(model);
    let (mut s, b) = scene_with(body);
    let initial = s.doc().clone();
    let undo_base = {
        let mut n = 0;
        while s.can_undo() {
            s.undo();
            n += 1;
        }
        for _ in 0..n {
            s.redo();
        }
        n
    };
    let mut applied = 0usize;
    for a in steps {
        let Some(op) = resolve(&mut s, b, a) else {
            continue;
        };
        match s.apply(op.clone()) {
            Ok(_) => {
                ACCEPTED.fetch_add(1, Ordering::Relaxed);
                applied += 1;
                let issues: Vec<_> = s
                    .validate(b)
                    .into_iter()
                    .filter(|i| i.kind != IssueKind::FanFallback)
                    .collect();
                prop_assert!(
                    issues.is_empty(),
                    "model {model}: {:?} accepted but invalid: {:?}",
                    op,
                    issues.iter().map(|i| &i.message).collect::<Vec<_>>()
                );
            }
            Err(r) => {
                REFUSED.fetch_add(1, Ordering::Relaxed);
                prop_assert!(!r.reason.is_empty());
                if r.reason.starts_with("can't do that: ")
                    && !r.reason.contains("no longer")
                    && !r.reason.contains("share a polygon")
                    && !r.reason.contains("non-manifold")
                    && !r.reason.contains("flip")
                {
                    GATE_REFUSALS.fetch_add(1, Ordering::Relaxed);
                    eprintln!("gate refusal on model {model}: {:?}: {}", op, r.reason);
                }
            }
        }
    }
    // The cached (incremental) output equals a from-scratch evaluation.
    {
        let fresh = newtopo_core::eval::evaluate(s.ctx(b).unwrap(), s.doc(), b);
        let cached = s.output_of(b).unwrap().clone();
        prop_assert!(
            same_output(&cached, &fresh.mesh),
            "model {model}: incremental output differs from a full evaluation"
        );
    }
    // Save → load → evaluate gives the same output.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.ntopo");
    s.save(&path).unwrap();
    let mut s2 = Scene::load(Arc::new(MockKernel::new()), &path).unwrap();
    prop_assert_eq!(s2.doc(), s.doc());
    let o1 = s.output_of(b).unwrap().clone();
    let o2 = s2.output_of(b).unwrap().clone();
    prop_assert!(
        same_output(&o1, &o2),
        "model {model}: save/load changed the output"
    );
    // Undo everything → initial document.
    let _ = (undo_base, applied);
    let mut guard = 0;
    while s.doc() != &initial && guard < 100 {
        prop_assert!(
            s.undo(),
            "ran out of undo steps before reaching the initial document"
        );
        guard += 1;
    }
    prop_assert_eq!(s.doc(), &initial);
    // And the output is back to the untouched one.
    let back = s.output_of(b).unwrap().clone();
    let mut fresh = scene_with(models().swap_remove(model));
    let orig = fresh.0.output_of(fresh.1).unwrap().clone();
    prop_assert!(same_output(&back, &orig));
    Ok(())
}

#[test]
fn random_op_sequences() {
    let cases: u32 = std::env::var("NT_PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(40);
    let mut runner = proptest::test_runner::TestRunner::new(Config {
        cases,
        rng_seed: RngSeed::Fixed(0x6e74_6f70_6f31),
        failure_persistence: None,
        ..Config::default()
    });
    let strat = (0usize..10, proptest::collection::vec(arb_a(), 1..7));
    let res = runner.run(&strat, |(model, steps)| run_case(model, &steps));
    eprintln!(
        "property: accepted {}, refused {}, evaluator-gate refusals {}",
        ACCEPTED.load(Ordering::Relaxed),
        REFUSED.load(Ordering::Relaxed),
        GATE_REFUSALS.load(Ordering::Relaxed)
    );
    if let Err(e) = res {
        panic!("{e}");
    }
}
