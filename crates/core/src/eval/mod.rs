//! Rule evaluation → arrangement → output mesh (SPEC §4.6). OWNER: engine.
//!
//! Pipeline per body:
//! 1. per face: patch arrangement of boundary curves + split lines
//!    ([`face::stage1`]);
//! 2. rules → lines per face ([`lines`]), clipped to patches ([`clip`]);
//! 3. per face: final arrangement with crossings, glued across seams with
//!    the topology guard ([`face::stage3`], [`arr`]);
//! 4. global assembly: vertices shared along CAD edges, seam-only vertices
//!    dropped, placeholders on short closed loops ([`assemble`]);
//! 5. edits ([`hmesh`]); 6. bridging; 7. auto-fill ([`bridge`]);
//! 8. emission ([`emit`]) and validation ([`validate`]).

pub mod arr;
pub mod assemble;
pub mod bridge;
pub mod clip;
pub mod ctx;
pub mod emit;
pub mod face;
pub mod geom2;
pub mod hmesh;
pub mod lines;
pub mod validate;

use crate::doc::{Document, Edit, Rule, SliceScope};
use crate::keys::{EdgeKey, LineKey, RuleId, SplitId, VertexKey};
use crate::mesh::{Anchor, OutputMesh};
use assemble::{FaceRes, HMesh};
pub use ctx::BodyCtx;
use glam::{DVec2, DVec3};
use lines::{BodyPlans, GLine, Patch};
use newtopo_kernel::{BodyId, EdgeId, FaceId};
use std::collections::{HashMap, HashSet};
pub use validate::{Issue, IssueKind, validate, validate_anchors};

/// What a curve / chain / half-edge lies on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Owner {
    Cad(EdgeId),
    /// Seam of the face (0 = u-seam, 1 = v-seam).
    Seam(u8),
    /// Degenerate (pole/apex) edge.
    Pole(EdgeId),
    Split(SplitId),
    Line(LineKey),
    Fill,
}

/// Result of evaluating one body.
#[derive(Clone, Debug, Default)]
pub struct BodyEval {
    pub mesh: OutputMesh,
    /// Edit ids whose keys don't exist (or that couldn't apply).
    pub dormant: Vec<u32>,
    /// Edit ids that apply, with a reason for each dormant one.
    pub dormant_reasons: Vec<(u32, String)>,
    /// Patches per face.
    pub patches: Vec<Patch>,
    /// Evaluator-internal problems (bridging failures etc.).
    pub internal: Vec<Issue>,
    /// Number of generated lines per rule (pieces).
    pub lines_per_rule: HashMap<RuleId, u32>,
}

/// Faces touched by a rule (for dirty tracking and patch queries).
pub fn rule_faces(r: &Rule, body: BodyId, ctx: &BodyCtx) -> Vec<FaceId> {
    let mut v: Vec<FaceId> = match r {
        Rule::Connect { patches, .. } => patches.iter().filter(|p| p.body == body).map(|p| p.face).collect(),
        Rule::Grid { patch, .. } => {
            if patch.body == body {
                vec![patch.face]
            } else {
                vec![]
            }
        }
        Rule::Slice { scope, .. } => lines::slice_faces(ctx, body, scope),
        Rule::Iso { pieces, .. } => pieces.iter().filter(|p| p.body == body).map(|p| p.face).collect(),
        Rule::Cut { points, .. } => points
            .iter()
            .flat_map(|p| match p {
                crate::doc::CutPoint::Face { body: b, face, .. } if *b == body => vec![*face],
                crate::doc::CutPoint::CadEdge { body: b, edge, .. } if *b == body => ctx.topo.edge(*edge).faces.clone(),
                crate::doc::CutPoint::Vertex { body: b, .. } if *b == body => ctx.topo.face_ids().collect(),
                _ => vec![],
            })
            .collect(),
        Rule::Join { body: b, .. } => {
            if *b == body {
                ctx.topo.face_ids().collect()
            } else {
                vec![]
            }
        }
    };
    v.sort();
    v.dedup();
    v
}

