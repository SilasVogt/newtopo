//! Unit tests for the mesh-mode tools added with the mesh mode UI (SPEC
//! §9.5.9–§9.5.13): loop and shortest-path selection, Edge Slide, Relax,
//! Space, Target Merge and "Stay on CAD surface".

use super::synth::*;
use super::tests::{assert_outcome, assert_sound};
use super::*;
use glam::DVec3;

fn gid(ny: usize, i: usize, j: usize) -> u32 {
    (i * (ny + 1) + j) as u32
}

fn row(nx: usize, ny: usize, j: usize) -> Vec<[u32; 2]> {
    (0..nx)
        .map(|i| [gid(ny, i, j), gid(ny, i + 1, j)])
        .collect()
}

fn sorted(mut v: Vec<[u32; 2]>) -> Vec<[u32; 2]> {
    for e in &mut v {
        e.sort_unstable();
    }
    v.sort_unstable();
    v
}

// ------------------------------------------------------------------ select

#[test]
fn loop_runs_across_a_grid_and_stops_at_the_boundary() {
    let (n, j) = (6, 3);
    let m = plane(n, 1.0);
    let l = edge_loop(&m, gid(n, 2, j), gid(n, 3, j)).unwrap();
    assert_eq!(sorted(l.clone()), sorted(row(n, n, j)));
    // Walk order: consecutive edges share a vertex.
    for w in l.windows(2) {
        assert!(w[0].iter().any(|v| w[1].contains(v)), "{l:?}");
    }
    // Same loop from any of its edges.
    let l2 = edge_loop(&m, gid(n, 5, j), gid(n, 6, j)).unwrap();
    assert_eq!(sorted(l2), sorted(row(n, n, j)));
}

#[test]
fn loop_closes_around_a_cylinder_and_follows_boundaries() {
    let (segs, rings) = (12, 4);
    let m = cylinder(segs, rings, 1.0, 2.0);
    // Vertices: ring k, segment i → k * segs + i.
    let v = |k: usize, i: usize| (k * segs + i % segs) as u32;
    let l = edge_loop(&m, v(2, 0), v(2, 1)).unwrap();
    assert_eq!(l.len(), segs, "closed ring {l:?}");
    let expect: Vec<[u32; 2]> = (0..segs).map(|i| [v(2, i), v(2, i + 1)]).collect();
    assert_eq!(sorted(l), sorted(expect));
    // An open boundary edge follows the boundary all the way round.
    let p = plane(4, 1.0);
    let b = edge_loop(&p, gid(4, 1, 0), gid(4, 2, 0)).unwrap();
    assert_eq!(b.len(), 16, "the whole border of a 4 × 4 plate");
    assert_eq!(
        sorted(b),
        sorted(p.boundary_edges().iter().map(|&(a, b)| [a, b]).collect())
    );
}

#[test]
fn loop_refuses_a_non_edge() {
    let m = plane(3, 1.0);
    assert!(edge_loop(&m, 0, 5).is_err());
    assert!(edge_loop(&m, 0, 0).is_err());
}

#[test]
fn shortest_path_on_a_grid() {
    let n = 6;
    let m = plane(n, 1.0);
    let (a, b) = (gid(n, 1, 1), gid(n, 4, 3));
    let p = shortest_path(&m, a, b).unwrap();
    assert_eq!(p.first(), Some(&a));
    assert_eq!(p.last(), Some(&b));
    assert_eq!(p.len(), 3 + 2 + 1, "Manhattan path {p:?}");
    let len: f64 = p
        .windows(2)
        .map(|w| {
            assert!(m.edges().contains(&(w[0].min(w[1]), w[0].max(w[1]))));
            m.positions[w[0] as usize].distance(m.positions[w[1] as usize])
        })
        .sum();
    assert!((len - 5.0 / n as f64).abs() < 1e-12);
    assert_eq!(shortest_path(&m, a, b).unwrap(), p, "deterministic");
    assert_eq!(shortest_path(&m, a, a).unwrap(), vec![a]);
}

