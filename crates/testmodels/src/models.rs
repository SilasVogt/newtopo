use anyhow::{Context, Result, anyhow};
use glam::{DMat3, DMat4, DVec3};
use newtopo_occt_shim::testshapes::ffi as ts;
use newtopo_occt_shim::{self as shim, Ctx, ffi};
use std::f64::consts::{FRAC_PI_2, PI};
use std::path::{Path, PathBuf};
use std::pin::Pin;

pub use ts::XcafNode;

/// Builds shapes on the OCCT thread and remembers every body it made so they
/// can be unloaded afterwards.
pub struct Builder<'a> {
    ctx: &'a mut Ctx,
    made: Vec<u32>,
}

/// One XCAF node per part; see [`ts::XcafNode`].
pub type Nodes = Vec<XcafNode>;

/// A reference model of SPEC §11.
pub struct RefModel {
    /// File stem, e.g. `filleted_box`.
    pub name: &'static str,
    pub build: fn(&mut Builder) -> Result<Nodes>,
}

/// Every reference model of SPEC §11, in a fixed order.
pub const MODELS: &[RefModel] = &[
    RefModel { name: "box", build: box_model },
    RefModel { name: "filleted_box", build: filleted_box },
    RefModel { name: "cylinder_chamfered", build: cylinder_chamfered },
    RefModel { name: "plate_holes", build: plate_holes },
    RefModel { name: "bottle", build: bottle },
    RefModel { name: "torus", build: torus },
    RefModel { name: "sphere", build: sphere },
    RefModel { name: "cone_apex", build: cone_apex },
    RefModel { name: "torus_hole", build: torus_hole },
    RefModel { name: "bracket", build: bracket },
    RefModel { name: "flange", build: flange },
    RefModel { name: "assembly", build: assembly },
];

fn err(e: shim::cxx::Exception) -> anyhow::Error {
    anyhow!("{}", e.what())
}

fn row_major(m: DMat4) -> [f64; 16] {
    m.transpose().to_cols_array()
}

fn rigid(m: DMat4) -> [f64; 12] {
    let a = row_major(m);
    let mut out = [0.0; 12];
    out.copy_from_slice(&a[..12]);
    out
}

/// A node with an identity placement.
pub fn node(name: &str, parent: i32, body: i32, layer: &str, color: Option<[f32; 3]>, place: DMat4) -> XcafNode {
    XcafNode {
        name: name.into(),
        parent,
        body,
        layer: layer.into(),
        has_color: color.is_some(),
        color: color.unwrap_or([0.0; 3]),
        transform: row_major(place),
    }
}

fn single(name: &str, body: u32) -> Nodes {
    vec![node(name, -1, body as i32, "", None, DMat4::IDENTITY)]
}

impl Builder<'_> {
    fn store(&mut self) -> Pin<&mut ffi::Store> {
        self.ctx.store()
    }
    fn keep(&mut self, r: Result<u32, shim::cxx::Exception>) -> Result<u32> {
        let id = r.map_err(err)?;
        self.made.push(id);
        Ok(id)
    }
    pub fn boxed(&mut self, at: DVec3, size: DVec3) -> Result<u32> {
        let r = ts::ts_box(self.store(), at.x, at.y, at.z, size.x, size.y, size.z);
        self.keep(r)
    }
    pub fn cylinder(&mut self, r: f64, h: f64) -> Result<u32> {
        let r = ts::ts_cylinder(self.store(), r, h);
        self.keep(r)
    }
    pub fn cone(&mut self, r1: f64, r2: f64, h: f64) -> Result<u32> {
        let r = ts::ts_cone(self.store(), r1, r2, h);
        self.keep(r)
    }
    pub fn sphere(&mut self, r: f64) -> Result<u32> {
        let r = ts::ts_sphere(self.store(), r);
        self.keep(r)
    }
    pub fn torus(&mut self, major: f64, minor: f64) -> Result<u32> {
        let r = ts::ts_torus(self.store(), major, minor);
        self.keep(r)
    }
    pub fn transform(&mut self, body: u32, m: DMat4) -> Result<u32> {
        let r = ts::ts_transform(self.store(), body, &rigid(m));
        self.keep(r)
    }
    /// Cylinder of radius `r` from `from` along unit `dir` for length `h`.
    pub fn rod(&mut self, from: DVec3, dir: DVec3, r: f64, h: f64) -> Result<u32> {
        let c = self.cylinder(r, h)?;
        let z = dir.normalize();
        let x = if z.x.abs() < 0.9 { DVec3::X } else { DVec3::Y };
        let x = (x - z * x.dot(z)).normalize();
        let y = z.cross(x);
        let m = DMat4::from_cols(x.extend(0.0), y.extend(0.0), z.extend(0.0), from.extend(1.0));
        self.transform(c, m)
    }
    pub fn fillet(&mut self, body: u32, edges: &[u32], r: f64) -> Result<u32> {
        let res = ts::ts_fillet(self.store(), body, edges, r);
        self.keep(res)
    }
    pub fn chamfer(&mut self, body: u32, edges: &[u32], d: f64) -> Result<u32> {
        let res = ts::ts_chamfer(self.store(), body, edges, d);
        self.keep(res)
    }
    pub fn fuse(&mut self, a: u32, tools: &[u32]) -> Result<u32> {
        let r = ts::ts_boolean(self.store(), 0, a, tools);
        self.keep(r)
    }
    pub fn cut(&mut self, a: u32, tools: &[u32]) -> Result<u32> {
        let r = ts::ts_boolean(self.store(), 1, a, tools);
        self.keep(r)
    }
    pub fn revolve(&mut self, profile: &[(f64, f64)], spline: (i32, i32)) -> Result<u32> {
        let flat: Vec<f64> = profile.iter().flat_map(|&(r, z)| [r, z]).collect();
        let r = ts::ts_revolve(self.store(), &flat, spline.0, spline.1);
        self.keep(r)
    }
    pub fn prism(&mut self, polygon: &[(f64, f64)], h: f64) -> Result<u32> {
        let flat: Vec<f64> = polygon.iter().flat_map(|&(x, y)| [x, y]).collect();
        let r = ts::ts_prism(self.store(), &flat, h);
        self.keep(r)
    }
    /// Edge ids of `body` whose end vertices both satisfy `pred`.
    pub fn edges_where(&mut self, body: u32, pred: impl Fn(DVec3, DVec3, f64) -> bool) -> Result<Vec<u32>> {
        let t = ffi::topology(self.store(), body).map_err(err)?;
        let vtx = |i: i64| DVec3::new(t.vertices[3 * i as usize], t.vertices[3 * i as usize + 1], t.vertices[3 * i as usize + 2]);
        Ok(t.edges
            .iter()
            .enumerate()
            .filter(|(_, e)| e.v0 >= 0 && e.v1 >= 0 && !e.seam && !e.degenerate && pred(vtx(e.v0), vtx(e.v1), e.length))
            .map(|(i, _)| i as u32)
            .collect())
    }
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

