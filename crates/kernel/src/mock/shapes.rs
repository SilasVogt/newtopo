//! Builders for the analytic test bodies (own design).

use super::geom::{Curve2, Curve3, Surface};
use super::{MEdge, MFace, MPcurve, MockBody, ce};
use crate::{CoEdge, EdgeId, FaceId, VertexId, WireTopo};
use glam::{DVec2, DVec3};
use std::collections::HashMap;
use std::f64::consts::{FRAC_PI_2, PI, TAU};

impl MockBody {
    fn named(name: &str) -> Self {
        MockBody {
            name: name.into(),
            ..Default::default()
        }
    }

    pub fn add_vertex(&mut self, p: DVec3) -> VertexId {
        self.vertices.push(p);
        VertexId(self.vertices.len() as u32 - 1)
    }

    pub fn add_face(&mut self, surface: Surface, reversed: bool) -> FaceId {
        self.faces.push(MFace {
            surface,
            reversed,
            wires: vec![],
        });
        FaceId(self.faces.len() as u32 - 1)
    }

    pub fn add_edge(
        &mut self,
        curve: Curve3,
        t0: f64,
        t1: f64,
        v0: Option<VertexId>,
        v1: Option<VertexId>,
    ) -> EdgeId {
        self.edges.push(MEdge {
            curve,
            t0,
            t1,
            v0,
            v1,
            pcurves: vec![],
        });
        EdgeId(self.edges.len() as u32 - 1)
    }

    pub fn add_pcurve(&mut self, e: EdgeId, face: FaceId, side: Option<bool>, pc: Curve2) {
        self.edges[e.idx()].pcurves.push(MPcurve { face, side, pc });
    }

    pub fn add_wire(&mut self, f: FaceId, outer: bool, coedges: Vec<CoEdge>) {
        self.faces[f.idx()].wires.push(WireTopo { outer, coedges });
    }

    /// Straight edge between two vertices, `t ∈ [0, length]`.
    pub fn line_edge(&mut self, a: VertexId, b: VertexId) -> EdgeId {
        let pa = self.vertices[a.idx()];
        let pb = self.vertices[b.idx()];
        let l = (pb - pa).length();
        self.add_edge(
            Curve3::Line {
                p0: pa,
                d: (pb - pa) / l,
            },
            0.0,
            l,
            Some(a),
            Some(b),
        )
    }

    /// Full circle with its vertex at `t = 0`.
    pub fn circle_edge(&mut self, c: DVec3, x: DVec3, y: DVec3, r: f64, v: VertexId) -> EdgeId {
        self.add_edge(Curve3::Circle { c, x, y, r }, 0.0, TAU, Some(v), Some(v))
    }

    /// Pcurve of a line or circle edge on a planar face, by projection.
    pub fn add_plane_pcurve(&mut self, e: EdgeId, f: FaceId) {
        let Surface::Plane { o, x, y } = self.faces[f.idx()].surface else {
            panic!("not planar")
        };
        let proj = |p: DVec3| DVec2::new((p - o).dot(x), (p - o).dot(y));
        let dir = |d: DVec3| DVec2::new(d.dot(x), d.dot(y));
        let pc = match self.edges[e.idx()].curve {
            Curve3::Line { p0, d } => Curve2::Line {
                o: proj(p0),
                d: dir(d),
            },
            Curve3::Circle { c, x: cx, y: cy, r } => Curve2::Ellipse {
                c: proj(c),
                a: dir(cx) * r,
                b: dir(cy) * r,
            },
            _ => panic!("unsupported curve on plane"),
        };
        self.add_pcurve(e, f, None, pc);
    }

    /// Coedge of a closed circle used as a wire on a face with normal `n`.
    fn circle_coedge(&self, e: EdgeId, n: DVec3, outer: bool) -> CoEdge {
        let Curve3::Circle { x, y, .. } = self.edges[e.idx()].curve else {
            panic!("not a circle")
        };
        let ccw = x.cross(y).dot(n) > 0.0;
        ce(e, if outer { !ccw } else { ccw })
    }

