//! Orbit camera (perspective / orthographic) and the framing math.
//!
//! World space is right-handed. The "up" axis is Z by default (CAD
//! convention) or Y. The camera orbits `target` at `distance`; `yaw` turns
//! about the up axis and `pitch` tilts towards it.

use glam::{DMat4, DVec2, DVec3, Mat4, Vec3};
use serde::{Deserialize, Serialize};

/// Which world axis points up on screen when the camera is level.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpAxis {
    Y,
    #[default]
    Z,
}

impl UpAxis {
    pub fn vec(self) -> DVec3 {
        match self {
            UpAxis::Y => DVec3::Y,
            UpAxis::Z => DVec3::Z,
        }
    }
}

/// A ray in world space. `dir` is unit length.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    pub origin: DVec3,
    pub dir: DVec3,
}

impl Ray {
    pub fn at(&self, t: f64) -> DVec3 {
        self.origin + self.dir * t
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrbitCamera {
    pub target: DVec3,
    /// Radians about the up axis.
    pub yaw: f64,
    /// Radians above the horizon, clamped to just under ±90°.
    pub pitch: f64,
    pub distance: f64,
    pub ortho: bool,
    /// Vertical field of view in radians (perspective). Orthographic uses
    /// the same frustum height at `distance`, so toggling keeps the size.
    pub fov_y: f64,
    pub up: UpAxis,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        OrbitCamera {
            target: DVec3::ZERO,
            yaw: -0.9,
            pitch: 0.5,
            distance: 100.0,
            ortho: false,
            fov_y: 35f64.to_radians(),
            up: UpAxis::Z,
        }
    }
}

pub const PITCH_LIMIT: f64 = 1.5607; // ~89.4°

/// Named views for the view cube and the CLI (`--view iso|front|...`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NamedView {
    Front,
    Back,
    Left,
    Right,
    Top,
    Bottom,
    Iso,
}

impl NamedView {
    pub fn parse(s: &str) -> Option<NamedView> {
        Some(match s.to_ascii_lowercase().as_str() {
            "front" => NamedView::Front,
            "back" => NamedView::Back,
            "left" => NamedView::Left,
            "right" => NamedView::Right,
            "top" => NamedView::Top,
            "bottom" => NamedView::Bottom,
            "iso" | "isometric" => NamedView::Iso,
            _ => return None,
        })
    }

    /// (yaw, pitch) for this view. Front looks along +forward (towards -Y in
    /// Z-up, towards -Z in Y-up), i.e. the camera sits on the -Y side.
    pub fn angles(self) -> (f64, f64) {
        use std::f64::consts::{FRAC_PI_2, PI};
        match self {
            NamedView::Front => (-FRAC_PI_2, 0.0),
            NamedView::Back => (FRAC_PI_2, 0.0),
            NamedView::Right => (0.0, 0.0),
            NamedView::Left => (PI, 0.0),
            NamedView::Top => (-FRAC_PI_2, PITCH_LIMIT),
            NamedView::Bottom => (-FRAC_PI_2, -PITCH_LIMIT),
            NamedView::Iso => (-std::f64::consts::FRAC_PI_4, 0.6155),
        }
    }
}

impl OrbitCamera {
    /// Basis (right, forward-in-plane, up) of the world for the chosen up axis.
    pub fn world_basis(&self) -> (DVec3, DVec3, DVec3) {
        match self.up {
            UpAxis::Z => (DVec3::X, DVec3::Y, DVec3::Z),
            // Y-up: x right, -z "forward in plane", y up; keeps right-handedness.
            UpAxis::Y => (DVec3::X, DVec3::NEG_Z, DVec3::Y),
        }
    }

    /// Unit vector from target to eye.
    pub fn offset_dir(&self) -> DVec3 {
        let (x, y, z) = self.world_basis();
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        x * (cp * cy) + y * (cp * sy) + z * sp
    }

    pub fn eye(&self) -> DVec3 {
        self.target + self.offset_dir() * self.distance
    }

    pub fn forward(&self) -> DVec3 {
        -self.offset_dir()
    }

    pub fn right(&self) -> DVec3 {
        let up = self.up.vec();
        let r = self.forward().cross(up);
        if r.length_squared() < 1e-12 {
            DVec3::X
        } else {
            r.normalize()
        }
    }

