//! What the renderer draws: plain world-space triangles, line segments,
//! points and translucent surfaces. The app (and the CLI) build a
//! [`RenderScene`] from the core's `Scene`; the renderer never sees core types.
//!
//! All colours are sRGB with straight alpha, 0..1.

use bytemuck::{Pod, Zeroable};
use glam::{Vec3, Vec4};
use serde::{Deserialize, Serialize};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct MeshVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 4],
}

/// One screen-space-width line segment.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct LineSeg {
    pub a: [f32; 3],
    /// Width in logical pixels.
    pub width: f32,
    pub b: [f32; 3],
    /// 0 = solid, otherwise dash period in pixels.
    pub dash: f32,
    pub color: [f32; 4],
}

/// A round dot of fixed screen size.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct PointMark {
    pub pos: [f32; 3],
    /// Diameter in logical pixels.
    pub size: f32,
    pub color: [f32; 4],
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TriMesh {
    pub vertices: Vec<MeshVertex>,
    pub indices: Vec<u32>,
}

impl TriMesh {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Append another mesh.
    pub fn extend(&mut self, other: &TriMesh) {
        let base = self.vertices.len() as u32;
        self.vertices.extend_from_slice(&other.vertices);
        self.indices.extend(other.indices.iter().map(|i| i + base));
    }

    pub fn push_triangle(&mut self, p: [Vec3; 3], n: [Vec3; 3], color: [f32; 4]) {
        let base = self.vertices.len() as u32;
        for k in 0..3 {
            self.vertices.push(MeshVertex {
                pos: p[k].to_array(),
                normal: n[k].to_array(),
                color,
            });
        }
        self.indices.extend([base, base + 1, base + 2]);
    }
}

/// Everything one draw of the viewport needs, in world space.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RenderScene {
    /// Opaque shaded surfaces.
    pub mesh: TriMesh,
    /// Translucent surfaces (symmetry planes, selection tints), drawn after
    /// the opaque pass with a small depth bias and no depth writes.
    pub translucent: TriMesh,
    pub lines: Vec<LineSeg>,
    pub points: Vec<PointMark>,
}

impl RenderScene {
    pub fn is_empty(&self) -> bool {
        self.mesh.is_empty()
            && self.translucent.is_empty()
            && self.lines.is_empty()
            && self.points.is_empty()
    }

    pub fn clear(&mut self) {
        *self = RenderScene::default();
    }

    pub fn add_segment(&mut self, a: Vec3, b: Vec3, color: [f32; 4], width: f32) {
        self.lines.push(LineSeg {
            a: a.to_array(),
            width,
            b: b.to_array(),
            dash: 0.0,
            color,
        });
    }

    pub fn add_dashed_segment(
        &mut self,
        a: Vec3,
        b: Vec3,
        color: [f32; 4],
        width: f32,
        period_px: f32,
    ) {
        self.lines.push(LineSeg {
            a: a.to_array(),
            width,
            b: b.to_array(),
            dash: period_px,
            color,
        });
    }

    pub fn add_polyline(&mut self, pts: &[Vec3], color: [f32; 4], width: f32) {
        for w in pts.windows(2) {
            self.add_segment(w[0], w[1], color, width);
        }
    }

    pub fn add_point(&mut self, p: Vec3, color: [f32; 4], size: f32) {
        self.points.push(PointMark {
            pos: p.to_array(),
            size,
            color,
        });
    }

    /// An arrow lying on a surface: shaft from `from` along `dir` with length
    /// `len`, head opening in the plane spanned by `dir` and `side`.
    pub fn add_arrow(
        &mut self,
        from: Vec3,
        dir: Vec3,
        side: Vec3,
        len: f32,
        color: [f32; 4],
        width: f32,
    ) {
        let d = dir.normalize_or_zero();
        let s = side.normalize_or_zero();
        if d == Vec3::ZERO {
            return;
        }
        let tip = from + d * len;
        let head = len * 0.32;
        self.add_segment(from, tip, color, width);
        self.add_segment(tip, tip - d * head + s * head * 0.55, color, width);
        self.add_segment(tip, tip - d * head - s * head * 0.55, color, width);
    }

    /// A translucent square plane through `center` with normal `normal`.
    pub fn add_plane(&mut self, center: Vec3, normal: Vec3, half: f32, color: [f32; 4]) {
        let n = normal.normalize_or_zero();
        if n == Vec3::ZERO {
            return;
        }
        let t = n.any_orthonormal_vector();
        let b = n.cross(t);
        let c = [
            center + (t + b) * half,
            center + (t - b) * half,
            center + (-t - b) * half,
            center + (-t + b) * half,
        ];
        self.translucent
            .push_triangle([c[0], c[1], c[2]], [n; 3], color);
        self.translucent
            .push_triangle([c[0], c[2], c[3]], [n; 3], color);
    }