pub fn box_model(b: &mut Builder) -> Result<Nodes> {
    let id = b.boxed(DVec3::ZERO, DVec3::new(40.0, 30.0, 20.0))?;
    Ok(single("Box", id))
}

pub fn filleted_box(b: &mut Builder) -> Result<Nodes> {
    let bx = b.boxed(DVec3::ZERO, DVec3::new(40.0, 30.0, 20.0))?;
    let id = b.fillet(bx, &[], 4.0)?;
    Ok(vec![node("Filleted box", -1, id as i32, "Parts", Some([0.2, 0.45, 0.8]), DMat4::IDENTITY)])
}

pub fn cylinder_chamfered(b: &mut Builder) -> Result<Nodes> {
    let c = b.cylinder(15.0, 40.0)?;
    let id = b.chamfer(c, &[], 2.0)?;
    Ok(single("Chamfered cylinder", id))
}

pub fn plate_holes(b: &mut Builder) -> Result<Nodes> {
    let plate = b.boxed(DVec3::ZERO, DVec3::new(100.0, 100.0, 8.0))?;
    let mut holes = Vec::new();
    for i in 0..4 {
        for j in 0..4 {
            let at = DVec3::new(12.5 + 25.0 * i as f64, 12.5 + 25.0 * j as f64, -1.0);
            holes.push(b.rod(at, DVec3::Z, 4.0, 10.0)?);
        }
    }
    let id = b.cut(plate, &holes)?;
    Ok(single("Plate", id))
}

pub fn bottle(b: &mut Builder) -> Result<Nodes> {
    let profile = [(0.0, 0.0), (25.0, 0.0), (26.0, 30.0), (24.0, 60.0), (14.0, 80.0), (9.0, 92.0), (9.0, 100.0), (0.0, 100.0)];
    let id = b.revolve(&profile, (1, 5))?;
    Ok(single("Bottle", id))
}

pub fn torus(b: &mut Builder) -> Result<Nodes> {
    let id = b.torus(40.0, 10.0)?;
    Ok(single("Torus", id))
}

pub fn sphere(b: &mut Builder) -> Result<Nodes> {
    let id = b.sphere(25.0)?;
    Ok(single("Sphere", id))
}

pub fn cone_apex(b: &mut Builder) -> Result<Nodes> {
    let id = b.cone(20.0, 0.0, 40.0)?;
    Ok(single("Cone", id))
}

/// Torus with one blind hole: its toroidal face is trimmed by a single loop.
pub fn torus_hole(b: &mut Builder) -> Result<Nodes> {
    let t = b.torus(40.0, 12.0)?;
    let drill = b.rod(DVec3::new(45.0, 0.0, 0.0), DVec3::X, 4.0, 15.0)?;
    let id = b.cut(t, &[drill])?;
    Ok(single("Torus with hole", id))
}

