//! Quadric error metric (Garland & Heckbert, "Surface Simplification Using
//! Quadric Error Metrics", SIGGRAPH 1997): the error of a position is the
//! weighted sum of squared distances to a set of planes, stored as a
//! symmetric 4×4 matrix.

use glam::DVec3;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Quadric {
    // xx xy xz xw yy yz yw zz zw ww
    m: [f64; 10],
}

impl Quadric {
    /// Plane through `p` with unit normal `n`, weighted by `w`.
    pub fn plane(n: DVec3, p: DVec3, w: f64) -> Quadric {
        let d = -n.dot(p);
        let (a, b, c) = (n.x, n.y, n.z);
        Quadric {
            m: [
                w * a * a,
                w * a * b,
                w * a * c,
                w * a * d,
                w * b * b,
                w * b * c,
                w * b * d,
                w * c * c,
                w * c * d,
                w * d * d,
            ],
        }
    }

    pub fn add(&mut self, o: &Quadric) {
        for i in 0..10 {
            self.m[i] += o.m[i];
        }
    }

    pub fn sum(&self, o: &Quadric) -> Quadric {
        let mut q = *self;
        q.add(o);
        q
    }

    /// `[p 1] Q [p 1]ᵀ`, clamped at zero against round-off.
    pub fn eval(&self, p: DVec3) -> f64 {
        let m = &self.m;
        let (x, y, z) = (p.x, p.y, p.z);
        let e = m[0] * x * x
            + 2.0 * m[1] * x * y
            + 2.0 * m[2] * x * z
            + 2.0 * m[3] * x
            + m[4] * y * y
            + 2.0 * m[5] * y * z
            + 2.0 * m[6] * y
            + m[7] * z * z
            + 2.0 * m[8] * z
            + m[9];
        e.max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plane_distance_squared() {
        let q = Quadric::plane(DVec3::Z, DVec3::new(0.0, 0.0, 1.0), 2.0);
        assert!((q.eval(DVec3::new(5.0, -3.0, 4.0)) - 18.0).abs() < 1e-12);
        let s = q.sum(&Quadric::plane(DVec3::X, DVec3::ZERO, 1.0));
        assert!((s.eval(DVec3::new(1.0, 0.0, 1.0)) - 1.0).abs() < 1e-12);
    }
}