    /// Planar face through `verts` (any order; sorted CCW around `n`), with
    /// optional inner wires (closed circle edges).
    fn planar_face(
        &mut self,
        verts: &[VertexId],
        n: DVec3,
        holes: &[EdgeId],
        cache: &mut HashMap<(u32, u32), EdgeId>,
    ) -> FaceId {
        let pts: Vec<DVec3> = verts.iter().map(|v| self.vertices[v.idx()]).collect();
        let c = pts.iter().copied().sum::<DVec3>() / pts.len() as f64;
        let b1 = (pts[0] - c - n * (pts[0] - c).dot(n)).normalize();
        let b2 = n.cross(b1);
        let mut order: Vec<usize> = (0..verts.len()).collect();
        order.sort_by(|&i, &j| {
            let ai = (pts[i] - c).dot(b2).atan2((pts[i] - c).dot(b1));
            let aj = (pts[j] - c).dot(b2).atan2((pts[j] - c).dot(b1));
            ai.total_cmp(&aj)
        });
        // Start at the corner with the smallest coordinates so the UV axes
        // follow the first edge and the domain starts at (0, 0).
        let start = (0..order.len())
            .min_by(|&a, &b| {
                let pa = pts[order[a]];
                let pb = pts[order[b]];
                (pa.x + pa.y + pa.z).total_cmp(&(pb.x + pb.y + pb.z))
            })
            .unwrap();
        order.rotate_left(start);
        let o = pts[order[0]];
        let x = (pts[order[1]] - o).normalize();
        let f = self.add_face(
            Surface::Plane {
                o,
                x,
                y: n.cross(x),
            },
            false,
        );
        let mut coedges = Vec::new();
        for k in 0..order.len() {
            let a = verts[order[k]];
            let b = verts[order[(k + 1) % order.len()]];
            let key = (a.0.min(b.0), a.0.max(b.0));
            let e = match cache.get(&key) {
                Some(&e) => e,
                None => {
                    let e = self.line_edge(a, b);
                    cache.insert(key, e);
                    e
                }
            };
            self.add_plane_pcurve(e, f);
            coedges.push(ce(e, self.edges[e.idx()].v0 != Some(a)));
        }
        self.add_wire(f, true, coedges);
        for &h in holes {
            self.add_plane_pcurve(h, f);
            let c = self.circle_coedge(h, n, false);
            self.add_wire(f, false, vec![c]);
        }
        f
    }

    /// Disk bounded by one closed circle edge, normal `n`.
    fn disk_face(&mut self, circle: EdgeId, n: DVec3) -> FaceId {
        let Curve3::Circle { c, x, .. } = self.edges[circle.idx()].curve else {
            panic!()
        };
        let f = self.add_face(
            Surface::Plane {
                o: c,
                x,
                y: n.cross(x),
            },
            false,
        );
        self.add_plane_pcurve(circle, f);
        let co = self.circle_coedge(circle, n, true);
        self.add_wire(f, true, vec![co]);
        f
    }

    /// Cylindrical side between two circles (`bottom` at v = 0, `top` at
    /// v = h, both parameterised like the surface's u) with a seam line.
    #[allow(clippy::too_many_arguments)]
    fn cylinder_side(
        &mut self,
        o: DVec3,
        z: DVec3,
        x: DVec3,
        r: f64,
        h: f64,
        reversed: bool,
        bottom: EdgeId,
        top: EdgeId,
        seam: EdgeId,
    ) -> FaceId {
        let f = self.add_face(Surface::Cylinder { o, z, x, r }, reversed);
        self.add_pcurve(
            bottom,
            f,
            None,
            Curve2::Line {
                o: DVec2::ZERO,
                d: DVec2::X,
            },
        );
        self.add_pcurve(
            top,
            f,
            None,
            Curve2::Line {
                o: DVec2::new(0.0, h),
                d: DVec2::X,
            },
        );
        self.seam_pcurves_u(seam, f, reversed, 1.0);
        let w = if !reversed {
            vec![
                ce(bottom, false),
                ce(seam, false),
                ce(top, true),
                ce(seam, true),
            ]
        } else {
            vec![
                ce(bottom, true),
                ce(seam, false),
                ce(top, false),
                ce(seam, true),
            ]
        };
        self.add_wire(f, true, w);
        f
    }

