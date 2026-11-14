//! CPU picking.
//!
//! SPEC §5.2 asks for GPU id-buffer picking refined on the CPU. This module
//! is the CPU half and currently does the whole job: a BVH over the display
//! triangles answers "which body/face is under the cursor" (ray cast), and
//! screen-space searches find the nearest edge or vertex. Exact anchors are
//! refined afterwards through the core (`project_to_face` etc.). A GPU id
//! pass can later replace the ray cast for the first hit without changing
//! this API.

use crate::camera::{CameraMatrices, Ray};
use glam::{DVec2, DVec3, Vec3};

/// Identity of a picked triangle: (body, face) as plain u32s.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FaceRef {
    pub body: u32,
    pub face: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub tri: u32,
    pub face: FaceRef,
    /// Barycentric weights of the triangle's three corners.
    pub bary: [f64; 3],
    pub point: DVec3,
    pub t: f64,
}

#[derive(Clone, Copy, Debug)]
struct Node {
    min: Vec3,
    max: Vec3,
    /// Leaf: first triangle index into `order`; inner: left child (right = left + 1).
    start: u32,
    count: u32,
}

/// Triangles with ids and a BVH.
#[derive(Clone, Debug, Default)]
pub struct PickMesh {
    pub positions: Vec<Vec3>,
    pub tris: Vec<[u32; 3]>,
    pub ids: Vec<FaceRef>,
    order: Vec<u32>,
    nodes: Vec<Node>,
}

const LEAF: u32 = 4;

impl PickMesh {
    pub fn new(positions: Vec<Vec3>, tris: Vec<[u32; 3]>, ids: Vec<FaceRef>) -> Self {
        assert_eq!(tris.len(), ids.len());
        let mut m = PickMesh {
            positions,
            tris,
            ids,
            order: Vec::new(),
            nodes: Vec::new(),
        };
        m.build();
        m
    }

    pub fn is_empty(&self) -> bool {
        self.tris.is_empty()
    }

    fn tri_bounds(&self, t: u32) -> (Vec3, Vec3) {
        let [a, b, c] = self.tris[t as usize];
        let (a, b, c) = (
            self.positions[a as usize],
            self.positions[b as usize],
            self.positions[c as usize],
        );
        (a.min(b).min(c), a.max(b).max(c))
    }

    fn build(&mut self) {
        self.order = (0..self.tris.len() as u32).collect();
        self.nodes.clear();
        if self.tris.is_empty() {
            return;
        }
        let centroids: Vec<Vec3> = (0..self.tris.len() as u32)
            .map(|t| {
                let (a, b) = self.tri_bounds(t);
                (a + b) * 0.5
            })
            .collect();
        self.nodes.push(Node {
            min: Vec3::ZERO,
            max: Vec3::ZERO,
            start: 0,
            count: self.tris.len() as u32,
        });
        let mut stack = vec![0usize];
        while let Some(ni) = stack.pop() {
            let Node { start, count, .. } = self.nodes[ni];
            let range = start as usize..(start + count) as usize;
            let mut min = Vec3::splat(f32::INFINITY);
            let mut max = Vec3::splat(f32::NEG_INFINITY);
            let mut cmin = Vec3::splat(f32::INFINITY);
            let mut cmax = Vec3::splat(f32::NEG_INFINITY);
            for &t in &self.order[range.clone()] {
                let (a, b) = self.tri_bounds(t);
                min = min.min(a);
                max = max.max(b);
                cmin = cmin.min(centroids[t as usize]);
                cmax = cmax.max(centroids[t as usize]);
            }
            self.nodes[ni].min = min;
            self.nodes[ni].max = max;
            if count <= LEAF {
                continue;
            }
            let ext = cmax - cmin;
            let axis = if ext.x >= ext.y && ext.x >= ext.z {
                0
            } else if ext.y >= ext.z {
                1
            } else {
                2
            };
            let slice = &mut self.order[range];
            let mid = slice.len() / 2;
            slice.select_nth_unstable_by(mid, |&a, &b| {
                centroids[a as usize][axis].total_cmp(&centroids[b as usize][axis])
            });
            let left = self.nodes.len();
            self.nodes.push(Node {
                min,
                max,
                start,
                count: mid as u32,
            });
            self.nodes.push(Node {
                min,
                max,
                start: start + mid as u32,
                count: count - mid as u32,
            });
            // Mark the parent as inner: start = left child, count = 0.
            self.nodes[ni].start = left as u32;
            self.nodes[ni].count = 0;
            stack.push(left);
            stack.push(left + 1);
        }
    }

