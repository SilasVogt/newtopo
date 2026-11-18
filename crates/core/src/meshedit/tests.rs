//! Unit tests for the mesh tools on synthetic meshes (SPEC §9.5.8).

use super::mesh::area_vec;
use super::synth::*;
use super::*;
use glam::DVec3;

/// Invariants every tool must keep.
pub(crate) fn assert_sound(input: &EditMesh, out: &EditMesh, what: &str) {
    let issues = out.issues();
    assert!(issues.is_empty(), "{what}: {issues:?}");
    let flipped = out.flipped_polygons();
    assert!(flipped.is_empty(), "{what}: flipped polygons {flipped:?}");
    assert_eq!(input.topology(), out.topology(), "{what}: topology changed");
    // Pinned vertices survive at the same position.
    for (i, &p) in input.pinned.iter().enumerate() {
        if p && input.polygons.iter().any(|q| q.verts.contains(&(i as u32))) {
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
    for p in &out.polygons {
        for n in &p.normals {
            assert!((n.length() - 1.0).abs() < 1e-9, "{what}: normal not unit");
        }
    }
}

/// [`assert_sound`] plus: every polygon of the result that breaks a shape
/// guarantee (`EditMesh::polygon_problems`) was already such a polygon of
/// the input, on the same vertices (a tool may leave it, or change it
/// without making it worse, but never make a new one).
pub(crate) fn assert_outcome(input: &EditMesh, r: &EditOutcome, what: &str) {
    assert_sound(input, &r.mesh, what);
    let old: std::collections::BTreeSet<Vec<u32>> = input
        .polygon_problems(60.0)
        .into_iter()
        .filter_map(|i| {
            let mut k: Vec<u32> = input.polygons[i]
                .verts
                .iter()
                .map(|&v| r.vertex_map[v as usize])
                .collect::<Option<Vec<u32>>>()?;
            k.sort_unstable();
            Some(k)
        })
        .collect();
    for i in r.mesh.polygon_problems(60.0) {
        let mut k = r.mesh.polygons[i].verts.clone();
        k.sort_unstable();
        assert!(
            old.contains(&k),
            "{what}: new bad polygon {i}: {:?}",
            r.mesh.polygon_positions(i)
        );
    }
}

fn grid_id(ny: usize, i: usize, j: usize) -> u32 {
    (i * (ny + 1) + j) as u32
}

/// Edges of grid row `j` (constant y) of a heightfield.
fn row(nx: usize, ny: usize, j: usize) -> Vec<[u32; 2]> {
    (0..nx)
        .map(|i| [grid_id(ny, i, j), grid_id(ny, i + 1, j)])
        .collect()
}

fn flagged_length(m: &EditMesh) -> f64 {
    m.flagged
        .iter()
        .map(|e| m.positions[e.a as usize].distance(m.positions[e.b as usize]))
        .sum()
}

/// Every flagged edge of a cube lies on a cube edge line.
fn on_cube_edges(m: &EditMesh, s: f64) -> bool {
    let extreme = |p: DVec3| {
        [p.x, p.y, p.z]
            .iter()
            .filter(|c| (c.abs() - s).abs() < 1e-9)
            .count()
    };
    m.flagged.iter().all(|e| {
        let (a, b) = (m.positions[e.a as usize], m.positions[e.b as usize]);
        extreme(a) >= 2
            && extreme(b) >= 2
            && [0, 1, 2]
                .iter()
                .filter(|&&c| (a[c].abs() - s).abs() < 1e-9 && (a[c] - b[c]).abs() < 1e-9)
                .count()
                >= 2
    })
}

fn collapse(ratio: f64) -> DecimateParams {
    DecimateParams {
        target: Target::Ratio(ratio),
        ..Default::default()
    }
}

// ------------------------------------------------------------------ decimate

#[test]
fn decimate_cube_keeps_shape_and_edges() {
    let m = cube(8, 1.0);
    let r = decimate(&m, &collapse(0.1)).unwrap();
    assert_outcome(&m, &r, "cube");
    assert!(r.report.reached_target, "{}", r.report.message);
    assert!(r.mesh.tri_count() <= (0.1 * m.tri_count() as f64).ceil() as usize);
    assert!(hausdorff(&m, &r.mesh) < 1e-9, "a cube stays a cube");
    assert!((flagged_length(&r.mesh) - 24.0).abs() < 1e-9);
    assert!(on_cube_edges(&r.mesh, 1.0));
    // Corner normals are still the exact face normals.
    for p in &r.mesh.polygons {
        let n = p.normals[0];
        assert!(p.normals.iter().all(|x| x.distance(n) < 1e-12));
        assert!(
            r.mesh
                .polygon_normal(r.mesh.polygons.iter().position(|q| q == p).unwrap())
                .dot(n)
                > 0.999
        );
    }
}

#[test]
fn decimate_cube_to_the_minimum() {
    let m = cube(4, 1.0);
    let r = decimate(&m, &collapse(0.0001)).unwrap();
    assert_outcome(&m, &r, "cube min");
    // Can't go below the 8 corners; each side is at worst a few polygons.
    assert!(r.mesh.stats().verts <= 16, "{:?}", r.mesh.stats());
    assert!(!r.report.reached_target);
    assert!(hausdorff(&m, &r.mesh) < 1e-9);
}

#[test]
fn decimate_sphere_quads_and_shape() {
    let m = sphere(32, 16, 1.0);
    let r = decimate(&m, &collapse(0.3)).unwrap();
    assert_outcome(&m, &r, "sphere");
    assert!(r.report.reached_target);
    let s = r.mesh.stats();
    let h = hausdorff(&m, &r.mesh);
    eprintln!("sphere 30%: {s:?} hausdorff {h:.4}");
    assert!(h < 0.08, "hausdorff {h}");
    assert!(s.quads * 2 >= s.faces, "mostly quads: {s:?}");
}

#[test]
fn decimate_without_quads_makes_triangles() {
    let m = sphere(24, 12, 1.0);
    let p = DecimateParams {
        keep_quads: false,
        ..collapse(0.3)
    };
    let r = decimate(&m, &p).unwrap();
    assert_outcome(&m, &r, "sphere tris");
    assert!(r.report.reached_target);
}

#[test]
fn decimate_respects_max_deviation() {
    let m = sphere(32, 16, 1.0);
    let tol = 0.01;
    let p = DecimateParams {
        max_deviation: Some(tol),
        ..collapse(0.05)
    };
    let r = decimate(&m, &p).unwrap();
    assert_outcome(&m, &r, "sphere tol");
    let d = vertex_deviation(&m, &r.mesh);
    assert!(d <= tol + 1e-12, "deviation {d}");
    assert!(!r.report.reached_target, "5% can't be reached within 0.01");
    assert!(r.mesh.tri_count() < m.tri_count());
}

#[test]
fn decimate_torus_keeps_genus() {
    let m = torus(32, 12, 3.0, 1.0);
    let r = decimate(&m, &collapse(0.25)).unwrap();
    assert_outcome(&m, &r, "torus");
    assert_eq!(r.mesh.topology().0, 0);
}

#[test]
fn decimate_cylinder_keeps_rims_and_seam_uvs() {
    let m = cylinder(32, 8, 1.0, 2.0);
    let r = decimate(&m, &collapse(0.25)).unwrap();
    assert_outcome(&m, &r, "cylinder");
    let out = &r.mesh;
    // Rims: flagged edges stay on the rim circles, all at the baked rim
    // vertices.
    for e in &out.flagged {
        if e.flags.cad {
            for v in [e.a, e.b] {
                let p = out.positions[v as usize];
                assert!((p.truncate().length() - 1.0).abs() < 1e-9);
                assert!(p.z.abs() < 1e-9 || (p.z - 2.0).abs() < 1e-9);
            }
        }
    }
    // The UV seam survives and side UVs are exact for the kept vertices.
    assert!(out.flagged.iter().any(|e| e.flags.seam));
    for p in &out.polygons {
        if p.group.0 != 0 {
            continue;
        }
        let uv = p.uvs.as_ref().unwrap();
        let us: Vec<f64> = uv.iter().map(|u| u.x).collect();
        let span = us.iter().cloned().fold(f64::MIN, f64::max)
            - us.iter().cloned().fold(f64::MAX, f64::min);
        assert!(
            span < 0.5,
            "polygon wraps across the seam in UV: {uv:?} at {:?}",
            p.verts
                .iter()
                .map(|&v| out.positions[v as usize])
                .collect::<Vec<_>>()
        );
        for (k, &v) in p.verts.iter().enumerate() {
            let q = out.positions[v as usize];
            let a = q.y.atan2(q.x).rem_euclid(std::f64::consts::TAU) / std::f64::consts::TAU;
            let du = (uv[k].x - a).abs();
            assert!(du < 1e-9 || (du - 1.0).abs() < 1e-9, "u {} vs {a}", uv[k].x);
            assert!((uv[k].y - q.z / 2.0).abs() < 1e-9);
        }
    }
}

#[test]
fn decimate_is_deterministic_and_stable() {
    let m = sphere(20, 10, 1.0);
    let a = decimate(&m, &collapse(0.4)).unwrap();
    let b = decimate(&m, &collapse(0.4)).unwrap();
    assert_eq!(a.mesh, b.mesh);
    // Ratio 1 changes nothing.
    let c = decimate(&a.mesh, &collapse(1.0)).unwrap();
    assert_eq!(c.mesh, a.mesh);
}

#[test]
fn decimate_vertex_map() {
    let m = cube(3, 1.0);
    let r = decimate(&m, &collapse(0.5)).unwrap();
    assert_eq!(r.vertex_map.len(), m.positions.len());
    for (i, o) in r.vertex_map.iter().enumerate() {
        if let Some(o) = o {
            assert_eq!(r.mesh.positions[*o as usize], m.positions[i]);
        }
    }
}

#[test]
fn planar_cube_becomes_six_quads() {
    let m = cube(6, 1.0);
    let p = DecimateParams {
        mode: DecimateMode::Planar,
        ..Default::default()
    };
    let r = decimate(&m, &p).unwrap();
    assert_outcome(&m, &r, "planar cube");
    let s = r.mesh.stats();
    assert_eq!((s.verts, s.quads, s.faces), (8, 6, 6), "{s:?}");
    assert!(hausdorff(&m, &r.mesh) < 1e-9);
}

#[test]
fn planar_leaves_curved_surfaces_alone() {
    let m = sphere(16, 8, 1.0);
    let p = DecimateParams {
        mode: DecimateMode::Planar,
        // Pole-fan triangles of a 16-segment sphere are ~4° apart.
        planar_angle_deg: 1.0,
        ..Default::default()
    };
    let r = decimate(&m, &p).unwrap();
    assert_outcome(&m, &r, "planar sphere");
    assert_eq!(r.mesh.stats(), m.stats());
}

#[test]
fn planar_cylinder_caps_merge() {
    let m = cylinder(16, 2, 1.0, 2.0);
    let p = DecimateParams {
        mode: DecimateMode::Planar,
        ..Default::default()
    };
    let r = decimate(&m, &p).unwrap();
    assert_outcome(&m, &r, "planar cylinder");
    // Side columns merge top to bottom (the middle ring dissolves) and
    // the caps lose their centre vertex: only the two 16-vertex rims stay.
    assert_eq!(r.mesh.stats().verts, 32);
    assert!(r.mesh.stats().ngons == 0);
}

// ------------------------------------------------------------------ refusals

fn tri(verts: Vec<u32>) -> EditPolygon {
    EditPolygon {
        normals: vec![DVec3::Z; verts.len()],
        verts,
        group: newtopo_kernel::FaceId(0),
        uvs: None,
    }
}

fn raw(positions: Vec<DVec3>, polys: Vec<Vec<u32>>) -> EditMesh {
    EditMesh {
        pinned: vec![false; positions.len()],
        positions,
        polygons: polys.into_iter().map(tri).collect(),
        ..Default::default()
    }
}

#[test]
fn degenerate_inputs_are_refused() {
    let p = |x: f64, y: f64| DVec3::new(x, y, 0.0);
    let cases = vec![
        ("empty", raw(vec![], vec![])),
        (
            "three polygons on one edge",
            raw(
                vec![p(0., 0.), p(1., 0.), p(0., 1.), p(0., -1.), p(1., 1.)],
                vec![vec![0, 1, 2], vec![1, 0, 3], vec![0, 1, 4]],
            ),
        ),
        (
            "inconsistent winding",
            raw(
                vec![p(0., 0.), p(1., 0.), p(0., 1.), p(1., 1.)],
                vec![vec![0, 1, 2], vec![1, 2, 3]],
            ),
        ),
        (
            "repeated vertex",
            raw(
                vec![p(0., 0.), p(1., 0.), p(0., 1.)],
                vec![vec![0, 1, 2, 1]],
            ),
        ),
        (
            "zero area",
            raw(vec![p(0., 0.), p(1., 0.), p(2., 0.)], vec![vec![0, 1, 2]]),
        ),
        (
            "missing vertex",
            raw(vec![p(0., 0.), p(1., 0.)], vec![vec![0, 1, 2]]),
        ),
        (
            "bow-tie vertex",
            raw(
                vec![p(0., 0.), p(1., 0.), p(1., 1.), p(-1., 0.), p(-1., -1.)],
                vec![vec![0, 1, 2], vec![0, 3, 4]],
            ),
        ),
    ];
    for (what, m) in cases {
        let r = decimate(&m, &DecimateParams::default());
        assert!(r.is_err(), "{what}: accepted");
        let reason = r.unwrap_err().reason;
        assert!(!reason.is_empty());
        assert!(
            cleanup(&m, &Selection::AllFeatures, &CleanupParams::default()).is_err(),
            "{what}"
        );
        assert!(
            resample(&m, &Selection::AllFeatures, &ResampleParams::default()).is_err(),
            "{what}"
        );
    }
}

#[test]
fn bad_parameters_are_refused() {
    let m = cube(2, 1.0);
    for t in [Target::Ratio(0.0), Target::Ratio(1.5), Target::Triangles(0)] {
        let p = DecimateParams {
            target: t,
            ..Default::default()
        };
        assert!(decimate(&m, &p).is_err(), "{t:?}");
    }
    let p = DecimateParams {
        max_deviation: Some(-1.0),
        ..Default::default()
    };
    assert!(decimate(&m, &p).is_err());
    let p = CleanupParams {
        tolerance: -1.0,
        ..Default::default()
    };
    assert!(cleanup(&m, &Selection::AllFeatures, &p).is_err());
    assert!(
        cleanup(&m, &Selection::Edges(vec![]), &CleanupParams::default()).is_err(),
        "empty selection"
    );
    assert!(
        cleanup(
            &m,
            &Selection::Edges(vec![[0, 999]]),
            &CleanupParams::default()
        )
        .is_err(),
        "not an edge"
    );
    for s in [
        Spacing::Count(1),
        Spacing::EdgeLength(0.0),
        Spacing::EdgeLength(f64::NAN),
    ] {
        let p = ResampleParams {
            spacing: s,
            ..Default::default()
        };
        assert!(resample(&m, &Selection::AllFeatures, &p).is_err(), "{s:?}");
    }
    // A plane without CAD edges still has its boundary as a feature.
    assert!(
        resample(
            &plane(2, 1.0),
            &Selection::AllFeatures,
            &ResampleParams::default()
        )
        .is_ok()
    );
}

#[test]
fn decimate_ratio_one_is_a_no_op_on_triangles() {
    // Two triangles that pairing would join into a quad.
    let p = |x: f64, y: f64| DVec3::new(x, y, 0.0);
    let m = raw(
        vec![p(0., 0.), p(1., 0.), p(1., 1.), p(0., 1.)],
        vec![vec![0, 1, 2], vec![0, 2, 3]],
    );
    let r = decimate(&m, &collapse(1.0)).unwrap();
    assert_eq!(r.mesh, m);
    assert!(r.report.reached_target);
}

#[test]
fn planar_keeps_groups_apart() {
    // Without CAD/hard preservation the group border is no feature edge,
    // but polygons of different groups must still not merge.
    let mut m = plane(4, 1.0);
    for (i, q) in m.polygons.iter_mut().enumerate() {
        q.group = newtopo_kernel::FaceId((i % 2) as u32);
    }
    let p = DecimateParams {
        mode: DecimateMode::Planar,
        preserve: Preserve {
            cad_edges: false,
            hard_edges: false,
        },
        ..Default::default()
    };
    let r = decimate(&m, &p).unwrap();
    assert_outcome(&m, &r, "planar groups");
    let area = |m: &EditMesh, g: u32| -> f64 {
        (0..m.polygons.len())
            .filter(|&i| m.polygons[i].group.0 == g)
            .map(|i| area_vec(&m.polygon_positions(i)).length() / 2.0)
            .sum()
    };
    for g in 0..2 {
        assert!((area(&m, g) - area(&r.mesh, g)).abs() < 1e-12, "group {g}");
    }
}

#[test]
fn planar_never_removes_a_flagged_edge_in_a_shared_chain() {
    // Two pentagons share the chain 4-5-6; only 5-6 is hard.
    let p = |x: f64, y: f64| DVec3::new(x, y, 0.0);
    let mut m = raw(
        vec![
            p(0., 0.),
            p(1., 0.),
            p(1., 1.),
            p(0., 1.),
            p(0.5, 0.),
            p(0.5, 0.5),
            p(0.5, 1.),
        ],
        vec![vec![0, 4, 5, 6, 3], vec![4, 1, 2, 6, 5]],
    );
    m.flagged = vec![FlaggedEdge {
        a: 5,
        b: 6,
        flags: EdgeFlags {
            hard: true,
            ..EdgeFlags::NONE
        },
    }];
    let p = DecimateParams {
        mode: DecimateMode::Planar,
        ..Default::default()
    };
    let r = decimate(&m, &p).unwrap();
    assert_outcome(&m, &r, "shared chain");
    let at = |q: DVec3| r.mesh.positions.iter().position(|&x| x.distance(q) < 1e-12);
    let (a, b) = (at(DVec3::new(0.5, 0.5, 0.0)), at(DVec3::new(0.5, 1.0, 0.0)));
    let (a, b) = (a.expect("5 kept") as u32, b.expect("6 kept") as u32);
    assert!(r.mesh.flags(a, b).hard, "hard edge lost");
}

// ------------------------------------------------------------------- cleanup

/// Plane `nx × ny` with row `j` zig-zagging by `amp` in y.
fn zigzag(nx: usize, ny: usize, j: usize, amp: f64) -> EditMesh {
    let mut m = heightfield(nx, ny, 1.0, |_, _| 0.0);
    for i in 1..nx {
        let v = grid_id(ny, i, j) as usize;
        m.positions[v].y += if i % 2 == 0 { amp } else { -amp };
    }
    m
}

#[test]
fn cleanup_straightens_and_thins_a_jagged_run() {
    let (nx, ny) = (24, 6);
    let m = zigzag(nx, ny, 3, 0.01);
    let sel = Selection::Edges(row(nx, ny, 3));
    let p = CleanupParams {
        tolerance: 0.02,
        ..CleanupParams::for_mesh(&m)
    };
    let r = cleanup(&m, &sel, &p).unwrap();
    assert_outcome(&m, &r, "zigzag");
    // The plane is flat, so nothing left the plane.
    assert!(hausdorff(&m, &r.mesh) < 1e-9);
    // The run is straight now and has fewer vertices.
    let mut run_verts = std::collections::BTreeSet::new();
    for e in &r.selection {
        run_verts.insert(e[0]);
        run_verts.insert(e[1]);
    }
    assert!(!r.selection.is_empty());
    for &v in &run_verts {
        let y = r.mesh.positions[v as usize].y;
        // Within 1 % of the original ±1e-2 zig-zag (the shape guard may
        // stop the last micro-slides).
        assert!((y - 0.5).abs() < 1e-4, "run vertex off the line: {y}");
    }
    assert!(run_verts.len() < nx + 1, "{} run vertices", run_verts.len());
    eprintln!("{}", r.report.message);
}

#[test]
fn cleanup_keeps_a_curved_surface_within_tolerance() {
    let (nx, ny) = (20, 8);
    let mut m = heightfield(nx, ny, 1.0, |x, y| 0.2 * (3.0 * x).sin() * (2.0 * y).cos());
    for i in 1..nx {
        let v = grid_id(ny, i, 4) as usize;
        m.positions[v].y += if i % 2 == 0 { 0.008 } else { -0.008 };
        let p = m.positions[v];
        m.positions[v].z = 0.2 * (3.0 * p.x).sin() * (2.0 * p.y).cos();
    }
    let tol = 0.004;
    let p = CleanupParams {
        tolerance: tol,
        straight_angle_deg: 20.0,
        ..CleanupParams::for_mesh(&m)
    };
    let r = cleanup(&m, &Selection::Edges(row(nx, ny, 4)), &p).unwrap();
    assert_outcome(&m, &r, "curved");
    let d = vertex_deviation(&m, &r.mesh);
    assert!(d <= tol + 1e-12, "deviation {d}");
}

#[test]
fn cleanup_merges_a_sliver_row() {
    let (nx, ny) = (10, 6);
    let mut m = heightfield(nx, ny, 1.0, |_, _| 0.0);
    // Row 4 sits 1e-5 above row 3.
    for i in 0..=nx {
        let v = grid_id(ny, i, 4) as usize;
        m.positions[v].y = 0.5 + 1e-5;
    }
    let p = CleanupParams {
        merge_distance: 1e-4,
        slide: false,
        straight_angle_deg: 0.0,
        short_edge: 0.0,
        ..CleanupParams::for_mesh(&m)
    };
    let r = cleanup(&m, &Selection::Edges(row(nx, ny, 3)), &p).unwrap();
    assert_outcome(&m, &r, "sliver");
    assert_eq!(r.mesh.stats().verts, m.stats().verts - (nx + 1));
    assert!(r.mesh.edges().iter().all(|&(a, b)| {
        r.mesh.positions[a as usize].distance(r.mesh.positions[b as usize]) > 1e-4
    }));
}

#[test]
fn cleanup_collapses_short_run_edges() {
    let (nx, ny) = (12, 4);
    let mut m = heightfield(nx, ny, 1.0, |_, _| 0.0);
    // Column 6 nearly on column 5 in the run row only.
    let v = grid_id(ny, 6, 2) as usize;
    m.positions[v].x = m.positions[grid_id(ny, 5, 2) as usize].x + 1e-3;
    let p = CleanupParams {
        short_edge: 5e-3,
        slide: false,
        straight_angle_deg: 0.0,
        ..CleanupParams::for_mesh(&m)
    };
    let r = cleanup(&m, &Selection::Edges(row(nx, ny, 2)), &p).unwrap();
    assert_outcome(&m, &r, "short");
    assert_eq!(r.mesh.stats().verts, m.stats().verts - 1);
    assert_eq!(r.selection.len(), nx - 1);
}

#[test]
fn cleanup_is_idempotent() {
    let (nx, ny) = (16, 6);
    let m = zigzag(nx, ny, 3, 0.01);
    let sel = Selection::Edges(row(nx, ny, 3));
    for slide in [false, true] {
        let p = CleanupParams {
            slide,
            tolerance: 0.02,
            ..CleanupParams::for_mesh(&m)
        };
        let a = cleanup(&m, &sel, &p).unwrap();
        let b = cleanup(&a.mesh, &Selection::Edges(a.selection.clone()), &p).unwrap();
        assert_eq!(a.mesh, b.mesh, "slide {slide}: {}", b.report.message);
        assert_eq!(a.selection, b.selection);
    }
}

#[test]
fn cleanup_cube_edges_keeps_the_cube() {
    let m = cube(6, 1.0);
    let p = CleanupParams::for_mesh(&m);
    let r = cleanup(&m, &Selection::AllFeatures, &p).unwrap();
    assert_outcome(&m, &r, "cube cleanup");
    assert!(hausdorff(&m, &r.mesh) < 1e-9);
    assert!((flagged_length(&r.mesh) - 24.0).abs() < 1e-9);
    assert!(on_cube_edges(&r.mesh, 1.0));
    assert!(r.mesh.stats().verts < m.stats().verts);
}

// ------------------------------------------------------------------ resample

/// Plane with the columns of row `j` unevenly spaced.
fn uneven(nx: usize, ny: usize, j: usize) -> EditMesh {
    let mut m = heightfield(nx, ny, 1.0, |_, _| 0.0);
    for i in 1..nx {
        let v = grid_id(ny, i, j) as usize;
        let x = i as f64 / nx as f64;
        m.positions[v].x = x + 0.3 / nx as f64 * (i as f64 * 1.7).sin();
    }
    m
}

fn run_lengths(m: &EditMesh, sel: &[[u32; 2]]) -> Vec<f64> {
    sel.iter()
        .map(|e| m.positions[e[0] as usize].distance(m.positions[e[1] as usize]))
        .collect()
}

#[test]
fn resample_even_spacing_and_idempotent() {
    let (nx, ny) = (12, 6);
    let m = uneven(nx, ny, 3);
    let r = resample(
        &m,
        &Selection::Edges(row(nx, ny, 3)),
        &ResampleParams::default(),
    )
    .unwrap();
    assert_outcome(&m, &r, "even");
    let l = run_lengths(&r.mesh, &r.selection);
    assert_eq!(l.len(), nx);
    for x in &l {
        assert!((x - 1.0 / nx as f64).abs() < 1e-9, "{l:?}");
    }
    let again = resample(
        &r.mesh,
        &Selection::Edges(r.selection.clone()),
        &ResampleParams::default(),
    )
    .unwrap();
    assert_eq!(again.mesh, r.mesh);
}

#[test]
fn resample_to_more_and_fewer_vertices() {
    let (nx, ny) = (10, 6);
    let m = heightfield(nx, ny, 1.0, |_, _| 0.0);
    for (count, edges) in [(21u32, 20usize), (6, 5), (11, 10)] {
        let p = ResampleParams {
            spacing: Spacing::Count(count),
            ..Default::default()
        };
        let r = resample(&m, &Selection::Edges(row(nx, ny, 3)), &p).unwrap();
        assert_outcome(&m, &r, &format!("count {count}"));
        let l = run_lengths(&r.mesh, &r.selection);
        assert_eq!(l.len(), edges, "count {count}");
        for x in &l {
            assert!((x - 1.0 / edges as f64).abs() < 1e-9);
        }
        assert_eq!(r.mesh.stats().ngons, 0, "n-gons refilled");
        assert!(hausdorff(&m, &r.mesh) < 1e-9);
    }
}

#[test]
fn resample_by_edge_length() {
    let (nx, ny) = (10, 4);
    let m = heightfield(nx, ny, 1.0, |_, _| 0.0);
    let p = ResampleParams {
        spacing: Spacing::EdgeLength(0.25),
        ..Default::default()
    };
    let r = resample(&m, &Selection::Edges(row(nx, ny, 2)), &p).unwrap();
    assert_outcome(&m, &r, "length");
    assert_eq!(r.selection.len(), 4);
}

#[test]
fn resample_keeps_corners() {
    let (nx, ny) = (8, 8);
    let m = plane(nx, 1.0);
    // L-shaped run: along row 2 from column 0 to 5, then up column 5.
    let mut sel: Vec<[u32; 2]> = (0..5)
        .map(|i| [grid_id(ny, i, 2), grid_id(ny, i + 1, 2)])
        .collect();
    sel.extend((2..6).map(|j| [grid_id(ny, 5, j), grid_id(ny, 5, j + 1)]));
    let corner = m.positions[grid_id(ny, 5, 2) as usize];
    let p = ResampleParams {
        spacing: Spacing::Count(6),
        ..Default::default()
    };
    let r = resample(&m, &Selection::Edges(sel.clone()), &p).unwrap();
    assert_outcome(&m, &r, "L");
    assert!(r.mesh.positions.iter().any(|q| q.distance(corner) < 1e-12));
    assert_eq!(r.selection.len(), 5);
    // Two fixed ends and a corner: a 2-vertex count can't work.
    let p = ResampleParams {
        spacing: Spacing::Count(2),
        ..Default::default()
    };
    let e = resample(&m, &Selection::Edges(sel), &p).unwrap_err();
    assert!(e.reason.contains("corner"), "{}", e.reason);
}

#[test]
fn resample_closed_ring_on_cylinder() {
    let mut m = cylinder(24, 4, 1.0, 2.0);
    // Jitter the middle ring around the axis (still on the circle).
    let ring: Vec<u32> = (0..m.positions.len() as u32)
        .filter(|&v| {
            (m.positions[v as usize].z - 1.0).abs() < 1e-9
                && m.positions[v as usize].truncate().length() > 0.5
        })
        .collect();
    assert_eq!(ring.len(), 24);
    for (k, &v) in ring.iter().enumerate() {
        let p = m.positions[v as usize];
        let a = p.y.atan2(p.x) + 0.05 * (k as f64 * 2.3).sin();
        m.positions[v as usize] = DVec3::new(a.cos(), a.sin(), 1.0);
    }
    let edges: Vec<[u32; 2]> = m
        .edges()
        .into_iter()
        .filter(|&(a, b)| ring.contains(&a) && ring.contains(&b))
        .map(|(a, b)| [a, b])
        .collect();
    assert_eq!(edges.len(), 24);
    let p = ResampleParams {
        corner_angle_deg: 60.0,
        ..Default::default()
    };
    let r = resample(&m, &Selection::Edges(edges.clone()), &p).unwrap();
    assert_outcome(&m, &r, "ring even");
    // Even by arc length along the old polygon; chords differ slightly
    // where a new vertex straddles an old corner.
    let l = run_lengths(&r.mesh, &r.selection);
    let mean = l.iter().sum::<f64>() / l.len() as f64;
    assert!(l.iter().all(|x| (x - mean).abs() < 0.01 * mean), "{l:?}");
    let p = ResampleParams {
        spacing: Spacing::Count(36),
        corner_angle_deg: 60.0,
        ..Default::default()
    };
    let r = resample(&m, &Selection::Edges(edges), &p).unwrap();
    assert_outcome(&m, &r, "ring 36");
    assert_eq!(r.selection.len(), 36);
    // UVs: no side polygon wraps across the seam, and every side corner's
    // u follows its angle (the input ring was jittered by up to 0.05 rad
    // with its UVs left in place, so within that).
    for q in r.mesh.polygons.iter().filter(|q| q.group.0 == 0) {
        let uv = q.uvs.as_ref().unwrap();
        let us: Vec<f64> = uv.iter().map(|u| u.x).collect();
        let span = us.iter().cloned().fold(f64::MIN, f64::max)
            - us.iter().cloned().fold(f64::MAX, f64::min);
        assert!(span < 0.5, "wraps across the seam: {uv:?}");
        for (k, &v) in q.verts.iter().enumerate() {
            let p = r.mesh.positions[v as usize];
            let a = p.y.atan2(p.x).rem_euclid(std::f64::consts::TAU) / std::f64::consts::TAU;
            let du = (uv[k].x - a).abs();
            let du = du.min((du - 1.0).abs());
            assert!(
                du < 0.05 / std::f64::consts::TAU + 1e-3,
                "u {} vs angle {a}",
                uv[k].x
            );
        }
    }
}

#[test]
fn resample_refills_triangles_beside_the_run() {
    // Cap triangles beside each rim: one new vertex per rim edge turns them
    // into quads with a straight corner until they are refilled.
    let m = cylinder(24, 3, 1.0, 2.0);
    let p = ResampleParams {
        spacing: Spacing::Count(48),
        ..Default::default()
    };
    let r = resample(&m, &Selection::AllFeatures, &p).unwrap();
    assert_outcome(&m, &r, "rims 48");
    assert!(r.report.skipped.is_empty(), "{:?}", r.report.skipped);
    assert!(r.mesh.stats().verts >= m.stats().verts + 48);
}

#[test]
fn resample_refuses_huge_counts() {
    let m = plane(4, 1.0);
    let sel = Selection::Edges(row(4, 4, 2));
    for spacing in [Spacing::Count(1_000_000), Spacing::EdgeLength(1e-12)] {
        let p = ResampleParams {
            spacing,
            ..Default::default()
        };
        let err = resample(&m, &sel, &p).unwrap_err();
        assert!(err.reason.contains("more than"), "{}", err.reason);
    }
}

#[test]
fn resample_cad_rims_count() {
    let m = cylinder(24, 3, 1.0, 2.0);
    let p = ResampleParams {
        spacing: Spacing::Count(12),
        corner_angle_deg: 60.0,
        ..Default::default()
    };
    let r = resample(&m, &Selection::AllFeatures, &p).unwrap();
    assert_outcome(&m, &r, "rims");
    let rim: Vec<_> = r.mesh.flagged.iter().filter(|e| e.flags.cad).collect();
    assert_eq!(rim.len(), 24, "two rims of 12");
    for e in rim {
        for v in [e.a, e.b] {
            let p = r.mesh.positions[v as usize];
            // On the old rim polygon: between the chord and the circle.
            let rad = p.truncate().length();
            let sag = 1.0 - (std::f64::consts::PI / 24.0).cos();
            assert!(rad <= 1.0 + 1e-9 && rad >= 1.0 - sag - 1e-9, "{rad}");
        }
    }
}

#[test]
fn resample_cube_features_stay_on_edges() {
    let m = cube(5, 1.0);
    let p = ResampleParams {
        spacing: Spacing::EdgeLength(0.5),
        ..Default::default()
    };
    let r = resample(&m, &Selection::AllFeatures, &p).unwrap();
    assert_outcome(&m, &r, "cube resample");
    assert!(on_cube_edges(&r.mesh, 1.0));
    assert!((flagged_length(&r.mesh) - 24.0).abs() < 1e-9);
    // 12 cube edges of length 2, 4 edges each.
    assert_eq!(r.mesh.flagged.len(), 48);
    assert!(hausdorff(&m, &r.mesh) < 1e-9);
}

// -------------------------------------------------------------- vertex slide

fn toward(x: u32, y: u32) -> SlideEdge {
    SlideEdge::Toward(vec![[x, y]])
}

fn slide(m: &EditMesh, e: &SlideEdge, factor: f64) -> Result<EditOutcome, crate::ops::Refusal> {
    vertex_slide(
        m,
        e,
        &SlideParams {
            factor,
            ..Default::default()
        },
    )
}

#[test]
fn slide_factor_zero_is_identity() {
    let m = sphere(12, 6, 1.0);
    let e = SlideEdge::Direction {
        vertices: vec![20, 21, 33],
        dir: DVec3::X,
    };
    let r = slide(&m, &e, 0.0).unwrap();
    assert_eq!(r.mesh, m);
    assert_eq!(
        r.vertex_map,
        (0..m.positions.len() as u32).map(Some).collect::<Vec<_>>()
    );
    assert_eq!(r.selected_vertices, vec![20, 21, 33]);
}

#[test]
fn slide_moves_along_the_edge() {
    let m = plane(4, 1.0);
    let (x, y) = (grid_id(4, 2, 2), grid_id(4, 3, 2));
    let (px, py) = (m.positions[x as usize], m.positions[y as usize]);
    for t in [0.25, 0.5, 0.9] {
        let r = slide(&m, &toward(x, y), t).unwrap();
        assert_outcome(&m, &r, "slide");
        assert!(r.report.reached_target);
        let nx = r.vertex_map[x as usize].unwrap() as usize;
        assert!(r.mesh.positions[nx].distance(px.lerp(py, t)) < 1e-12);
        // Nothing else moves.
        for v in 0..m.positions.len() {
            if v as u32 != x {
                let o = r.vertex_map[v].unwrap() as usize;
                assert_eq!(r.mesh.positions[o], m.positions[v]);
            }
        }
        assert_eq!(
            r.selection,
            vec![[nx as u32, r.vertex_map[y as usize].unwrap()]]
        );
    }
}

#[test]
fn slide_to_one_lands_on_the_neighbour() {
    let m = plane(4, 1.0);
    let (x, y) = (grid_id(4, 2, 2), grid_id(4, 3, 2));
    let r = slide(&m, &toward(x, y), 1.0).unwrap();
    assert_outcome(&m, &r, "slide to 1");
    assert_eq!(r.vertex_map[x as usize], None, "merged into the neighbour");
    let ny = r.vertex_map[y as usize].unwrap() as usize;
    assert_eq!(r.mesh.positions[ny], m.positions[y as usize]);
    assert_eq!(r.mesh.stats().verts, m.stats().verts - 1);
    assert_eq!(r.selected_vertices, vec![ny as u32]);
    // Clamped: anything above 1 is 1.
    let r2 = slide(&m, &toward(x, y), 3.0).unwrap();
    assert_eq!(r2.mesh, r.mesh);
}

#[test]
fn slide_unclamped_extends_and_stops_before_folding() {
    let m = plane(4, 1.0);
    let (x, y) = (grid_id(4, 2, 2), grid_id(4, 3, 2));
    let p = SlideParams {
        factor: -0.5,
        clamp: false,
        ..Default::default()
    };
    let r = vertex_slide(&m, &toward(x, y), &p).unwrap();
    assert_outcome(&m, &r, "extend");
    assert!(r.report.reached_target);
    let nx = r.vertex_map[x as usize].unwrap() as usize;
    assert!((r.mesh.positions[nx].x - 0.375).abs() < 1e-12);
    // Clamped, the same factor stays at 0.
    let c = vertex_slide(&m, &toward(x, y), &SlideParams { clamp: true, ..p }).unwrap();
    assert_eq!(c.mesh, m);
    // Far beyond the opposite neighbour (x = 0.25) the quads would fold:
    // the slide stops short of it and says so.
    let far = vertex_slide(&m, &toward(x, y), &SlideParams { factor: -3.0, ..p }).unwrap();
    assert_outcome(&m, &far, "fold limit");
    assert!(!far.report.reached_target, "{}", far.report.message);
    let fx = far.mesh.positions[far.vertex_map[x as usize].unwrap() as usize].x;
    assert!(fx > 0.25 && fx < 0.26, "{fx}");
}

#[test]
fn slide_even_moves_the_same_distance() {
    let mut m = plane(4, 4.0);
    // Column 2 shifted right: (1, 1) → (2, 1) is 1.5 long, (2, 3) → (3, 3)
    // is 0.5 long.
    for j in 0..=4 {
        m.positions[grid_id(4, 2, j) as usize].x = 2.5;
    }
    let (a, b) = (grid_id(4, 1, 1), grid_id(4, 2, 3));
    let e = SlideEdge::Direction {
        vertices: vec![a, b],
        dir: DVec3::X,
    };
    let plan = SlidePlan::new(&m, &e, Preserve::default(), 60.0).unwrap();
    let even = plan.apply(0.5, true, true).unwrap();
    let rel = plan.apply(0.5, false, true).unwrap();
    let dx = |r: &EditOutcome, v: u32| {
        r.mesh.positions[r.vertex_map[v as usize].unwrap() as usize].x - m.positions[v as usize].x
    };
    assert!((dx(&even, a) - 0.25).abs() < 1e-12 && (dx(&even, b) - 0.25).abs() < 1e-12);
    assert!((dx(&rel, a) - 0.75).abs() < 1e-12 && (dx(&rel, b) - 0.25).abs() < 1e-12);
    // The preview positions agree with the result.
    for (v, p) in plan.positions(0.5, true, true) {
        assert_eq!(
            even.mesh.positions[even.vertex_map[v as usize].unwrap() as usize],
            p
        );
    }
}

#[test]
fn slide_boundary_vertices_stay_on_the_boundary() {
    let m = plane(4, 1.0);
    let x = grid_id(4, 0, 2); // on the x = 0 border
    let e = SlideEdge::Direction {
        vertices: vec![x],
        dir: DVec3::new(1.0, 0.1, 0.0), // into the plane, a little upwards
    };
    let r = slide(&m, &e, 0.5).unwrap();
    assert_outcome(&m, &r, "boundary slide");
    let p = r.mesh.positions[r.vertex_map[x as usize].unwrap() as usize];
    assert_eq!(p.x, 0.0);
    assert!((p.y - 0.625).abs() < 1e-12, "{p}");
    // Explicitly towards the interior: refused with a reason.
    let err = slide(&m, &toward(x, grid_id(4, 1, 2)), 0.5).unwrap_err();
    assert!(err.reason.contains("feature"), "{}", err.reason);
}

#[test]
fn slide_feature_vertices_stay_on_their_feature_unless_overridden() {
    let m = cube(4, 1.0);
    // A vertex in the middle of a cube edge: on a CAD + hard curve.
    let x = (0..m.positions.len() as u32)
        .find(|&v| {
            let p = m.positions[v as usize];
            !m.pinned[v as usize]
                && [p.x, p.y, p.z]
                    .iter()
                    .filter(|c| (c.abs() - 1.0).abs() < 1e-12)
                    .count()
                    == 2
        })
        .unwrap();
    let e = |dir| SlideEdge::Direction {
        vertices: vec![x],
        dir,
    };
    let p = m.positions[x as usize];
    // Pointing at the cube centre: the best feature edge is chosen.
    let r = slide(&m, &e(-p), 0.5).unwrap();
    assert_outcome(&m, &r, "feature slide");
    assert!(on_cube_edges(&r.mesh, 1.0));
    assert!(hausdorff(&m, &r.mesh) < 1e-9);
    assert!((flagged_length(&r.mesh) - flagged_length(&m)).abs() < 1e-9);
    // With CAD and hard edges not preserved it may leave the edge.
    let free = vertex_slide(
        &m,
        &e(-p),
        &SlideParams {
            preserve: Preserve {
                cad_edges: false,
                hard_edges: false,
            },
            ..Default::default()
        },
    )
    .unwrap();
    assert_outcome(&m, &free, "override");
    let q = free.mesh.positions[free.vertex_map[x as usize].unwrap() as usize];
    let extreme = [q.x, q.y, q.z]
        .iter()
        .filter(|c| (c.abs() - 1.0).abs() < 1e-9)
        .count();
    assert_eq!(extreme, 1, "left the cube edge onto a face: {q}");
    // Corners (pinned) never slide.
    let corner = m.pinned.iter().position(|&p| p).unwrap() as u32;
    let err = slide(
        &m,
        &SlideEdge::Direction {
            vertices: vec![corner],
            dir: DVec3::X,
        },
        0.5,
    )
    .unwrap_err();
    assert!(err.reason.contains("corner"), "{}", err.reason);
}

#[test]
fn slide_refusals_and_skips() {
    let m = plane(3, 1.0);
    let (x, y) = (grid_id(3, 1, 1), grid_id(3, 2, 1));
    assert!(slide(&m, &SlideEdge::Toward(vec![]), 0.5).is_err());
    assert!(
        slide(&m, &toward(x, grid_id(3, 3, 3)), 0.5).is_err(),
        "not an edge"
    );
    assert!(
        slide(
            &m,
            &SlideEdge::Toward(vec![[x, y], [x, grid_id(3, 1, 2)]]),
            0.5
        )
        .is_err()
    );
    let zero = SlideEdge::Direction {
        vertices: vec![x],
        dir: DVec3::ZERO,
    };
    assert!(slide(&m, &zero, 0.5).is_err());
    assert!(slide(&m, &toward(x, y), f64::NAN).is_err());
    let p = SlideParams {
        max_normal_change_deg: 0.0,
        ..Default::default()
    };
    assert!(vertex_slide(&m, &toward(x, y), &p).is_err());
    // Two neighbours sliding into each other: both skipped, so refused.
    let err = slide(&m, &SlideEdge::Toward(vec![[x, y], [y, x]]), 0.5).unwrap_err();
    assert!(err.reason.contains("slides too"), "{}", err.reason);
    // With a third vertex that can slide, the pair is skipped and reported.
    let z = grid_id(3, 1, 2);
    let r = slide(
        &m,
        &SlideEdge::Toward(vec![[x, y], [y, x], [z, grid_id(3, 0, 2)]]),
        0.5,
    );
    let r = r.unwrap();
    assert_eq!(r.report.skipped.len(), 2, "{:?}", r.report.skipped);
}

#[test]
fn slide_plan_is_reusable_and_matches_one_shot() {
    let m = sphere(16, 8, 1.0);
    let e = SlideEdge::Direction {
        vertices: vec![40, 41, 42, 43],
        dir: DVec3::Z,
    };
    let plan = SlidePlan::new(&m, &e, Preserve::default(), 60.0).unwrap();
    for k in 0..=10 {
        let t = k as f64 / 10.0;
        let r = plan.apply(t, false, true).unwrap();
        assert_outcome(&m, &r, &format!("sphere slide {t}"));
        assert_eq!(r.mesh, slide(&m, &e, t).unwrap().mesh, "t = {t}");
        // Every vertex stays on the input surface (it moved along an edge).
        assert!(vertex_deviation(&r.mesh, &m) < 1e-9);
    }
}

// --------------------------------------------------------- merge by distance

/// Two `nx × ny` sheets on top of each other in y, the second shifted by
/// `1 + gap`: a straight crack along y = 1 with duplicated vertices.
fn cracked(nx: usize, ny: usize, gap: f64) -> EditMesh {
    let a = heightfield(nx, ny, 1.0, |_, _| 0.0);
    let mut b = a.clone();
    for p in &mut b.positions {
        p.y += 1.0 + gap;
    }
    let off = a.positions.len() as u32;
    let mut m = a.clone();
    m.positions.extend(b.positions);
    m.pinned.extend(b.pinned);
    for mut q in b.polygons {
        for v in &mut q.verts {
            *v += off;
        }
        m.polygons.push(q);
    }
    m
}

#[test]
fn merge_welds_a_crack() {
    let m = cracked(4, 2, 1e-6);
    let whole = heightfield(4, 4, 1.0, |_, _| 0.0);
    assert_eq!(m.topology(), (2, 2, 2));
    for mode in [MergeMode::All, MergeMode::Unconnected] {
        let p = MergeParams {
            mode,
            ..MergeParams::default()
        };
        let r = merge_by_distance(&m, &VertexScope::All, &p).unwrap();
        let out = &r.mesh;
        assert!(out.issues().is_empty(), "{:?}", out.issues());
        assert!(out.flipped_polygons().is_empty());
        assert_eq!(out.topology(), whole.topology(), "{mode:?}");
        assert_eq!(
            out.stats().verts,
            m.stats().verts - 5,
            "{}",
            r.report.message
        );
        assert!(
            r.report.message.contains("Removed 5"),
            "{}",
            r.report.message
        );
        // Centroid: the interior seam vertices sit halfway, the two seam
        // ends on the outer border keep the survivor's position.
        let seam: Vec<DVec3> = out
            .positions
            .iter()
            .copied()
            .filter(|p| (p.y - 1.0).abs() < 1e-5)
            .collect();
        assert_eq!(seam.len(), 5);
        for p in seam {
            let border = p.x == 0.0 || p.x == 1.0;
            let want = if border { 1.0 } else { 1.0 + 5e-7 };
            assert!((p.y - want).abs() < 1e-15, "{p}");
        }
        // Each side keeps its own UVs (u = x on both sheets), and the glued
        // edges are flagged as the UV seam they now are.
        for q in &out.polygons {
            for (k, &v) in q.verts.iter().enumerate() {
                let uv = q.uvs.as_ref().unwrap()[k];
                assert!(
                    (uv.x - out.positions[v as usize].x).abs() < 1e-12,
                    "{uv} at {v}"
                );
            }
        }
        assert_eq!(out.flagged.iter().filter(|e| e.flags.seam).count(), 4);
        // Idempotent.
        let again = merge_by_distance(out, &VertexScope::All, &p).unwrap();
        assert_eq!(&again.mesh, out);
    }
    // Survivor placement keeps the first sheet's seam exactly.
    let r = merge_by_distance(
        &m,
        &VertexScope::All,
        &MergeParams {
            position: MergePosition::Survivor,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(r.mesh.positions.iter().filter(|p| p.y == 1.0).count() == 5);
    // Connected only: the crack has no edges across it.
    let c = merge_by_distance(
        &m,
        &VertexScope::All,
        &MergeParams {
            mode: MergeMode::Connected,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(c.mesh.stats().verts, m.stats().verts);
}

#[test]
fn merge_scope_and_unselected() {
    let m = cracked(4, 2, 1e-6);
    let off = 15u32; // first vertex of the second sheet
    // Only the first sheet's seam selected: nothing pairs up...
    let seam_a: Vec<u32> = (0..5).map(|i| grid_id(2, i, 2)).collect();
    let r = merge_by_distance(
        &m,
        &VertexScope::Vertices(seam_a.clone()),
        &MergeParams::default(),
    )
    .unwrap();
    assert_eq!(r.mesh, m);
    // ...unless merging with unselected vertices is allowed.
    let r = merge_by_distance(
        &m,
        &VertexScope::Vertices(seam_a.clone()),
        &MergeParams {
            with_unselected: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(r.mesh.stats().verts, m.stats().verts - 5);
    assert_eq!(r.selected_vertices.len(), 5);
    // Both seams selected.
    let mut both = seam_a;
    both.extend((0..5).map(|i| off + grid_id(2, i, 0)));
    let r = merge_by_distance(&m, &VertexScope::Vertices(both), &MergeParams::default()).unwrap();
    assert_eq!(r.mesh.stats().verts, m.stats().verts - 5);
    assert!(
        merge_by_distance(&m, &VertexScope::Vertices(vec![]), &MergeParams::default()).is_err()
    );
    assert!(
        merge_by_distance(
            &m,
            &VertexScope::Vertices(vec![999]),
            &MergeParams::default()
        )
        .is_err()
    );
    for d in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let p = MergeParams {
            distance: d,
            ..Default::default()
        };
        assert!(merge_by_distance(&m, &VertexScope::All, &p).is_err(), "{d}");
    }
}

#[test]
fn merge_collapses_a_short_edge_and_keeps_topology() {
    let mut m = plane(4, 1.0);
    let (x, y) = (grid_id(4, 2, 2), grid_id(4, 3, 2));
    m.positions[x as usize].x = m.positions[y as usize].x - 1e-5;
    let p = MergeParams {
        mode: MergeMode::Connected,
        ..MergeParams::for_mesh(&m)
    };
    let r = merge_by_distance(&m, &VertexScope::All, &p).unwrap();
    assert_outcome(&m, &r, "short edge");
    assert_eq!(r.mesh.stats().verts, m.stats().verts - 1);
    // The lower index survives, moved to the middle of the pair.
    let nx = r.vertex_map[x as usize].unwrap() as usize;
    assert_eq!(r.vertex_map[y as usize], None);
    assert!((r.mesh.positions[nx].x - (0.75 - 5e-6)).abs() < 1e-12);
    // One chart: the merged vertex has one UV, no seam appears.
    assert!(r.mesh.flagged.is_empty(), "{:?}", r.mesh.flagged);
    let again = merge_by_distance(&r.mesh, &VertexScope::All, &p).unwrap();
    assert_eq!(again.mesh, r.mesh);
}

#[test]
fn merge_keeps_pinned_and_feature_vertices_in_place() {
    let m = cube(4, 1.0);
    let corner = m.pinned.iter().position(|&p| p).unwrap() as u32;
    let pc = m.positions[corner as usize];
    // Pull the edge neighbours of the corner (on cube edges) and a face
    // vertex next to an edge vertex very close.
    let mut m2 = m.clone();
    let nbrs: Vec<u32> = m
        .edges()
        .into_iter()
        .filter_map(|(a, b)| {
            if a == corner {
                Some(b)
            } else if b == corner {
                Some(a)
            } else {
                None
            }
        })
        .collect();
    let pulled = nbrs[0];
    m2.positions[pulled as usize] = pc.lerp(m.positions[pulled as usize], 1e-6);
    let r = merge_by_distance(&m2, &VertexScope::All, &MergeParams::for_mesh(&m2)).unwrap();
    assert_outcome(&m2, &r, "cube merge");
    assert_eq!(r.mesh.stats().verts, m2.stats().verts - 1);
    let nc = r.vertex_map[corner as usize].unwrap() as usize;
    assert_eq!(r.mesh.positions[nc], pc, "the pinned corner stays exactly");
    assert!(r.mesh.pinned[nc]);
    assert!(on_cube_edges(&r.mesh, 1.0));
    assert!((flagged_length(&r.mesh) - flagged_length(&m)).abs() < 1e-9);
    assert!(hausdorff(&m, &r.mesh) < 1e-9);
}

#[test]
fn merge_refuses_non_manifold_welds() {
    // Two triangles touching at one corner (a bow-tie once welded).
    let p = |x: f64, y: f64| DVec3::new(x, y, 0.0);
    let m = raw(
        vec![
            p(0., 0.),
            p(1., 0.),
            p(0., 1.),
            p(1e-6, 1e-6),
            p(-1., 0.),
            p(0., -1.),
        ],
        vec![vec![0, 1, 2], vec![3, 4, 5]],
    );
    assert!(m.issues().is_empty());
    let r = merge_by_distance(&m, &VertexScope::All, &MergeParams::default()).unwrap();
    assert_eq!(r.mesh, m);
    assert_eq!(r.report.skipped.len(), 1, "{:?}", r.report.skipped);
    assert!(
        r.report.skipped[0].contains("non-manifold"),
        "{:?}",
        r.report.skipped
    );
    assert!(
        r.report.message.contains("1 cluster(s) skipped"),
        "{}",
        r.report.message
    );
}

#[test]
fn merge_without_close_vertices_changes_nothing() {
    for m in [cube(3, 1.0), sphere(12, 6, 1.0), torus(8, 6, 3.0, 1.0)] {
        let r = merge_by_distance(&m, &VertexScope::All, &MergeParams::for_mesh(&m)).unwrap();
        assert_eq!(r.mesh, m);
        assert_eq!(
            r.report.message,
            "No vertices closer than the merge distance"
        );
    }
}
