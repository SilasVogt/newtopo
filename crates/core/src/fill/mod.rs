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
pub fn fill_polygon(input: &FillInput) -> FillResult {
    let _ = input;
    todo!("fill")
}
