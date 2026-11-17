//! Analytic surfaces and curves of the mock kernel.
//!
//! Parameterisations (all own design, textbook formulas):
//! * plane     `P = o + u·x + v·y`
//! * cylinder  `P = o + r(cos u·x + sin u·y) + v·z`
//! * cone      `P = o + (r0 + k·v)(cos u·x + sin u·y) + v·z`
//! * sphere    `P = o + r cos v (cos u·x + sin u·y) + r sin v·z`
//! * torus     `P = o + (R + r cos v)(cos u·x + sin u·y) + r sin v·z`
//!
//! With these formulas `du × dv` points away from the axis (outward) for the
//! closed surfaces; faces whose material is on the other side are marked
//! `reversed`.

use glam::{DVec2, DVec3};
use serde::{Deserialize, Serialize};
use std::f64::consts::TAU;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Surface {
    Plane {
        o: DVec3,
        x: DVec3,
        y: DVec3,
    },
    Cylinder {
        o: DVec3,
        z: DVec3,
        x: DVec3,
        r: f64,
    },
    Cone {
        o: DVec3,
        z: DVec3,
        x: DVec3,
        r0: f64,
        k: f64,
    },
    Sphere {
        o: DVec3,
        z: DVec3,
        x: DVec3,
        r: f64,
    },
    Torus {
        o: DVec3,
        z: DVec3,
        x: DVec3,
        major: f64,
        minor: f64,
    },
}

impl Surface {
    /// Point and first derivatives.
    pub fn eval(&self, uv: DVec2) -> (DVec3, DVec3, DVec3) {
        let (u, v) = (uv.x, uv.y);
        match *self {
            Surface::Plane { o, x, y } => (o + x * u + y * v, x, y),
            Surface::Cylinder { o, z, x, r } => {
                let y = z.cross(x);
                let (s, c) = u.sin_cos();
                let rad = x * c + y * s;
                (o + rad * r + z * v, (y * c - x * s) * r, z)
            }
            Surface::Cone { o, z, x, r0, k } => {
                let y = z.cross(x);
                let (s, c) = u.sin_cos();
                let rad = x * c + y * s;
                let rv = r0 + k * v;
                (o + rad * rv + z * v, (y * c - x * s) * rv, rad * k + z)
            }
            Surface::Sphere { o, z, x, r } => {
                let y = z.cross(x);
                let (su, cu) = u.sin_cos();
                let (sv, cv) = v.sin_cos();
                let rad = x * cu + y * su;
                let p = o + rad * (r * cv) + z * (r * sv);
                let du = (y * cu - x * su) * (r * cv);
                let dv = rad * (-r * sv) + z * (r * cv);
                (p, du, dv)
            }
            Surface::Torus {
                o,
                z,
                x,
                major,
                minor,
            } => {
                let y = z.cross(x);
                let (su, cu) = u.sin_cos();
                let (sv, cv) = v.sin_cos();
                let rad = x * cu + y * su;
                let rr = major + minor * cv;
                let p = o + rad * rr + z * (minor * sv);
                let du = (y * cu - x * su) * rr;
                let dv = rad * (-minor * sv) + z * (minor * cv);
                (p, du, dv)
            }
        }
    }

    pub fn point(&self, uv: DVec2) -> DVec3 {
        self.eval(uv).0
    }

    /// Unit normal `du × dv` (not yet oriented by the face). At singular
    /// points (poles, apex) the limit of nearby normals is used.
    pub fn normal(&self, uv: DVec2) -> DVec3 {
        let (_, du, dv) = self.eval(uv);
        let n = du.cross(dv);
        let scale = du.length() * dv.length();
        if scale > 1e-20 && n.length() > 1e-9 * scale {
            return n.normalize();
        }
        // Singular: average normals on a small ring around the point.
        let mut acc = DVec3::ZERO;
        for dvs in [1e-5, -1e-5] {
            for i in 0..8 {
                let a = i as f64 * TAU / 8.0;
                let q = DVec2::new(uv.x + a, uv.y + dvs);
                let (_, du, dv) = self.eval(q);
                let m = du.cross(dv);
                if m.length() > 1e-14 {
                    acc += m.normalize();
                }
            }
        }
        if acc.length() > 1e-12 {
            acc.normalize()
        } else {
            self.axis().unwrap_or(DVec3::Z)
        }
    }