    /// Nearest hit along the ray, optionally skipping faces.
    pub fn raycast(&self, ray: &Ray, skip: impl Fn(FaceRef) -> bool) -> Option<Hit> {
        if self.nodes.is_empty() {
            return None;
        }
        let o = ray.origin.as_vec3();
        let d = ray.dir.as_vec3();
        let inv = Vec3::ONE / d;
        let mut best: Option<Hit> = None;
        let mut best_t = f64::INFINITY;
        let mut stack = vec![0usize];
        while let Some(ni) = stack.pop() {
            let n = self.nodes[ni];
            if !ray_box(o, inv, n.min, n.max, best_t as f32) {
                continue;
            }
            if n.count == 0 {
                stack.push(n.start as usize);
                stack.push(n.start as usize + 1);
                continue;
            }
            for &t in &self.order[n.start as usize..(n.start + n.count) as usize] {
                let id = self.ids[t as usize];
                if skip(id) {
                    continue;
                }
                let [a, b, c] = self.tris[t as usize];
                let (pa, pb, pc) = (
                    self.positions[a as usize].as_dvec3(),
                    self.positions[b as usize].as_dvec3(),
                    self.positions[c as usize].as_dvec3(),
                );
                if let Some((tt, u, v)) = ray_triangle(ray, pa, pb, pc)
                    && tt < best_t
                {
                    best_t = tt;
                    best = Some(Hit {
                        tri: t,
                        face: id,
                        bary: [1.0 - u - v, u, v],
                        point: ray.at(tt),
                        t: tt,
                    });
                }
            }
        }
        best
    }

    /// True if `p` is visible from the camera: nothing in the mesh lies in
    /// front of it (within `eps` world units).
    pub fn is_visible(&self, cam: &CameraMatrices, p: DVec3, eps: f64) -> bool {
        let (origin, dir) = if cam.ortho {
            let fwd = (cam.view.inverse().transform_vector3(DVec3::NEG_Z)).normalize();
            (p - fwd * (cam.far - cam.near).abs().max(1.0), fwd)
        } else {
            (cam.eye, (p - cam.eye).normalize())
        };
        let dist = (p - origin).length();
        let ray = Ray { origin, dir };
        match self.raycast(&ray, |_| false) {
            Some(h) => h.t >= dist - eps,
            None => true,
        }
    }
}

fn ray_box(o: Vec3, inv: Vec3, min: Vec3, max: Vec3, tmax: f32) -> bool {
    let t1 = (min - o) * inv;
    let t2 = (max - o) * inv;
    let tmin_v = t1.min(t2);
    let tmax_v = t1.max(t2);
    let lo = tmin_v.x.max(tmin_v.y).max(tmin_v.z).max(0.0);
    let hi = tmax_v.x.min(tmax_v.y).min(tmax_v.z).min(tmax);
    // NaN-safe: comparisons with NaN are false, so treat as a hit.
    lo <= hi * 1.000_01 + 1e-6 || lo.is_nan() || hi.is_nan()
}

/// Möller–Trumbore, two-sided. Returns (t, u, v).
pub fn ray_triangle(ray: &Ray, a: DVec3, b: DVec3, c: DVec3) -> Option<(f64, f64, f64)> {
    let e1 = b - a;
    let e2 = c - a;
    let p = ray.dir.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-14 {
        return None;
    }
    let inv = 1.0 / det;
    let s = ray.origin - a;
    let u = s.dot(p) * inv;
    if !(-1e-9..=1.0 + 1e-9).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = ray.dir.dot(q) * inv;
    if v < -1e-9 || u + v > 1.0 + 1e-9 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 1e-9).then_some((t, u, v))
}

/// Distance from `p` to segment `ab` in 2D, and the parameter along it.
pub fn point_segment_2d(p: DVec2, a: DVec2, b: DVec2) -> (f64, f64) {
    let ab = b - a;
    let len2 = ab.length_squared();
    let t = if len2 < 1e-18 {
        0.0
    } else {
        ((p - a).dot(ab) / len2).clamp(0.0, 1.0)
    };
    ((a + ab * t - p).length(), t)
}

