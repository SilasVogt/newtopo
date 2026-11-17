//! Auto-fill (SPEC §7): decompose simple polygons into quads and triangles
//! using only existing vertices. OWNER: fill.
//!
//! Pipeline per polygon:
//! 1. Canonical order: rotate so the lowest-`rank` vertex comes first (ties
//!    by input index) and make the polygon counter-clockwise in UV. Every
//!    later stage iterates in this order, so results depend only on the
//!    polygon and its vertex keys, not on where the caller started it.
//! 2. `n <= 48`: exact dynamic program ([`dp`]); `n > 48`: constrained
//!    Delaunay + greedy quad pairing ([`cdt`]).
//! 3. Fallbacks: ear clipping in UV, ear clipping in the best-fit 3D plane,
//!    then a fan from the first (lowest-rank) vertex.
//!
//! Output polygons keep the input winding: if the input is clockwise in UV,
//! the polygons are clockwise too (their edges run the same way as the
//! input's boundary edges).

mod cdt;
mod dp;
mod ear;
pub mod geom;
mod quality;

use glam::{DVec2, DVec3};

/// Largest polygon filled by the exact DP (SPEC §7 step 3).
pub const DP_MAX_SIDES: usize = 48;

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

impl FillResult {
    /// True if only the fan fallback could fill it (flagged by validation).
    pub fn is_fan(&self) -> bool {
        self.method == "fan"
    }
    pub fn quads(&self) -> usize {
        self.polys.iter().filter(|p| p.len() == 4).count()
    }
}

/// Fill one simple polygon. Never fails; falls back to a fan (SPEC §7).
pub fn fill_polygon(input: &FillInput) -> FillResult {
    let n = input.uv.len();
    if n < 3 {
        return FillResult {
            polys: Vec::new(),
            method: "fan",
        };
    }
    let rank_of = |i: usize| input.rank.get(i).copied().unwrap_or(i as u32);
    let start = (0..n).min_by_key(|&i| (rank_of(i), i)).unwrap_or(0);
    let mut order: Vec<usize> = (0..n).map(|k| (start + k) % n).collect();
    let mut luv: Vec<DVec2> = order.iter().map(|&i| input.uv[i]).collect();
    let area2 = geom::signed_area2(&luv);
    let flipped = area2 < 0.0;
    if flipped {
        // Keep the first vertex, reverse the rest.
        order[1..].reverse();
        luv[1..].reverse();
    }
    let lpos: Vec<DVec3> = order
        .iter()
        .map(|&i| {
            input
                .pos
                .get(i)
                .copied()
                .unwrap_or(DVec3::new(input.uv[i].x, input.uv[i].y, 0.0))
        })
        .collect();

    let (local, method) = fill_local(&luv, &lpos, input.iso_dirs);
    let polys = local
        .into_iter()
        .map(|p| {
            let mut q: Vec<u32> = p.iter().map(|&k| order[k as usize] as u32).collect();
            if flipped {
                q.reverse();
            }
            q
        })
        .collect();
    FillResult { polys, method }
}

/// Fill in canonical local order (counter-clockwise in UV unless degenerate).
fn fill_local(uv: &[DVec2], pos: &[DVec3], iso: [DVec2; 2]) -> (Vec<Vec<u32>>, &'static str) {
    let n = uv.len();
    if n == 3 {
        return (vec![vec![0, 1, 2]], "dp");
    }
    let uv_ok = uv.iter().all(|p| p.is_finite()) && geom::signed_area2(uv) > 0.0;
    if uv_ok {
        let eps = area_epsilon(uv);
        let r = if n <= DP_MAX_SIDES {
            dp::dp_fill(uv, &quality::Scorer::with_tables(uv, pos, iso), eps).map(|p| (p, "dp"))
        } else {
            cdt::cdt_fill(uv, &quality::Scorer::new(uv, pos, iso), eps).map(|p| (p, "cdt"))
        };
        if let Some(r) = r {
            return r;
        }
        if let Some(t) = ear::ear_clip(uv) {
            return (tris_to_polys(t), "ear-uv");
        }
    }
    if let Some(t) = geom::project_best_fit(pos).and_then(|p| ear::ear_clip(&p)) {
        return (tris_to_polys(t), "ear-3d");
    }
    (
        (1..n - 1)
            .map(|i| vec![0, i as u32, i as u32 + 1])
            .collect(),
        "fan",
    )
}

fn tris_to_polys(t: Vec<[usize; 3]>) -> Vec<Vec<u32>> {
    t.into_iter()
        .map(|t| t.iter().map(|&x| x as u32).collect())
        .collect()
}

/// "Tiny epsilon" for quad admissibility: relative to the polygon's extent.
fn area_epsilon(uv: &[DVec2]) -> f64 {
    let (mut lo, mut hi) = (uv[0], uv[0]);
    for p in uv {
        lo = lo.min(*p);
        hi = hi.max(*p);
    }
    1e-12 * (hi - lo).length_squared()
}

/// Triangulate a 3D polygon for export (no UV available): quads split along
/// the shorter valid diagonal, larger polygons ear-clipped in their best-fit
/// plane, a fan as last resort. Triangles keep the polygon's winding.
pub fn triangulate_3d(pos: &[DVec3]) -> Vec<[u32; 3]> {
    let n = pos.len();
    match n {
        0..=2 => Vec::new(),
        3 => vec![[0, 1, 2]],
        _ => {
            if n == 4 {
                let nrm = geom::newell_normal(pos);
                let ok = |a: usize, b: usize, c: usize| {
                    (pos[b] - pos[a]).cross(pos[c] - pos[a]).dot(nrm) > 0.0
                };
                let split02 = ok(0, 1, 2) && ok(0, 2, 3);
                let split13 = ok(0, 1, 3) && ok(1, 2, 3);
                let d02 = pos[0].distance_squared(pos[2]);
                let d13 = pos[1].distance_squared(pos[3]);
                if split02 && (!split13 || d02 <= d13) {
                    return vec![[0, 1, 2], [0, 2, 3]];
                }
                if split13 {
                    return vec![[0, 1, 3], [1, 2, 3]];
                }
            }
            if let Some(t) = geom::project_best_fit(pos).and_then(|p| ear::ear_clip(&p)) {
                return t
                    .into_iter()
                    .map(|t| [t[0] as u32, t[1] as u32, t[2] as u32])
                    .collect();
            }
            (1..n - 1).map(|i| [0, i as u32, i as u32 + 1]).collect()
        }
    }
}

#[cfg(test)]
mod tests;
