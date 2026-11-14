use glam::DVec3;
use newtopo_core::doc::Document;
use newtopo_core::eval::{self, BodyCtx};
use newtopo_kernel::mock::{MockBody, MockKernel};
use newtopo_kernel::{BodyId, CadKernel};
use std::sync::Arc;

fn run(name: &str, body: MockBody, autofill: bool) {
    let k = Arc::new(MockKernel::new());
    let id = k.add_body(body);
    let kd: Arc<dyn CadKernel> = k.clone();
    let ctx = BodyCtx::build(kd, id).unwrap();
    let mut doc = Document::default();
    doc.settings.autofill = autofill;
    let t = std::time::Instant::now();
    let ev = eval::evaluate(&ctx, &doc, BodyId(0));
    let dt = t.elapsed();
    let issues = eval::validate_body(&ctx, &ev);
    let s = ev.mesh.stats();
    println!("{name} fill={autofill}: {:?} in {:?}; issues {}", s, dt, issues.len());
    for i in issues.iter().take(8) {
        println!("   {:?} {} {:?}", i.kind, i.message, i.vertices);
    }
}

#[test]
fn diag_untouched() {
    for af in [false, true] {
        run("box", MockBody::make_box(DVec3::new(4.0, 3.0, 2.0)), af);
        run("cylinder", MockBody::cylinder(2.0, 5.0), af);
        run("plate", MockBody::plate_with_holes(2, 2), af);
        run("cone", MockBody::cone(2.0, 4.0), af);
        run("sphere", MockBody::sphere(3.0), af);
        run("torus", MockBody::torus(4.0, 1.0), af);
        run("trimmed_torus", MockBody::trimmed_torus(4.0, 1.0), af);
        run("ring", MockBody::ring(3.0, 1.0), af);
        run("symmetric", MockBody::symmetric_part(), af);
        run("split_cyl", MockBody::split_cylinder(2.0, 4.0), af);
    }
}

pub fn dump(body: MockBody, autofill: bool) {
    let k = Arc::new(MockKernel::new());
    let id = k.add_body(body);
    let kd: Arc<dyn CadKernel> = k.clone();
    let ctx = BodyCtx::build(kd, id).unwrap();
    let mut doc = Document::default();
    doc.settings.autofill = autofill;
    let ev = eval::evaluate(&ctx, &doc, BodyId(0));
    for (i, v) in ev.mesh.vertices.iter().enumerate() {
        println!("v{i} {:?} {:?} {:.3?}", v.key, v.anchor, v.position);
    }
    for p in &ev.mesh.polygons {
        println!("poly face {} {:?}", p.face, p.verts);
    }
    for e in &ev.mesh.edges {
        println!("edge {} {} {:?}", e.a, e.b, e.kind);
    }
}

#[test]
fn diag_dump() {
    let which = std::env::var("DUMP").unwrap_or_default();
    match which.as_str() {
        "cylinder" => dump(MockBody::cylinder(2.0, 5.0), false),
        "sphere" => dump(MockBody::sphere(3.0), false),
        "torus" => dump(MockBody::torus(4.0, 1.0), false),
        "ring" => dump(MockBody::ring(3.0, 1.0), false),
        "split" => dump(MockBody::split_cylinder(2.0, 4.0), false),
        "cone" => dump(MockBody::cone(2.0, 4.0), false),
        _ => {}
    }
}

#[test]
fn diag_face() {
    let Ok(f) = std::env::var("FACE") else { return };
    let f: u32 = f.parse().unwrap();
    let k = Arc::new(MockKernel::new());
    let id = k.add_body(MockBody::cylinder(2.0, 5.0));
    let kd: Arc<dyn CadKernel> = k.clone();
    let ctx = BodyCtx::build(kd, id).unwrap();
    let fg = ctx.face(newtopo_kernel::FaceId(f));
    println!("face {f}: lo {:?} hi {:?} uper {:?} curves {}", fg.lo, fg.hi, fg.uper, fg.curves.len());
    for c in &fg.curves {
        println!("  curve {:?} seam {:?} pts {} par {:?}..{:?} ends {:?}", c.owner, c.seam, c.pts.len(), c.par[0], c.par[c.par.len()-1], c.ends);
    }
    let s1 = eval::face::stage1(&ctx, BodyId(0), fg, &[]);
    println!("nodes {} chains {} cells {}", s1.arr.nodes.len(), s1.arr.chains.len(), s1.arr.cells.len());
    for (i, c) in s1.arr.cells.iter().enumerate() {
        println!("  cell {i} inside {} sample {:?} holes {:?} area {}", c.inside, c.sample, c.holes, s1.arr.area[c.outer]);
    }
    for (i, o) in s1.arr.orbits.iter().enumerate() {
        println!("  orbit {i} {:?} area {}", o, s1.arr.area[i]);
    }
    println!("regions {:?}", s1.glued.regions);
}