/// Patches of every face of a body (stage 1 only).
pub fn body_patches(ctx: &BodyCtx, doc: &Document, body: BodyId) -> Vec<Patch> {
    let mut out = Vec::new();
    for f in ctx.topo.face_ids() {
        let splits: Vec<&crate::doc::SplitLine> = doc.split_lines.iter().filter(|s| s.body == body && s.face == f).collect();
        let s1 = face::stage1(ctx, body, ctx.face(f), &splits);
        out.extend(s1.patches);
    }
    out
}

fn has_spread_rule(doc: &Document, p: &Patch, body: BodyId) -> bool {
    doc.rules.iter().any(|r| match &r.rule {
        Rule::Connect { patches, .. } => patches.contains(&p.id),
        Rule::Grid { patch, .. } => *patch == p.id,
        Rule::Slice { scope: SliceScope::Body(b), .. } => *b == body,
        Rule::Slice { scope: SliceScope::Patches(list), .. } => list.contains(&p.id),
        _ => false,
    })
}

/// Evaluate one body.
pub fn evaluate(ctx: &BodyCtx, doc: &Document, body: BodyId) -> BodyEval {
    let mut ev = BodyEval::default();
    let nf = ctx.topo.faces.len();
    // 1. Patches.
    let mut s1s = Vec::with_capacity(nf);
    for f in ctx.topo.face_ids() {
        let splits: Vec<&crate::doc::SplitLine> = doc.split_lines.iter().filter(|s| s.body == body && s.face == f).collect();
        s1s.push(face::stage1(ctx, body, ctx.face(f), &splits));
    }
    for s in &s1s {
        ev.patches.extend(s.patches.iter().cloned());
    }
    // 2. Plans.
    let mut plans = BodyPlans::default();
    let mut rules: Vec<&crate::doc::RuleEntry> = doc.rules.iter().collect();
    rules.sort_by_key(|r| (r.ordinal, r.id));
    for r in &rules {
        if let Rule::Slice { scope, count, orient, frozen_planes, .. } = &r.rule {
            let faces = lines::slice_faces(ctx, body, scope);
            if faces.is_empty() {
                continue;
            }
            let planes = match frozen_planes {
                Some(p) => p.clone(),
                None => lines::compute_slice_planes(ctx, &faces, *count, orient),
            };
            plans.slice_planes.insert(r.id, planes);
        }
    }
    let needs_known = rules.iter().any(|r| match &r.rule {
        Rule::Cut { points, .. } => points.iter().any(|p| matches!(p, crate::doc::CutPoint::Vertex { key, body: b } if *b == body && !matches!(key, VertexKey::Cad(_)))),
        Rule::Join { body: b, .. } => *b == body,
        _ => false,
    });
    let gen_lines = |plans: &BodyPlans, with_cuts: bool| -> Vec<Vec<GLine>> {
        let mut per_face: Vec<Vec<GLine>> = vec![vec![]; nf];
        for f in ctx.topo.face_ids() {
            let fg = ctx.face(f);
            let s1 = &s1s[f.idx()];
            let out = &mut per_face[f.idx()];
            for r in &rules {
                if !with_cuts && matches!(r.rule, Rule::Cut { .. } | Rule::Join { .. }) {
                    continue;
                }
                lines::rule_lines(ctx, doc, body, fg, &s1.patches, r.id, &r.rule, plans, out);
            }
            for p in &s1.patches {
                if p.closed_patch(fg) && !has_spread_rule(doc, p, body) {
                    lines::implicit_lines(ctx, doc, body, fg, p, out);
                }
            }
        }
        per_face
    };
    let mut known: HashMap<VertexKey, Vec<(FaceId, DVec2)>> = HashMap::new();
    if needs_known {
        let pass1 = gen_lines(&plans, false);
        for f in ctx.topo.face_ids() {
            let (_, glued) = face::stage3(ctx, ctx.face(f), &s1s[f.idx()], &pass1[f.idx()]);
            for g in &glued.gnodes {
                for k in &g.keys {
                    known.entry(k.clone()).or_default().push((f, g.uv));
                }
            }
        }
    }
    for r in &rules {
        let pts: Option<(Vec<crate::doc::CutPoint>, bool)> = match &r.rule {
            Rule::Cut { points, closed } => Some((points.clone(), *closed)),
            Rule::Join { body: b, a, b: bb } if *b == body => Some((
                vec![crate::doc::CutPoint::Vertex { body, key: a.clone() }, crate::doc::CutPoint::Vertex { body, key: bb.clone() }],
                false,
            )),
            _ => None,
        };
        let Some((pts, closed)) = pts else { continue };
        let pts: Vec<crate::doc::CutPoint> = pts
            .into_iter()
            .filter(|p| match p {
                crate::doc::CutPoint::Face { body: b, .. } | crate::doc::CutPoint::CadEdge { body: b, .. } | crate::doc::CutPoint::Vertex { body: b, .. } => *b == body,
            })
            .collect();
        if pts.len() < 2 {
            continue;
        }
        match lines::plan_cut(ctx, r.id, &pts, closed, &known) {
            Some(segs) => {
                plans.cut_segments.insert(r.id, segs);
            }
            None => plans.unresolved.push(r.id),
        }
    }
    let per_face = gen_lines(&plans, true);
    for lines in &per_face {
        for l in lines {
            if let clip::Who::Line(k) = l.who {
                *ev.lines_per_rule.entry(k.rule).or_default() += 1;
            }
        }
    }
    // 3. Final arrangements.
    let mut faces: Vec<FaceRes> = Vec::with_capacity(nf);
    for f in ctx.topo.face_ids() {
        let (arr, glued) = face::stage3(ctx, ctx.face(f), &s1s[f.idx()], &per_face[f.idx()]);
        faces.push(FaceRes { face: f, arr, glued });
    }
    // 4. Assembly.
    let mut mesh = assemble::assemble(ctx, doc, body, &faces);
    let pole_target = pole_targets(ctx, &mesh);
    // 5. Edits.
    let mut hard: HashMap<EdgeKey, bool> = HashMap::new();
    for ed in doc.edits.iter().filter(|e| e.edit.body() == body) {
        let res = apply_edit(ctx, &mut mesh, &pole_target, &ed.edit, &mut hard);
        if let Err(reason) = res {
            ev.dormant.push(ed.id);
            ev.dormant_reasons.push((ed.id, reason));
        }
    }
    // 6. Bridging.
    let failed = bridge::bridge_all(ctx, &mut mesh, &pole_target);
    for r in failed {
        ev.internal.push(Issue {
            kind: IssueKind::NotSimple,
            message: format!("could not make polygon region {r} on face {} simple", mesh.regions[r].face),
            polygon: None,
            vertices: vec![],
        });
    }
    // 7. Auto-fill.
    let fan = if doc.settings.autofill { bridge::autofill_all(ctx, &mut mesh, &pole_target) } else { HashSet::new() };
    // 8. Emit.
    ev.mesh = emit::emit(ctx, body, &mesh, &pole_target, &hard, doc.settings.sharp_angle_deg, &fan);
    ev
}

