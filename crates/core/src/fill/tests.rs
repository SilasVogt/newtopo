//! Auto-fill acceptance tests (SPEC §7 AC).

use super::*;
use glam::{DVec2, DVec3};
use std::collections::HashMap;

/// Small seeded PRNG (splitmix64) so the tests need no extra crate.
pub(crate) struct Rng(u64);
impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Rng(seed)
    }
    pub(crate) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub(crate) fn f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub(crate) fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.f64()
    }
    pub(crate) fn usize(&mut self, lo: usize, hi_incl: usize) -> usize {
        lo + (self.next_u64() % (hi_incl - lo + 1) as u64) as usize
    }
}

/// Random star-shaped polygon (counter-clockwise) around the origin.
/// Angular gaps stay below 0.9 pi so every edge stays inside its wedge and
/// the polygon is simple and counter-clockwise.
pub(crate) fn star_polygon(rng: &mut Rng, n: usize) -> Vec<DVec2> {
    use std::f64::consts::{PI, TAU};
    loop {
        let gaps: Vec<f64> = (0..n).map(|_| rng.range(0.15, 1.0)).collect();
        let sum: f64 = gaps.iter().sum();
        if gaps.iter().any(|g| g / sum * TAU >= 0.9 * PI) {
            continue;
        }
        let start = rng.range(0.0, TAU);
        let mut a = start;
        let mut pts = Vec::with_capacity(n);
        for g in &gaps {
            pts.push(DVec2::from_angle(a) * rng.range(0.3, 1.0));
            a += g / sum * TAU;
        }
        return pts;
    }
}

/// Lift UV onto a gently curved 3D surface.
pub(crate) fn lift(uv: &[DVec2]) -> Vec<DVec3> {
    uv.iter()
        .map(|p| DVec3::new(p.x, p.y, 0.1 * (p.x * 1.3).sin() * (p.y * 0.7).cos()))
        .collect()
}

fn run(uv: &[DVec2]) -> FillResult {
    let pos = lift(uv);
    let rank: Vec<u32> = (0..uv.len() as u32).collect();
    fill_polygon(&FillInput {
        uv,
        pos: &pos,
        rank: &rank,
        iso_dirs: [DVec2::X, DVec2::Y],
    })
}

fn area2(uv: &[DVec2], p: &[u32]) -> f64 {
    let pts: Vec<DVec2> = p.iter().map(|&i| uv[i as usize]).collect();
    geom::signed_area2(&pts)
}

/// Checks a decomposition: indices valid, every vertex kept, every piece a
/// strictly positive triangle or strictly convex quad (with the polygon's
/// orientation), boundary edges used once, interior edges paired, areas sum.
pub(crate) fn check_decomposition(uv: &[DVec2], r: &FillResult) {
    let n = uv.len();
    let sign = if geom::signed_area2(uv) >= 0.0 {
        1.0
    } else {
        -1.0
    };
    let mut used = vec![false; n];
    let mut directed: HashMap<(u32, u32), u32> = HashMap::new();
    let mut sum = 0.0;
    for p in &r.polys {
        assert!(p.len() == 3 || p.len() == 4, "piece of size {}", p.len());
        for (k, &i) in p.iter().enumerate() {
            assert!((i as usize) < n);
            used[i as usize] = true;
            let j = p[(k + 1) % p.len()];
            assert_ne!(i, j);
            *directed.entry((i, j)).or_default() += 1;
            let a = uv[p[(k + p.len() - 1) % p.len()] as usize];
            let b = uv[i as usize];
            let c = uv[j as usize];
            assert!(
                sign * geom::orient(a, b, c) > 0.0,
                "flipped or non-convex corner in {:?} ({})",
                p,
                r.method
            );
        }
        sum += area2(uv, p);
    }
    assert!(used.iter().all(|&u| u), "a vertex was dropped");
    for (&(a, b), &c) in &directed {
        assert_eq!(c, 1, "directed edge {a}->{b} used {c} times");
        // Output keeps the input winding, so boundary edges run i -> i+1.
        let boundary = (a as usize + 1) % n == b as usize;
        if !boundary {
            assert!(
                directed.contains_key(&(b, a)),
                "unpaired interior edge {a}->{b}"
            );
        }
    }
    for i in 0..n {
        let j = (i + 1) % n;
        assert!(
            directed.contains_key(&(i as u32, j as u32)),
            "boundary edge {i}-{j} missing"
        );
    }
    let total = geom::signed_area2(uv);
    assert!(
        (sum - total).abs() <= 1e-9 * total.abs().max(1e-300),
        "areas {sum} vs {total}"
    );
    assert_eq!(r.polys.iter().map(|p| p.len() - 2).sum::<usize>(), n - 2);
}

