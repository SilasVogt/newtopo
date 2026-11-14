//! Dynamic program over polygon diagonals (SPEC §7 step 3).
//!
//! Like the classic optimal-triangulation DP over sub-chains, but each
//! sub-chain `(i, j)` closed by the chord `i-j` may also be split by a quad
//! `(i, k, l, j)`. Objective, lexicographic: quad count, then summed quality.
//! Own design after the textbook polygon-triangulation DP.

use super::geom::{diagonal_table, strictly_convex_quad};
use super::quality::Scorer;
use glam::DVec2;

#[derive(Clone, Copy)]
enum Choice {
    None,
    Tri(u16),
    Quad(u16, u16),
}

/// Quality ties closer than this keep the earlier candidate (deterministic).
const TIE: f64 = 1e-9;

#[inline]
fn better(a: (u32, f64), b: Option<(u32, f64)>) -> bool {
    match b {
        None => true,
        Some(b) => a.0 > b.0 || (a.0 == b.0 && a.1 > b.1 + TIE),
    }
}

/// Signed area of a quad (counter-clockwise positive).
#[inline]
fn quad_area(a: DVec2, b: DVec2, c: DVec2, d: DVec2) -> f64 {
    0.5 * ((a.x * b.y - a.y * b.x)
        + (b.x * c.y - b.y * c.x)
        + (c.x * d.y - c.y * d.x)
        + (d.x * a.y - d.y * a.x))
}

/// Admissible quad: strictly convex in UV and area above `eps` (SPEC §7).
#[inline]
pub fn admissible(uv: &[DVec2], a: usize, b: usize, c: usize, d: usize, eps: f64) -> bool {
    strictly_convex_quad(uv[a], uv[b], uv[c], uv[d]) && quad_area(uv[a], uv[b], uv[c], uv[d]) > eps
}

/// Optimal decomposition of the simple counter-clockwise polygon `uv` into
/// triangles and admissible quads. `None` if no triangulation exists (the
/// polygon is not simple in UV).
pub fn dp_fill(uv: &[DVec2], sc: &Scorer, area_eps: f64) -> Option<Vec<Vec<u32>>> {
    let n = uv.len();
    if n < 3 || n > u16::MAX as usize {
        return None;
    }
    let valid = diagonal_table(uv);
    let at = |i: usize, j: usize| i * n + j;
    let mut best: Vec<Option<(u32, f64)>> = vec![None; n * n];
    let mut choice = vec![Choice::None; n * n];
    for i in 0..n - 1 {
        best[at(i, i + 1)] = Some((0, 0.0));
    }
    for d in 2..n {
        for i in 0..n - d {
            let j = i + d;
            if !valid[at(i, j)] {
                continue;
            }
            let mut cur: Option<(u32, f64)> = None;
            let mut ch = Choice::None;
            for k in i + 1..j {
                if !valid[at(i, k)] {
                    continue;
                }
                let Some(left) = best[at(i, k)] else { continue };
                // Triangle (i, k, j).
                if valid[at(k, j)]
                    && let Some(right) = best[at(k, j)]
                {
                    let cand = (left.0 + right.0, left.1 + right.1 + sc.tri(i, k, j));
                    if better(cand, cur) {
                        cur = Some(cand);
                        ch = Choice::Tri(k as u16);
                    }
                }
                // Quads (i, k, l, j).
                for l in k + 1..j {
                    if !valid[at(k, l)] || !valid[at(l, j)] {
                        continue;
                    }
                    let (Some(mid), Some(right)) = (best[at(k, l)], best[at(l, j)]) else {
                        continue;
                    };
                    let quads = left.0 + mid.0 + right.0 + 1;
                    // Cheap bound before the exact convexity test.
                    if let Some(c) = cur
                        && quads < c.0
                    {
                        continue;
                    }
                    if !admissible(uv, i, k, l, j, area_eps) {
                        continue;
                    }
                    let cand = (quads, left.1 + mid.1 + right.1 + sc.quad(i, k, l, j));
                    if better(cand, cur) {
                        cur = Some(cand);
                        ch = Choice::Quad(k as u16, l as u16);
                    }
                }
            }
            best[at(i, j)] = cur;
            choice[at(i, j)] = ch;
        }
    }
    best[at(0, n - 1)]?;
    let mut out = Vec::with_capacity(n - 2);
    let mut stack = vec![(0usize, n - 1)];
    while let Some((i, j)) = stack.pop() {
        if j == i + 1 {
            continue;
        }
        match choice[at(i, j)] {
            Choice::None => return None,
            Choice::Tri(k) => {
                let k = k as usize;
                out.push(vec![i as u32, k as u32, j as u32]);
                stack.push((k, j));
                stack.push((i, k));
            }
            Choice::Quad(k, l) => {
                let (k, l) = (k as usize, l as usize);
                out.push(vec![i as u32, k as u32, l as u32, j as u32]);
                stack.push((l, j));
                stack.push((k, l));
                stack.push((i, k));
            }
        }
    }
    Some(out)
}