/// Result of a screen-space search over polylines.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NearestPolyline {
    pub index: usize,
    pub dist_px: f64,
    /// Segment within the polyline and parameter along it.
    pub segment: usize,
    pub t: f64,
    pub point: DVec3,
}

/// Find the polyline nearest to `cursor` (pixels) within `max_px`.
/// `visible` filters candidate points (e.g. with [`PickMesh::is_visible`]).
pub fn nearest_polyline<'a>(
    cam: &CameraMatrices,
    cursor: DVec2,
    max_px: f64,
    polylines: impl Iterator<Item = (usize, &'a [Vec3])>,
    visible: impl Fn(DVec3) -> bool,
) -> Option<NearestPolyline> {
    let mut best: Option<NearestPolyline> = None;
    for (index, pts) in polylines {
        for (si, w) in pts.windows(2).enumerate() {
            let (a3, b3) = (w[0].as_dvec3(), w[1].as_dvec3());
            let (Some((a, _)), Some((b, _))) = (cam.project(a3), cam.project(b3)) else {
                continue;
            };
            // Cheap reject on the segment's screen box.
            if cursor.x < a.x.min(b.x) - max_px
                || cursor.x > a.x.max(b.x) + max_px
                || cursor.y < a.y.min(b.y) - max_px
                || cursor.y > a.y.max(b.y) + max_px
            {
                continue;
            }
            let (d, t) = point_segment_2d(cursor, a, b);
            if d <= max_px && best.is_none_or(|bst| d < bst.dist_px) {
                let point = a3.lerp(b3, t);
                if visible(point) {
                    best = Some(NearestPolyline {
                        index,
                        dist_px: d,
                        segment: si,
                        t,
                        point,
                    });
                }
            }
        }
    }
    best
}

/// Nearest point to `cursor` within `max_px` (screen space).
pub fn nearest_point(
    cam: &CameraMatrices,
    cursor: DVec2,
    max_px: f64,
    points: impl Iterator<Item = (usize, Vec3)>,
    visible: impl Fn(DVec3) -> bool,
) -> Option<(usize, f64)> {
    let mut best: Option<(usize, f64)> = None;
    for (i, p) in points {
        let Some((s, _)) = cam.project(p.as_dvec3()) else {
            continue;
        };
        let d = (s - cursor).length();
        if d <= max_px && best.is_none_or(|b| d < b.1) && visible(p.as_dvec3()) {
            best = Some((i, d));
        }
    }
    best
}