// ---- Independent exhaustive search (test only) -------------------------

fn orient(a: DVec2, b: DVec2, c: DVec2) -> f64 {
    geom::orient(a, b, c)
}

fn point_in_polygon(uv: &[DVec2], p: DVec2) -> bool {
    let mut inside = false;
    let n = uv.len();
    for i in 0..n {
        let (a, b) = (uv[i], uv[(i + 1) % n]);
        if (a.y > p.y) != (b.y > p.y) {
            let x = a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x);
            if p.x < x {
                inside = !inside;
            }
        }
    }
    inside
}

/// Diagonal check written independently from `geom::is_diagonal`: the open
/// segment touches no boundary point and its midpoint is inside.
fn brute_diagonal(uv: &[DVec2], i: usize, j: usize) -> bool {
    let n = uv.len();
    if (i + 1) % n == j || (j + 1) % n == i {
        return true; // polygon edge
    }
    let (p, q) = (uv[i], uv[j]);
    for k in 0..n {
        if k != i && k != j && orient(p, q, uv[k]) == 0.0 {
            let t = (uv[k] - p).dot(q - p) / (q - p).length_squared();
            if t > 0.0 && t < 1.0 {
                return false;
            }
        }
        let (a, b) = (uv[k], uv[(k + 1) % n]);
        let shares = k == i || k == j || (k + 1) % n == i || (k + 1) % n == j;
        if shares {
            continue;
        }
        let (o1, o2, o3, o4) = (
            orient(p, q, a),
            orient(p, q, b),
            orient(a, b, p),
            orient(a, b, q),
        );
        if o1 * o2 < 0.0 && o3 * o4 < 0.0 {
            return false;
        }
    }
    point_in_polygon(uv, (p + q) * 0.5)
}

fn brute_admissible(uv: &[DVec2], q: [usize; 4]) -> bool {
    let p: Vec<DVec2> = q.iter().map(|&i| uv[i]).collect();
    let convex = (0..4).all(|k| orient(p[k], p[(k + 1) % 4], p[(k + 2) % 4]) > 0.0);
    let (mut lo, mut hi) = (uv[0], uv[0]);
    for x in uv {
        lo = lo.min(*x);
        hi = hi.max(*x);
    }
    convex && 0.5 * geom::signed_area2(&p) > 1e-12 * (hi - lo).length_squared()
}

/// All triangulations of the sub-chain i..=j closed by chord (i, j).
fn triangulations(
    diag: &[Vec<bool>],
    i: usize,
    j: usize,
    memo: &mut HashMap<(usize, usize), Vec<Vec<[usize; 3]>>>,
) -> Vec<Vec<[usize; 3]>> {
    if j == i + 1 {
        return vec![Vec::new()];
    }
    if let Some(v) = memo.get(&(i, j)) {
        return v.clone();
    }
    let mut out = Vec::new();
    for k in i + 1..j {
        if !diag[i][k] || !diag[k][j] {
            continue;
        }
        let left = triangulations(diag, i, k, memo);
        let right = triangulations(diag, k, j, memo);
        for l in &left {
            for r in &right {
                let mut t = l.clone();
                t.extend_from_slice(r);
                t.push([i, k, j]);
                out.push(t);
            }
        }
    }
    memo.insert((i, j), out.clone());
    out
}