    /// World-space bounds of everything drawn (None when empty).
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        let mut any = false;
        let mut add = |p: [f32; 3]| {
            let p = Vec3::from_array(p);
            min = min.min(p);
            max = max.max(p);
            any = true;
        };
        for v in &self.mesh.vertices {
            add(v.pos);
        }
        for l in &self.lines {
            add(l.a);
            add(l.b);
        }
        for p in &self.points {
            add(p.pos);
        }
        any.then_some((min, max))
    }
}

/// Overlay colours (SPEC §5.7 "every overlay colour"). sRGB + alpha.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OverlayColors {
    pub cad_edge: [f32; 4],
    pub line: [f32; 4],
    pub fill_edge: [f32; 4],
    pub selection: [f32; 4],
    pub hover: [f32; 4],
    pub vertex: [f32; 4],
    pub arrow_a: [f32; 4],
    pub arrow_b: [f32; 4],
    pub linked_tint: [f32; 4],
    pub symmetry_plane: [f32; 4],
    pub split_preview: [f32; 4],
    pub warning: [f32; 4],
    pub weld_from: [f32; 4],
    pub weld_to: [f32; 4],
    pub preview_surface: [f32; 4],
    pub preview_wire: [f32; 4],
    pub background_top: [f32; 4],
    pub background_bottom: [f32; 4],
    pub grid: [f32; 4],
}

impl Default for OverlayColors {
    fn default() -> Self {
        OverlayColors {
            cad_edge: hex("#1c2026", 1.0),
            line: hex("#f2a93b", 1.0),
            fill_edge: hex("#4fb6c7", 0.85),
            selection: hex("#ffd84a", 1.0),
            hover: hex("#ffffff", 0.9),
            vertex: hex("#e9edf2", 1.0),
            arrow_a: hex("#ff6b6b", 1.0),
            arrow_b: hex("#5ec2ff", 1.0),
            linked_tint: hex("#8f7cff", 0.35),
            symmetry_plane: hex("#5ec2ff", 0.08),
            split_preview: hex("#ff5fa2", 1.0),
            warning: hex("#ff7a45", 0.45),
            weld_from: hex("#ff6b6b", 1.0),
            weld_to: hex("#6be08a", 1.0),
            preview_surface: hex("#aeb6c2", 1.0),
            preview_wire: hex("#20242b", 1.0),
            background_top: hex("#2b3038", 1.0),
            background_bottom: hex("#16191e", 1.0),
            grid: hex("#ffffff", 0.035),
        }
    }
}

impl OverlayColors {
    /// Light-theme friendly defaults.
    pub fn light() -> Self {
        OverlayColors {
            cad_edge: hex("#262a31", 1.0),
            background_top: hex("#eef1f5", 1.0),
            background_bottom: hex("#c9d0da", 1.0),
            grid: hex("#000000", 0.06),
            hover: hex("#1d6fe0", 0.9),
            selection: hex("#f0a500", 1.0),
            ..OverlayColors::default()
        }
    }
}

/// Parse `#rrggbb` into sRGB + alpha. Invalid input gives magenta.
pub fn hex(s: &str, alpha: f32) -> [f32; 4] {
    let s = s.trim_start_matches('#');
    let p = |i: usize| {
        u8::from_str_radix(s.get(i..i + 2).unwrap_or("ff"), 16).unwrap_or(255) as f32 / 255.0
    };
    if s.len() != 6 {
        return [1.0, 0.0, 1.0, alpha];
    }
    [p(0), p(2), p(4), alpha]
}

pub fn with_alpha(c: [f32; 4], a: f32) -> [f32; 4] {
    [c[0], c[1], c[2], a]
}

/// Mix two sRGB colours (in sRGB space; good enough for tints).
pub fn mix(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let x = Vec4::from_array(a).lerp(Vec4::from_array(b), t);
    x.to_array()
}

/// How the viewport shades (SPEC §5.2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShadeMode {
    /// Smooth normals as given.
    #[default]
    Smooth,
    /// Per-triangle normals from screen-space derivatives.
    Flat,
}

/// Per-draw options.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderOptions {
    pub shade: ShadeMode,
    pub background_top: [f32; 4],
    pub background_bottom: [f32; 4],
    /// Pixel scale (logical → physical pixels) for line widths and dots.
    pub pixels_per_point: f32,
    /// Fraction of view depth overlays move towards the camera.
    pub overlay_bias: f32,
}

impl Default for RenderOptions {
    fn default() -> Self {
        let c = OverlayColors::default();
        RenderOptions {
            shade: ShadeMode::Smooth,
            background_top: c.background_top,
            background_bottom: c.background_bottom,
            pixels_per_point: 1.0,
            overlay_bias: 0.0015,
        }
    }
}
