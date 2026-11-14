//! Exact 2D predicates and polygon diagonal validity (SPEC §7 step 3).
//!
//! Orientation uses Shewchuk's adaptive exact predicate through the `robust`
//! crate, so every validity decision is exact for the given `f64` input.

use glam::DVec2;

/// Exact orientation: > 0 if `a, b, c` turn counter-clockwise, < 0 if
/// clockwise, 0 if collinear.
#[inline]
pub fn orient(a: DVec2, b: DVec2, c: DVec2) -> f64 {
    robust::orient2d(
        robust::Coord { x: a.x, y: a.y },
        robust::Coord { x: b.x, y: b.y },
        robust::Coord { x: c.x, y: c.y },
    )
}

/// Twice the signed area (shoelace). Positive for counter-clockwise.
pub fn signed_area2(pts: &[DVec2]) -> f64 {
    let n = pts.len();
    let mut s = 0.0;
    for i in 0..n {
        let a = pts[i];
        let b = pts[(i + 1) % n];
        s += a.x * b.y - a.y * b.x;
    }
    s
}

/// True if `p` lies on the closed segment `a-b`, given that the three points
/// are already known to be collinear.
#[inline]
fn on_segment_collinear(a: DVec2, b: DVec2, p: DVec2) -> bool {
    p.x >= a.x.min(b.x) && p.x <= a.x.max(b.x) && p.y >= a.y.min(b.y) && p.y <= a.y.max(b.y)
}

/// Closed segment intersection (touching counts), exact.
pub fn segments_touch(p1: DVec2, p2: DVec2, q1: DVec2, q2: DVec2) -> bool {
    // Bounding boxes must overlap (exact comparisons, so this is safe).
    if p1.x.max(p2.x) < q1.x.min(q2.x)
        || q1.x.max(q2.x) < p1.x.min(p2.x)
        || p1.y.max(p2.y) < q1.y.min(q2.y)
        || q1.y.max(q2.y) < p1.y.min(p2.y)
    {
        return false;
    }
    let o1 = orient(p1, p2, q1);
    let o2 = orient(p1, p2, q2);
    if (o1 > 0.0 && o2 > 0.0) || (o1 < 0.0 && o2 < 0.0) {
        return false;
    }
    let o3 = orient(q1, q2, p1);
    let o4 = orient(q1, q2, p2);
    if (o3 > 0.0 && o4 > 0.0) || (o3 < 0.0 && o4 < 0.0) {
        return false;
    }
    if o1 != 0.0 && o2 != 0.0 && o3 != 0.0 && o4 != 0.0 {
        return true; // proper crossing
    }
    (o1 == 0.0 && on_segment_collinear(p1, p2, q1))
        || (o2 == 0.0 && on_segment_collinear(p1, p2, q2))
        || (o3 == 0.0 && on_segment_collinear(q1, q2, p1))
        || (o4 == 0.0 && on_segment_collinear(q1, q2, p2))
}

/// Strict "in cone" test: is the direction from vertex `i` towards `p`
/// strictly inside the interior angle at `i` of a counter-clockwise polygon
/// with neighbours `prev` and `next`? Directions collinear with an incident
/// edge are rejected.
pub fn in_cone(prev: DVec2, v: DVec2, next: DVec2, p: DVec2) -> bool {
    // The interior sweeps counter-clockwise from (next - v) to (prev - v).
    if orient(v, next, prev) > 0.0 {
        // Convex corner (< 180 degrees).
        orient(v, next, p) > 0.0 && orient(v, p, prev) > 0.0
    } else {
        // Reflex or straight corner: inside unless in the closed complement,
        // which sweeps counter-clockwise from (prev - v) to (next - v).
        !(orient(v, prev, p) >= 0.0 && orient(v, p, next) >= 0.0)
    }
}

/// Closed point-in-triangle test for a counter-clockwise triangle.
#[inline]
pub fn in_triangle_closed(a: DVec2, b: DVec2, c: DVec2, p: DVec2) -> bool {
    orient(a, b, p) >= 0.0 && orient(b, c, p) >= 0.0 && orient(c, a, p) >= 0.0
}

/// Strictly convex counter-clockwise quad (every corner a strict left turn).
#[inline]
pub fn strictly_convex_quad(a: DVec2, b: DVec2, c: DVec2, d: DVec2) -> bool {
    orient(a, b, c) > 0.0 && orient(b, c, d) > 0.0 && orient(c, d, a) > 0.0 && orient(d, a, b) > 0.0
}