    /// Two pcurves of a u-seam (v = t·scale). For a forward face the coedge
    /// with `reversed == false` runs up the u = 2π copy; for a reversed face
    /// it runs up the u = 0 copy.
    fn seam_pcurves_u(&mut self, seam: EdgeId, f: FaceId, reversed: bool, scale: f64) {
        let d = DVec2::new(0.0, scale);
        let (fwd, bwd) = if !reversed { (TAU, 0.0) } else { (0.0, TAU) };
        self.add_pcurve(
            seam,
            f,
            Some(false),
            Curve2::Line {
                o: DVec2::new(fwd, 0.0),
                d,
            },
        );
        self.add_pcurve(
            seam,
            f,
            Some(true),
            Curve2::Line {
                o: DVec2::new(bwd, 0.0),
                d,
            },
        );
    }

    /// Axis-aligned box centred at the origin.
    pub fn make_box(size: DVec3) -> MockBody {
        let mut b = MockBody::named("box");
        let h = size * 0.5;
        let mut vs = Vec::new();
        for i in 0..8 {
            let p = DVec3::new(
                if i & 1 != 0 { h.x } else { -h.x },
                if i & 2 != 0 { h.y } else { -h.y },
                if i & 4 != 0 { h.z } else { -h.z },
            );
            vs.push(b.add_vertex(p));
        }
        let mut cache = HashMap::new();
        let sel = |bit: usize, val: bool| -> Vec<VertexId> {
            (0..8)
                .filter(|i| (i & bit != 0) == val)
                .map(|i| vs[i])
                .collect()
        };
        b.planar_face(&sel(4, false), -DVec3::Z, &[], &mut cache);
        b.planar_face(&sel(4, true), DVec3::Z, &[], &mut cache);
        b.planar_face(&sel(1, false), -DVec3::X, &[], &mut cache);
        b.planar_face(&sel(1, true), DVec3::X, &[], &mut cache);
        b.planar_face(&sel(2, false), -DVec3::Y, &[], &mut cache);
        b.planar_face(&sel(2, true), DVec3::Y, &[], &mut cache);
        b
    }

    /// Solid cylinder on the XY plane, axis +Z: side face 0 (u-closed, seam
    /// edge), bottom cap face 1, top cap face 2.
    pub fn cylinder(r: f64, h: f64) -> MockBody {
        Self::cylinder_with_seam(r, h, 0.0)
    }

    /// The same cylinder with its seam (and u = 0) rotated by `angle`
    /// radians about the axis: identical geometry, different parameterisation.
    pub fn cylinder_with_seam(r: f64, h: f64, angle: f64) -> MockBody {
        let mut b = MockBody::named("cylinder");
        let x = DVec3::new(angle.cos(), angle.sin(), 0.0);
        let y = DVec3::Z.cross(x);
        let v0 = b.add_vertex(x * r);
        let v1 = b.add_vertex(x * r + DVec3::Z * h);
        let bot = b.circle_edge(DVec3::ZERO, x, y, r, v0);
        let top = b.circle_edge(DVec3::Z * h, x, y, r, v1);
        let seam = b.line_edge(v0, v1);
        b.cylinder_side(DVec3::ZERO, DVec3::Z, x, r, h, false, bot, top, seam);
        b.disk_face(bot, -DVec3::Z);
        b.disk_face(top, DVec3::Z);
        b
    }