    pub fn cam_up(&self) -> DVec3 {
        self.right().cross(self.forward()).normalize()
    }

    pub fn view(&self) -> DMat4 {
        look_at_rh(self.eye(), self.target, self.cam_up())
    }

    /// Height of the view frustum at the target distance (world units).
    pub fn view_height(&self) -> f64 {
        2.0 * self.distance * (self.fov_y * 0.5).tan()
    }

    /// Near/far planes for a scene of radius `scene_radius` around `scene_center`.
    pub fn clip_planes(&self, scene_center: DVec3, scene_radius: f64) -> (f64, f64) {
        let r = scene_radius.max(1e-6);
        let d = (self.eye() - scene_center).length();
        if self.ortho {
            // Ortho can put the eye inside the scene; pull the near plane back.
            let near = -(d + r * 2.0);
            let far = d + r * 2.0;
            (near, far)
        } else {
            let far = (d + r * 1.5).max(self.distance * 2.0);
            let near = (d - r * 1.5).max(far * 1e-4).max(self.distance * 1e-3);
            (near, far)
        }
    }

    pub fn proj(&self, aspect: f64, near: f64, far: f64) -> DMat4 {
        if self.ortho {
            let h = self.view_height() * 0.5;
            let w = h * aspect;
            orthographic_rh(-w, w, -h, h, near, far)
        } else {
            perspective_rh(self.fov_y, aspect, near, far)
        }
    }

    /// Full matrices for a viewport of `size_px` showing a scene with the
    /// given bounding sphere.
    pub fn matrices(
        &self,
        size_px: [f32; 2],
        scene_center: DVec3,
        scene_radius: f64,
    ) -> CameraMatrices {
        let aspect = (size_px[0].max(1.0) / size_px[1].max(1.0)) as f64;
        let (near, far) = self.clip_planes(scene_center, scene_radius);
        let view = self.view();
        let proj = self.proj(aspect, near, far);
        CameraMatrices {
            view,
            proj,
            view_proj: proj * view,
            eye: self.eye(),
            ortho: self.ortho,
            size_px,
            near,
            far,
        }
    }

