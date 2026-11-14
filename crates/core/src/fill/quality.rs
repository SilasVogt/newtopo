//! Quality score for fill elements (SPEC §7 step 3). Quality only ranks
//! admissible elements; it never rejects one. Own design: a weighted sum of
//! terms in [0, 1], higher is better.
//!
//! Quad terms: corner squareness in UV (`1 - |cos|`), planarity in 3D (gap
//! between the two diagonals relative to their length), aspect in 3D
//! (shortest over longest side), and alignment of the sides with the iso
//! directions. Triangle terms: 3D shape (1 for equilateral) and alignment.

use glam::{DVec2, DVec3};

/// Per-pair data cached for the DP (n <= 48, so at most 2,304 entries).
struct Pairs {
    n: usize,
    dir: Vec<DVec2>,
    len: Vec<f64>,
    align: Vec<f64>,
}

/// Shared inputs for scoring.
pub struct Scorer<'a> {
    uv: &'a [DVec2],
    pos: &'a [DVec3],
    iso: [DVec2; 2],
    has_iso: bool,
    pairs: Option<Pairs>,
}

impl<'a> Scorer<'a> {
    pub fn new(uv: &'a [DVec2], pos: &'a [DVec3], iso: [DVec2; 2]) -> Self {
        let iso = [iso[0].normalize_or_zero(), iso[1].normalize_or_zero()];
        let has_iso = iso[0] != DVec2::ZERO || iso[1] != DVec2::ZERO;
        Scorer {
            uv,
            pos,
            iso,
            has_iso,
            pairs: None,
        }
    }

    /// Same scores, with all pair terms precomputed (for the O(n^4) DP).
    pub fn with_tables(uv: &'a [DVec2], pos: &'a [DVec3], iso: [DVec2; 2]) -> Self {
        let mut s = Self::new(uv, pos, iso);
        let n = uv.len();
        let mut p = Pairs {
            n,
            dir: vec![DVec2::ZERO; n * n],
            len: vec![0.0; n * n],
            align: vec![0.0; n * n],
        };
        for i in 0..n {
            for j in 0..n {
                p.dir[i * n + j] = s.dir_raw(i, j);
                p.len[i * n + j] = s.pos[i].distance(s.pos[j]);
                p.align[i * n + j] = s.align_raw(i, j);
            }
        }
        s.pairs = Some(p);
        s
    }

    #[inline]
    fn dir_raw(&self, a: usize, b: usize) -> DVec2 {
        (self.uv[b] - self.uv[a]).normalize_or_zero()
    }

    #[inline]
    fn align_raw(&self, a: usize, b: usize) -> f64 {
        if !self.has_iso {
            return 1.0;
        }
        let d = self.dir_raw(a, b);
        d.dot(self.iso[0]).abs().max(d.dot(self.iso[1]).abs())
    }

    #[inline]
    fn dir(&self, a: usize, b: usize) -> DVec2 {
        match &self.pairs {
            Some(p) => p.dir[a * p.n + b],
            None => self.dir_raw(a, b),
        }
    }

    #[inline]
    fn len(&self, a: usize, b: usize) -> f64 {
        match &self.pairs {
            Some(p) => p.len[a * p.n + b],
            None => self.pos[a].distance(self.pos[b]),
        }
    }

    /// Alignment of UV edge `a-b` with the nearest iso direction, in [0, 1].
    #[inline]
    fn align(&self, a: usize, b: usize) -> f64 {
        match &self.pairs {
            Some(p) => p.align[a * p.n + b],
            None => self.align_raw(a, b),
        }
    }

    /// Quality of the counter-clockwise quad `a, b, c, d`.
    pub fn quad(&self, a: usize, b: usize, c: usize, d: usize) -> f64 {
        let corner = |p: usize, v: usize, n: usize| 1.0 - self.dir(v, p).dot(self.dir(v, n)).abs();
        let angle = 0.25 * (corner(d, a, b) + corner(a, b, c) + corner(b, c, d) + corner(c, d, a));
        // Planarity: distance between the two diagonals, relative to them.
        let (pa, pb, pc, pd) = (self.pos[a], self.pos[b], self.pos[c], self.pos[d]);
        let (lac, lbd) = (self.len(a, c), self.len(b, d));
        let nrm = (pc - pa).cross(pd - pb).normalize_or_zero();
        let gap = (pb - pa).dot(nrm).abs();
        let planar = if lac + lbd > 0.0 {
            (1.0 - 4.0 * gap / (lac + lbd)).max(0.0)
        } else {
            0.0
        };
        let l = [
            self.len(a, b),
            self.len(b, c),
            self.len(c, d),
            self.len(d, a),
        ];
        let lmax = l[0].max(l[1]).max(l[2]).max(l[3]);
        let lmin = l[0].min(l[1]).min(l[2]).min(l[3]);
        let aspect = if lmax > 0.0 { lmin / lmax } else { 0.0 };
        let align =
            0.25 * (self.align(a, b) + self.align(b, c) + self.align(c, d) + self.align(d, a));
        0.35 * angle + 0.25 * planar + 0.15 * aspect + 0.25 * align
    }

    /// Quality of the triangle `a, b, c`.
    pub fn tri(&self, a: usize, b: usize, c: usize) -> f64 {
        let (pa, pb, pc) = (self.pos[a], self.pos[b], self.pos[c]);
        let area2 = (pb - pa).cross(pc - pa).length();
        let (lab, lbc, lca) = (self.len(a, b), self.len(b, c), self.len(c, a));
        let s = lab * lab + lbc * lbc + lca * lca;
        // 2 * sqrt(3) * |cross| / sum(l^2) == 4 * sqrt(3) * A / sum(l^2).
        let shape = if s > 0.0 {
            (2.0 * 3f64.sqrt() * area2 / s).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let align = (self.align(a, b) + self.align(b, c) + self.align(c, a)) / 3.0;
        0.6 * shape + 0.4 * align
    }
}