#[test]
fn shortest_edge_path_joins_two_edges() {
    let n = 6;
    let m = plane(n, 1.0);
    let r = row(n, n, 2);
    let p = shortest_edge_path(&m, r[0], r[4]).unwrap();
    assert_eq!(sorted(p.clone()), sorted(r[0..5].to_vec()), "{p:?}");
    assert_eq!(p[0], r[0]);
    assert_eq!(*p.last().unwrap(), r[4]);
    assert_eq!(shortest_edge_path(&m, r[1], r[1]).unwrap(), vec![r[1]]);
}

#[test]
fn shortest_path_refuses_separate_parts() {
    let a = plane(2, 1.0);
    let mut m = a.clone();
    let off = m.positions.len() as u32;
    m.positions
        .extend(a.positions.iter().map(|p| *p + DVec3::new(5.0, 0.0, 0.0)));
    m.pinned.extend(a.pinned.iter());
    for mut q in a.polygons.clone() {
        q.verts.iter_mut().for_each(|v| *v += off);
        m.polygons.push(q);
    }
    let r = shortest_path(&m, 0, off);
    assert!(r.unwrap_err().reason.contains("separate parts"));
}

// -------------------------------------------------------------- edge slide

#[test]
fn edge_slide_moves_a_row_to_one_side() {
    let (n, j) = (6, 3);
    let m = plane(n, 1.0);
    let sel = Selection::Edges(row(n, n, j));
    let y0 = j as f64 / n as f64;
    let mut sides = Vec::new();
    for f in [0.5, -0.5] {
        let r = edge_slide(
            &m,
            &sel,
            &EdgeSlideParams {
                factor: f,
                ..Default::default()
            },
        )
        .unwrap();
        assert_outcome(&m, &r, "edge slide");
        // The whole row (ends slide along the border) moves half a cell.
        let ys: Vec<f64> = (0..=n)
            .map(|i| r.mesh.positions[gid(n, i, j) as usize].y)
            .collect();
        for (i, y) in ys.iter().enumerate() {
            assert!(((y - y0).abs() - 0.5 / n as f64).abs() < 1e-12, "{ys:?}");
            assert!(
                (r.mesh.positions[gid(n, i, j) as usize].x - i as f64 / n as f64).abs() < 1e-12
            );
            assert_eq!((y - y0).signum(), (ys[0] - y0).signum(), "one side");
        }
        sides.push((ys[0] - y0).signum());
        assert_eq!(
            sorted(r.selection.clone()),
            sorted(row(n, n, j)),
            "selection follows"
        );
        // Everything else stays.
        for (v, p) in m.positions.iter().enumerate() {
            if !(0..=n).any(|i| gid(n, i, j) as usize == v) {
                assert_eq!(r.mesh.positions[v], *p);
            }
        }
    }
    assert_eq!(sides[0], -sides[1], "the sign picks the side");
}