#[test]
fn diag_grid() {
    let Ok(which) = std::env::var("GRID") else { return };
    use newtopo_core::doc::{PatchId, Rule, RuleEntry, Spacing};
    use newtopo_core::keys::RuleId;
    let (body, face, cu, cv) = match which.as_str() {
        "cyl" => (MockBody::cylinder(2.0, 5.0), 0, 4, 0),
        "cylv" => (MockBody::cylinder(2.0, 5.0), 0, 4, 2),
        "tt" => (MockBody::trimmed_torus(4.0, 1.0), 0, 8, 8),
        _ => (MockBody::sphere(3.0), 0, 4, 1),
    };
    let k = Arc::new(MockKernel::new());
    let id = k.add_body(body);
    let kd: Arc<dyn CadKernel> = k.clone();
    let ctx = BodyCtx::build(kd, id).unwrap();
    let mut doc = Document::default();
    doc.settings.autofill = false;
    doc.rules.push(RuleEntry {
        id: RuleId(1),
        ordinal: 1,
        rule: Rule::Grid { patch: PatchId::whole(BodyId(0), newtopo_kernel::FaceId(face)), count_u: cu, count_v: cv, start_on_seam: false, spacing: Spacing::ArcLength },
        link_group: None,
    });
    let ev = eval::evaluate(&ctx, &doc, BodyId(0));
    let issues = eval::validate_body(&ctx, &ev);
    for (i, v) in ev.mesh.vertices.iter().enumerate() {
        println!("v{i} {:?} {:?} {:.3?}", v.key, v.anchor, v.position);
    }
    for p in &ev.mesh.polygons {
        println!("poly face {} {:?}", p.face, p.verts);
    }
    for i in issues.iter().take(10) {
        println!("ISSUE {:?} {} {:?}", i.kind, i.message, i.vertices);
    }
    for e in &ev.mesh.edges {
        println!("edge {} {} {:?}", e.a, e.b, e.kind);
    }
}

#[test]
fn diag_glue() {
    let Ok(_) = std::env::var("GLUE") else { return };
    use newtopo_core::doc::{PatchId, Spacing};
    let k = Arc::new(MockKernel::new());
    let id = k.add_body(MockBody::cylinder(2.0, 5.0));
    let kd: Arc<dyn CadKernel> = k.clone();
    let ctx = BodyCtx::build(kd, id).unwrap();
    let doc = Document::default();
    let fg = ctx.face(newtopo_kernel::FaceId(0));
    let s1 = eval::face::stage1(&ctx, BodyId(0), fg, &[]);
    let p = &s1.patches[0];
    println!("patch {:?} wraps {:?} lo {:?} hi {:?}", p.id, p.wraps, p.lo, p.hi);
    let mut lines = Vec::new();
    let r = newtopo_core::doc::Rule::Grid { patch: PatchId::whole(BodyId(0), fg.id), count_u: 4, count_v: 0, start_on_seam: false, spacing: Spacing::ArcLength };
    eval::lines::rule_lines(&ctx, &doc, BodyId(0), fg, &s1.patches, newtopo_core::keys::RuleId(1), &r, &Default::default(), &mut lines);
    for l in &lines {
        println!("line {:?} pts {:?} closed {}", l.who, l.pts, l.closed);
    }
    let (arr, gl) = eval::face::stage3(&ctx, fg, &s1, &lines);
    for (i, c) in arr.curves.iter().enumerate() {
        println!("curve {i} {:?} seam {:?} pts {:?}", c.owner, c.seam, c.pts);
    }
    for (i, n) in arr.nodes.iter().enumerate() {
        println!("node {i} {:?} uv {:?} incs {:?}", n.key, n.uv, n.incs);
    }
    for (i, c) in arr.chains.iter().enumerate() {
        println!("chain {i} curve {} {}->{} pa {} pb {}", c.curve, c.a, c.b, c.pa, c.pb);
    }
    for (i, e) in gl.gedges.iter().enumerate() {
        println!("gedge {i} {:?} {}->{} chains {:?} dissolved {} left {:?} {:?}", e.owner, e.a, e.b, e.chains, gl.dissolved[i], gl.left[i*2], gl.left[i*2+1]);
    }
    println!("cells {:?}", arr.cells.iter().map(|c| (c.inside, c.holes.len())).collect::<Vec<_>>());
    println!("regions {:?}", gl.regions);
}

