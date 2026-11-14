//! Ear clipping fallback (SPEC §7, fallback chain). Textbook ear clipping
//! with exact predicates; prefers the best-shaped ear on small polygons.

use super::geom::{in_triangle_closed, orient};
use glam::DVec2;

/// Ear size up to which the best-shaped ear is chosen (else the first one).
const BEST_EAR_MAX: usize = 64;

/// Triangulate the counter-clockwise polygon `pts`. `None` if it gets stuck
/// (the polygon is not simple, or is degenerate).
pub fn ear_clip(pts: &[DVec2]) -> Option<Vec<[usize; 3]>> {
    let n = pts.len();
    if n < 3 || pts.iter().any(|p| !p.is_finite()) {
        return None;
    }
    let mut prev: Vec<usize> = (0..n).map(|i| (i + n - 1) % n).collect();
    let mut next: Vec<usize> = (0..n).map(|i| (i + 1) % n).collect();
    let mut alive = vec![true; n];
    let mut remaining = n;
    let mut out = Vec::with_capacity(n - 2);
    let mut cursor = 0usize;

    let reflex = |prev: &[usize], next: &[usize], i: usize| {
        orient(pts[prev[i]], pts[i], pts[next[i]]) <= 0.0
    };
    let is_ear = |prev: &[usize], next: &[usize], alive: &[bool], i: usize| -> bool {
        let (p, q) = (prev[i], next[i]);
        let (a, b, c) = (pts[p], pts[i], pts[q]);
        if orient(a, b, c) <= 0.0 {
            return false;
        }
        for r in 0..n {
            if !alive[r] || r == p || r == i || r == q {
                continue;
            }
            if reflex(prev, next, r) && in_triangle_closed(a, b, c, pts[r]) {
                return false;
            }
        }
        true
    };
    // Shape of the ear at i: smallest corner sine (bigger is better).
    let shape = |prev: &[usize], next: &[usize], i: usize| -> f64 {
        let (a, b, c) = (pts[prev[i]], pts[i], pts[next[i]]);
        let corner = |p: DVec2, q: DVec2, r: DVec2| {
            let u = (p - q).normalize_or_zero();
            let v = (r - q).normalize_or_zero();
            u.perp_dot(v).abs()
        };
        corner(c, a, b).min(corner(a, b, c)).min(corner(b, c, a))
    };

    while remaining > 3 {
        let mut pick = None;
        if remaining <= BEST_EAR_MAX {
            let mut best = f64::NEG_INFINITY;
            let mut i = cursor;
            for _ in 0..remaining {
                if is_ear(&prev, &next, &alive, i) {
                    let s = shape(&prev, &next, i);
                    if s > best + 1e-12 {
                        best = s;
                        pick = Some(i);
                    }
                }
                i = next[i];
            }
        } else {
            let mut i = cursor;
            for _ in 0..remaining {
                if is_ear(&prev, &next, &alive, i) {
                    pick = Some(i);
                    break;
                }
                i = next[i];
            }
        }
        let i = pick?;
        let (p, q) = (prev[i], next[i]);
        out.push([p, i, q]);
        alive[i] = false;
        next[p] = q;
        prev[q] = p;
        remaining -= 1;
        cursor = q;
    }
    let a = (0..n).find(|&i| alive[i])?;
    let (b, c) = (next[a], next[next[a]]);
    if orient(pts[a], pts[b], pts[c]) <= 0.0 {
        return None;
    }
    out.push([a, b, c]);
    Some(out)
}