/// Even-odd point-in-polygon test (lasso selection).
pub fn point_in_polygon(p: DVec2, poly: &[DVec2]) -> bool {
    let mut inside = false;
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Region used by box and lasso selection, in viewport pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum Region {
    Rect { min: DVec2, max: DVec2 },
    Lasso(Vec<DVec2>),
}

impl Region {
    pub fn contains(&self, p: DVec2) -> bool {
        match self {
            Region::Rect { min, max } => {
                p.x >= min.x && p.x <= max.x && p.y >= min.y && p.y <= max.y
            }
            Region::Lasso(pts) => point_in_polygon(p, pts),
        }
    }

    /// Does the projected world point fall in the region?
    pub fn contains_world(&self, cam: &CameraMatrices, p: DVec3) -> bool {
        cam.project(p)
            .is_some_and(|(s, z)| (-1.0..=1.0).contains(&z) && self.contains(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::OrbitCamera;

    fn grid_mesh(n: u32) -> PickMesh {
        // n×n quads in the z=0 plane, two faces split at x = n/2.
        let mut pos = Vec::new();
        for j in 0..=n {
            for i in 0..=n {
                pos.push(Vec3::new(i as f32, j as f32, 0.0));
            }
        }
        let mut tris = Vec::new();
        let mut ids = Vec::new();
        for j in 0..n {
            for i in 0..n {
                let a = j * (n + 1) + i;
                let face = if i < n / 2 { 0 } else { 1 };
                tris.push([a, a + 1, a + n + 2]);
                tris.push([a, a + n + 2, a + n + 1]);
                ids.push(FaceRef { body: 7, face });
                ids.push(FaceRef { body: 7, face });
            }
        }
        PickMesh::new(pos, tris, ids)
    }

    #[test]
    fn raycast_hits_correct_face() {
        let m = grid_mesh(40);
        let ray = Ray {
            origin: DVec3::new(30.3, 10.6, 10.0),
            dir: DVec3::NEG_Z,
        };
        let h = m.raycast(&ray, |_| false).expect("hit");
        assert_eq!(h.face, FaceRef { body: 7, face: 1 });
        assert!((h.point - DVec3::new(30.3, 10.6, 0.0)).length() < 1e-5);
        assert!((h.t - 10.0).abs() < 1e-6);
        let ray = Ray {
            origin: DVec3::new(3.3, 10.6, 10.0),
            dir: DVec3::NEG_Z,
        };
        assert_eq!(m.raycast(&ray, |_| false).unwrap().face.face, 0);
        // Skipping face 0 → miss.
        assert!(m.raycast(&ray, |f| f.face == 0).is_none());
        // Outside → miss.
        let ray = Ray {
            origin: DVec3::new(-3.0, 10.6, 10.0),
            dir: DVec3::NEG_Z,
        };
        assert!(m.raycast(&ray, |_| false).is_none());
    }

    #[test]
    fn bvh_matches_brute_force() {
        let m = grid_mesh(16);
        let mut seed = 12345u64;
        let mut rnd = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 33) as f64) / (1u64 << 31) as f64
        };
        for _ in 0..500 {
            let o = DVec3::new(rnd() * 20.0 - 2.0, rnd() * 20.0 - 2.0, 5.0 + rnd() * 5.0);
            let d = DVec3::new(rnd() - 0.5, rnd() - 0.5, -1.0).normalize();
            let ray = Ray { origin: o, dir: d };
            let fast = m.raycast(&ray, |_| false).map(|h| h.tri);
            let mut slow: Option<(f64, u32)> = None;
            for (i, t) in m.tris.iter().enumerate() {
                let p = |k: u32| m.positions[k as usize].as_dvec3();
                if let Some((tt, _, _)) = ray_triangle(&ray, p(t[0]), p(t[1]), p(t[2]))
                    && slow.is_none_or(|s| tt < s.0)
                {
                    slow = Some((tt, i as u32));
                }
            }
            // Ties at shared triangle edges can pick either; compare distance.
            match (fast, slow) {
                (None, None) => {}
                (Some(_), Some(_)) => {
                    let ft = m.raycast(&ray, |_| false).unwrap().t;
                    assert!((ft - slow.unwrap().0).abs() < 1e-6);
                }
                other => panic!("mismatch {other:?}"),
            }
        }
    }

    #[test]
    fn screen_picking() {
        let mut c = OrbitCamera {
            ortho: true,
            pitch: crate::camera::PITCH_LIMIT,
            yaw: -std::f64::consts::FRAC_PI_2,
            ..Default::default()
        };
        c.frame_aabb(DVec3::ZERO, DVec3::new(10.0, 10.0, 0.0), 1.0);
        let cam = c.matrices([500.0, 500.0], DVec3::new(5.0, 5.0, 0.0), 8.0);
        let lines: Vec<Vec<Vec3>> = vec![
            vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 0.0)],
            vec![Vec3::new(0.0, 5.0, 0.0), Vec3::new(10.0, 5.0, 0.0)],
        ];
        let (target, _) = cam.project(DVec3::new(4.0, 5.0, 0.0)).unwrap();
        let near = nearest_polyline(
            &cam,
            target + DVec2::new(0.0, 3.0),
            8.0,
            lines.iter().map(|l| l.as_slice()).enumerate(),
            |_| true,
        )
        .expect("near");
        assert_eq!(near.index, 1);
        assert!(near.dist_px < 3.5);
        let pts = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(4.0, 5.0, 0.0)];
        let p =
            nearest_point(&cam, target, 6.0, pts.iter().copied().enumerate(), |_| true).unwrap();
        assert_eq!(p.0, 1);
    }

    #[test]
    fn lasso() {
        let poly = vec![
            DVec2::new(0.0, 0.0),
            DVec2::new(10.0, 0.0),
            DVec2::new(10.0, 10.0),
            DVec2::new(0.0, 10.0),
        ];
        assert!(point_in_polygon(DVec2::new(5.0, 5.0), &poly));
        assert!(!point_in_polygon(DVec2::new(15.0, 5.0), &poly));
    }
}