#[test]
fn diag_clip_tt() {
    let Ok(_) = std::env::var("CLIPTT") else { return };
    use glam::DVec2;
    let k = Arc::new(MockKernel::new());
    let id = k.add_body(MockBody::trimmed_torus(4.0, 1.0));
    let kd: Arc<dyn CadKernel> = k.clone();
    let ctx = BodyCtx::build(kd, id).unwrap();
    let fg = ctx.face(newtopo_kernel::FaceId(0));
    let s1 = eval::face::stage1(&ctx, BodyId(0), fg, &[]);
    let pts = vec![DVec2::new(0.0, std::f64::consts::FRAC_PI_2), DVec2::new(std::f64::consts::TAU, std::f64::consts::FRAC_PI_2)];
    let input = eval::clip::ClipInput {
        who: eval::clip::Who::Line(newtopo_core::keys::LineKey { rule: newtopo_core::keys::RuleId(1), index: 9, sub: 0 }),
        pts: &pts,
        closed: true,
        allowed: Some(&[0]),
        through: false,
        seed: None,
        end_keys: [None, None],
    };
    let mut loc = |uv: DVec2| s1.locate(fg, uv);
    let out = eval::clip::clip(fg, &input, &s1.curves, &mut loc);
    for c in &out.curves {
        println!("curve {:?} pts {:?} par {:?}", c.owner, c.pts, c.par);
    }
    for e in &out.events {
        println!("event {:?} uv {:?} inc {:?}", e.key, e.uv, e.inc);
    }
}

#[test]
fn diag_tt_keys() {
    let Ok(_) = std::env::var("TTKEYS") else { return };
    use newtopo_core::doc::{PatchId, Spacing};
    let k = Arc::new(MockKernel::new());
    let id = k.add_body(MockBody::trimmed_torus(4.0, 1.0));
    let kd: Arc<dyn CadKernel> = k.clone();
    let ctx = BodyCtx::build(kd, id).unwrap();
    let doc = Document::default();
    let fg = ctx.face(newtopo_kernel::FaceId(0));
    let s1 = eval::face::stage1(&ctx, BodyId(0), fg, &[]);
    let mut lines = Vec::new();
    let r = newtopo_core::doc::Rule::Grid { patch: PatchId::whole(BodyId(0), fg.id), count_u: 8, count_v: 8, start_on_seam: false, spacing: Spacing::ArcLength };
    eval::lines::rule_lines(&ctx, &doc, BodyId(0), fg, &s1.patches, newtopo_core::keys::RuleId(1), &r, &Default::default(), &mut lines);
    let (arr, gl) = eval::face::stage3(&ctx, fg, &s1, &lines);
    let mut seen: std::collections::HashMap<newtopo_core::keys::VertexKey, usize> = Default::default();
    for (i, g) in gl.gnodes.iter().enumerate() {
        for kk in &g.keys {
            if let Some(o) = seen.insert(kk.clone(), i) {
                println!("DUP key {:?} in gnodes {} and {}: {:?} / {:?}", kk, o, i, gl.gnodes[o].uv, g.uv);
            }
        }
    }
    for (i, g) in gl.gnodes.iter().enumerate() {
        if g.keys.len() > 1 { println!("multi {i} {:?} {:?}", g.uv, g.keys); }
    }
    println!("nodes {} gnodes {}", arr.nodes.len(), gl.gnodes.len());
}
