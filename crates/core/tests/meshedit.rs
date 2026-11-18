//! Mesh mode (SPEC §9.4): Finish (bake) real evaluated meshes from the mock
//! kernel and run every mesh tool on them; property tests with random
//! meshes and random tool sequences.
mod common;
use common::*;
use newtopo_core::Op;
use newtopo_core::doc::Spacing as GridSpacing;
use newtopo_core::meshedit::synth::{self, hausdorff, vertex_deviation};
use newtopo_core::meshedit::*;
use newtopo_core::ops::PatchSel;
use newtopo_kernel::mock::MockBody;
use proptest::prelude::*;

fn assert_sound(input: &EditMesh, out: &EditMesh, what: &str) {
    let issues = out.issues();
    assert!(issues.is_empty(), "{what}: {issues:?}");
    let flipped = out.flipped_polygons();
    assert!(flipped.is_empty(), "{what}: flipped polygons {flipped:?}");
    assert_eq!(input.topology(), out.topology(), "{what}: topology changed");
    for (i, &p) in input.pinned.iter().enumerate() {
        if p {
            let x = input.positions[i];
            assert!(
                out.positions
                    .iter()
                    .zip(&out.pinned)
                    .any(|(q, &pin)| pin && q.distance(x) < 1e-12),
                "{what}: pinned vertex {i} lost"
            );
        }
    }
}

/// Every flagged output edge with a CAD flag runs between output vertices
/// that were on CAD edges in the input (subset placement / sliding along
/// the CAD run), and no CAD flag appears where there was none.
fn cad_edges_on_cad(input: &EditMesh, out: &EditMesh, tol: f64) {
    let segs: Vec<(glam::DVec3, glam::DVec3)> = input
        .flagged
        .iter()
        .filter(|e| e.flags.cad)
        .map(|e| (input.positions[e.a as usize], input.positions[e.b as usize]))
        .collect();
    let dist = |p: glam::DVec3| {
        segs.iter()
            .map(|&(a, b)| {
                let d = b - a;
                let t = ((p - a).dot(d) / d.length_squared()).clamp(0.0, 1.0);
                p.distance(a + d * t)
            })
            .fold(f64::INFINITY, f64::min)
    };
    for e in out.flagged.iter().filter(|e| e.flags.cad) {
        for v in [e.a, e.b] {
            let d = dist(out.positions[v as usize]);
            assert!(d <= tol, "CAD vertex {v} is {d} off the input CAD edges");
        }
    }
}

fn baked(body: MockBody, grid: u32) -> EditMesh {
    let (mut s, b) = scene_with(body);
    set_autofill(&mut s, true);
    let nf = s.ctx(b).unwrap().topo.faces.len() as u32;
    for f in 0..nf {
        let _ = s.apply(Op::Grid {
            patches: PatchSel {
                faces: vec![f],
                ..Default::default()
            },
            u: grid,
            v: grid,
            start_on_seam: false,
            spacing: GridSpacing::ArcLength,
        });
    }
    assert_valid(&mut s, b, "before bake");
    let out = s.output_of(b).unwrap().clone();
    let m = EditMesh::bake(&out).expect("bake");
    assert_eq!(m.stats().faces, out.stats().faces);
    m
}

fn bodies() -> Vec<MockBody> {
    vec![
        MockBody::make_box(glam::DVec3::new(4.0, 3.0, 2.0)),
        MockBody::cylinder(2.0, 5.0),
        MockBody::plate_with_holes(2, 2),
        MockBody::sphere(3.0),
        MockBody::torus(4.0, 1.0),
        MockBody::ring(3.0, 1.0),
        MockBody::cone(2.0, 4.0),
    ]
}

#[test]
fn baked_models_survive_every_tool() {
    for body in bodies() {
        let name = body.name.clone();
        let m = baked(body, 6);
        let tol = 1e-9 * m.diagonal().max(1.0);
        let d = decimate(&m, &DecimateParams::default()).unwrap();
        assert_sound(&m, &d.mesh, &format!("{name} decimate"));
        cad_edges_on_cad(&m, &d.mesh, tol);
        assert!(
            d.mesh.tri_count() < m.tri_count(),
            "{name}: {}",
            d.report.message
        );

        let pl = decimate(
            &m,
            &DecimateParams {
                mode: DecimateMode::Planar,
                ..Default::default()
            },
        )
        .unwrap();
        assert_sound(&m, &pl.mesh, &format!("{name} planar"));
        cad_edges_on_cad(&m, &pl.mesh, tol);

        let has_features = !m.flagged.is_empty() || !m.boundary_edges().is_empty();
        let c = cleanup(&m, &Selection::AllFeatures, &CleanupParams::for_mesh(&m));
        if !has_features {
            // A closed body without CAD edges (sphere, torus): nothing to
            // select, refused with a reason.
            assert!(c.is_err(), "{name}");
            continue;
        }
        let c = c.unwrap();
        assert_sound(&m, &c.mesh, &format!("{name} cleanup"));
        assert!(vertex_deviation(&m, &c.mesh) <= 1e-3 * m.diagonal() + 1e-12);

        for spacing in [Spacing::Even, Spacing::EdgeLength(0.1 * m.diagonal())] {
            let p = ResampleParams {
                spacing,
                ..Default::default()
            };
            let r = resample(&m, &Selection::AllFeatures, &p).unwrap();
            assert_sound(&m, &r.mesh, &format!("{name} resample {spacing:?}"));
            cad_edges_on_cad(&m, &r.mesh, 1e-6 * m.diagonal());
        }
    }
}