#[test]
fn edge_slide_zero_and_refusals() {
    let (n, j) = (4, 2);
    let m = plane(n, 1.0);
    let sel = Selection::Edges(row(n, n, j));
    let r = edge_slide(
        &m,
        &sel,
        &EdgeSlideParams {
            factor: 0.0,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(r.mesh, m);
    assert!(
        edge_slide(
            &m,
            &sel,
            &EdgeSlideParams {
                factor: 1.5,
                ..Default::default()
            }
        )
        .is_err()
    );
    assert!(edge_slide(&m, &Selection::Edges(vec![]), &EdgeSlideParams::default()).is_err());
    // A cube edge only slides along the cube edges' own direction, which
    // isn't a cross edge: every vertex is skipped, so it refuses.
    let c = cube(3, 1.0);
    let e = c.flagged.iter().find(|e| e.flags.cad).unwrap();
    let l = edge_loop(&c, e.a, e.b).unwrap();
    let r = edge_slide(&c, &Selection::Edges(l), &EdgeSlideParams::default());
    match r {
        Err(e) => assert!(!e.reason.is_empty()),
        Ok(r) => assert_eq!(r.mesh, c, "cube edges stay"),
    }
}

// ------------------------------------------------------------- relax/space

/// Plane with row `j` zig-zagging in y by `amp`.
fn zigzag(n: usize, j: usize, amp: f64) -> EditMesh {
    let mut m = plane(n, 1.0);
    for i in 1..n {
        m.positions[gid(n, i, j) as usize].y += if i % 2 == 0 { amp } else { -amp };
    }
    m
}

fn wiggle(m: &EditMesh, n: usize, j: usize) -> f64 {
    let y0 = j as f64 / n as f64;
    (0..=n)
        .map(|i| (m.positions[gid(n, i, j) as usize].y - y0).abs())
        .fold(0.0, f64::max)
}

#[test]
fn relax_smooths_a_jagged_row() {
    let (n, j, amp) = (10, 5, 0.03);
    let m = zigzag(n, j, amp);
    let sel = Selection::Edges(row(n, n, j));
    for interpolation in [Interpolation::Linear, Interpolation::Cubic] {
        for even in [false, true] {
            let p = RelaxParams {
                iterations: 25,
                interpolation,
                even,
                ..Default::default()
            };
            let r = relax(&m, &sel, &p).unwrap();
            assert_outcome(&m, &r, &format!("relax {interpolation:?} {even}"));
            let w = wiggle(&r.mesh, n, j);
            assert!(w < 0.3 * amp, "{interpolation:?} even {even}: still {w}");
            // Run ends and everything off the run stay.
            for (v, q) in m.positions.iter().enumerate() {
                let on = (1..n).any(|i| gid(n, i, j) as usize == v);
                if !on {
                    assert_eq!(r.mesh.positions[v], *q, "vertex {v} moved");
                }
            }
            if even {
                let l: Vec<f64> = row(n, n, j)
                    .iter()
                    .map(|e| {
                        r.mesh.positions[e[0] as usize].distance(r.mesh.positions[e[1] as usize])
                    })
                    .collect();
                let (lo, hi) = l
                    .iter()
                    .fold((f64::MAX, 0.0f64), |(a, b), &x| (a.min(x), b.max(x)));
                assert!(hi - lo < 0.02 / n as f64, "{l:?}");
            }
            assert_eq!(sorted(r.selection.clone()), sorted(row(n, n, j)));
        }
    }
    // More iterations smooth more.
    let few = relax(
        &m,
        &sel,
        &RelaxParams {
            iterations: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let many = relax(
        &m,
        &sel,
        &RelaxParams {
            iterations: 10,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(wiggle(&many.mesh, n, j) < wiggle(&few.mesh, n, j));
}

#[test]
fn relax_keeps_features_and_refuses_when_nothing_moves() {
    // A border run: every vertex is on the boundary, nothing is free.
    let n = 4;
    let m = plane(n, 1.0);
    let r = relax(&m, &Selection::Edges(row(n, n, 0)), &RelaxParams::default());
    assert!(r.unwrap_err().reason.contains("Nothing to relax"));
    // Cube edges: corners and CAD-edge vertices stay.
    let c = cube(3, 1.0);
    let r = relax(&c, &Selection::AllFeatures, &RelaxParams::default());
    assert!(r.is_err());
    assert!(
        relax(
            &m,
            &Selection::Edges(row(n, n, 2)),
            &RelaxParams {
                iterations: 0,
                ..Default::default()
            }
        )
        .is_err()
    );
}

#[test]
fn relax_on_a_curved_surface_leaves_it_but_stays_valid() {
    let n = 12;
    let mut m = heightfield(n, n, 1.0, |x, y| 0.2 * (3.0 * x).sin() * (2.0 * y).cos());
    for i in 1..n {
        m.positions[gid(n, i, 6) as usize].y += if i % 2 == 0 { 0.02 } else { -0.02 };
    }
    let r = relax(
        &m,
        &Selection::Edges(row(n, n, 6)),
        &RelaxParams {
            iterations: 25,
            ..Default::default()
        },
    )
    .unwrap();
    assert_outcome(&m, &r, "curved relax");
    assert!(r.report.message.contains("Relaxed 1 run"));
}

/// Plane with the vertices of row `j` unevenly spaced along x.
fn uneven(n: usize, j: usize) -> EditMesh {
    let mut m = plane(n, 1.0);
    for i in 1..n {
        let x = i as f64 / n as f64;
        m.positions[gid(n, i, j) as usize].x = x + 0.3 / n as f64 * (i as f64 * 1.7).sin();
    }
    m
}

#[test]
fn space_evens_out_a_run_and_is_idempotent() {
    for j in [3, 0] {
        // Row 0 is the border: its vertices are on the boundary curve,
        // which is the run, so they may move along it.
        let n = 8;
        let m = uneven(n, j);
        let sel = Selection::Edges(row(n, n, j));
        let r = space(&m, &sel, &SpaceParams::default()).unwrap();
        assert_outcome(&m, &r, "space");
        for e in row(n, n, j) {
            let l = r.mesh.positions[e[0] as usize].distance(r.mesh.positions[e[1] as usize]);
            assert!((l - 1.0 / n as f64).abs() < 1e-9, "row {j}: {l}");
        }
        for i in 0..=n {
            let p = r.mesh.positions[gid(n, i, j) as usize];
            assert!((p.y - j as f64 / n as f64).abs() < 1e-12 && p.z == 0.0);
        }
        let again = space(
            &r.mesh,
            &Selection::Edges(r.selection.clone()),
            &SpaceParams::default(),
        )
        .unwrap();
        assert!(
            again
                .mesh
                .positions
                .iter()
                .zip(&r.mesh.positions)
                .all(|(a, b)| a.distance(*b) < 1e-12)
        );
        assert!(again.report.message.contains("Already evenly spaced") || again.mesh == r.mesh);
    }
}

#[test]
fn space_a_jittered_ring_on_a_cylinder() {
    let (segs, rings) = (16, 3);
    let mut m = cylinder(segs, rings, 1.0, 2.0);
    let v = |i: usize| (segs + i % segs) as u32;
    for i in 0..segs {
        let a = std::f64::consts::TAU * (i as f64 + 0.3 * (i as f64 * 2.3).sin()) / segs as f64;
        let p = &mut m.positions[v(i) as usize];
        *p = DVec3::new(a.cos(), a.sin(), p.z);
    }
    let sel: Vec<[u32; 2]> = (0..segs).map(|i| [v(i), v(i + 1)]).collect();
    let r = space(&m, &Selection::Edges(sel.clone()), &SpaceParams::default()).unwrap();
    assert_outcome(&m, &r, "ring space");
    let chords: Vec<f64> = sel
        .iter()
        .map(|e| r.mesh.positions[e[0] as usize].distance(r.mesh.positions[e[1] as usize]))
        .collect();
    let mean = chords.iter().sum::<f64>() / segs as f64;
    for c in &chords {
        assert!((c - mean).abs() < 0.05 * mean, "{chords:?}");
    }
}

// ----------------------------------------------------------- target merge

#[test]
fn target_merge_into_a_neighbour() {
    let n = 4;
    let m = plane(n, 1.0);
    // On a flat grid the diagonal merge would leave a quad with three
    // corners in a line: refused, with the reason.
    let d = target_merge(
        &m,
        gid(n, 1, 1),
        gid(n, 2, 2),
        &TargetMergeParams::default(),
    );
    assert!(d.unwrap_err().reason.contains("fold, flip or flatten"));
    for (from, to) in [(gid(n, 1, 1), gid(n, 2, 1)), (gid(n, 2, 2), gid(n, 2, 1))] {
        let r = target_merge(&m, from, to, &TargetMergeParams::default()).unwrap();
        assert_outcome(&m, &r, "target merge");
        assert_eq!(r.mesh.stats().verts + 1, m.stats().verts);
        assert_eq!(r.vertex_map[from as usize], None, "merged away");
        let t = r.vertex_map[to as usize].unwrap();
        assert_eq!(
            r.mesh.positions[t as usize], m.positions[to as usize],
            "target stays"
        );
        assert_eq!(r.selected_vertices, vec![t]);
    }
}

#[test]
fn target_merge_refusals() {
    let n = 4;
    let m = plane(n, 1.0);
    // A border vertex may only merge along the border.
    let r = target_merge(
        &m,
        gid(n, 2, 0),
        gid(n, 2, 1),
        &TargetMergeParams::default(),
    );
    assert!(r.unwrap_err().reason.contains("feature edge"));
    assert!(
        target_merge(
            &m,
            gid(n, 2, 0),
            gid(n, 3, 0),
            &TargetMergeParams::default()
        )
        .is_ok()
    );
    assert!(target_merge(&m, 3, 3, &TargetMergeParams::default()).is_err());
    assert!(target_merge(&m, 0, 999, &TargetMergeParams::default()).is_err());
    // A cube corner is pinned.
    let c = cube(2, 1.0);
    let corner = c.pinned.iter().position(|&p| p).unwrap() as u32;
    let nb = c
        .edges()
        .into_iter()
        .find_map(|(a, b)| (a == corner).then_some(b).or((b == corner).then_some(a)))
        .unwrap();
    let r = target_merge(&c, corner, nb, &TargetMergeParams::default());
    assert!(r.unwrap_err().reason.contains("corner"));
}

/// A 4 × 4 plate slit along row 2 from the left border to x = 2: the
/// vertices (0, 2) and (1, 2) are doubled for the quads below the slit
/// (cells (0, 1) and (1, 1); heightfield polygons are numbered i × ny + j).
fn slit() -> (EditMesh, u32, u32) {
    let n = 4;
    let mut m = plane(n, 1.0);
    let mut dups = Vec::new();
    for v in [gid(n, 0, 2), gid(n, 1, 2)] {
        m.positions.push(m.positions[v as usize]);
        m.pinned.push(false);
        dups.push((v, (m.positions.len() - 1) as u32));
    }
    for cell in [n + 1, 1] {
        for v in &mut m.polygons[cell].verts {
            if let Some(&(_, d)) = dups.iter().find(|(o, _)| o == v) {
                *v = d;
            }
        }
    }
    (m, dups[1].1, gid(n, 1, 2))
}

#[test]
fn target_merge_welds_across_a_slit() {
    let (m, from, to) = slit();
    assert!(m.issues().is_empty(), "{:?}", m.issues());
    let r = target_merge(&m, from, to, &TargetMergeParams::default()).unwrap();
    assert_sound(&m, &r.mesh, "slit weld");
    assert_eq!(r.mesh.stats().verts + 1, m.stats().verts);
    assert!(r.report.message.contains("Welded"));
    // The other doubled vertex, at the open end of the slit, can't weld on
    // its own once its neighbour is welded either way: it's on the border.
    assert_eq!(
        r.mesh.topology().1,
        m.topology().1,
        "still one boundary loop"
    );
}

/// The weld keeps the picked target, even where Merge by Distance's role
/// order would keep the source (both on the border, the source lower).
#[test]
fn target_merge_weld_keeps_the_target() {
    let (m, dup, orig) = slit();
    assert!(orig < dup);
    let r = target_merge(&m, orig, dup, &TargetMergeParams::default()).unwrap();
    assert_sound(&m, &r.mesh, "slit weld onto the duplicate");
    assert!(r.vertex_map[orig as usize].is_none(), "the source goes");
    let kept = r.vertex_map[dup as usize].expect("the target stays");
    assert_eq!(r.selected_vertices, vec![kept]);
    assert_eq!(r.mesh.positions[kept as usize], m.positions[dup as usize]);
}

/// Moving the source onto the target reshapes the source's polygons; they
/// are checked like any move. Here the target sits inside the cells above
/// the slit, so moving vertex 7 there would make those quads concave.
#[test]
fn target_merge_weld_refuses_to_distort_the_source_polygons() {
    let (mut m, dup, orig) = slit();
    m.positions[dup as usize] = DVec3::new(0.25, 0.8, 0.0);
    assert!(m.issues().is_empty(), "{:?}", m.issues());
    assert!(m.polygon_problems(60.0).is_empty(), "the input is convex");
    let r = target_merge(&m, orig, dup, &TargetMergeParams::default());
    let e = r.expect_err("refused");
    assert!(e.reason.contains("fold, flip or flatten"), "{}", e.reason);
    // The other way round the duplicate's quads return to their squares.
    let r = target_merge(&m, dup, orig, &TargetMergeParams::default()).unwrap();
    assert_sound(&m, &r.mesh, "slit weld back");
    assert!(r.mesh.polygon_problems(60.0).is_empty());
}

// ---------------------------------------------------------- stay on surface

#[test]
fn stay_on_surface_projects_moved_vertices_only() {
    let n = 10;
    let f = |x: f64, y: f64| 0.1 * (3.0 * x).sin() + 0.05 * (2.0 * y).cos();
    let mut m = heightfield(n, n, 1.0, f);
    for i in 1..n {
        m.positions[gid(n, i, 5) as usize].y += if i % 2 == 0 { 0.03 } else { -0.03 };
        let p = m.positions[gid(n, i, 5) as usize];
        m.positions[gid(n, i, 5) as usize].z = f(p.x, p.y);
    }
    let r = relax(
        &m,
        &Selection::Edges(row(n, n, 5)),
        &RelaxParams {
            iterations: 10,
            ..Default::default()
        },
    )
    .unwrap();
    // "Project" vertically onto the height field.
    let mut calls = 0;
    let mut proj = |_: newtopo_kernel::FaceId, pts: &[DVec3]| {
        calls += 1;
        pts.iter()
            .map(|p| Some(DVec3::new(p.x, p.y, f(p.x, p.y))))
            .collect()
    };
    let moved: Vec<usize> = (0..m.positions.len())
        .filter(|&v| r.mesh.positions[v] != m.positions[v])
        .collect();
    assert!(!moved.is_empty());
    let s = stay_on_surface(&m, r.clone(), 60.0, &mut proj);
    assert!(calls > 0);
    assert_sound(&m, &s.mesh, "projected");
    for v in 0..m.positions.len() {
        let p = s.mesh.positions[v];
        if moved.contains(&v) {
            assert!(
                (p.z - f(p.x, p.y)).abs() < 1e-12,
                "vertex {v} not on the surface"
            );
        } else {
            assert_eq!(p, m.positions[v]);
        }
    }
    assert!(s.report.message.contains("put back on the CAD surface"));
}

/// The projection may not tilt a polygon past the normal limit, even when
/// the polygon keeps its shape and doesn't flip: one moved vertex whose
/// "closest point" is far above a flat plate stays where the tool put it.
#[test]
fn stay_on_surface_keeps_the_normal_limit() {
    let n = 10;
    let mut m = plane(n, 1.0);
    for i in 1..n {
        m.positions[gid(n, i, 5) as usize].y += if i % 2 == 0 { 0.03 } else { -0.03 };
    }
    let r = relax(
        &m,
        &Selection::Edges(row(n, n, 5)),
        &RelaxParams {
            iterations: 10,
            ..Default::default()
        },
    )
    .unwrap();
    let v = (0..m.positions.len())
        .find(|&v| r.mesh.positions[v] != m.positions[v])
        .unwrap();
    let at = r.mesh.positions[v];
    // A CAD face far above this one vertex (spacing 0.1, lift 0.5: about
    // 74° of tilt on the quads around it).
    let lifted = at + DVec3::Z * 0.5;
    let mut proj = |_: newtopo_kernel::FaceId, pts: &[DVec3]| {
        pts.iter()
            .map(|p| Some(if *p == at { lifted } else { *p }))
            .collect()
    };
    // Shape and orientation alone would accept the lift, so only the
    // normal limit holds the vertex back.
    let diag = r.mesh.diagonal();
    for (pi, p) in r.mesh.polygons.iter().enumerate() {
        if p.verts.contains(&(v as u32)) {
            let b = r.mesh.polygon_positions(pi);
            let a: Vec<DVec3> = p
                .verts
                .iter()
                .map(|&x| {
                    if x as usize == v {
                        lifted
                    } else {
                        r.mesh.positions[x as usize]
                    }
                })
                .collect();
            assert!(super::smooth::polygon_acceptable(&b, &a, diag));
        }
    }
    let s = stay_on_surface(&m, r.clone(), 60.0, &mut proj);
    assert_eq!(s.mesh.positions[v], at, "held where the tool put it");
    assert!(s.mesh.polygon_problems(60.0).is_empty());
    assert!(
        s.report.skipped.iter().any(|x| x.contains("normal limit")),
        "{:?}",
        s.report.skipped
    );
    // A looser limit lets the same projection through.
    let s = stay_on_surface(&m, r, 89.0, &mut proj);
    assert_eq!(s.mesh.positions[v], lifted);
}

/// Each projection is checked against the polygon as earlier projections
/// left it; the result must also keep the orientation the tool gave it.
/// Here the polygon is already past the normal limit (its CAD normals are
/// +x), so only orientation guards it: B then C each turn it by less than
/// 90°, together by more.
#[test]
fn stay_on_surface_never_reverses_a_polygon_in_steps() {
    let p = |x: f64, y: f64, z: f64| DVec3::new(x, y, z);
    let (b0, c0) = (p(1.0, 0.0, 0.0), p(0.0, 1.0, 0.0));
    let tri = |b: DVec3, c: DVec3| EditMesh {
        positions: vec![p(0.0, 0.0, 0.0), b, c],
        pinned: vec![false; 3],
        polygons: vec![EditPolygon {
            verts: vec![0, 1, 2],
            group: newtopo_kernel::FaceId(0),
            normals: vec![DVec3::X; 3],
            uvs: None,
        }],
        ..Default::default()
    };
    // The tool moved B and C (from slightly elsewhere) to b0, c0.
    let before = tri(p(1.0, -0.1, 0.0), p(-0.1, 1.0, 0.0));
    let out = EditOutcome {
        mesh: tri(b0, c0),
        vertex_map: vec![Some(0), Some(1), Some(2)],
        selection: vec![],
        selected_vertices: vec![],
        report: Default::default(),
    };
    let (b1, c1) = (p(1.0, 1.0, 1.0), p(2.0, 1.0, -1.0));
    let mut proj = |_: newtopo_kernel::FaceId, pts: &[DVec3]| {
        pts.iter()
            .map(|q| {
                Some(if *q == b0 {
                    b1
                } else if *q == c0 {
                    c1
                } else {
                    *q
                })
            })
            .collect()
    };
    let s = stay_on_surface(&before, out.clone(), 60.0, &mut proj);
    let n = |m: &EditMesh| super::mesh::area_vec(&m.polygon_positions(0));
    assert!(n(&s.mesh).dot(n(&out.mesh)) > 0.0, "{:?}", s.mesh.positions);
    assert_eq!(s.mesh.positions[1], b1, "B alone keeps the orientation");
    assert_eq!(s.mesh.positions[2], c0, "C after B would reverse it");
}
