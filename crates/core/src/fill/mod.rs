//! Auto-fill (SPEC §7): decompose simple polygons into quads and triangles
//! using only existing vertices. OWNER: fill.

use glam::{DVec2, DVec3};

/// One simple polygon to fill, in a planar domain.
pub struct FillInput<'a> {
    /// Domain coordinates (metric-rescaled UV, or flattened), counter-clockwise.
    pub uv: &'a [DVec2],
    /// 3D positions, same order (quality: planarity, aspect).
    pub pos: &'a [DVec3],
    /// Tie-break rank per vertex (from vertex key order); lower first.
    pub rank: &'a [u32],
    /// Unit iso directions in the domain for edge-alignment quality.
    pub iso_dirs: [DVec2; 2],
}

/// Result: polygons as index lists into the input (each 3 or 4 long, CCW).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FillResult {
    pub polys: Vec<Vec<u32>>,
    /// Which stage produced it: "dp", "cdt", "ear-uv", "ear-3d", "fan".
    pub method: &'static str,
}

/// Fill one simple polygon. Never fails; falls back to a fan (SPEC §7).
// TEMP(engine): simple ear clipping + greedy convex quad pairing so the
// engine tests run; the fill worker's implementation replaces this body.
pub fn fill_polygon(input: &FillInput) -> FillResult {
    let n = input.uv.len();
    if n <= 4 {
        return FillResult { polys: vec![(0..n as u32).collect()], method: "dp" };
    }
    let p = input.uv;
    let orient = |a: usize, b: usize, c: usize| (p[b] - p[a]).perp_dot(p[c] - p[a]);
    let mut idx: Vec<usize> = (0..n).collect();
    let mut tris: Vec<[usize; 3]> = Vec::new();
    let mut guard = 0;
    let mut method = "ear-uv";
    while idx.len() > 3 && guard < n * n {
        guard += 1;
        let m = idx.len();
        let mut best: Option<(f64, usize)> = None;
        for k in 0..m {
            let (a, b, c) = (idx[(k + m - 1) % m], idx[k], idx[(k + 1) % m]);
            if orient(a, b, c) <= 0.0 {
                continue;
            }
            let ear = idx.iter().all(|&q| {
                q == a || q == b || q == c || !(orient(a, b, q) >= 0.0 && orient(b, c, q) >= 0.0 && orient(c, a, q) >= 0.0)
            });
            if !ear {
                continue;
            }
            // Prefer well-shaped ears: maximise the minimum angle proxy.
            let (la, lb, lc) = ((p[b] - p[a]).length(), (p[c] - p[b]).length(), (p[a] - p[c]).length());
            let q = orient(a, b, c) / (la * la + lb * lb + lc * lc).max(1e-300);
            if best.is_none_or(|(bq, _)| q > bq) {
                best = Some((q, k));
            }
        }
        let Some((_, k)) = best else {
            method = "fan";
            break;
        };
        let m = idx.len();
        tris.push([idx[(k + m - 1) % m], idx[k], idx[(k + 1) % m]]);
        idx.remove(k);
    }
    if method == "fan" {
        let polys = (1..n - 1).map(|i| vec![0, i as u32, i as u32 + 1]).collect();
        return FillResult { polys, method };
    }
    tris.push([idx[0], idx[1], idx[2]]);
    // Greedy pairing of adjacent triangles into strictly convex quads.
    let mut used = vec![false; tris.len()];
    let mut polys: Vec<Vec<u32>> = Vec::new();
    for i in 0..tris.len() {
        if used[i] {
            continue;
        }
        let mut paired = false;
        for j in i + 1..tris.len() {
            if used[j] {
                continue;
            }
            if let Some(q) = quad_of(&tris[i], &tris[j])
                && (0..4).all(|k| orient(q[k], q[(k + 1) % 4], q[(k + 2) % 4]) > 1e-12)
            {
                polys.push(q.iter().map(|&x| x as u32).collect());
                used[i] = true;
                used[j] = true;
                paired = true;
                break;
            }
        }
        if !paired {
            used[i] = true;
            polys.push(tris[i].iter().map(|&x| x as u32).collect());
        }
    }
    FillResult { polys, method }
}

/// Two CCW triangles sharing an edge → the CCW quad.
fn quad_of(t1: &[usize; 3], t2: &[usize; 3]) -> Option<[usize; 4]> {
    for k in 0..3 {
        let (a, b) = (t1[k], t1[(k + 1) % 3]);
        for m in 0..3 {
            if t2[m] == b && t2[(m + 1) % 3] == a {
                let c = t1[(k + 2) % 3];
                let d = t2[(m + 2) % 3];
                return Some([a, d, b, c]);
            }
        }
    }
    None
}