#[test]
fn bake_keeps_cad_flags_corners_and_normals() {
    let m = baked(MockBody::make_box(glam::DVec3::new(4.0, 3.0, 2.0)), 4);
    assert_eq!(m.pinned.iter().filter(|&&p| p).count(), 8, "box corners");
    // All 12 box edges are CAD and hard.
    let len: f64 = m
        .flagged
        .iter()
        .filter(|e| e.flags.cad && e.flags.hard)
        .map(|e| m.positions[e.a as usize].distance(m.positions[e.b as usize]))
        .sum();
    assert!((len - 4.0 * (4.0 + 3.0 + 2.0)).abs() < 1e-9, "{len}");
    assert!(m.flipped_polygons().is_empty());
    // Decimating a box keeps it a box exactly.
    let d = decimate(
        &m,
        &DecimateParams {
            target: Target::Ratio(0.01),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(hausdorff(&m, &d.mesh) < 1e-9);
}

// ------------------------------------------------------------- properties

#[derive(Clone, Debug)]
enum Tool {
    Decimate(f64, bool),
    Planar(f64),
    Cleanup(bool, f64),
    Resample(u8, u32),
}

fn arb_tool() -> impl Strategy<Value = Tool> {
    prop_oneof![
        (0.05f64..1.0, any::<bool>()).prop_map(|(r, q)| Tool::Decimate(r, q)),
        (0.5f64..20.0).prop_map(Tool::Planar),
        (any::<bool>(), 1.0f64..30.0).prop_map(|(s, a)| Tool::Cleanup(s, a)),
        (0u8..3, 3u32..40).prop_map(|(k, n)| Tool::Resample(k, n)),
    ]
}

#[derive(Clone, Debug)]
enum Shape {
    Height(usize, usize, f64, f64),
    Cube(usize),
    Cylinder(usize, usize),
    Sphere(usize, usize),
    Torus(usize, usize),
}

fn arb_shape() -> impl Strategy<Value = Shape> {
    prop_oneof![
        (2usize..10, 2usize..10, 0.0f64..0.4, 0.5f64..4.0)
            .prop_map(|(a, b, h, f)| Shape::Height(a, b, h, f)),
        (1usize..6).prop_map(Shape::Cube),
        (3usize..20, 1usize..5).prop_map(|(a, b)| Shape::Cylinder(a, b)),
        (4usize..16, 3usize..9).prop_map(|(a, b)| Shape::Sphere(a, b)),
        (4usize..16, 3usize..9).prop_map(|(a, b)| Shape::Torus(a, b)),
    ]
}

fn build(s: &Shape) -> EditMesh {
    match *s {
        Shape::Height(a, b, h, f) => {
            synth::heightfield(a, b, 1.0, |x, y| h * (f * x).sin() * (f * y).cos())
        }
        Shape::Cube(n) => synth::cube(n, 1.0),
        Shape::Cylinder(a, b) => synth::cylinder(a, b, 1.0, 2.0),
        Shape::Sphere(a, b) => synth::sphere(a, b, 1.0),
        Shape::Torus(a, b) => synth::torus(a, b, 3.0, 1.0),
    }
}

/// A selection: all feature runs, or one edge loop picked from the mesh.
fn selection(m: &EditMesh, k: u8) -> Selection {
    if k == 0 {
        return Selection::AllFeatures;
    }
    // Walk a run greedily from the lowest edge: straightest continuation.
    let edges = m.edges();
    let (a, b) = edges[(k as usize * 7919) % edges.len()];
    let mut run = vec![[a, b]];
    let mut used = std::collections::BTreeSet::from([(a.min(b), a.max(b))]);
    let (mut prev, mut cur) = (a, b);
    for _ in 0..32 {
        let d = (m.positions[cur as usize] - m.positions[prev as usize]).normalize();
        let next = edges
            .iter()
            .filter_map(|&(x, y)| {
                if x == cur {
                    Some(y)
                } else if y == cur {
                    Some(x)
                } else {
                    None
                }
            })
            .filter(|&n| !used.contains(&(cur.min(n), cur.max(n))))
            .max_by(|&p, &q| {
                let dp = (m.positions[p as usize] - m.positions[cur as usize])
                    .normalize()
                    .dot(d);
                let dq = (m.positions[q as usize] - m.positions[cur as usize])
                    .normalize()
                    .dot(d);
                dp.total_cmp(&dq).then(q.cmp(&p))
            });
        let Some(n) = next else { break };
        let dn = (m.positions[n as usize] - m.positions[cur as usize])
            .normalize()
            .dot(d);
        if dn < 0.5 {
            break;
        }
        used.insert((cur.min(n), cur.max(n)));
        run.push([cur, n]);
        prev = cur;
        cur = n;
    }
    Selection::Edges(run)
}

fn run_tool(m: &EditMesh, t: &Tool) -> Result<EditOutcome, newtopo_core::Refusal> {
    match *t {
        Tool::Decimate(r, q) => decimate(
            m,
            &DecimateParams {
                target: Target::Ratio(r),
                keep_quads: q,
                ..Default::default()
            },
        ),
        Tool::Planar(a) => decimate(
            m,
            &DecimateParams {
                mode: DecimateMode::Planar,
                planar_angle_deg: a,
                ..Default::default()
            },
        ),
        Tool::Cleanup(slide, a) => cleanup(
            m,
            &selection(m, (a as u8) % 3),
            &CleanupParams {
                slide,
                straight_angle_deg: a,
                short_edge: 0.02 * m.diagonal(),
                tolerance: 0.01 * m.diagonal(),
                ..CleanupParams::for_mesh(m)
            },
        ),
        Tool::Resample(k, n) => {
            let spacing = match n % 3 {
                0 => Spacing::Even,
                1 => Spacing::Count(n),
                _ => Spacing::EdgeLength(m.diagonal() / n as f64),
            };
            resample(
                m,
                &selection(m, k),
                &ResampleParams {
                    spacing,
                    ..Default::default()
                },
            )
        }
    }
}

/// Deterministic pseudo-random jitter of every vertex by up to `amount`
/// mean edge lengths (positions only; the baked normals stay analytic).
fn jitter(mut m: EditMesh, amount: f64) -> EditMesh {
    if amount <= 0.0 {
        return m;
    }
    let edges = m.edges();
    let mean = edges
        .iter()
        .map(|&(a, b)| m.positions[a as usize].distance(m.positions[b as usize]))
        .sum::<f64>()
        / edges.len() as f64;
    for (i, p) in m.positions.iter_mut().enumerate() {
        let h = |k: f64| ((i as f64 * k).sin() * 43758.5453).fract();
        *p += glam::DVec3::new(h(12.9898), h(78.233), h(37.719)) * amount * mean;
    }
    m
}

proptest! {
    // MESHEDIT_CASES=1000 for a longer local soak.
    #![proptest_config(ProptestConfig {
        cases: std::env::var("MESHEDIT_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(64),
        ..ProptestConfig::default()
    })]

    /// Every tool either refuses with a reason or returns a valid,
    /// unflipped mesh with the same topology, deterministically.
    #[test]
    fn random_tool_sequences(
        shape in arb_shape(),
        amount in prop_oneof![Just(0.0), 0.0f64..0.25],
        tools in proptest::collection::vec(arb_tool(), 1..5),
    ) {
        let start = jitter(build(&shape), amount);
        prop_assume!(start.issues().is_empty() && start.flipped_polygons().is_empty());
        let mut m = start.clone();
        for t in &tools {
            match run_tool(&m, t) {
                Ok(r) => {
                    assert_sound(&m, &r.mesh, &format!("{shape:?} {t:?}"));
                    let again = run_tool(&m, t).unwrap();
                    prop_assert_eq!(&again.mesh, &r.mesh, "not deterministic");
                    if let Tool::Cleanup(..) = t {
                        let d = vertex_deviation(&m, &r.mesh);
                        prop_assert!(d <= 0.01 * m.diagonal() + 1e-12, "cleanup deviation {}", d);
                    }
                    if let Tool::Decimate(..) = t {
                        prop_assert!(r.mesh.tri_count() <= m.tri_count());
                        prop_assert!(vertex_deviation(&r.mesh, &m) < 1e-9, "decimate keeps vertices on the input");
                    }
                    m = r.mesh;
                }
                Err(e) => prop_assert!(!e.reason.is_empty()),
            }
        }
        prop_assert_eq!(start.topology(), m.topology());
    }

    /// Decimating reaches the target (or reports why not) and never
    /// increases the triangle count.
    #[test]
    fn decimate_hits_target(shape in arb_shape(), ratio in 0.05f64..1.0) {
        let m = build(&shape);
        let r = decimate(&m, &DecimateParams { target: Target::Ratio(ratio), ..Default::default() }).unwrap();
        let target = (ratio * m.tri_count() as f64).ceil() as usize;
        if r.report.reached_target {
            prop_assert!(r.mesh.tri_count() <= target);
            // One merge removes at most a vertex's fan.
            prop_assert!(r.mesh.tri_count() + 24 >= target.min(m.tri_count()), "overshoot: {} for {}", r.mesh.tri_count(), target);
        } else {
            prop_assert!(r.mesh.tri_count() > target);
        }
    }
}