    pub fn orbit(&mut self, d_yaw: f64, d_pitch: f64) {
        self.yaw = wrap_angle(self.yaw + d_yaw);
        self.pitch = (self.pitch + d_pitch).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    /// Pan by a screen-space delta in pixels for a viewport of `height_px`.
    pub fn pan_px(&mut self, dx: f64, dy: f64, height_px: f64) {
        let per_px = self.view_height() / height_px.max(1.0);
        self.target += self.right() * (-dx * per_px) + self.cam_up() * (dy * per_px);
    }

    /// Zoom by a factor (< 1 zooms in). Keeps the point under `anchor` (world)
    /// fixed on screen when given.
    pub fn zoom(&mut self, factor: f64, anchor: Option<DVec3>) {
        let factor = factor.clamp(0.05, 20.0);
        let new_dist = (self.distance * factor).clamp(1e-4, 1e9);
        if let Some(p) = anchor {
            // Move the target towards the anchor by the same ratio so the
            // anchor stays under the cursor.
            let k = 1.0 - new_dist / self.distance;
            self.target += (p - self.target) * k;
        }
        self.distance = new_dist;
    }

    pub fn set_view(&mut self, v: NamedView) {
        let (yaw, pitch) = v.angles();
        self.yaw = yaw;
        self.pitch = pitch;
    }

    /// Frame a bounding sphere so it fills the viewport with a small margin.
    pub fn frame_sphere(&mut self, center: DVec3, radius: f64, aspect: f64) {
        let radius = radius.max(1e-3);
        let half_v = self.fov_y * 0.5;
        let half_h = (half_v.tan() * aspect.max(0.05)).atan();
        let half = half_v.min(half_h);
        self.target = center;
        self.distance = radius * 1.02 / half.sin();
    }

    /// Frame an axis-aligned box.
    pub fn frame_aabb(&mut self, min: DVec3, max: DVec3, aspect: f64) {
        let c = (min + max) * 0.5;
        let r = ((max - min).length() * 0.5).max(1e-3);
        self.frame_sphere(c, r, aspect);
    }

    /// Ray through a pixel (`px` measured from the viewport's top-left).
    pub fn ray(&self, px: DVec2, size_px: [f32; 2]) -> Ray {
        let w = size_px[0].max(1.0) as f64;
        let h = size_px[1].max(1.0) as f64;
        let ndc_x = px.x / w * 2.0 - 1.0;
        let ndc_y = 1.0 - px.y / h * 2.0;
        let aspect = w / h;
        let half_h = self.view_height() * 0.5;
        let fwd = self.forward();
        let right = self.right();
        let up = self.cam_up();
        if self.ortho {
            let origin = self.eye() + right * (ndc_x * half_h * aspect) + up * (ndc_y * half_h)
                - fwd * self.distance * 4.0;
            Ray { origin, dir: fwd }
        } else {
            let t = (self.fov_y * 0.5).tan();
            let dir = (fwd + right * (ndc_x * t * aspect) + up * (ndc_y * t)).normalize();
            Ray {
                origin: self.eye(),
                dir,
            }
        }
    }

    pub fn to_doc(&self) -> newtopo_core::doc::Camera {
        newtopo_core::doc::Camera {
            target: self.target,
            yaw: self.yaw,
            pitch: self.pitch,
            distance: self.distance,
            ortho: self.ortho,
        }
    }

    pub fn from_doc(c: &newtopo_core::doc::Camera, up: UpAxis) -> Self {
        OrbitCamera {
            target: c.target,
            yaw: c.yaw,
            pitch: c.pitch,
            distance: c.distance,
            ortho: c.ortho,
            up,
            ..Default::default()
        }
    }
}

/// Precomputed matrices for one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraMatrices {
    pub view: DMat4,
    pub proj: DMat4,
    pub view_proj: DMat4,
    pub eye: DVec3,
    pub ortho: bool,
    pub size_px: [f32; 2],
    pub near: f64,
    pub far: f64,
}

impl CameraMatrices {
    /// World → pixel (from the viewport's top-left) and NDC depth.
    /// `None` if the point is behind the camera.
    pub fn project(&self, p: DVec3) -> Option<(DVec2, f64)> {
        let c = self.view_proj * p.extend(1.0);
        if c.w <= 1e-12 {
            return None;
        }
        let ndc = c.truncate() / c.w;
        let x = (ndc.x * 0.5 + 0.5) * self.size_px[0] as f64;
        let y = (0.5 - ndc.y * 0.5) * self.size_px[1] as f64;
        Some((DVec2::new(x, y), ndc.z))
    }

    pub fn view_f32(&self) -> Mat4 {
        self.view.as_mat4()
    }
    pub fn proj_f32(&self) -> Mat4 {
        self.proj.as_mat4()
    }
    pub fn eye_f32(&self) -> Vec3 {
        self.eye.as_vec3()
    }
}

/// Right-handed view matrix (camera looks down -Z in view space).
pub fn look_at_rh(eye: DVec3, center: DVec3, up: DVec3) -> DMat4 {
    let f = (center - eye).normalize();
    let s = f.cross(up).normalize();
    let u = s.cross(f);
    DMat4::from_cols(
        glam::DVec4::new(s.x, u.x, -f.x, 0.0),
        glam::DVec4::new(s.y, u.y, -f.y, 0.0),
        glam::DVec4::new(s.z, u.z, -f.z, 0.0),
        glam::DVec4::new(-s.dot(eye), -u.dot(eye), f.dot(eye), 1.0),
    )
}

/// Right-handed perspective with depth in [0, 1] (wgpu convention).
pub fn perspective_rh(fov_y: f64, aspect: f64, near: f64, far: f64) -> DMat4 {
    let h = 1.0 / (0.5 * fov_y).tan();
    let w = h / aspect;
    let r = far / (near - far);
    DMat4::from_cols(
        glam::DVec4::new(w, 0.0, 0.0, 0.0),
        glam::DVec4::new(0.0, h, 0.0, 0.0),
        glam::DVec4::new(0.0, 0.0, r, -1.0),
        glam::DVec4::new(0.0, 0.0, r * near, 0.0),
    )
}