    /// Plate `10n × 10m × 2` (centred in XY, z ∈ [0, 2]) with an `n × m`
    /// grid of through-holes of radius 2.5. Faces: bottom, top, 4 sides,
    /// then one reversed cylindrical wall per hole.
    pub fn plate_with_holes(n: u32, m: u32) -> MockBody {
        let mut b = MockBody::named("plate");
        let (w, hgt, t, rho) = (10.0 * n as f64, 10.0 * m as f64, 2.0, 2.5);
        let mut vs = Vec::new();
        for i in 0..8 {
            let p = DVec3::new(
                if i & 1 != 0 { w / 2.0 } else { -w / 2.0 },
                if i & 2 != 0 { hgt / 2.0 } else { -hgt / 2.0 },
                if i & 4 != 0 { t } else { 0.0 },
            );
            vs.push(b.add_vertex(p));
        }
        let mut holes = Vec::new();
        for j in 0..m {
            for i in 0..n {
                let c = DVec3::new(
                    -w / 2.0 + 10.0 * (i as f64 + 0.5),
                    -hgt / 2.0 + 10.0 * (j as f64 + 0.5),
                    0.0,
                );
                let vb = b.add_vertex(c + DVec3::X * rho);
                let vt = b.add_vertex(c + DVec3::X * rho + DVec3::Z * t);
                let cb = b.circle_edge(c, DVec3::X, DVec3::Y, rho, vb);
                let ct = b.circle_edge(c + DVec3::Z * t, DVec3::X, DVec3::Y, rho, vt);
                let seam = b.line_edge(vb, vt);
                holes.push((c, cb, ct, seam));
            }
        }
        let mut cache = HashMap::new();
        let sel = |bit: usize, val: bool| -> Vec<VertexId> {
            (0..8)
                .filter(|i| (i & bit != 0) == val)
                .map(|i| vs[i])
                .collect()
        };
        let bots: Vec<EdgeId> = holes.iter().map(|h| h.1).collect();
        let tops: Vec<EdgeId> = holes.iter().map(|h| h.2).collect();
        b.planar_face(&sel(4, false), -DVec3::Z, &bots, &mut cache);
        b.planar_face(&sel(4, true), DVec3::Z, &tops, &mut cache);
        b.planar_face(&sel(1, false), -DVec3::X, &[], &mut cache);
        b.planar_face(&sel(1, true), DVec3::X, &[], &mut cache);
        b.planar_face(&sel(2, false), -DVec3::Y, &[], &mut cache);
        b.planar_face(&sel(2, true), DVec3::Y, &[], &mut cache);
        for (c, cb, ct, seam) in holes {
            b.cylinder_side(c, DVec3::Z, DVec3::X, rho, t, true, cb, ct, seam);
        }
        b
    }

    /// Cone with base radius `r` on the XY plane and its apex at (0, 0, h).
    /// Face 0 is the side (seam + degenerate apex edge), face 1 the base.
    pub fn cone(r: f64, h: f64) -> MockBody {
        let mut b = MockBody::named("cone");
        let vb = b.add_vertex(DVec3::new(r, 0.0, 0.0));
        let va = b.add_vertex(DVec3::new(0.0, 0.0, h));
        let base = b.circle_edge(DVec3::ZERO, DVec3::X, DVec3::Y, r, vb);
        let seam = b.line_edge(vb, va);
        let l = (b.vertices[va.idx()] - b.vertices[vb.idx()]).length();
        let apex = b.add_edge(
            Curve3::Point {
                p: DVec3::new(0.0, 0.0, h),
            },
            0.0,
            TAU,
            Some(va),
            Some(va),
        );
        let f = b.add_face(
            Surface::Cone {
                o: DVec3::ZERO,
                z: DVec3::Z,
                x: DVec3::X,
                r0: r,
                k: -r / h,
            },
            false,
        );
        b.add_pcurve(
            base,
            f,
            None,
            Curve2::Line {
                o: DVec2::ZERO,
                d: DVec2::X,
            },
        );
        b.seam_pcurves_u(seam, f, false, h / l);
        b.add_pcurve(
            apex,
            f,
            None,
            Curve2::Line {
                o: DVec2::new(0.0, h),
                d: DVec2::X,
            },
        );
        b.add_wire(
            f,
            true,
            vec![
                ce(base, false),
                ce(seam, false),
                ce(apex, true),
                ce(seam, true),
            ],
        );
        b.disk_face(base, -DVec3::Z);
        b
    }

