//! 2D helpers in parameter space. Orientation tests use Shewchuk's adaptive
//! exact predicates (`robust` crate); constructions are plain f64.

use glam::DVec2;
use robust::{Coord, orient2d};

#[inline]
pub fn orient(a: DVec2, b: DVec2, c: DVec2) -> f64 {
    orient2d(
        Coord { x: a.x, y: a.y },
        Coord { x: b.x, y: b.y },
        Coord { x: c.x, y: c.y },
    )
}

pub fn dist_point_seg(p: DVec2, a: DVec2, b: DVec2) -> (f64, f64) {
    let ab = b - a;
    let l2 = ab.length_squared();
    let t = if l2 > 0.0 {
        ((p - a).dot(ab) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    ((a + ab * t - p).length(), t)
}

/// Intersections of segments `ab` and `cd` as `(s, t)` parameters on each
/// (0..1). Proper crossings use exact orientation signs; endpoint contacts
/// within `eps` are reported as touches; collinear overlaps report their
/// end points.
pub fn seg_hits(a: DVec2, b: DVec2, c: DVec2, d: DVec2, eps: f64) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = Vec::new();
    let lo1 = a.min(b) - DVec2::splat(eps);
    let hi1 = a.max(b) + DVec2::splat(eps);
    let lo2 = c.min(d);
    let hi2 = c.max(d);
    if lo1.x > hi2.x || lo1.y > hi2.y || lo2.x > hi1.x || lo2.y > hi1.y {
        return out;
    }
    fn push(out: &mut Vec<(f64, f64)>, s: f64, t: f64) {
        if !out
            .iter()
            .any(|&(s2, t2)| (s2 - s).abs() < 1e-9 && (t2 - t).abs() < 1e-9)
        {
            out.push((s, t));
        }
    }
    // Endpoint contacts.
    for (p, s) in [(a, 0.0), (b, 1.0)] {
        let (dd, t) = dist_point_seg(p, c, d);
        if dd <= eps {
            push(&mut out, s, t);
        }
    }
    for (p, t) in [(c, 0.0), (d, 1.0)] {
        let (dd, s) = dist_point_seg(p, a, b);
        if dd <= eps {
            push(&mut out, s, t);
        }
    }
    let o1 = orient(a, b, c);
    let o2 = orient(a, b, d);
    let o3 = orient(c, d, a);
    let o4 = orient(c, d, b);
    if ((o1 > 0.0 && o2 < 0.0) || (o1 < 0.0 && o2 > 0.0))
        && ((o3 > 0.0 && o4 < 0.0) || (o3 < 0.0 && o4 > 0.0))
    {
        let s = o3 / (o3 - o4);
        let t = o1 / (o1 - o2);
        // Skip if this is the same point as an endpoint contact already found.
        let p = a + (b - a) * s;
        let dup = out
            .iter()
            .any(|&(s2, _)| (a + (b - a) * s2 - p).length() <= eps);
        if !dup {
            push(&mut out, s, t);
        }
    }
    out
}

pub fn signed_area(poly: &[DVec2]) -> f64 {
    let n = poly.len();
    if n < 3 {
        return 0.0;
    }
    let mut s = 0.0;
    for i in 0..n {
        s += poly[i].perp_dot(poly[(i + 1) % n]);
    }
    0.5 * s
}

/// Even–odd point in polygon.
pub fn point_in_poly(p: DVec2, poly: &[DVec2]) -> bool {
    let n = poly.len();
    let mut inside = false;
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        if (a.y > p.y) != (b.y > p.y) {
            let x = a.x + (p.y - a.y) * (b.x - a.x) / (b.y - a.y);
            if p.x < x {
                inside = !inside;
            }
        }
    }
    inside
}

/// Counter-clockwise angle from `a` to `b` in (0, 2π]; equal directions give 2π.
pub fn ccw_angle(a: DVec2, b: DVec2) -> f64 {
    let ang = a.perp_dot(b).atan2(a.dot(b));
    if ang <= 1e-15 {
        ang + std::f64::consts::TAU
    } else {
        ang
    }
}

/// Polyline length.
pub fn poly_len(p: &[DVec2]) -> f64 {
    p.windows(2).map(|w| (w[1] - w[0]).length()).sum()
}

/// Point on a polyline at parameter `s` (segment index + fraction).
pub fn poly_at(p: &[DVec2], s: f64) -> DVec2 {
    if p.len() == 1 {
        return p[0];
    }
    let n = p.len() - 1;
    let s = s.clamp(0.0, n as f64);
    let i = (s.floor() as usize).min(n - 1);
    let f = s - i as f64;
    p[i] + (p[i + 1] - p[i]) * f
}

/// Sub-polyline between parameters `s0 < s1` (segment index + fraction).
pub fn poly_slice(p: &[DVec2], s0: f64, s1: f64) -> Vec<DVec2> {
    let mut out = vec![poly_at(p, s0)];
    let first = s0.floor() as usize + 1;
    let mut i = first;
    while (i as f64) < s1 && i < p.len() {
        if (i as f64) > s0 {
            out.push(p[i]);
        }
        i += 1;
    }
    out.push(poly_at(p, s1));
    out
}

/// Interpolate a monotone parameter table: given points' parameters `par`
/// (increasing), map a polyline parameter `s` to the curve parameter.
pub fn par_at(par: &[f64], s: f64) -> f64 {
    if par.len() == 1 {
        return par[0];
    }
    let n = par.len() - 1;
    let s = s.clamp(0.0, n as f64);
    let i = (s.floor() as usize).min(n - 1);
    let f = s - i as f64;
    par[i] + (par[i + 1] - par[i]) * f
}

/// Inverse of [`par_at`]: polyline parameter for a curve parameter.
pub fn s_of_par(par: &[f64], t: f64) -> f64 {
    let n = par.len();
    if n < 2 {
        return 0.0;
    }
    let inc = par[n - 1] >= par[0];
    for i in 0..n - 1 {
        let (a, b) = (par[i], par[i + 1]);
        let inside = if inc {
            t >= a && t <= b
        } else {
            t <= a && t >= b
        };
        if inside {
            let f = if (b - a).abs() > 0.0 {
                (t - a) / (b - a)
            } else {
                0.0
            };
            return i as f64 + f;
        }
    }
    if (t - par[0]).abs() < (t - par[n - 1]).abs() {
        0.0
    } else {
        (n - 1) as f64
    }
}