/// Pole edge → the vertex all of its pole-line vertices collapse to.
fn pole_targets(ctx: &BodyCtx, mesh: &HMesh) -> HashMap<EdgeId, usize> {
    let mut out = HashMap::new();
    for (i, v) in mesh.verts.iter().enumerate() {
        let Some(e) = v.pole else { continue };
        if !v.alive {
            continue;
        }
        let apex = ctx.topo.edge(e).v0;
        let is_apex = matches!(v.anchor, Anchor::CadVertex { vertex } if Some(vertex) == apex);
        if is_apex || !out.contains_key(&e) {
            if is_apex {
                out.insert(e, i);
            } else {
                out.entry(e).or_insert(i);
            }
        }
    }
    out
}

/// Anchor priority used by collapse (SPEC §4.4).
fn priority(a: &Anchor) -> u8 {
    a.priority()
}

fn apply_edit(ctx: &BodyCtx, mesh: &mut HMesh, pt: &HashMap<EdgeId, usize>, edit: &Edit, hard: &mut HashMap<EdgeKey, bool>) -> Result<(), String> {
    match edit {
        Edit::RemoveEdge { edge, .. } => {
            let h = mesh.find_edge(edge, pt).ok_or("edge no longer exists")?;
            mesh.remove_edge(h)
        }
        Edit::RemoveCadEdge { edge, .. } => {
            let hs: Vec<usize> = mesh.alive_he().filter(|&h| mesh.he[h].owner == Owner::Cad(*edge)).collect();
            if hs.is_empty() {
                return Err("CAD edge has no output edges".into());
            }
            let mut done = HashSet::new();
            for h in hs {
                if done.contains(&h) || !mesh.he[h].alive {
                    continue;
                }
                if let Some(t) = mesh.he[h].twin {
                    done.insert(t);
                }
                mesh.remove_edge(h)?;
            }
            // Vertices left with two edges on a straight-enough run are dropped
            // by keeping them (they stay valid polygon vertices); see notes.
            Ok(())
        }
        Edit::Collapse { edge, .. } => {
            let h = mesh.find_edge(edge, pt).ok_or("edge no longer exists")?;
            let (a, b) = (mesh.he[h].from, mesh.he[h].to);
            if mesh.verts[a].pole.is_some() || mesh.verts[b].pole.is_some() {
                return Err("can't collapse into a pole".into());
            }
            let (pa, pb) = (priority(&mesh.verts[a].anchor), priority(&mesh.verts[b].anchor));
            let (keep_to, anchor) = if pa > pb {
                (false, None)
            } else if pb > pa {
                (true, None)
            } else {
                // Tie: the surface-path midpoint (Face), or the lower key.
                let keep_to = mesh.verts[b].key < mesh.verts[a].key;
                let he = &mesh.he[h];
                let mid = match (mesh.verts[a].anchor, mesh.verts[b].anchor) {
                    (Anchor::Face { face, .. }, Anchor::Face { .. }) if !he.uv.is_empty() && he.face == face => {
                        let uv = he.uv[he.uv.len() / 2];
                        let uv = if he.uv.len() == 2 { (he.uv[0] + he.uv[1]) * 0.5 } else { uv };
                        let fg = ctx.face(face);
                        let uv = fg.wrap(uv);
                        Some((Anchor::Face { face, uv }, ctx.point(face, uv)))
                    }
                    (Anchor::CadEdge { edge: e1, t: t1 }, Anchor::CadEdge { edge: e2, t: t2 }) if e1 == e2 => {
                        let t = 0.5 * (t1 + t2);
                        Some((Anchor::CadEdge { edge: e1, t }, ctx.edge_point(e1, t)))
                    }
                    _ => None,
                };
                (keep_to, mid)
            };
            mesh.collapse(h, keep_to, anchor).map(|_| ())
        }
        Edit::Weld { from, to, .. } => {
            let s = mesh.vertex_of(from).ok_or("source vertex no longer exists")?;
            let t = mesh.vertex_of(to).ok_or("target vertex no longer exists")?;
            if mesh.verts[s].pole.is_some() || mesh.verts[t].pole.is_some() {
                return Err("can't weld pole vertices".into());
            }
            mesh.weld(s, t).map(|_| ())
        }
        Edit::SetHard { edge, hard: h, .. } => {
            if mesh.find_edge(edge, pt).is_none() {
                return Err("edge no longer exists".into());
            }
            hard.insert(edge.clone(), *h);
            Ok(())
        }
    }
}

/// Default length tolerance for validation of a body.
pub fn tolerance(ctx: &BodyCtx) -> f64 {
    1e-6 * ctx.size
}

/// Full validation of an evaluated body (topology + anchors + internal).
pub fn validate_body(ctx: &BodyCtx, ev: &BodyEval) -> Vec<Issue> {
    let tol = tolerance(ctx);
    let mut v = validate(&ev.mesh, &ctx.topo, tol);
    v.extend(validate_anchors(&ev.mesh, ctx, tol * 10.0));
    v.extend(ev.internal.iter().cloned());
    v
}

/// Surface point of an anchor.
pub fn anchor_point(ctx: &BodyCtx, a: &Anchor) -> DVec3 {
    match *a {
        Anchor::CadVertex { vertex } => ctx.topo.vertex(vertex).point,
        Anchor::CadEdge { edge, t } => ctx.edge_point(edge, t),
        Anchor::Face { face, uv } => ctx.point(face, uv),
    }
}