    /// Full sphere: one face with a seam and two degenerate pole edges.
    pub fn sphere(r: f64) -> MockBody {
        let mut b = MockBody::named("sphere");
        let s = b.add_vertex(DVec3::new(0.0, 0.0, -r));
        let n = b.add_vertex(DVec3::new(0.0, 0.0, r));
        let seam = b.add_edge(
            Curve3::Circle {
                c: DVec3::ZERO,
                x: DVec3::X,
                y: DVec3::Z,
                r,
            },
            -FRAC_PI_2,
            FRAC_PI_2,
            Some(s),
            Some(n),
        );
        let ds = b.add_edge(
            Curve3::Point {
                p: DVec3::new(0.0, 0.0, -r),
            },
            0.0,
            TAU,
            Some(s),
            Some(s),
        );
        let dn = b.add_edge(
            Curve3::Point {
                p: DVec3::new(0.0, 0.0, r),
            },
            0.0,
            TAU,
            Some(n),
            Some(n),
        );
        let f = b.add_face(
            Surface::Sphere {
                o: DVec3::ZERO,
                z: DVec3::Z,
                x: DVec3::X,
                r,
            },
            false,
        );
        b.seam_pcurves_u(seam, f, false, 1.0);
        b.add_pcurve(
            ds,
            f,
            None,
            Curve2::Line {
                o: DVec2::new(0.0, -FRAC_PI_2),
                d: DVec2::X,
            },
        );
        b.add_pcurve(
            dn,
            f,
            None,
            Curve2::Line {
                o: DVec2::new(0.0, FRAC_PI_2),
                d: DVec2::X,
            },
        );
        b.add_wire(
            f,
            true,
            vec![ce(ds, false), ce(seam, false), ce(dn, true), ce(seam, true)],
        );
        b
    }

    fn torus_base(b: &mut MockBody, major: f64, minor: f64) -> FaceId {
        let v = b.add_vertex(DVec3::new(major + minor, 0.0, 0.0));
        let useam = b.circle_edge(DVec3::X * major, DVec3::X, DVec3::Z, minor, v);
        let vseam = b.circle_edge(DVec3::ZERO, DVec3::X, DVec3::Y, major + minor, v);
        let f = b.add_face(
            Surface::Torus {
                o: DVec3::ZERO,
                z: DVec3::Z,
                x: DVec3::X,
                major,
                minor,
            },
            false,
        );
        b.seam_pcurves_u(useam, f, false, 1.0);
        b.add_pcurve(
            vseam,
            f,
            Some(false),
            Curve2::Line {
                o: DVec2::ZERO,
                d: DVec2::X,
            },
        );
        b.add_pcurve(
            vseam,
            f,
            Some(true),
            Curve2::Line {
                o: DVec2::new(0.0, TAU),
                d: DVec2::X,
            },
        );
        b.add_wire(
            f,
            true,
            vec![
                ce(vseam, false),
                ce(useam, false),
                ce(vseam, true),
                ce(useam, true),
            ],
        );
        f
    }

    /// Full torus around +Z: no boundary, a u-seam and a v-seam.
    pub fn torus(major: f64, minor: f64) -> MockBody {
        let mut b = MockBody::named("torus");
        Self::torus_base(&mut b, major, minor);
        b
    }

    /// Torus trimmed by one hole: a UV circle of radius 0.6 centred at
    /// (π, π/2). Its 3D curve is the surface along the pcurve. Open shell.
    pub fn trimmed_torus(major: f64, minor: f64) -> MockBody {
        let mut b = MockBody::named("trimmed_torus");
        let f = Self::torus_base(&mut b, major, minor);
        let pc = Curve2::Ellipse {
            c: DVec2::new(PI, FRAC_PI_2),
            a: DVec2::new(0.6, 0.0),
            b: DVec2::new(0.0, 0.6),
        };
        let p0 = b.faces[f.idx()].surface.point(pc.eval(0.0));
        let hv = b.add_vertex(p0);
        let hole = b.add_edge(
            Curve3::OnFace {
                face: f.0,
                pc: pc.clone(),
            },
            0.0,
            TAU,
            Some(hv),
            Some(hv),
        );
        b.add_pcurve(hole, f, None, pc);
        b.add_wire(f, false, vec![ce(hole, true)]);
        b
    }