/// L-bracket symmetric under x → −x, with a filleted inner corner and holes.
pub fn bracket(b: &mut Builder) -> Result<Nodes> {
    let base = b.boxed(DVec3::new(-40.0, 0.0, 0.0), DVec3::new(80.0, 40.0, 8.0))?;
    let wall = b.boxed(DVec3::new(-40.0, 0.0, 0.0), DVec3::new(80.0, 8.0, 50.0))?;
    let l = b.fuse(base, &[wall])?;
    let corner = b.edges_where(l, |p, q, _| near(p.y, 8.0) && near(p.z, 8.0) && near(q.y, 8.0) && near(q.z, 8.0))?;
    anyhow::ensure!(corner.len() == 1, "bracket: expected one inner corner edge, found {}", corner.len());
    let filleted = b.fillet(l, &corner, 5.0)?;
    let mut holes = Vec::new();
    for x in [-25.0, 25.0] {
        holes.push(b.rod(DVec3::new(x, 26.0, -1.0), DVec3::Z, 4.0, 10.0)?);
    }
    for x in [-20.0, 20.0] {
        holes.push(b.rod(DVec3::new(x, -1.0, 32.0), DVec3::Y, 4.0, 10.0)?);
    }
    let id = b.cut(filleted, &holes)?;
    Ok(vec![node("Bracket", -1, id as i32, "Brackets", Some([0.6, 0.62, 0.66]), DMat4::IDENTITY)])
}

/// Flange with 6-fold symmetry about Z: disk, filleted hub, bore, bolt circle.
pub fn flange(b: &mut Builder) -> Result<Nodes> {
    let disk = b.cylinder(60.0, 10.0)?;
    let hub = b.cylinder(30.0, 30.0)?;
    let body = b.fuse(disk, &[hub])?;
    let ring = b.edges_where(body, |p, q, len| {
        near(p.z, 10.0) && near(q.z, 10.0) && near(len, 2.0 * PI * 30.0) && near(p.truncate().length(), 30.0)
    })?;
    anyhow::ensure!(!ring.is_empty(), "flange: hub/disk edge not found");
    let filleted = b.fillet(body, &ring, 4.0)?;
    let mut tools = vec![b.rod(DVec3::new(0.0, 0.0, -1.0), DVec3::Z, 15.0, 32.0)?];
    for k in 0..6 {
        let a = k as f64 * PI / 3.0;
        tools.push(b.rod(DVec3::new(45.0 * a.cos(), 45.0 * a.sin(), -1.0), DVec3::Z, 5.0, 12.0)?);
    }
    let id = b.cut(filleted, &tools)?;
    Ok(single("Flange", id))
}

/// Small assembly with a nested sub-assembly, a shared prototype placed twice,
/// layers, names and colours.
pub fn assembly(b: &mut Builder) -> Result<Nodes> {
    let base = b.boxed(DVec3::ZERO, DVec3::new(80.0, 20.0, 10.0))? as i32;
    let post = b.cylinder(4.0, 40.0)? as i32;
    let knob = b.sphere(8.0)? as i32;
    let ring = b.torus(10.0, 2.0)? as i32;
    let t = |x: f64, y: f64, z: f64| DMat4::from_translation(DVec3::new(x, y, z));
    let rot_x = DMat4::from_mat3(DMat3::from_rotation_x(FRAC_PI_2));
    Ok(vec![
        node("Bench", -1, -1, "", None, DMat4::IDENTITY),
        node("Base", 0, base, "Structure", Some([0.35, 0.4, 0.5]), DMat4::IDENTITY),
        node("Post left", 0, post, "Structure", Some([0.8, 0.8, 0.82]), t(10.0, 10.0, 10.0)),
        node("Post right", 0, post, "Structure", None, t(70.0, 10.0, 10.0)),
        node("Cap", 0, -1, "", None, t(40.0, 10.0, 50.0)),
        node("Knob", 4, knob, "Hardware", Some([0.85, 0.1, 0.1]), DMat4::IDENTITY),
        node("Ring", 4, ring, "Hardware", Some([0.95, 0.8, 0.1]), t(0.0, 0.0, -6.0) * rot_x),
    ])
}

/// Builds `model` and writes it as STEP to `path`.
pub fn write_model(model: &RefModel, path: &Path) -> Result<()> {
    let build = model.build;
    let path_s = path.to_str().context("path is not UTF-8")?.to_owned();
    let name = model.name;
    shim::run(move |ctx| {
        let mut b = Builder { ctx, made: Vec::new() };
        let res = build(&mut b).and_then(|nodes| {
            ts::ts_write_step(b.ctx.store(), &path_s, &nodes).map_err(err)?;
            Ok(())
        });
        for id in std::mem::take(&mut b.made) {
            ffi::unload(b.ctx.store(), id);
        }
        res.with_context(|| format!("reference model {name}"))
    })
    .map_err(|e| anyhow!(e))?
}

/// Writes every reference model as `<dir>/<name>.step`.
pub fn write_all(dir: &Path) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir)?;
    MODELS
        .iter()
        .map(|m| {
            let p = dir.join(format!("{}.step", m.name));
            write_model(m, &p)?;
            Ok(p)
        })
        .collect()
}

/// Looks a model up by name.
pub fn model(name: &str) -> Option<&'static RefModel> {
    MODELS.iter().find(|m| m.name == name)
}