/// Is chord `i-j` a valid interior diagonal of the simple counter-clockwise
/// polygon `pts`? Strict: it must not touch any vertex or edge except at its
/// two ends and must leave both ends strictly inside the interior angle.
pub fn is_diagonal(pts: &[DVec2], i: usize, j: usize) -> bool {
    let n = pts.len();
    if i == j || (i + 1) % n == j || (j + 1) % n == i {
        return false;
    }
    let (pi, pj) = (pts[i], pts[j]);
    if pi == pj {
        return false;
    }
    if !in_cone(pts[(i + n - 1) % n], pi, pts[(i + 1) % n], pj)
        || !in_cone(pts[(j + n - 1) % n], pj, pts[(j + 1) % n], pi)
    {
        return false;
    }
    for k in 0..n {
        let k2 = (k + 1) % n;
        if k == i || k == j || k2 == i || k2 == j {
            continue;
        }
        if segments_touch(pi, pj, pts[k], pts[k2]) {
            return false;
        }
    }
    true
}

/// Validity table for all pairs: polygon edges and valid diagonals are
/// `true`. Row-major `n * n`, symmetric.
pub fn diagonal_table(pts: &[DVec2]) -> Vec<bool> {
    let n = pts.len();
    let mut t = vec![false; n * n];
    for i in 0..n {
        let j = (i + 1) % n;
        t[i * n + j] = true;
        t[j * n + i] = true;
    }
    for i in 0..n {
        for j in i + 2..n {
            if i == 0 && j == n - 1 {
                continue;
            }
            if is_diagonal(pts, i, j) {
                t[i * n + j] = true;
                t[j * n + i] = true;
            }
        }
    }
    t
}

/// Unit Newell normal of a 3D polygon (zero if degenerate).
pub fn newell_normal(pos: &[glam::DVec3]) -> glam::DVec3 {
    let n = pos.len();
    let mut s = glam::DVec3::ZERO;
    for i in 0..n {
        let a = pos[i];
        let b = pos[(i + 1) % n];
        s.x += (a.y - b.y) * (a.z + b.z);
        s.y += (a.z - b.z) * (a.x + b.x);
        s.z += (a.x - b.x) * (a.y + b.y);
    }
    s.normalize_or_zero()
}

/// Project a 3D polygon onto its best-fit (Newell) plane so that it winds
/// counter-clockwise in 2D. `None` if the polygon has no usable plane.
pub fn project_best_fit(pos: &[glam::DVec3]) -> Option<Vec<DVec2>> {
    let nrm = newell_normal(pos);
    if nrm == glam::DVec3::ZERO || !nrm.is_finite() {
        return None;
    }
    let u = nrm.any_orthonormal_vector();
    let v = nrm.cross(u);
    let c = pos.iter().copied().sum::<glam::DVec3>() / pos.len() as f64;
    Some(
        pos.iter()
            .map(|p| DVec2::new((*p - c).dot(u), (*p - c).dot(v)))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64) -> DVec2 {
        DVec2::new(x, y)
    }

    #[test]
    fn cone_straight_corner() {
        let (a, i, b) = (v(-1.0, 0.0), v(0.0, 0.0), v(1.0, 0.0));
        assert!(in_cone(a, i, b, v(0.0, 1.0)));
        assert!(!in_cone(a, i, b, v(0.0, -1.0)));
        assert!(!in_cone(a, i, b, v(2.0, 0.0)));
    }

    #[test]
    fn diagonal_through_vertex_rejected() {
        // Square with a midpoint on the bottom edge: (0,0)-(2,0) chord would
        // pass along the edge; (1,0)-(1,2)... only proper diagonals count.
        let p = [
            v(0.0, 0.0),
            v(1.0, 0.0),
            v(2.0, 0.0),
            v(2.0, 2.0),
            v(0.0, 2.0),
        ];
        assert!(!is_diagonal(&p, 0, 2));
        assert!(is_diagonal(&p, 1, 3));
        assert!(is_diagonal(&p, 1, 4));
        assert!(is_diagonal(&p, 0, 3));
    }

    #[test]
    fn diagonal_outside_reflex_rejected() {
        // Arrow / chevron: reflex vertex 3.
        let p = [v(0.0, 0.0), v(2.0, 1.0), v(4.0, 0.0), v(2.0, 3.0)];
        assert!(signed_area2(&p) > 0.0);
        let p = [
            v(0.0, 0.0),
            v(4.0, 0.0),
            v(2.0, 1.0),
            v(4.0, 3.0),
            v(0.0, 3.0),
        ];
        // 1-3 crosses outside around reflex vertex 2.
        assert!(!is_diagonal(&p, 1, 3));
        assert!(is_diagonal(&p, 2, 0));
        assert!(is_diagonal(&p, 2, 4));
    }
}