    /// Planar annulus in the XY plane (normal +Z). Open shell.
    pub fn ring(outer: f64, inner: f64) -> MockBody {
        let mut b = MockBody::named("ring");
        let vo = b.add_vertex(DVec3::new(outer, 0.0, 0.0));
        let vi = b.add_vertex(DVec3::new(inner, 0.0, 0.0));
        let co = b.circle_edge(DVec3::ZERO, DVec3::X, DVec3::Y, outer, vo);
        let ci = b.circle_edge(DVec3::ZERO, DVec3::X, DVec3::Y, inner, vi);
        let f = b.add_face(
            Surface::Plane {
                o: DVec3::ZERO,
                x: DVec3::X,
                y: DVec3::Y,
            },
            false,
        );
        b.add_plane_pcurve(co, f);
        b.add_plane_pcurve(ci, f);
        b.add_wire(f, true, vec![ce(co, false)]);
        b.add_wire(f, false, vec![ce(ci, true)]);
        b
    }

    /// A part symmetric about the YZ plane (mirror X): box 8 × 6 × 4 with a
    /// cylindrical boss (r = 1.2, length 2) on the +X and −X faces.
    pub fn symmetric_part() -> MockBody {
        let mut b = MockBody::named("symmetric");
        let (a, bb, c, rho, l) = (4.0, 3.0, 2.0, 1.2, 2.0);
        let mut vs = Vec::new();
        for i in 0..8 {
            let p = DVec3::new(
                if i & 1 != 0 { a } else { -a },
                if i & 2 != 0 { bb } else { -bb },
                if i & 4 != 0 { c } else { -c },
            );
            vs.push(b.add_vertex(p));
        }
        // Bosses: (origin, axis, y-direction of the circle param).
        let mut bosses = Vec::new();
        for s in [1.0, -1.0] {
            let o = DVec3::X * (a * s);
            let z = DVec3::X * s;
            let x = DVec3::Y;
            let y = z.cross(x);
            let vb = b.add_vertex(o + x * rho);
            let ve = b.add_vertex(o + z * l + x * rho);
            let cb = b.circle_edge(o, x, y, rho, vb);
            let cend = b.circle_edge(o + z * l, x, y, rho, ve);
            let seam = b.line_edge(vb, ve);
            bosses.push((o, z, x, cb, cend, seam));
        }
        let mut cache = HashMap::new();
        let sel = |bit: usize, val: bool| -> Vec<VertexId> {
            (0..8)
                .filter(|i| (i & bit != 0) == val)
                .map(|i| vs[i])
                .collect()
        };
        b.planar_face(&sel(4, false), -DVec3::Z, &[], &mut cache);
        b.planar_face(&sel(4, true), DVec3::Z, &[], &mut cache);
        b.planar_face(&sel(1, false), -DVec3::X, &[bosses[1].3], &mut cache);
        b.planar_face(&sel(1, true), DVec3::X, &[bosses[0].3], &mut cache);
        b.planar_face(&sel(2, false), -DVec3::Y, &[], &mut cache);
        b.planar_face(&sel(2, true), DVec3::Y, &[], &mut cache);
        for &(o, z, x, cb, cend, seam) in &bosses {
            b.cylinder_side(o, z, x, rho, l, false, cb, cend, seam);
            b.disk_face(cend, z);
        }
        b
    }