/// Right-handed orthographic with depth in [0, 1].
pub fn orthographic_rh(left: f64, right: f64, bottom: f64, top: f64, near: f64, far: f64) -> DMat4 {
    let rcp_w = 1.0 / (right - left);
    let rcp_h = 1.0 / (top - bottom);
    let r = 1.0 / (near - far);
    DMat4::from_cols(
        glam::DVec4::new(2.0 * rcp_w, 0.0, 0.0, 0.0),
        glam::DVec4::new(0.0, 2.0 * rcp_h, 0.0, 0.0),
        glam::DVec4::new(0.0, 0.0, r, 0.0),
        glam::DVec4::new(
            -(left + right) * rcp_w,
            -(top + bottom) * rcp_h,
            r * near,
            1.0,
        ),
    )
}

fn wrap_angle(a: f64) -> f64 {
    use std::f64::consts::{PI, TAU};
    let mut a = (a + PI) % TAU;
    if a < 0.0 {
        a += TAU;
    }
    a - PI
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eye_is_at_distance() {
        let c = OrbitCamera {
            distance: 42.0,
            ..Default::default()
        };
        assert!(((c.eye() - c.target).length() - 42.0).abs() < 1e-9);
    }

    #[test]
    fn centre_ray_hits_target() {
        for ortho in [false, true] {
            let c = OrbitCamera {
                ortho,
                target: DVec3::new(1.0, 2.0, 3.0),
                ..Default::default()
            };
            let r = c.ray(DVec2::new(400.0, 300.0), [800.0, 600.0]);
            // Closest point of the ray to the target is the target.
            let t = (c.target - r.origin).dot(r.dir);
            assert!((r.at(t) - c.target).length() < 1e-6, "ortho={ortho}");
        }
    }

    #[test]
    fn project_and_ray_agree() {
        for ortho in [false, true] {
            let c = OrbitCamera {
                ortho,
                yaw: 0.3,
                pitch: 0.4,
                distance: 50.0,
                ..Default::default()
            };
            let m = c.matrices([640.0, 480.0], DVec3::ZERO, 20.0);
            let p = DVec3::new(5.0, -3.0, 2.0);
            let (px, _) = m.project(p).unwrap();
            let r = c.ray(px, [640.0, 480.0]);
            let t = (p - r.origin).dot(r.dir);
            assert!(
                (r.at(t) - p).length() < 1e-6,
                "ortho={ortho} err={}",
                (r.at(t) - p).length()
            );
        }
    }

    #[test]
    fn frame_fits_sphere() {
        let mut c = OrbitCamera::default();
        c.frame_sphere(DVec3::new(10.0, 0.0, 0.0), 5.0, 1.5);
        let m = c.matrices([900.0, 600.0], c.target, 5.0);
        // Points on the sphere project inside the viewport.
        for d in [
            DVec3::X,
            DVec3::NEG_X,
            DVec3::Y,
            DVec3::NEG_Y,
            DVec3::Z,
            DVec3::NEG_Z,
        ] {
            let (px, _) = m.project(c.target + d * 5.0).unwrap();
            assert!(
                px.x > 0.0 && px.x < 900.0 && px.y > 0.0 && px.y < 600.0,
                "{d:?} -> {px:?}"
            );
        }
    }

    #[test]
    fn y_up_level_view_has_y_on_screen_up() {
        let c = OrbitCamera {
            up: UpAxis::Y,
            pitch: 0.0,
            ..Default::default()
        };
        assert!(c.cam_up().dot(DVec3::Y) > 0.999);
    }

    #[test]
    fn zoom_keeps_anchor() {
        let mut c = OrbitCamera {
            distance: 100.0,
            ..Default::default()
        };
        let anchor = DVec3::new(10.0, 0.0, 0.0);
        let m0 = c.matrices([800.0, 600.0], DVec3::ZERO, 50.0);
        let (p0, _) = m0.project(anchor).unwrap();
        c.zoom(0.5, Some(anchor));
        let m1 = c.matrices([800.0, 600.0], DVec3::ZERO, 50.0);
        let (p1, _) = m1.project(anchor).unwrap();
        assert!((p0 - p1).length() < 1e-6);
    }
}