    pub fn axis(&self) -> Option<DVec3> {
        match *self {
            Surface::Plane { x, y, .. } => Some(x.cross(y).normalize()),
            Surface::Cylinder { z, .. }
            | Surface::Cone { z, .. }
            | Surface::Sphere { z, .. }
            | Surface::Torus { z, .. } => Some(z),
        }
    }

    pub fn origin(&self) -> DVec3 {
        match *self {
            Surface::Plane { o, .. }
            | Surface::Cylinder { o, .. }
            | Surface::Cone { o, .. }
            | Surface::Sphere { o, .. }
            | Surface::Torus { o, .. } => o,
        }
    }

    pub fn u_periodic(&self) -> bool {
        !matches!(self, Surface::Plane { .. })
    }
    pub fn v_periodic(&self) -> bool {
        matches!(self, Surface::Torus { .. })
    }

    /// Unconstrained closest-point guess in parameter space.
    pub fn invert_guess(&self, p: DVec3) -> DVec2 {
        match *self {
            Surface::Plane { o, x, y } => DVec2::new((p - o).dot(x), (p - o).dot(y)),
            Surface::Cylinder { o, z, x, .. } | Surface::Cone { o, z, x, .. } => {
                let y = z.cross(x);
                let q = p - o;
                DVec2::new(q.dot(y).atan2(q.dot(x)), q.dot(z))
            }
            Surface::Sphere { o, z, x, .. } => {
                let y = z.cross(x);
                let q = p - o;
                let l = q.length().max(1e-300);
                DVec2::new(
                    q.dot(y).atan2(q.dot(x)),
                    (q.dot(z) / l).clamp(-1.0, 1.0).asin(),
                )
            }
            Surface::Torus { o, z, x, major, .. } => {
                let y = z.cross(x);
                let q = p - o;
                let u = q.dot(y).atan2(q.dot(x));
                let rad = x * u.cos() + y * u.sin();
                let c = rad * major;
                let w = q - c;
                DVec2::new(u, w.dot(z).atan2(w.dot(rad)))
            }
        }
    }

    /// Gauss–Newton refinement of a closest-point parameter.
    pub fn refine_closest(&self, p: DVec3, mut uv: DVec2) -> DVec2 {
        for _ in 0..12 {
            let (q, du, dv) = self.eval(uv);
            let r = q - p;
            let a11 = du.dot(du);
            let a12 = du.dot(dv);
            let a22 = dv.dot(dv);
            let b1 = -r.dot(du);
            let b2 = -r.dot(dv);
            let det = a11 * a22 - a12 * a12;
            if det.abs() < 1e-30 {
                break;
            }
            let d = DVec2::new((b1 * a22 - b2 * a12) / det, (a11 * b2 - a12 * b1) / det);
            uv += d;
            if d.length() < 1e-14 {
                break;
            }
        }
        uv
    }
}

/// A 2D parameter-space curve: `Line` is `o + t·d`, `Ellipse` is
/// `c + cos t·a + sin t·b` (circles in UV, with any phase and direction).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Curve2 {
    Line { o: DVec2, d: DVec2 },
    Ellipse { c: DVec2, a: DVec2, b: DVec2 },
}

impl Curve2 {
    pub fn eval(&self, t: f64) -> DVec2 {
        match *self {
            Curve2::Line { o, d } => o + d * t,
            Curve2::Ellipse { c, a, b } => {
                let (s, co) = t.sin_cos();
                c + a * co + b * s
            }
        }
    }
    pub fn deriv(&self, t: f64) -> DVec2 {
        match *self {
            Curve2::Line { d, .. } => d,
            Curve2::Ellipse { a, b, .. } => {
                let (s, co) = t.sin_cos();
                b * co - a * s
            }
        }
    }
}

/// A 3D edge curve.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Curve3 {
    /// `p0 + t·d`.
    Line { p0: DVec3, d: DVec3 },
    /// `c + r(cos t·x + sin t·y)`.
    Circle {
        c: DVec3,
        x: DVec3,
        y: DVec3,
        r: f64,
    },
    /// Degenerate edge (pole or apex).
    Point { p: DVec3 },
    /// The surface of face `face` along the parameter curve `pc`.
    OnFace { face: u32, pc: Curve2 },
}