/// Maximum number of admissible quads over all decompositions into
/// triangles and admissible quads: every such decomposition is a
/// triangulation with a matching of adjacent triangle pairs merged, so take
/// the max over triangulations of the maximum matching (exhaustive over
/// edge subsets).
fn exhaustive_max_quads(uv: &[DVec2]) -> usize {
    let n = uv.len();
    let diag: Vec<Vec<bool>> = (0..n)
        .map(|i| (0..n).map(|j| i != j && brute_diagonal(uv, i, j)).collect())
        .collect();
    let mut memo = HashMap::new();
    let all = triangulations(&diag, 0, n - 1, &mut memo);
    assert!(!all.is_empty(), "no triangulation of a simple polygon");
    let mut best = 0;
    for tris in &all {
        // Mergeable pairs.
        let mut pairs = Vec::new();
        for a in 0..tris.len() {
            for b in a + 1..tris.len() {
                let ta = tris[a];
                let tb = tris[b];
                for k in 0..3 {
                    let (u, v, w1) = (ta[k], ta[(k + 1) % 3], ta[(k + 2) % 3]);
                    for m in 0..3 {
                        if tb[m] == v && tb[(m + 1) % 3] == u {
                            let w2 = tb[(m + 2) % 3];
                            if brute_admissible(uv, [u, w2, v, w1]) {
                                pairs.push((a, b));
                            }
                        }
                    }
                }
            }
        }
        for mask in 0u32..(1 << pairs.len()) {
            let cnt = mask.count_ones() as usize;
            if cnt <= best {
                continue;
            }
            let mut taken = 0u64;
            let mut ok = true;
            for (e, &(a, b)) in pairs.iter().enumerate() {
                if mask & (1 << e) != 0 {
                    if taken & (1 << a) != 0 || taken & (1 << b) != 0 {
                        ok = false;
                        break;
                    }
                    taken |= (1 << a) | (1 << b);
                }
            }
            if ok {
                best = cnt;
            }
        }
    }
    best
}

// ---- Tests ---------------------------------------------------------------

#[test]
fn triangle_and_degenerate_inputs() {
    let tri = [DVec2::ZERO, DVec2::X, DVec2::Y];
    assert_eq!(run(&tri).polys, vec![vec![0, 1, 2]]);
    let r = run(&[DVec2::ZERO, DVec2::X]);
    assert!(r.polys.is_empty());
    // All points identical: only the fan is left.
    let same = [DVec2::ONE; 6];
    let pos = [DVec3::ONE; 6];
    let r = fill_polygon(&FillInput {
        uv: &same,
        pos: &pos,
        rank: &[0, 1, 2, 3, 4, 5],
        iso_dirs: [DVec2::X, DVec2::Y],
    });
    assert_eq!(r.method, "fan");
    assert_eq!(r.polys.len(), 4);
}

#[test]
fn convex_even_polygons_fill_with_quads_only() {
    let mut rng = Rng::new(7);
    for n in (4..=DP_MAX_SIDES).step_by(2) {
        for case in 0..6 {
            // Regular polygon, or random convex points on an ellipse.
            let uv: Vec<DVec2> = if case == 0 {
                (0..n)
                    .map(|k| DVec2::from_angle(k as f64 * std::f64::consts::TAU / n as f64))
                    .collect()
            } else {
                let mut a: Vec<f64> = (0..n)
                    .map(|_| rng.range(0.0, std::f64::consts::TAU))
                    .collect();
                a.sort_by(f64::total_cmp);
                a.dedup();
                let (sx, sy) = (rng.range(0.2, 3.0), rng.range(0.2, 3.0));
                a.iter()
                    .map(|&t| DVec2::new(sx * t.cos(), sy * t.sin()))
                    .collect()
            };
            assert_eq!(uv.len(), n);
            let r = run(&uv);
            assert_eq!(r.method, "dp");
            check_decomposition(&uv, &r);
            assert!(
                r.polys.iter().all(|p| p.len() == 4),
                "n={n} case={case}: {:?}",
                r.polys
            );
            assert_eq!(r.polys.len(), (n - 2) / 2);
        }
    }
}

#[test]
fn vertex_count_unchanged_and_no_overlap_random() {
    let mut rng = Rng::new(42);
    for _ in 0..2_000 {
        let n = rng.usize(3, 40);
        let uv = star_polygon(&mut rng, n);
        let r = run(&uv);
        assert_eq!(r.method, "dp");
        check_decomposition(&uv, &r);
    }
}

#[test]
fn dp_quad_count_matches_exhaustive_search() {
    // SPEC §7 AC: <= 9 vertices, 10,000 seeded random star-shaped polygons.
    let mut rng = Rng::new(0x5EED);
    let mut total_quads = 0;
    for case in 0..10_000 {
        let n = 4 + case % 6; // 4..=9
        let uv = star_polygon(&mut rng, n);
        let r = run(&uv);
        assert_eq!(r.method, "dp");
        check_decomposition(&uv, &r);
        let want = exhaustive_max_quads(&uv);
        assert_eq!(
            r.quads(),
            want,
            "case {case}: n={n} uv={uv:?} got {:?}",
            r.polys
        );
        total_quads += want;
    }
    assert!(
        total_quads > 10_000,
        "generator should produce many quads ({total_quads})"
    );
}

