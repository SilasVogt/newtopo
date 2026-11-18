//! Unit tests for the mesh tools on synthetic meshes (SPEC §9.4.6).

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
    assert_sound(&m, &r.mesh, "cube");
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
    assert_sound(&m, &r.mesh, "cube min");
    // Can't go below the 8 corners; each side is at worst a few polygons.
    assert!(r.mesh.stats().verts <= 16, "{:?}", r.mesh.stats());
    assert!(!r.report.reached_target);
    assert!(hausdorff(&m, &r.mesh) < 1e-9);
}

#[test]
fn decimate_sphere_quads_and_shape() {
    let m = sphere(32, 16, 1.0);
    let r = decimate(&m, &collapse(0.3)).unwrap();
    assert_sound(&m, &r.mesh, "sphere");
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
    assert_sound(&m, &r.mesh, "sphere tris");
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
    assert_sound(&m, &r.mesh, "sphere tol");
    let d = vertex_deviation(&m, &r.mesh);
    assert!(d <= tol + 1e-12, "deviation {d}");
    assert!(!r.report.reached_target, "5% can't be reached within 0.01");
    assert!(r.mesh.tri_count() < m.tri_count());
}

#[test]
fn decimate_torus_keeps_genus() {
    let m = torus(32, 12, 3.0, 1.0);
    let r = decimate(&m, &collapse(0.25)).unwrap();
    assert_sound(&m, &r.mesh, "torus");
    assert_eq!(r.mesh.topology().0, 0);
}

#[test]
fn decimate_cylinder_keeps_rims_and_seam_uvs() {
    let m = cylinder(32, 8, 1.0, 2.0);
    let r = decimate(&m, &collapse(0.25)).unwrap();
    assert_sound(&m, &r.mesh, "cylinder");
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
    assert_sound(&m, &r.mesh, "planar cube");
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
    assert_sound(&m, &r.mesh, "planar sphere");
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
    assert_sound(&m, &r.mesh, "planar cylinder");
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
    assert_sound(&m, &r.mesh, "zigzag");
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
    assert_sound(&m, &r.mesh, "curved");
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
    assert_sound(&m, &r.mesh, "sliver");
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
    assert_sound(&m, &r.mesh, "short");
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
    assert_sound(&m, &r.mesh, "cube cleanup");
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
    assert_sound(&m, &r.mesh, "even");
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
        assert_sound(&m, &r.mesh, &format!("count {count}"));
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
    assert_sound(&m, &r.mesh, "length");
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
    assert_sound(&m, &r.mesh, "L");
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
    assert_sound(&m, &r.mesh, "ring even");
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
    assert_sound(&m, &r.mesh, "ring 36");
    assert_eq!(r.selection.len(), 36);
    // UVs: the seam is still a seam and interpolated UVs follow the angle.
    assert!(r.mesh.flagged.iter().any(|e| e.flags.seam));
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
    assert_sound(&m, &r.mesh, "rims");
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
    assert_sound(&m, &r.mesh, "cube resample");
    assert!(on_cube_edges(&r.mesh, 1.0));
    assert!((flagged_length(&r.mesh) - 24.0).abs() < 1e-9);
    // 12 cube edges of length 2, 4 edges each.
    assert_eq!(r.mesh.flagged.len(), 48);
    assert!(hausdorff(&m, &r.mesh) < 1e-9);
}