    /// Cylinder whose side is two half-cylinder faces (u ∈ [0, π] and
    /// [π, 2π]) joined by two tangent generator edges; rims are two arcs
    /// each. Faces: 0, 1 sides; 2 bottom; 3 top.
    pub fn split_cylinder(r: f64, h: f64) -> MockBody {
        let mut b = MockBody::named("split_cylinder");
        let b0 = b.add_vertex(DVec3::new(r, 0.0, 0.0));
        let b1 = b.add_vertex(DVec3::new(-r, 0.0, 0.0));
        let t0 = b.add_vertex(DVec3::new(r, 0.0, h));
        let t1 = b.add_vertex(DVec3::new(-r, 0.0, h));
        let circ = |zc: f64| Curve3::Circle {
            c: DVec3::Z * zc,
            x: DVec3::X,
            y: DVec3::Y,
            r,
        };
        let a0 = b.add_edge(circ(0.0), 0.0, PI, Some(b0), Some(b1));
        let a1 = b.add_edge(circ(0.0), PI, TAU, Some(b1), Some(b0));
        let a2 = b.add_edge(circ(h), 0.0, PI, Some(t0), Some(t1));
        let a3 = b.add_edge(circ(h), PI, TAU, Some(t1), Some(t0));
        let g0 = b.line_edge(b0, t0);
        let g1 = b.line_edge(b1, t1);
        let surf = Surface::Cylinder {
            o: DVec3::ZERO,
            z: DVec3::Z,
            x: DVec3::X,
            r,
        };
        let f0 = b.add_face(surf.clone(), false);
        let f1 = b.add_face(surf, false);
        let lx = |v: f64| Curve2::Line {
            o: DVec2::new(0.0, v),
            d: DVec2::X,
        };
        let ly = |u: f64| Curve2::Line {
            o: DVec2::new(u, 0.0),
            d: DVec2::Y,
        };
        b.add_pcurve(a0, f0, None, lx(0.0));
        b.add_pcurve(a2, f0, None, lx(h));
        b.add_pcurve(g0, f0, None, ly(0.0));
        b.add_pcurve(g1, f0, None, ly(PI));
        b.add_wire(
            f0,
            true,
            vec![ce(a0, false), ce(g1, false), ce(a2, true), ce(g0, true)],
        );
        b.add_pcurve(a1, f1, None, lx(0.0));
        b.add_pcurve(a3, f1, None, lx(h));
        b.add_pcurve(g1, f1, None, ly(PI));
        b.add_pcurve(g0, f1, None, ly(TAU));
        b.add_wire(
            f1,
            true,
            vec![ce(a1, false), ce(g0, false), ce(a3, true), ce(g1, true)],
        );
        let fb = b.add_face(
            Surface::Plane {
                o: DVec3::ZERO,
                x: DVec3::X,
                y: -DVec3::Y,
            },
            false,
        );
        b.add_plane_pcurve(a0, fb);
        b.add_plane_pcurve(a1, fb);
        b.add_wire(fb, true, vec![ce(a1, true), ce(a0, true)]);
        let ft = b.add_face(
            Surface::Plane {
                o: DVec3::Z * h,
                x: DVec3::X,
                y: DVec3::Y,
            },
            false,
        );
        b.add_plane_pcurve(a2, ft);
        b.add_plane_pcurve(a3, ft);
        b.add_wire(ft, true, vec![ce(a2, false), ce(a3, false)]);
        b
    }
}

impl MockBody {
    /// The same body moved by a rigid motion (rotation + translation);
    /// parameterisations are carried along, so faces map one to one.
    pub fn transformed(&self, m: glam::DAffine3) -> MockBody {
        let p = |v: DVec3| m.transform_point3(v);
        let d = |v: DVec3| m.transform_vector3(v);
        let mut b = self.clone();
        for v in &mut b.vertices {
            *v = p(*v);
        }
        for f in &mut b.faces {
            f.surface = match f.surface.clone() {
                Surface::Plane { o, x, y } => Surface::Plane {
                    o: p(o),
                    x: d(x),
                    y: d(y),
                },
                Surface::Cylinder { o, z, x, r } => Surface::Cylinder {
                    o: p(o),
                    z: d(z),
                    x: d(x),
                    r,
                },
                Surface::Cone { o, z, x, r0, k } => Surface::Cone {
                    o: p(o),
                    z: d(z),
                    x: d(x),
                    r0,
                    k,
                },
                Surface::Sphere { o, z, x, r } => Surface::Sphere {
                    o: p(o),
                    z: d(z),
                    x: d(x),
                    r,
                },
                Surface::Torus {
                    o,
                    z,
                    x,
                    major,
                    minor,
                } => Surface::Torus {
                    o: p(o),
                    z: d(z),
                    x: d(x),
                    major,
                    minor,
                },
            };
        }
        for e in &mut b.edges {
            e.curve = match e.curve.clone() {
                Curve3::Line { p0, d: dd } => Curve3::Line {
                    p0: p(p0),
                    d: d(dd),
                },
                Curve3::Circle { c, x, y, r } => Curve3::Circle {
                    c: p(c),
                    x: d(x),
                    y: d(y),
                    r,
                },
                Curve3::Point { p: q } => Curve3::Point { p: p(q) },
                c @ Curve3::OnFace { .. } => c,
            };
        }
        b
    }
}