#[test]
fn large_polygons_use_cdt_and_pair_quads() {
    let mut rng = Rng::new(99);
    for n in [49, 64, 100, 257] {
        let circle: Vec<DVec2> = (0..n)
            .map(|k| DVec2::from_angle(k as f64 * std::f64::consts::TAU / n as f64))
            .collect();
        let r = run(&circle);
        assert_eq!(r.method, "cdt");
        check_decomposition(&circle, &r);
        assert!(
            r.quads() * 2 >= (n - 2) / 2,
            "quad-dominant: {} quads for n={n}",
            r.quads()
        );
        let star = star_polygon(&mut rng, n);
        let r = run(&star);
        assert_eq!(r.method, "cdt");
        check_decomposition(&star, &r);
    }
}

#[test]
fn rotation_and_start_invariance() {
    // Same polygon started at a different vertex, ranks rotated with it,
    // gives the same pieces (ties broken by rank, not by input position).
    let mut rng = Rng::new(3);
    for _ in 0..300 {
        let n = rng.usize(5, 20);
        let uv = star_polygon(&mut rng, n);
        let pos = lift(&uv);
        let rank: Vec<u32> = (0..n as u32).map(|i| (i * 7919) % 1009).collect();
        let base = fill_polygon(&FillInput {
            uv: &uv,
            pos: &pos,
            rank: &rank,
            iso_dirs: [DVec2::X, DVec2::Y],
        });
        let s = rng.usize(1, n - 1);
        let ruv: Vec<DVec2> = (0..n).map(|k| uv[(k + s) % n]).collect();
        let rpos: Vec<DVec3> = (0..n).map(|k| pos[(k + s) % n]).collect();
        let rrank: Vec<u32> = (0..n).map(|k| rank[(k + s) % n]).collect();
        let rot = fill_polygon(&FillInput {
            uv: &ruv,
            pos: &rpos,
            rank: &rrank,
            iso_dirs: [DVec2::X, DVec2::Y],
        });
        let canon = |polys: &[Vec<u32>], shift: usize| {
            let mut v: Vec<Vec<u32>> = polys
                .iter()
                .map(|p| {
                    let mut q: Vec<u32> = p
                        .iter()
                        .map(|&i| ((i as usize + shift) % n) as u32)
                        .collect();
                    let m = (0..q.len()).min_by_key(|&k| q[k]).unwrap();
                    q.rotate_left(m);
                    q
                })
                .collect();
            v.sort();
            v
        };
        assert_eq!(canon(&base.polys, 0), canon(&rot.polys, s));
        // Deterministic: same input, same output.
        assert_eq!(
            base,
            fill_polygon(&FillInput {
                uv: &uv,
                pos: &pos,
                rank: &rank,
                iso_dirs: [DVec2::X, DVec2::Y]
            })
        );
    }
}

#[test]
fn clockwise_input_keeps_winding() {
    let mut rng = Rng::new(11);
    for _ in 0..200 {
        let n = rng.usize(4, 30);
        let mut uv = star_polygon(&mut rng, n);
        uv.reverse();
        let r = run(&uv);
        check_decomposition(&uv, &r);
        for p in &r.polys {
            assert!(area2(&uv, p) < 0.0);
        }
    }
}

#[test]
fn self_intersecting_uv_falls_back_to_3d() {
    // Bow-tie in UV, but a proper planar hexagon in 3D.
    let pos: Vec<DVec3> = (0..6)
        .map(|k| DVec2::from_angle(k as f64 * std::f64::consts::TAU / 6.0).extend(0.0))
        .collect();
    let mut uv: Vec<DVec2> = pos.iter().map(|p| p.truncate()).collect();
    uv.swap(1, 4);
    let r = fill_polygon(&FillInput {
        uv: &uv,
        pos: &pos,
        rank: &[0, 1, 2, 3, 4, 5],
        iso_dirs: [DVec2::X, DVec2::Y],
    });
    assert_eq!(r.method, "ear-3d");
    assert_eq!(r.polys.len(), 4);
}

