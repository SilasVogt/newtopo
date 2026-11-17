//! Tool operations (SPEC §6): an [`Op`] plus the current document → a new
//! document, or a [`Refusal`] with a reason. Spread-rule ownership and
//! takeover (§4.3), split-line count distribution (§4.2), edit confirmation
//! rules (§6.8–§6.10).

use crate::doc::*;
use crate::eval::lines::{Bend, Patch, bend_class, compute_slice_planes, slice_faces};
use crate::eval::{self, BodyCtx};
use crate::keys::{EdgeKey, RuleId, VertexKey};
use crate::mesh::Anchor;
use crate::ops::{Op, OpOutcome, PatchSel, Refusal};
use crate::scene::{Scene, rule_owns};
use glam::{DAffine3, DVec2, DVec3};
use newtopo_kernel::{BodyId, EdgeId, FaceId};

type R<T> = Result<T, Refusal>;

fn next_ordinal(doc: &Document) -> u32 {
    doc.rules
        .iter()
        .map(|r| r.ordinal)
        .max()
        .map(|m| m + 1)
        .unwrap_or(1)
}

fn add_rule(doc: &mut Document, rule: Rule) -> RuleId {
    let id = RuleId(doc.fresh_id());
    let ordinal = next_ordinal(doc);
    doc.rules.push(RuleEntry {
        id,
        ordinal,
        rule,
        link_group: None,
    });
    id
}

fn push_edit(doc: &mut Document, edit: Edit) -> u32 {
    let id = doc.fresh_id();
    doc.edits.push(EditEntry { id, edit });
    id
}

/// Largest-remainder rounding of `total` split by `weights`, at least 1
/// per piece when `total >= 1` (SPEC §4.2).
pub fn distribute(total: u32, weights: &[f64]) -> Vec<u32> {
    let n = weights.len();
    if n == 0 {
        return vec![];
    }
    let sum: f64 = weights.iter().sum::<f64>().max(1e-300);
    let exact: Vec<f64> = weights.iter().map(|w| total as f64 * w / sum).collect();
    let mut out: Vec<u32> = exact.iter().map(|e| e.floor() as u32).collect();
    let mut rem = total.saturating_sub(out.iter().sum());
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        (exact[b] - exact[b].floor())
            .total_cmp(&(exact[a] - exact[a].floor()))
            .then(a.cmp(&b))
    });
    for &i in &order {
        if rem == 0 {
            break;
        }
        out[i] += 1;
        rem -= 1;
    }
    if total >= 1 {
        for i in 0..n {
            if out[i] == 0 {
                out[i] = 1;
                // Take one from the largest piece if possible.
                if let Some(j) = (0..n).filter(|&j| out[j] > 1).max_by_key(|&j| out[j])
                    && out.iter().sum::<u32>() > total
                {
                    out[j] -= 1;
                }
            }
        }
    }
    out
}

impl Scene {
    fn body_or_first(&self, b: Option<BodyId>) -> R<BodyId> {
        match b {
            Some(b) => {
                if self.doc().body(b).is_some() {
                    Ok(b)
                } else {
                    Err(Refusal::new(format!("there is no body {b}")))
                }
            }
            None => self
                .doc()
                .bodies
                .first()
                .map(|b| b.id)
                .ok_or_else(|| Refusal::new("the scene has no bodies")),
        }
    }

    fn ctx_of(&self, b: BodyId) -> R<&BodyCtx> {
        self.ctx(b)
            .ok_or_else(|| Refusal::new(format!("body {b} has no CAD data")))
    }

    fn patches_of_doc(&self, doc: &Document, body: BodyId) -> Vec<Patch> {
        self.patches_cached(doc, body)
    }

    fn resolve_patches(&mut self, sel: &PatchSel) -> R<(BodyId, Vec<PatchId>)> {
        let body = match sel.body {
            Some(b) => self.body_or_first(Some(b))?,
            None => match sel.patches.first() {
                Some(p) => p.body,
                None => self.body_or_first(None)?,
            },
        };
        let all = self.patch_records(body);
        let mut out: Vec<PatchId> = Vec::new();
        for f in &sel.faces {
            let fs: Vec<PatchId> = all
                .iter()
                .filter(|p| p.face.0 == *f)
                .map(|p| p.id)
                .collect();
            if fs.is_empty() {
                return Err(Refusal::new(format!(
                    "face {f} doesn't exist on body {body}"
                )));
            }
            out.extend(fs);
        }
        for p in &sel.patches {
            if !all.iter().any(|q| q.id == *p) {
                return Err(Refusal::new(
                    "a selected patch no longer exists (split lines changed); reselect it",
                ));
            }
            out.push(*p);
        }
        out.sort();
        out.dedup();
        if out.is_empty() {
            return Err(Refusal::new("select one or more faces first"));
        }
        Ok((body, out))
    }

    /// Remove `patches` from every spread rule (takeover, SPEC §4.3). Body
    /// slices losing a patch are first converted to patch scope with frozen
    /// planes.
    pub(crate) fn takeover(&self, doc: &mut Document, body: BodyId, patches: &[PatchId]) {
        let all = self.patches_of_doc(doc, body);
        let mut i = 0;
        while i < doc.rules.len() {
            let id = doc.rules[i].id;
            let mut delete = false;
            match &mut doc.rules[i].rule {
                Rule::Connect {
                    patches: list,
                    flipped,
                    ..
                } => {
                    list.retain(|p| !patches.contains(p));
                    flipped.retain(|p| !patches.contains(p));
                    delete = list.is_empty();
                }
                Rule::Grid { patch, .. } => delete = patches.contains(patch),
                Rule::Slice {
                    scope,
                    count,
                    orient,
                    frozen_planes,
                    ..
                } => {
                    if let SliceScope::Body(b) = scope
                        && *b == body
                        && !patches.is_empty()
                    {
                        let planes = match frozen_planes {
                            Some(p) => p.clone(),
                            None => match self.ctx(body) {
                                Some(c) => compute_slice_planes(
                                    c,
                                    &slice_faces(c, body, &SliceScope::Body(body)),
                                    *count,
                                    orient,
                                ),
                                None => vec![],
                            },
                        };
                        *frozen_planes = Some(planes);
                        *scope = SliceScope::Patches(all.iter().map(|p| p.id).collect());
                    }
                    if let SliceScope::Patches(list) = scope {
                        list.retain(|p| !patches.contains(p));
                        delete = list.is_empty();
                    }
                }
                _ => {}
            }
            let _ = id;
            if delete {
                doc.rules.remove(i);
            } else {
                i += 1;
            }
        }
    }

    /// Check an edit candidate: the edit must apply (not go dormant). The
    /// result's validity is checked by [`Scene::apply`] for every op.
    fn check_edit(&mut self, doc: &Document, body: BodyId, edit_id: u32) -> R<()> {
        let ev = self
            .eval_doc(doc, body)
            .ok_or_else(|| Refusal::new("body has no CAD data"))?;
        if let Some((_, reason)) = ev.dormant_reasons.iter().find(|(id, _)| *id == edit_id) {
            return Err(Refusal::new(format!("can't do that: {reason}")));
        }
        self.stash_eval(doc, body, ev);
        Ok(())
    }

    fn find_out_edge(
        &mut self,
        body: BodyId,
        key: &EdgeKey,
    ) -> R<(VertexKey, VertexKey, Anchor, Anchor)> {
        let m = self
            .output_of(body)
            .ok_or_else(|| Refusal::new("body has no output"))?;
        let e = m
            .edges
            .iter()
            .find(|e| e.key.as_ref() == Some(key))
            .ok_or_else(|| Refusal::new("that edge no longer exists"))?;
        let (a, b) = (&m.vertices[e.a as usize], &m.vertices[e.b as usize]);
        Ok((a.key.clone(), b.key.clone(), a.anchor, b.anchor))
    }