#[test]
fn collinear_vertices_are_handled() {
    // Rectangle with extra vertices along its sides (typical patch
    // boundary after lines end on it): 2 x 3 points per long side.
    let uv = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(2.0, 0.0),
        DVec2::new(3.0, 0.0),
        DVec2::new(3.0, 1.0),
        DVec2::new(2.0, 1.0),
        DVec2::new(1.0, 1.0),
        DVec2::new(0.0, 1.0),
    ];
    let r = run(&uv);
    assert_eq!(r.method, "dp");
    check_decomposition(&uv, &r);
    assert_eq!(r.quads(), 3);
    // L-shape with a straight vertex on the long side.
    let l = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(2.0, 0.0),
        DVec2::new(2.0, 1.0),
        DVec2::new(1.0, 1.0),
        DVec2::new(1.0, 2.0),
        DVec2::new(0.0, 2.0),
        DVec2::new(0.0, 1.0),
    ];
    let r = run(&l);
    check_decomposition(&l, &r);
    assert_eq!(r.quads(), 2, "{:?}", r.polys);
}

#[test]
fn iso_alignment_prefers_grid_diagonals() {
    // A 2x1 rectangle with a midpoint on both long sides: the quad split
    // along the vertical (iso) chord wins over slanted alternatives.
    let uv = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(2.0, 0.0),
        DVec2::new(2.0, 1.0),
        DVec2::new(1.0, 1.0),
        DVec2::new(0.0, 1.0),
    ];
    let r = run(&uv);
    let mut ps: Vec<Vec<u32>> = r.polys.clone();
    ps.sort();
    assert_eq!(ps, vec![vec![0, 1, 4, 5], vec![1, 2, 3, 4]]);
}

/// Typical polygons for the timing test: patches with 6-16 sides.
pub(crate) fn typical_polygons(count: usize) -> Vec<(Vec<DVec2>, Vec<DVec3>)> {
    let mut rng = Rng::new(2024);
    (0..count)
        .map(|i| {
            let n = 6 + i % 11;
            let uv = if i % 2 == 0 {
                star_polygon(&mut rng, n)
            } else {
                // Rectangle with extra points along its sides.
                let mut pts = Vec::new();
                let (w, h) = (rng.range(0.5, 3.0), rng.range(0.5, 3.0));
                let per = [n / 4 + n % 4, n / 4, n / 4, n / 4];
                let corners = [
                    DVec2::ZERO,
                    DVec2::new(w, 0.0),
                    DVec2::new(w, h),
                    DVec2::new(0.0, h),
                ];
                for s in 0..4 {
                    let (a, b) = (corners[s], corners[(s + 1) % 4]);
                    for k in 0..per[s] {
                        pts.push(a.lerp(b, k as f64 / per[s] as f64));
                    }
                }
                pts
            };
            let pos = lift(&uv);
            (uv, pos)
        })
        .collect()
}

#[test]
fn fill_2000_typical_polygons_fast() {
    let polys = typical_polygons(2_000);
    let ranks: Vec<Vec<u32>> = polys
        .iter()
        .map(|(uv, _)| (0..uv.len() as u32).collect())
        .collect();
    let t = std::time::Instant::now();
    let mut quads = 0;
    for ((uv, pos), rank) in polys.iter().zip(&ranks) {
        let r = fill_polygon(&FillInput {
            uv,
            pos,
            rank,
            iso_dirs: [DVec2::X, DVec2::Y],
        });
        quads += r.quads();
    }
    let ms = t.elapsed().as_secs_f64() * 1e3;
    eprintln!("filled 2000 typical polygons in {ms:.1} ms ({quads} quads)");
    if !cfg!(debug_assertions) {
        assert!(ms < 100.0, "fill took {ms:.1} ms");
    }
}

#[test]
fn triangulate_3d_quads_and_ngons() {
    let sq = [
        DVec3::ZERO,
        DVec3::X,
        DVec3::new(1.0, 2.0, 0.0),
        DVec3::new(0.0, 2.0, 0.0),
    ];
    assert_eq!(triangulate_3d(&sq).len(), 2);
    // Concave quad (dart): only the 1-3 split is valid.
    let dart = [
        DVec3::ZERO,
        DVec3::new(2.0, 1.0, 0.0),
        DVec3::new(0.0, 0.5, 0.0),
        DVec3::new(-2.0, 1.0, 0.0),
    ];
    let t = triangulate_3d(&dart);
    assert_eq!(t.len(), 2);
    for tri in &t {
        let n = (dart[tri[1] as usize] - dart[tri[0] as usize])
            .cross(dart[tri[2] as usize] - dart[tri[0] as usize]);
        assert!(n.z > 0.0, "{t:?}");
    }
    let hex: Vec<DVec3> = (0..6)
        .map(|k| DVec2::from_angle(k as f64).extend(0.0))
        .collect();
    assert_eq!(triangulate_3d(&hex).len(), 4);
}