    fn find_out_vertex(&mut self, body: BodyId, key: &VertexKey) -> R<Anchor> {
        let m = self
            .output_of(body)
            .ok_or_else(|| Refusal::new("body has no output"))?;
        m.vertices
            .iter()
            .find(|v| &v.key == key)
            .map(|v| v.anchor)
            .ok_or_else(|| Refusal::new("that vertex no longer exists"))
    }

    /// Link / symmetry expansion of a patch list (SPEC §6.11, §6.12):
    /// whole-face patches of partner faces are added, and the partners'
    /// phase references are set from the source through the mapping.
    fn expand_partners(
        &mut self,
        doc: &mut Document,
        body: BodyId,
        list: Vec<PatchId>,
    ) -> Vec<PatchId> {
        if !doc.settings.link && !doc.settings.symmetry {
            return list;
        }
        let records = self.patch_records(body);
        let mut out = list.clone();
        let mut map = crate::link::FaceMap::new();
        for p in &list {
            if *p != PatchId::whole(body, p.face) {
                continue;
            }
            for (g, t) in self.tool_partners(body, p.face) {
                let w = PatchId::whole(body, g);
                if records.iter().any(|r| r.id == w) && !out.contains(&w) {
                    out.push(w);
                    map.insert(p.face, (g, t));
                    if let Some(c) = self.ctx(body) {
                        let mut one = crate::link::FaceMap::new();
                        one.insert(p.face, (g, t));
                        crate::link::map_phases(doc, body, c, body, c, &one);
                    }
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// Link / symmetry copies of iso pieces.
    fn expand_pieces(
        &mut self,
        doc: &Document,
        body: BodyId,
        pieces: Vec<IsoPiece>,
    ) -> Vec<IsoPiece> {
        if !doc.settings.link && !doc.settings.symmetry {
            return pieces;
        }
        let mut out = pieces.clone();
        for p in &pieces {
            for (g, t) in self.tool_partners(body, p.face) {
                let Some(c) = self.ctx(body) else { continue };
                if let Some((dir, value, seed)) =
                    crate::link::map_iso(c, p.face, p.dir, p.value, p.seed, c, g, &t)
                    && !out
                        .iter()
                        .any(|q| q.face == g && q.dir == dir && (q.value - value).abs() < 1e-9)
                {
                    out.push(IsoPiece {
                        body,
                        face: g,
                        dir,
                        value,
                        seed,
                    });
                }
            }
        }
        out
    }

    /// Distinct transforms taking `faces` onto partner features under Link
    /// and symmetry (SPEC §6.11, §6.12).
    fn partner_transforms(&mut self, body: BodyId, faces: &[FaceId]) -> Vec<DAffine3> {
        if !self.doc().settings.link && !self.doc().settings.symmetry {
            return vec![];
        }
        let size = self.ctx(body).map_or(1.0, |c| c.size);
        let mut out: Vec<DAffine3> = Vec::new();
        for f in faces {
            for (_, t) in self.tool_partners(body, *f) {
                if !out.iter().any(|u| same_transform(u, &t, size)) {
                    out.push(t);
                }
            }
        }
        out
    }

    /// Link / symmetry copies of a cut (SPEC §6.11, §6.12): one mapped cut
    /// per partner transform. A face of the cut with no partner under a
    /// transform maps to itself when the transformed points stay on it (a
    /// cut from a hole wall onto the plate around it). Returns the mapped
    /// cuts and the number of partners that could not be mapped.
    fn partner_cuts(&mut self, body: BodyId, rule: &Rule) -> (Vec<Rule>, usize) {
        let Rule::Cut { points, closed } = rule else {
            return (vec![], 0);
        };
        if self.ctx(body).is_none() {
            return (vec![], 0);
        }
        // Vertex anchors (snapped generated or CAD vertices) resolve through
        // the current output: their position and the faces around them.
        let mesh = if points.iter().any(|p| matches!(p, CutPoint::Vertex { .. })) {
            match self.output_of(body) {
                Some(m) => Some(m.clone()),
                None => return (vec![], 0),
            }
        } else {
            None
        };
        let c = self.ctx(body).expect("checked above");
        let info: Option<Vec<(DVec3, Vec<FaceId>)>> = points
            .iter()
            .map(|p| match p {
                CutPoint::Face { face, uv, .. } => Some((c.point(*face, *uv), vec![*face])),
                CutPoint::CadEdge { edge, t, .. } => {
                    Some((c.edge_point(*edge, *t), c.topo.edge(*edge).faces.clone()))
                }
                CutPoint::Vertex { key, .. } => {
                    let m = mesh.as_ref()?;
                    let i = m.vertices.iter().position(|v| &v.key == key)?;
                    Some((m.vertices[i].position, faces_around(m, &[i as u32])))
                }
            })
            .collect();
        let Some(info) = info else {
            return (vec![], 0);
        };
        let mut faces: Vec<FaceId> = info.iter().flat_map(|(_, f)| f.iter().copied()).collect();
        faces.sort();
        faces.dedup();
        let transforms = self.partner_transforms(body, &faces);
        let partners: Vec<(FaceId, Vec<(FaceId, DAffine3)>)> = faces
            .iter()
            .map(|f| (*f, self.tool_partners(body, *f)))
            .collect();
        let c = self.ctx(body).expect("checked above");
        let tol = 1e-4 * c.size.max(1e-9);
        let original: Vec<DVec3> = info.iter().map(|(p, _)| *p).collect();
        let mut out: Vec<(Rule, Vec<DVec3>)> = Vec::new();
        let mut skipped = 0;
        for t in transforms {
            let mut map = crate::link::FaceMap::new();
            for (f, ps) in &partners {
                if let Some((g, _)) = ps.iter().find(|(_, u)| same_transform(u, &t, c.size)) {
                    map.insert(*f, (*g, t));
                    continue;
                }
                // No partner under `t`: the face maps to itself when the
                // cut's transformed points on it stay on it.
                let q: Vec<DVec3> = info
                    .iter()
                    .filter(|(_, fs)| fs.contains(f))
                    .map(|(p, _)| t.transform_point3(*p))
                    .collect();
                let stays = c.kernel.project_to_face(c.kbody, *f, &q).is_ok_and(|v| {
                    v.iter()
                        .zip(&q)
                        .all(|(r, p)| r.is_some_and(|(_, x)| (x - *p).length() < tol))
                });
                if stays {
                    map.insert(*f, (*f, t));
                }
            }
            let mapped: Option<Vec<(CutPoint, DVec3)>> = points
                .iter()
                .zip(&info)
                .map(|(p, (pos, _))| match p {
                    CutPoint::Vertex { .. } => {
                        let m = mesh.as_ref()?;
                        let j = nearest_vertex(m, t.transform_point3(*pos), tol)? as usize;
                        Some((
                            CutPoint::Vertex {
                                body,
                                key: m.vertices[j].key.clone(),
                            },
                            m.vertices[j].position,
                        ))
                    }
                    _ => {
                        let one = Rule::Cut {
                            points: vec![p.clone()],
                            closed: false,
                        };
                        let Rule::Cut { points: q, .. } =
                            crate::link::map_rule(&one, body, c, body, c, &map)?
                        else {
                            return None;
                        };
                        let q = q.into_iter().next()?;
                        let at = match &q {
                            CutPoint::Face { face, uv, .. } => c.point(*face, *uv),
                            CutPoint::CadEdge { edge, t, .. } => c.edge_point(*edge, *t),
                            CutPoint::Vertex { .. } => return None,
                        };
                        Some((q, at))
                    }
                })
                .collect();
            let Some(mapped) = mapped else {
                skipped += 1;
                continue;
            };
            let (pts, at): (Vec<CutPoint>, Vec<DVec3>) = mapped.into_iter().unzip();
            // A transform taking the cut onto itself (or onto a copy
            // already made) adds nothing.
            if same_points(&at, &original, tol) || out.iter().any(|(_, o)| same_points(&at, o, tol))
            {
                continue;
            }
            out.push((
                Rule::Cut {
                    points: pts,
                    closed: *closed,
                },
                at,
            ));
        }
        (out.into_iter().map(|(r, _)| r).collect(), skipped)
    }

    /// Link / symmetry copies of an output edge, matched by geometry on the
    /// current output: the edge's ends and middle, transformed, must land
    /// on an output edge. Returns the partner keys and the number of
    /// partners without a matching edge.
    fn partner_edges(&mut self, body: BodyId, key: &EdgeKey) -> (Vec<EdgeKey>, usize) {
        let Some(m) = self.output_of(body) else {
            return (vec![], 0);
        };
        let Some(e) = m.edges.iter().find(|e| e.key.as_ref() == Some(key)) else {
            return (vec![], 0);
        };
        let (pa, pb) = (
            m.vertices[e.a as usize].position,
            m.vertices[e.b as usize].position,
        );
        let mid = e.path[e.path.len() / 2];
        let faces = faces_around(m, &[e.a, e.b]);
        let transforms = self.partner_transforms(body, &faces);
        let tol = 1e-4 * self.ctx(body).map_or(1.0, |c| c.size);
        let m = self.output_of(body).expect("evaluated above");
        let mut out = Vec::new();
        let mut skipped = 0;
        for t in transforms {
            match match_edge(m, &t, pa, pb, mid, tol) {
                Some(k) if k == *key || out.contains(&k) => {}
                Some(k) => out.push(k),
                None => skipped += 1,
            }
        }
        (out, skipped)
    }

    /// Link / symmetry copies of a vertex pair (for Weld), matched by
    /// geometry on the current output.
    fn partner_vertex_pairs(
        &mut self,
        body: BodyId,
        from: &VertexKey,
        to: &VertexKey,
    ) -> (Vec<(VertexKey, VertexKey)>, usize) {
        let Some(m) = self.output_of(body) else {
            return (vec![], 0);
        };
        let find = |k: &VertexKey| m.vertices.iter().position(|v| &v.key == k);
        let (Some(i), Some(j)) = (find(from), find(to)) else {
            return (vec![], 0);
        };
        let (pf, pt) = (m.vertices[i].position, m.vertices[j].position);
        let faces = faces_around(m, &[i as u32, j as u32]);
        let transforms = self.partner_transforms(body, &faces);
        let tol = 1e-4 * self.ctx(body).map_or(1.0, |c| c.size);
        let m = self.output_of(body).expect("evaluated above");
        let mut out = Vec::new();
        let mut skipped = 0;
        for t in transforms {
            let a = nearest_vertex(m, t.transform_point3(pf), tol);
            let b = nearest_vertex(m, t.transform_point3(pt), tol);
            match (a, b) {
                (Some(a), Some(b)) if a != b => {
                    let pair = (
                        m.vertices[a as usize].key.clone(),
                        m.vertices[b as usize].key.clone(),
                    );
                    let same = |x: &(VertexKey, VertexKey), f: &VertexKey, t: &VertexKey| {
                        (&x.0 == f && &x.1 == t) || (&x.0 == t && &x.1 == f)
                    };
                    if !same(&pair, from, to)
                        && !out
                            .iter()
                            .any(|o: &(VertexKey, VertexKey)| same(&pair, &o.0, &o.1))
                    {
                        out.push(pair);
                    }
                }
                _ => skipped += 1,
            }
        }
        (out, skipped)
    }

    /// Check new edits: each `required` edit must apply (not go dormant);
    /// `optional` ones (Link / symmetry copies) that go dormant are dropped.
    /// Returns how many were dropped.
    fn check_edits(
        &mut self,
        doc: &mut Document,
        body: BodyId,
        required: &[u32],
        optional: &[u32],
    ) -> R<usize> {
        let ev = self
            .eval_doc(doc, body)
            .ok_or_else(|| Refusal::new("body has no CAD data"))?;
        if let Some((_, reason)) = ev
            .dormant_reasons
            .iter()
            .find(|(id, _)| required.contains(id))
        {
            return Err(Refusal::new(format!("can't do that: {reason}")));
        }
        let drop: Vec<u32> = ev
            .dormant_reasons
            .iter()
            .map(|(id, _)| *id)
            .filter(|id| optional.contains(id))
            .collect();
        if drop.is_empty() {
            self.stash_eval(doc, body, ev);
            return Ok(0);
        }
        doc.edits.retain(|e| !drop.contains(&e.id));
        let n = drop.len();
        let rest: Vec<u32> = optional
            .iter()
            .copied()
            .filter(|i| !drop.contains(i))
            .collect();
        Ok(n + self.check_edits(doc, body, required, &rest)?)
    }

    /// Run one op against the current document, refusing results that
    /// exceed the evaluator's work limits.
    pub(crate) fn run_op(&mut self, op: &Op) -> R<(Document, OpOutcome)> {
        let (doc, out) = self.run_op_inner(op)?;
        crate::scene::work_budget(&doc).map_err(|e| Refusal::new(format!("{e:#}")))?;
        Ok((doc, out))
    }

    fn run_op_inner(&mut self, op: &Op) -> R<(Document, OpOutcome)> {
        let mut doc = self.doc().clone();
        let mut out = OpOutcome::default();
        match op {
            Op::Connect {
                patches,
                a,
                b,
                start_on_seam,
            } => {
                let (body, list) = self.resolve_patches(patches)?;
                let list = self.expand_partners(&mut doc, body, list);
                if *a == 0 && *b == 0 {
                    self.takeover(&mut doc, body, &list);
                    out.message = Some("Connect removed".into());
                    return Ok((doc, out));
                }
                let c = self.ctx_of(body)?;
                let mut curved = Vec::new();
                let mut flat = 0;
                for p in &list {
                    let bend = bend_class(c, c.face(p.face));
                    if bend == Bend::Flat {
                        flat += 1;
                    } else {
                        curved.push((*p, bend));
                    }
                }
                if curved.is_empty() {
                    return Err(Refusal::new(
                        "Connect needs a curved face: flat faces have no bend to follow. Use Surface Grid, Slice or auto-fill instead.",
                    ));
                }
                let ids: Vec<PatchId> = curved.iter().map(|x| x.0).collect();
                self.takeover(&mut doc, body, &ids);
                for (_, bend) in &curved {
                    out.lines_added += match bend {
                        Bend::Two => a + b,
                        _ => *a,
                    };
                }
                add_rule(
                    &mut doc,
                    Rule::Connect {
                        patches: ids,
                        count_a: *a,
                        count_b: *b,
                        start_on_seam: *start_on_seam,
                        flipped: vec![],
                    },
                );
                if flat > 0 {
                    out.message = Some(format!("{flat} flat face(s) skipped (no bend direction)"));
                }
            }
            Op::Grid {
                patches,
                u,
                v,
                start_on_seam,
                spacing,
            } => {
                let (body, list) = self.resolve_patches(patches)?;
                let list = self.expand_partners(&mut doc, body, list);
                self.takeover(&mut doc, body, &list);
                if *u == 0 && *v == 0 {
                    out.message = Some("Surface Grid removed".into());
                    return Ok((doc, out));
                }
                for p in &list {
                    add_rule(
                        &mut doc,
                        Rule::Grid {
                            patch: *p,
                            count_u: *u,
                            count_v: *v,
                            start_on_seam: *start_on_seam,
                            spacing: *spacing,
                        },
                    );
                    out.lines_added += u + v;
                }
            }
            Op::Slice {
                body,
                patches,
                count,
                through,
                guide_edge,
            } => {
                let (b, scope) = match (body, patches) {
                    (Some(b), _) => {
                        let b = self.body_or_first(Some(*b))?;
                        (b, SliceScope::Body(b))
                    }
                    (None, Some(sel)) => {
                        let (b, list) = self.resolve_patches(sel)?;
                        let list = self.expand_partners(&mut doc, b, list);
                        (b, SliceScope::Patches(list))
                    }
                    (None, None) => {
                        let b = self.body_or_first(None)?;
                        (b, SliceScope::Body(b))
                    }
                };
                if let Some(e) = guide_edge {
                    let c = self.ctx_of(b)?;
                    if e.idx() >= c.topo.edges.len() {
                        return Err(Refusal::new(format!("there is no CAD edge {e}")));
                    }
                }
                // Replace any slice with the same scope.
                doc.rules
                    .retain(|r| !matches!(&r.rule, Rule::Slice { scope: s, .. } if *s == scope));
                let owned: Vec<PatchId> = match &scope {
                    SliceScope::Body(_) => {
                        self.patches_of_doc(&doc, b).iter().map(|p| p.id).collect()
                    }
                    SliceScope::Patches(l) => l.clone(),
                };
                self.takeover(&mut doc, b, &owned);
                if *count == 0 {
                    out.message = Some("Slice removed".into());
                    return Ok((doc, out));
                }
                let orient = match guide_edge {
                    Some(e) => SliceOrient::GuideEdge(b, *e),
                    None => SliceOrient::Auto,
                };
                add_rule(
                    &mut doc,
                    Rule::Slice {
                        scope,
                        count: *count,
                        orient,
                        through: *through,
                        frozen_planes: None,
                    },
                );
                out.lines_added = *count;
            }
            Op::Iso {
                body,
                face,
                dir,
                value,
                seed,
                split,
                through,
            } => {
                let b = self.body_or_first(*body)?;
                let c = self.ctx_of(b)?;
                let f = FaceId(*face);
                if f.idx() >= c.topo.faces.len() {
                    return Err(Refusal::new(format!("there is no face {face}")));
                }
                if !(0.0..=1.0).contains(value) {
                    return Err(Refusal::new(
                        "iso value must be between 0 and 1 of the face's parameter range",
                    ));
                }
                let fg = c.face(f);
                let d = if *dir == IsoDir::U { 0 } else { 1 };
                let closed_dir = if d == 0 {
                    fg.uper.is_some()
                } else {
                    fg.vper.is_some()
                };
                if !closed_dir && (*value <= 1e-9 || *value >= 1.0 - 1e-9) {
                    return Err(Refusal::new("that iso line lies on the face boundary"));
                }
                let pieces = if *through {
                    iso_walk(c, b, f, *dir, *value)
                } else {
                    vec![IsoPiece {
                        body: b,
                        face: f,
                        dir: *dir,
                        value: *value,
                        seed: *seed,
                    }]
                };
                let pieces = self.expand_pieces(&doc, b, pieces);
                if *split {
                    let group = doc.fresh_id();
                    let mut faces = Vec::new();
                    for p in &pieces {
                        let id = crate::keys::SplitId(doc.fresh_id());
                        doc.split_lines.push(SplitLine {
                            id,
                            group,
                            body: b,
                            face: p.face,
                            kind: SplitKind::Iso {
                                dir: p.dir,
                                value: p.value,
                                seed: if *through { None } else { p.seed },
                            },
                        });
                        faces.push((p.face, p.dir));
                    }
                    let msg = self.distribute_after_split(&mut doc, b, &faces);
                    out.message = msg;
                } else {
                    add_rule(
                        &mut doc,
                        Rule::Iso {
                            pieces: pieces.clone(),
                            through: *through,
                        },
                    );
                    out.lines_added = pieces.len() as u32;
                }
            }
            Op::Cut { points, closed } => {
                if points.len() < 2 || (*closed && points.len() < 3) {
                    return Err(Refusal::new(
                        "a cut needs at least two points (three to close it)",
                    ));
                }
                let body = match &points[0] {
                    CutPoint::Face { body, .. }
                    | CutPoint::CadEdge { body, .. }
                    | CutPoint::Vertex { body, .. } => *body,
                };
                let c = self.ctx_of(body)?;
                for p in points {
                    match p {
                        CutPoint::Face { face, .. } if face.idx() >= c.topo.faces.len() => {
                            return Err(Refusal::new("cut point on a missing face"));
                        }
                        CutPoint::CadEdge { edge, .. } if edge.idx() >= c.topo.edges.len() => {
                            return Err(Refusal::new("cut point on a missing edge"));
                        }
                        _ => {}
                    }
                }
                let id = add_rule(
                    &mut doc,
                    Rule::Cut {
                        points: points.clone(),
                        closed: *closed,
                    },
                );
                let ev = self
                    .eval_doc(&doc, body)
                    .ok_or_else(|| Refusal::new("body has no CAD data"))?;
                if !ev.lines_per_rule.contains_key(&id) {
                    return Err(Refusal::new(
                        "consecutive cut points must lie on the same face or on faces sharing an edge",
                    ));
                }
                out.lines_added = ev.lines_per_rule[&id];
                let rule = Rule::Cut {
                    points: points.clone(),
                    closed: *closed,
                };
                let (cuts, mut skipped) = self.partner_cuts(body, &rule);
                if !cuts.is_empty() {
                    let group = doc.fresh_id();
                    let mut ids = vec![];
                    for r in cuts {
                        ids.push(add_rule(&mut doc, r));
                    }
                    for r in doc.rules.iter_mut() {
                        if r.id == id || ids.contains(&r.id) {
                            r.link_group = Some(group);
                        }
                    }
                    let ev = self
                        .eval_doc(&doc, body)
                        .ok_or_else(|| Refusal::new("body has no CAD data"))?;
                    let dead: Vec<RuleId> = ids
                        .iter()
                        .copied()
                        .filter(|i| !ev.lines_per_rule.contains_key(i))
                        .collect();
                    out.lines_added += ids
                        .iter()
                        .filter_map(|i| ev.lines_per_rule.get(i))
                        .sum::<u32>();
                    doc.rules.retain(|r| !dead.contains(&r.id));
                    skipped += dead.len();
                    out.message = partner_note(ids.len() - dead.len(), skipped);
                } else {
                    out.message = partner_note(0, skipped);
                }
            }
            Op::Join { body, a, b } => {
                let bd = self.body_or_first(*body)?;
                if a == b {
                    return Err(Refusal::new("pick two different vertices"));
                }
                self.find_out_vertex(bd, a)?;
                self.find_out_vertex(bd, b)?;
                let id = add_rule(
                    &mut doc,
                    Rule::Join {
                        body: bd,
                        a: a.clone(),
                        b: b.clone(),
                    },
                );
                let ev = self
                    .eval_doc(&doc, bd)
                    .ok_or_else(|| Refusal::new("body has no CAD data"))?;
                if !ev.lines_per_rule.contains_key(&id) {
                    return Err(Refusal::new(
                        "the two vertices must lie on the same face or on faces sharing an edge",
                    ));
                }
                out.lines_added = 1;
            }
            Op::ClearPatches { patches } => {
                let (body, list) = self.resolve_patches(patches)?;
                let list = self.expand_partners(&mut doc, body, list);
                self.takeover(&mut doc, body, &list);
            }
            Op::DeleteRule { rule } => {
                let n = doc.rules.len();
                doc.rules.retain(|r| r.id != *rule);
                if doc.rules.len() == n {
                    return Err(Refusal::new(format!("there is no rule {}", rule.0)));
                }
            }
            Op::RemoveEdge { body, edge } => {
                let b = self.body_or_first(*body)?;
                match edge {
                    EdgeKey::Cad { .. } => {
                        return Err(Refusal::new(
                            "CAD edges stay in the output; use Remove CAD edge (Delete twice) for tangent or coplanar faces",
                        ));
                    }
                    EdgeKey::Split { split, .. } => {
                        self.find_out_edge(b, edge)?;
                        let msg = self.remove_split(&mut doc, b, *split);
                        out.message = msg;
                    }
                    EdgeKey::Line { .. } => {
                        self.find_out_edge(b, edge)?;
                        let id = doc.fresh_id();
                        doc.edits.push(EditEntry {
                            id,
                            edit: Edit::RemoveEdge {
                                body: b,
                                edge: edge.clone(),
                            },
                        });
                        let (keys, skipped) = self.partner_edges(b, edge);
                        let extra: Vec<u32> = keys
                            .into_iter()
                            .map(|k| push_edit(&mut doc, Edit::RemoveEdge { body: b, edge: k }))
                            .collect();
                        let dropped = self.check_edits(&mut doc, b, &[id], &extra)?;
                        out.message = partner_note(extra.len() - dropped, skipped + dropped);
                    }
                }
            }
            Op::RemoveEdges { body, edges } => {
                let b = self.body_or_first(*body)?;
                if edges.is_empty() {
                    return Err(Refusal::new("select one or more edges first"));
                }
                let mut ids = Vec::new();
                let mut extra = Vec::new();
                let mut skipped = 0;
                let n0 = doc.edits.len();
                for edge in edges {
                    match edge {
                        EdgeKey::Cad { .. } => {
                            return Err(Refusal::new(
                                "CAD edges stay in the output; use Remove CAD edge (Delete twice) for tangent or coplanar faces",
                            ));
                        }
                        EdgeKey::Split { split, .. } => {
                            self.find_out_edge(b, edge)?;
                            self.remove_split(&mut doc, b, *split);
                        }
                        EdgeKey::Line { .. } => {
                            self.find_out_edge(b, edge)?;
                            let id = doc.fresh_id();
                            doc.edits.push(EditEntry {
                                id,
                                edit: Edit::RemoveEdge {
                                    body: b,
                                    edge: edge.clone(),
                                },
                            });
                            ids.push(id);
                            let (keys, sk) = self.partner_edges(b, edge);
                            skipped += sk;
                            for k in keys {
                                let dup = edges.contains(&k)
                                    || doc.edits[n0..].iter().any(|e| {
                                        matches!(&e.edit, Edit::RemoveEdge { edge: x, .. } if *x == k)
                                    });
                                if !dup {
                                    extra.push(push_edit(
                                        &mut doc,
                                        Edit::RemoveEdge { body: b, edge: k },
                                    ));
                                }
                            }
                        }
                    }
                }
                let dropped = self.check_edits(&mut doc, b, &ids, &extra)?;
                out.message = partner_note(extra.len() - dropped, skipped + dropped);
            }
            Op::RemoveCadEdge {
                body,
                edge,
                confirmed,
            } => {
                let b = self.body_or_first(*body)?;
                let c = self.ctx_of(b)?;
                if edge.idx() >= c.topo.edges.len() {
                    return Err(Refusal::new(format!("there is no CAD edge {edge}")));
                }
                let et = c.topo.edge(*edge);
                if et.seam || et.degenerate || et.faces.len() != 2 {
                    return Err(Refusal::new(
                        "only an edge between two faces can be removed",
                    ));
                }
                let ang = et.dihedral.unwrap_or(std::f64::consts::PI).to_degrees();
                if ang > 5.0 {
                    return Err(Refusal::new(format!(
                        "the faces meet at {ang:.0}°; only tangent (G1) or coplanar faces can be merged"
                    )));
                }
                if !confirmed {
                    return Err(Refusal::confirm(
                        "Removing a CAD edge merges the output polygons across it; the output can depart from the CAD shape. Continue?",
                    ));
                }
                let id = doc.fresh_id();
                doc.edits.push(EditEntry {
                    id,
                    edit: Edit::RemoveCadEdge {
                        body: b,
                        edge: *edge,
                    },
                });
                self.check_edit(&doc, b, id)?;
            }
            Op::Collapse {
                body,
                edge,
                confirmed,
            } => {
                let b = self.body_or_first(*body)?;
                if edge.ends().0 == edge.ends().1 {
                    return Err(Refusal::new("degenerate edge"));
                }
                if matches!(edge, EdgeKey::Cad { .. }) && !confirmed {
                    // Collapsing a CAD edge segment: both ends are CAD-anchored.
                }
                let (_, _, aa, ab) = self.find_out_edge(b, edge)?;
                if aa.priority() >= 1 && ab.priority() >= 1 && !confirmed {
                    return Err(Refusal::confirm(
                        "Both ends sit on CAD edges or vertices; collapsing changes the CAD outline in the output (output only). Continue?",
                    ));
                }
                let id = doc.fresh_id();
                doc.edits.push(EditEntry {
                    id,
                    edit: Edit::Collapse {
                        body: b,
                        edge: edge.clone(),
                    },
                });
                let (keys, skipped) = self.partner_edges(b, edge);
                let extra: Vec<u32> = keys
                    .into_iter()
                    .map(|k| push_edit(&mut doc, Edit::Collapse { body: b, edge: k }))
                    .collect();
                let dropped = self.check_edits(&mut doc, b, &[id], &extra)?;
                out.message = partner_note(extra.len() - dropped, skipped + dropped);
            }
            Op::Weld {
                body,
                from,
                to,
                confirmed,
            } => {
                let b = self.body_or_first(*body)?;
                if from == to {
                    return Err(Refusal::new("pick two different vertices"));
                }
                let af = self.find_out_vertex(b, from)?;
                let at = self.find_out_vertex(b, to)?;
                if !confirmed
                    && (af.priority() > at.priority() || (af.priority() >= 1 && at.priority() >= 1))
                {
                    return Err(Refusal::confirm(
                        "This pulls a vertex off a CAD edge or corner, so the output departs from the CAD outline. Continue?",
                    ));
                }
                let id = doc.fresh_id();
                doc.edits.push(EditEntry {
                    id,
                    edit: Edit::Weld {
                        body: b,
                        from: from.clone(),
                        to: to.clone(),
                    },
                });
                let (pairs, skipped) = self.partner_vertex_pairs(b, from, to);
                let extra: Vec<u32> = pairs
                    .into_iter()
                    .map(|(f, t)| {
                        push_edit(
                            &mut doc,
                            Edit::Weld {
                                body: b,
                                from: f,
                                to: t,
                            },
                        )
                    })
                    .collect();
                let dropped = self.check_edits(&mut doc, b, &[id], &extra)?;
                out.message = partner_note(extra.len() - dropped, skipped + dropped);
            }
            Op::SetHard { body, edge, hard } => {
                let b = self.body_or_first(*body)?;
                self.find_out_edge(b, edge)?;
                doc.edits.retain(|e| !matches!(&e.edit, Edit::SetHard { body: bb, edge: ee, .. } if *bb == b && ee == edge));
                let id = doc.fresh_id();
                doc.edits.push(EditEntry {
                    id,
                    edit: Edit::SetHard {
                        body: b,
                        edge: edge.clone(),
                        hard: *hard,
                    },
                });
            }
            Op::PurgeDormant => {
                let dormant = self.dormant_edits();
                if dormant.is_empty() {
                    out.message = Some("no dormant edits".into());
                }
                doc.edits.retain(|e| !dormant.contains(&e.id));
                out.message = Some(format!("{} dormant edit(s) purged", dormant.len()));
            }
            Op::SetAutofill { on } => doc.settings.autofill = *on,
            Op::SetLink { on } => doc.settings.link = *on,
            Op::SetSymmetry { on, axes } => {
                doc.settings.symmetry = *on;
                doc.settings.symmetry_axes = axes.clone();
            }
            Op::SetUnits { units } => {
                let units = units.trim().to_ascii_lowercase();
                if crate::export::unit_metres(&units).is_none() {
                    return Err(Refusal::new(format!(
                        "unknown unit `{units}` (use mm, cm, m or in)"
                    )));
                }
                if units != doc.settings.units && !doc.bodies.is_empty() {
                    return Err(Refusal::new(
                        "the scene's units can't change once it holds objects: their geometry is stored in those units",
                    ));
                }
                doc.settings.units = units;
            }
            Op::RenameBody { body, name } => {
                let e = doc
                    .body_mut(*body)
                    .ok_or_else(|| Refusal::new(format!("there is no body {body}")))?;
                if name.trim().is_empty() {
                    return Err(Refusal::new("names can't be empty"));
                }
                e.name = name.clone();
            }
            Op::SetBodyLocked { body, locked } => {
                let e = doc
                    .body_mut(*body)
                    .ok_or_else(|| Refusal::new(format!("there is no body {body}")))?;
                e.locked = *locked;
            }
            Op::SetLayerVisible { layer, visible } => {
                let l = doc
                    .layers
                    .iter_mut()
                    .find(|l| l.id == *layer)
                    .ok_or_else(|| Refusal::new("there is no such layer"))?;
                l.visible = *visible;
            }
            Op::SetLayerLocked { layer, locked } => {
                let l = doc
                    .layers
                    .iter_mut()
                    .find(|l| l.id == *layer)
                    .ok_or_else(|| Refusal::new("there is no such layer"))?;
                l.locked = *locked;
            }
            Op::CreateLayer { name } => {
                if name.trim().is_empty() {
                    return Err(Refusal::new("names can't be empty"));
                }
                let id = LayerId(doc.fresh_id());
                doc.layers.push(Layer {
                    id,
                    name: name.clone(),
                    visible: true,
                    locked: false,
                });
                out.created = Some(id.0);
                out.message = Some(format!("layer {} created", id.0));
            }
            Op::DeleteLayer { layer } => {
                if !doc.layers.iter().any(|l| l.id == *layer) {
                    return Err(Refusal::new("there is no such layer"));
                }
                if doc.layers.len() == 1 {
                    return Err(Refusal::new("the last layer can't be deleted"));
                }
                doc.layers.retain(|l| l.id != *layer);
                if doc.active_layer == *layer {
                    doc.active_layer = doc.layers[0].id;
                }
                let to = doc.active_layer;
                let mut moved = 0;
                for b in &mut doc.bodies {
                    if b.layer == *layer {
                        b.layer = to;
                        moved += 1;
                    }
                }
                out.message = Some(format!("{moved} object(s) moved to layer {}", to.0));
            }
            Op::RenameLayer { layer, name } => {
                if name.trim().is_empty() {
                    return Err(Refusal::new("names can't be empty"));
                }
                let l = doc
                    .layers
                    .iter_mut()
                    .find(|l| l.id == *layer)
                    .ok_or_else(|| Refusal::new("there is no such layer"))?;
                l.name = name.clone();
            }
            Op::MoveBodyToLayer { body, layer } => {
                if !doc.layers.iter().any(|l| l.id == *layer) {
                    return Err(Refusal::new("there is no such layer"));
                }
                let e = doc
                    .body_mut(*body)
                    .ok_or_else(|| Refusal::new(format!("there is no body {body}")))?;
                e.layer = *layer;
            }
            Op::SetActiveLayer { layer } => {
                if !doc.layers.iter().any(|l| l.id == *layer) {
                    return Err(Refusal::new("there is no such layer"));
                }
                doc.active_layer = *layer;
            }
            Op::FlipConnect { patch } => {
                let r = doc
                    .rules
                    .iter_mut()
                    .find(|r| matches!(&r.rule, Rule::Connect { patches, .. } if patches.contains(patch)))
                    .ok_or_else(|| Refusal::new("that patch has no Connect lines to flip"))?;
                if let Rule::Connect { flipped, .. } = &mut r.rule {
                    if let Some(i) = flipped.iter().position(|p| p == patch) {
                        flipped.remove(i);
                    } else {
                        flipped.push(*patch);
                        flipped.sort();
                    }
                }
            }
            Op::DeleteEdit { edit } => {
                let n = doc.edits.len();
                doc.edits.retain(|e| e.id != *edit);
                if doc.edits.len() == n {
                    return Err(Refusal::new(format!("there is no edit {edit}")));
                }
            }
            Op::PasteRules { clipboard, targets } => {
                if targets.is_empty() {
                    return Err(Refusal::new("select one or more objects to paste onto"));
                }
                let mut msgs = Vec::new();
                for t in targets {
                    if *t == clipboard.source {
                        continue;
                    }
                    msgs.push(self.paste_into(&mut doc, clipboard, *t)?);
                }
                out.message = Some(msgs.join("; "));
            }
            Op::SetBodyVisible { body, visible } => {
                let e = doc
                    .body_mut(*body)
                    .ok_or_else(|| Refusal::new(format!("there is no body {body}")))?;
                e.visible = *visible;
            }
        }
        Ok((doc, out))
    }

    /// After adding split lines on `faces`, give the new pieces the old
    /// patch's spread rule with distributed counts (SPEC §4.2).
    fn distribute_after_split(
        &self,
        doc: &mut Document,
        body: BodyId,
        faces: &[(FaceId, IsoDir)],
    ) -> Option<String> {
        let mut before_doc = doc.clone();
        let new_ids: Vec<crate::keys::SplitId> = doc
            .split_lines
            .iter()
            .rev()
            .take(faces.len())
            .map(|s| s.id)
            .collect();
        before_doc.split_lines.retain(|s| !new_ids.contains(&s.id));
        let before = self.patches_of_doc(&before_doc, body);
        let after = self.patches_of_doc(doc, body);
        let c = self.ctx(body)?;
        let mut msgs = Vec::new();
        for (f, dir) in faces {
            let olds: Vec<&Patch> = before.iter().filter(|p| p.face == *f).collect();
            let news: Vec<&Patch> = after.iter().filter(|p| p.face == *f).collect();
            let fg = c.face(*f);
            for o in &olds {
                // Pieces of o: new patches whose cells' samples fall in o.
                let s1_old = eval::face::stage1(
                    c,
                    body,
                    fg,
                    &before_doc
                        .split_lines
                        .iter()
                        .filter(|s| s.body == body && s.face == *f)
                        .collect::<Vec<_>>(),
                );
                let pieces: Vec<&&Patch> = news
                    .iter()
                    .filter(|n| {
                        let s1n = eval::face::stage1(
                            c,
                            body,
                            fg,
                            &doc.split_lines
                                .iter()
                                .filter(|s| s.body == body && s.face == *f)
                                .collect::<Vec<_>>(),
                        );
                        let samp = n
                            .cells
                            .first()
                            .map(|&ci| s1n.arr.cells[ci].sample)
                            .unwrap_or(n.sample);
                        s1_old.locate(fg, samp) == Some(o.index)
                    })
                    .collect();
                if pieces.is_empty() {
                    continue;
                }
                let d = if *dir == IsoDir::U { 0 } else { 1 };
                let widths: Vec<f64> = pieces
                    .iter()
                    .map(|p| {
                        if d == 0 {
                            p.hi.x - p.lo.x
                        } else {
                            p.hi.y - p.lo.y
                        }
                    })
                    .collect();
                let piece_ids: Vec<PatchId> = pieces.iter().map(|p| p.id).collect();
                // Rules owning o.
                let owners: Vec<usize> = (0..doc.rules.len())
                    .filter(|&i| rule_owns(&doc.rules[i].rule, &o.id))
                    .collect();
                for i in owners.into_iter().rev() {
                    let entry = doc.rules[i].clone();
                    match entry.rule {
                        Rule::Grid {
                            count_u,
                            count_v,
                            start_on_seam,
                            spacing,
                            ..
                        } => {
                            let (par, cross) = if d == 0 {
                                (count_u, count_v)
                            } else {
                                (count_v, count_u)
                            };
                            let dist = distribute(par, &widths);
                            doc.rules.remove(i);
                            for (k, pid) in piece_ids.iter().enumerate() {
                                let (cu, cv) = if d == 0 {
                                    (dist[k], cross)
                                } else {
                                    (cross, dist[k])
                                };
                                let rule = Rule::Grid {
                                    patch: *pid,
                                    count_u: cu,
                                    count_v: cv,
                                    start_on_seam,
                                    spacing,
                                };
                                if k == 0 {
                                    doc.rules.insert(
                                        i,
                                        RuleEntry {
                                            rule,
                                            ..entry.clone()
                                        },
                                    );
                                } else {
                                    add_rule(doc, rule);
                                }
                            }
                            msgs.push(format!("Surface Grid counts split {:?}", dist));
                        }
                        Rule::Connect {
                            patches,
                            count_a,
                            count_b,
                            start_on_seam,
                            flipped,
                        } => {
                            let bend = bend_class(c, fg);
                            let flip = flipped.contains(&o.id);
                            // Which family is parallel to the split?
                            let a_dir = match bend {
                                Bend::OneU | Bend::Two => Some(if flip { 1 } else { 0 }),
                                Bend::OneV => Some(if flip { 0 } else { 1 }),
                                _ => None,
                            };
                            let a_par = a_dir == Some(d);
                            let b_par = bend == Bend::Two && !a_par;
                            let mut rest = patches.clone();
                            rest.retain(|p| *p != o.id);
                            if !a_par && !b_par {
                                // Counts are copied: pieces join the same rule.
                                rest.extend(piece_ids.iter().copied());
                                rest.sort();
                                doc.rules[i].rule = Rule::Connect {
                                    patches: rest,
                                    count_a,
                                    count_b,
                                    start_on_seam,
                                    flipped: flipped.clone(),
                                };
                            } else {
                                let da = if a_par {
                                    distribute(count_a, &widths)
                                } else {
                                    vec![count_a; pieces.len()]
                                };
                                let db = if b_par {
                                    distribute(count_b, &widths)
                                } else {
                                    vec![count_b; pieces.len()]
                                };
                                if rest.is_empty() {
                                    doc.rules.remove(i);
                                } else {
                                    doc.rules[i].rule = Rule::Connect {
                                        patches: rest,
                                        count_a,
                                        count_b,
                                        start_on_seam,
                                        flipped: flipped.clone(),
                                    };
                                }
                                for (k, pid) in piece_ids.iter().enumerate() {
                                    add_rule(
                                        doc,
                                        Rule::Connect {
                                            patches: vec![*pid],
                                            count_a: da[k],
                                            count_b: db[k],
                                            start_on_seam,
                                            flipped: vec![],
                                        },
                                    );
                                }
                                msgs.push(format!("Connect counts split {:?}/{:?}", da, db));
                            }
                        }
                        Rule::Slice {
                            scope: SliceScope::Patches(mut list),
                            count,
                            orient,
                            through,
                            frozen_planes,
                        } => {
                            list.retain(|p| *p != o.id);
                            list.extend(piece_ids.iter().copied());
                            list.sort();
                            doc.rules[i].rule = Rule::Slice {
                                scope: SliceScope::Patches(list),
                                count,
                                orient,
                                through,
                                frozen_planes,
                            };
                        }
                        _ => {}
                    }
                }
            }
        }
        if msgs.is_empty() {
            None
        } else {
            Some(msgs.join("; "))
        }
    }

    /// Remove a split line's group and merge patch rules direction-aware
    /// (SPEC §4.2).
    fn remove_split(
        &self,
        doc: &mut Document,
        body: BodyId,
        split: crate::keys::SplitId,
    ) -> Option<String> {
        let group = doc.split_lines.iter().find(|s| s.id == split)?.group;
        let removed: Vec<SplitLine> = doc
            .split_lines
            .iter()
            .filter(|s| s.group == group)
            .cloned()
            .collect();
        let before = self.patches_of_doc(doc, body);
        let before_doc = doc.clone();
        doc.split_lines.retain(|s| s.group != group);
        let after = self.patches_of_doc(doc, body);
        let c = self.ctx(body)?;
        let mut msgs = Vec::new();
        for sl in &removed {
            let f = sl.face;
            let SplitKind::Iso { dir, .. } = sl.kind;
            let d = if dir == IsoDir::U { 0 } else { 1 };
            let fg = c.face(f);
            let s1_new = eval::face::stage1(
                c,
                body,
                fg,
                &doc.split_lines
                    .iter()
                    .filter(|s| s.body == body && s.face == f)
                    .collect::<Vec<_>>(),
            );
            let s1_old = eval::face::stage1(
                c,
                body,
                fg,
                &before_doc
                    .split_lines
                    .iter()
                    .filter(|s| s.body == body && s.face == f)
                    .collect::<Vec<_>>(),
            );
            for m in after.iter().filter(|p| p.face == f) {
                let pieces: Vec<&Patch> = before
                    .iter()
                    .filter(|o| o.face == f)
                    .filter(|o| {
                        let samp = o
                            .cells
                            .first()
                            .map(|&ci| s1_old.arr.cells[ci].sample)
                            .unwrap_or(o.sample);
                        s1_new.locate(fg, samp) == Some(m.index)
                    })
                    .collect();
                if pieces.is_empty() {
                    continue;
                }
                let largest = pieces
                    .iter()
                    .max_by(|a, b| a.area.total_cmp(&b.area))
                    .unwrap();
                // Rules on the pieces.
                let rule_of = |doc: &Document, p: &PatchId| {
                    doc.rules.iter().position(|r| rule_owns(&r.rule, p))
                };
                let kinds: Vec<Option<usize>> =
                    pieces.iter().map(|p| rule_of(doc, &p.id)).collect();
                let lrule = rule_of(doc, &largest.id);
                let piece_ids: Vec<PatchId> = pieces.iter().map(|p| p.id).collect();
                match lrule.map(|i| doc.rules[i].rule.clone()) {
                    Some(Rule::Grid {
                        count_u,
                        count_v,
                        start_on_seam,
                        spacing,
                        ..
                    }) => {
                        let all_grid = kinds.iter().all(|k| {
                            k.is_some_and(|i| matches!(doc.rules[i].rule, Rule::Grid { .. }))
                        });
                        let (mut cu, mut cv) = (count_u, count_v);
                        if all_grid {
                            let sum: u32 = kinds
                                .iter()
                                .map(|k| match doc.rules[k.unwrap()].rule {
                                    Rule::Grid {
                                        count_u, count_v, ..
                                    } => {
                                        if d == 0 {
                                            count_u
                                        } else {
                                            count_v
                                        }
                                    }
                                    _ => 0,
                                })
                                .sum();
                            if d == 0 {
                                cu = sum;
                            } else {
                                cv = sum;
                            }
                        }
                        self.takeover(doc, body, &piece_ids);
                        add_rule(
                            doc,
                            Rule::Grid {
                                patch: m.id,
                                count_u: cu,
                                count_v: cv,
                                start_on_seam,
                                spacing,
                            },
                        );
                        msgs.push(format!("Surface Grid merged to {cu} × {cv}"));
                    }
                    Some(Rule::Connect {
                        count_a,
                        count_b,
                        start_on_seam,
                        flipped,
                        ..
                    }) => {
                        let bend = bend_class(c, fg);
                        let flip = flipped.contains(&largest.id);
                        let a_dir = match bend {
                            Bend::OneU | Bend::Two => Some(if flip { 1 } else { 0 }),
                            Bend::OneV => Some(if flip { 0 } else { 1 }),
                            _ => None,
                        };
                        let a_par = a_dir == Some(d);
                        let b_par = bend == Bend::Two && !a_par;
                        let counts: Vec<(u32, u32)> = kinds
                            .iter()
                            .filter_map(|k| match k.map(|i| &doc.rules[i].rule) {
                                Some(Rule::Connect {
                                    count_a, count_b, ..
                                }) => Some((*count_a, *count_b)),
                                _ => None,
                            })
                            .collect();
                        let ca = if a_par {
                            counts.iter().map(|c| c.0).sum()
                        } else {
                            count_a
                        };
                        let cb = if b_par {
                            counts.iter().map(|c| c.1).sum()
                        } else {
                            count_b
                        };
                        self.takeover(doc, body, &piece_ids);
                        add_rule(
                            doc,
                            Rule::Connect {
                                patches: vec![m.id],
                                count_a: ca,
                                count_b: cb,
                                start_on_seam,
                                flipped: vec![],
                            },
                        );
                        msgs.push(format!("Connect merged to A = {ca}, B = {cb}"));
                    }
                    Some(Rule::Slice { .. }) => {
                        // Slices are geometry: replace pieces by the merged patch.
                        for r in &mut doc.rules {
                            if let Rule::Slice {
                                scope: SliceScope::Patches(list),
                                ..
                            } = &mut r.rule
                                && list.iter().any(|p| piece_ids.contains(p))
                            {
                                list.retain(|p| !piece_ids.contains(p));
                                list.push(m.id);
                                list.sort();
                            }
                        }
                    }
                    _ => {
                        // No rule on the largest piece: drop the pieces' rules.
                        self.takeover(doc, body, &piece_ids);
                    }
                }
            }
        }
        if msgs.is_empty() {
            Some("split removed".into())
        } else {
            Some(msgs.join("; "))
        }
    }
}

/// Through-iso walk across G1-continuous neighbours (SPEC §6.5).
pub fn iso_walk(
    ctx: &BodyCtx,
    body: BodyId,
    face: FaceId,
    dir: IsoDir,
    value: f64,
) -> Vec<IsoPiece> {
    iso_walk_within(ctx, body, face, dir, value, None)
}

/// [`iso_walk`] restricted to a set of faces (Connect chains).
pub fn iso_walk_within(
    ctx: &BodyCtx,
    body: BodyId,
    face: FaceId,
    dir: IsoDir,
    value: f64,
    within: Option<&std::collections::HashSet<FaceId>>,
) -> Vec<IsoPiece> {
    let mut out = vec![IsoPiece {
        body,
        face,
        dir,
        value,
        seed: None,
    }];
    let mut queue = vec![(face, dir, value)];
    let key = |f: FaceId, d: IsoDir, v: f64| (f, d == IsoDir::U, (v * 1e6).round() as i64);
    let mut seen = std::collections::HashSet::new();
    seen.insert(key(face, dir, value));
    while let Some((f, d, v)) = queue.pop() {
        let fg = ctx.face(f);
        let di = if d == IsoDir::U { 0 } else { 1 };
        let abs = eval::lines::iso_abs(fg, di, v);
        let (pts, closed) = eval::lines::iso_line(fg, di, abs, None);
        let s1 = eval::face::stage1(ctx, body, fg, &[]);
        let input = eval::clip::ClipInput {
            who: eval::clip::Who::Line(crate::keys::LineKey {
                rule: RuleId(0),
                index: 0,
                sub: 0,
            }),
            pts: &pts,
            closed,
            allowed: None,
            through: true,
            seed: None,
            end_keys: [None, None],
        };
        let mut loc = |uv: DVec2| s1.locate(fg, uv);
        let res = eval::clip::clip(fg, &input, &s1.curves, &mut loc);
        for e in &res.events {
            for &(ci, t) in &e.inc {
                if ci < eval::clip::STOP {
                    continue;
                }
                let st = &s1.curves[ci - eval::clip::STOP];
                let eval::Owner::Cad(edge) = st.owner else {
                    continue;
                };
                let et = ctx.topo.edge(edge);
                if et.faces.len() != 2 || et.dihedral.is_none_or(|a| a.to_degrees() > 5.0) {
                    continue;
                }
                let Some(&f2) = et.faces.iter().find(|x| **x != f) else {
                    continue;
                };
                if within.is_some_and(|w| !w.contains(&f2)) {
                    continue;
                }
                let ev1 = ctx.face_eval(f, &[e.uv]);
                let Some(e1) = ev1.first() else { continue };
                let tan = if di == 0 { e1.dv } else { e1.du }.normalize_or_zero();
                let Ok(uv2) = ctx
                    .kernel
                    .edge_pcurve_eval(ctx.kbody, edge, f2, false, &[t])
                else {
                    continue;
                };
                let uv2 = uv2[0];
                let ev2 = ctx.face_eval(f2, &[uv2]);
                let Some(e2) = ev2.first() else { continue };
                let fg2 = ctx.face(f2);
                let lim = 15f64.to_radians().cos();
                // Constant-u line runs along dv; constant-v along du.
                let cand = [(IsoDir::U, e2.dv, 0usize), (IsoDir::V, e2.du, 1usize)];
                for (nd, t3, ci2) in cand {
                    if tan.dot(t3.normalize_or_zero()).abs() >= lim {
                        let val = if ci2 == 0 { uv2.x } else { uv2.y };
                        let nv = eval::lines::iso_norm(fg2, ci2, val);
                        let k = key(f2, nd, nv);
                        if seen.insert(k) {
                            out.push(IsoPiece {
                                body,
                                face: f2,
                                dir: nd,
                                value: nv,
                                seed: None,
                            });
                            queue.push((f2, nd, nv));
                        }
                        break;
                    }
                }
            }
        }
    }
    out
}

/// Faces of a CAD edge (helper for UIs).
pub fn edge_faces(ctx: &BodyCtx, e: EdgeId) -> Vec<FaceId> {
    ctx.topo.edge(e).faces.clone()
}

/// Two transforms agree (rotation entries and translation within tolerance).
fn same_transform(a: &DAffine3, b: &DAffine3, size: f64) -> bool {
    let r = (a.matrix3 - b.matrix3)
        .to_cols_array()
        .iter()
        .all(|x| x.abs() < 1e-6);
    r && (a.translation - b.translation).length() < 1e-6 * size.max(1.0)
}

/// Two point lists that are the same set of 3D points (in any order).
fn same_points(a: &[DVec3], b: &[DVec3], tol: f64) -> bool {
    a.len() == b.len() && a.iter().all(|p| b.iter().any(|q| (*p - *q).length() < tol))
}

/// CAD faces of the polygons holding all the given output vertices (or,
/// when no polygon holds them all, any of them).
pub(crate) fn faces_around(m: &crate::mesh::OutputMesh, verts: &[u32]) -> Vec<FaceId> {
    let pick = |all: bool| -> Vec<FaceId> {
        m.polygons
            .iter()
            .filter(|p| {
                if all {
                    verts.iter().all(|v| p.verts.contains(v))
                } else {
                    verts.iter().any(|v| p.verts.contains(v))
                }
            })
            .map(|p| p.face)
            .collect()
    };
    let mut f = pick(true);
    if f.is_empty() {
        f = pick(false);
    }
    f.sort();
    f.dedup();
    f
}

/// Output vertex nearest to `p`, if within `tol`.
pub(crate) fn nearest_vertex(m: &crate::mesh::OutputMesh, p: DVec3, tol: f64) -> Option<u32> {
    m.vertices
        .iter()
        .enumerate()
        .map(|(i, v)| (i, (v.position - p).length()))
        .filter(|(_, d)| *d < tol)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i as u32)
}

/// The keyed output edge whose ends are the transformed `pa`, `pb` (nearest
/// output vertices within `tol`), preferring the one whose middle is
/// nearest the transformed `mid`.
pub(crate) fn match_edge(
    m: &crate::mesh::OutputMesh,
    t: &DAffine3,
    pa: DVec3,
    pb: DVec3,
    mid: DVec3,
    tol: f64,
) -> Option<EdgeKey> {
    let ia = nearest_vertex(m, t.transform_point3(pa), tol)?;
    let ib = nearest_vertex(m, t.transform_point3(pb), tol)?;
    let (lo, hi) = (ia.min(ib), ia.max(ib));
    let qm = t.transform_point3(mid);
    let d = |e: &crate::mesh::OutEdge| (e.path[e.path.len() / 2] - qm).length();
    m.edges
        .iter()
        .filter(|e| e.a == lo && e.b == hi && e.key.is_some())
        .min_by(|x, y| d(x).total_cmp(&d(y)))
        .and_then(|e| e.key.clone())
}

/// Outcome note for Link / symmetry copies.
fn partner_note(copies: usize, skipped: usize) -> Option<String> {
    match (copies, skipped) {
        (0, 0) => None,
        (c, 0) => Some(format!("applied to {c} linked or mirrored partner(s)")),
        (c, s) => Some(format!(
            "applied to {c} linked or mirrored partner(s); {s} partner(s) skipped: no matching geometry there"
        )),
    }
}
