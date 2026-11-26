//! Scene API used by the app: bend arrows, iso preview, selection wrappers,
//! body replacement, camera, link/symmetry partners, copy rules.

use super::{BodyCache, Scene};
use crate::doc::{Camera, Document, Edit, IsoDir, PatchId, Rule};
use crate::eval::lines::{Bend, bend_class};
use crate::eval::{self, BodyCtx};
use crate::keys::{EdgeKey, RuleId, VertexKey};
use crate::link::{self, Clipboard, FaceMap, LinkSets};
use crate::mesh::OutputMesh;
use crate::select::Adjacency;
use anyhow::Context;
use glam::{DAffine3, DVec2, DVec3};
use newtopo_kernel::{BodyAttributes, BodyId, FaceId, HealReport};
use std::collections::HashMap;
use std::sync::Arc;

/// One bend direction of a patch, for drawing an arrow (body coordinates).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BendArrow {
    /// A point inside the patch.
    pub point: DVec3,
    /// Direction along which the lines are spread (the count direction).
    pub spread_dir: DVec3,
    /// Direction the generated lines run.
    pub line_dir: DVec3,
}

/// Connect directions available on a patch (SPEC §6.2).
#[derive(Clone, Debug, PartialEq)]
pub struct BendInfo {
    pub bend: Bend,
    /// A/B swapped on this patch (`Op::FlipConnect`).
    pub flipped: bool,
    pub a: Option<BendArrow>,
    pub b: Option<BendArrow>,
}

/// What [`Scene::replace_body_with_report`] carried over.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReplaceReport {
    pub new_body: Option<BodyId>,
    /// Old faces matched to a congruent new face.
    pub matched: usize,
    /// Old faces with no match (their rules were dropped).
    pub unmatched: Vec<FaceId>,
    pub rules_kept: usize,
    pub rules_dropped: usize,
    /// Edits are keyed by the old topology and are not carried over.
    pub edits_dropped: usize,
}

impl Scene {
    /// Connect directions and arrow placement for a patch.
    pub fn bend_info(&mut self, patch: &PatchId) -> Option<BendInfo> {
        let rec = self
            .patch_records(patch.body)
            .into_iter()
            .find(|p| p.id == *patch)?;
        let flipped =
            self.doc().rules.iter().any(
                |r| matches!(&r.rule, Rule::Connect { flipped, .. } if flipped.contains(patch)),
            );
        let c = self.ctx(patch.body)?;
        let fg = c.face(patch.face);
        let bend = bend_class(c, fg);
        let mid = if let Some(cen) = fg.ring {
            cen + (rec.sample - cen) * 0.9
        } else {
            let m = (rec.lo + rec.hi) * 0.5;
            // Prefer a point inside the patch: nudge towards the sample.
            let inside = c
                .kernel
                .face_classify_uv(c.kbody, fg.id, &[fg.wrap(m)])
                .ok()
                .is_some_and(|v| v[0] != newtopo_kernel::UvClass::Out);
            if inside {
                m
            } else {
                m + (rec.sample - m) * 0.5
            }
        };
        let e = c.face_eval(fg.id, &[fg.wrap(mid)]).into_iter().next()?;
        let (du, dv) = (e.du.normalize_or_zero(), e.dv.normalize_or_zero());
        let u_family = BendArrow {
            point: e.p,
            spread_dir: du,
            line_dir: dv,
        };
        let v_family = BendArrow {
            point: e.p,
            spread_dir: dv,
            line_dir: du,
        };
        let (a, b) = match bend {
            Bend::Flat => (None, None),
            Bend::Radial => {
                let centre = c
                    .face_eval(fg.id, &[fg.ring.unwrap_or(mid)])
                    .into_iter()
                    .next()?
                    .p;
                let radial = (e.p - centre).normalize_or_zero();
                (
                    Some(BendArrow {
                        point: e.p,
                        spread_dir: e.n.cross(radial).normalize_or_zero(),
                        line_dir: radial,
                    }),
                    None,
                )
            }
            Bend::OneU => (Some(if flipped { v_family } else { u_family }), None),
            Bend::OneV => (Some(if flipped { u_family } else { v_family }), None),
            Bend::Two => {
                if flipped {
                    (Some(v_family), Some(u_family))
                } else {
                    (Some(u_family), Some(v_family))
                }
            }
        };
        Some(BendInfo {
            bend,
            flipped,
            a,
            b,
        })
    }

    /// Polylines (body coordinates) the Iso op would create, through mode
    /// across tangent faces included.
    pub fn iso_preview(
        &mut self,
        body: BodyId,
        face: FaceId,
        dir: IsoDir,
        value: f64,
        seed: Option<f64>,
        through: bool,
    ) -> Vec<Vec<DVec3>> {
        let Some(c) = self.ctx(body) else {
            return vec![];
        };
        if face.idx() >= c.topo.faces.len() {
            return vec![];
        }
        let pieces = if through {
            crate::tools::iso_walk(c, body, face, dir, value)
        } else {
            vec![crate::doc::IsoPiece {
                body,
                face,
                dir,
                value,
                seed,
            }]
        };
        let mut out = Vec::new();
        for p in pieces {
            out.extend(iso_piece_polylines(
                c,
                body,
                p.face,
                p.dir,
                p.value,
                if through { None } else { p.seed },
                through,
            ));
        }
        out
    }

    /// Edge loop through output edge `edge` of `body` (indices into
    /// `OutputMesh::edges`).
    pub fn edge_loop(&mut self, body: BodyId, edge: usize) -> Vec<usize> {
        let Some(m) = self.output_of(body) else {
            return vec![];
        };
        if edge >= m.edges.len() {
            return vec![];
        }
        Adjacency::new(m).edge_loop(edge)
    }

    /// Edge ring through output edge `edge`.
    pub fn edge_ring(&mut self, body: BodyId, edge: usize) -> Vec<usize> {
        let Some(m) = self.output_of(body) else {
            return vec![];
        };
        if edge >= m.edges.len() {
            return vec![];
        }
        Adjacency::new(m).edge_ring(edge)
    }

    /// Face strip through polygon `poly`, crossing its side `edge`.
    pub fn face_strip(&mut self, body: BodyId, poly: usize, edge: usize) -> Vec<usize> {
        let Some(m) = self.output_of(body) else {
            return vec![];
        };
        if poly >= m.polygons.len() || edge >= m.edges.len() {
            return vec![];
        }
        Adjacency::new(m).face_loop(poly, edge)
    }

    /// Set the camera saved with the scene. Not an undo step; marks the
    /// scene modified.
    pub fn set_camera(&mut self, camera: Camera) {
        if self.doc.camera == camera {
            return;
        }
        let mut d = (*self.doc).clone();
        d.camera = camera;
        let old = std::mem::replace(&mut self.doc, Arc::new(d));
        // The camera is not an edit: the last mesh edit stays adjustable.
        self.carry_amend_mark(&old);
    }

    /// Faces congruent to `face` in the same body (its link set, including
    /// `face`; just `[face]` when it has no partner). SPEC §6.11.
    pub fn link_set(&mut self, body: BodyId, face: FaceId) -> Vec<FaceId> {
        let Some(ls) = self.link_sets(body) else {
            return vec![face];
        };
        let mut v: Vec<FaceId> = std::iter::once(face)
            .chain(ls.partners(face).into_iter().map(|(f, _)| f))
            .collect();
        v.sort();
        v
    }

    /// Mirror partners of `face` for the scene's symmetry axes (empty when
    /// symmetry is off or the face has none). SPEC §6.12.
    pub fn symmetry_partners(&mut self, body: BodyId, face: FaceId) -> Vec<FaceId> {
        let axes = self.doc.settings.symmetry_axes.clone();
        let mut out = Vec::new();
        for ax in axes {
            if let Some(m) = self.mirror_map(body, ax)
                && let Some((f, _)) = m.get(&face)
            {
                out.push(*f);
            }
        }
        out
    }

    pub(crate) fn link_sets(&mut self, body: BodyId) -> Option<&LinkSets> {
        if !self.links.contains_key(&body) {
            let c = &self.bodies.get(&body)?.ctx;
            let ls = LinkSets::compute(c);
            self.links.insert(body, ls);
        }
        self.links.get(&body)
    }

    pub(crate) fn mirror_map(&mut self, body: BodyId, axis: crate::doc::Axis) -> Option<&FaceMap> {
        if !self.mirrors.contains_key(&(body, axis)) {
            let xf = self.doc.body(body)?.transform;
            let c = &self.bodies.get(&body)?.ctx;
            let refl = link::mirror(axis, xf);
            let m = link::symmetry_partners(c, &refl);
            self.mirrors.insert((body, axis), m);
        }
        self.mirrors.get(&(body, axis))
    }

    /// Faces a tool applied to `face` also reaches (Link and Symmetry, as
    /// set in the document), with the face-to-face transforms.
    pub(crate) fn tool_partners(&mut self, body: BodyId, face: FaceId) -> Vec<(FaceId, DAffine3)> {
        let link_on = self.doc.settings.link;
        let sym_on = self.doc.settings.symmetry;
        let mut out: Vec<(FaceId, DAffine3)> = Vec::new();
        let push = |out: &mut Vec<(FaceId, DAffine3)>, f: FaceId, t: DAffine3| {
            if f != face && !out.iter().any(|(g, _)| *g == f) {
                out.push((f, t));
            }
        };
        if link_on && let Some(ls) = self.link_sets(body) {
            for (f, t) in ls.partners(face) {
                push(&mut out, f, t);
            }
        }
        if sym_on {
            let axes = self.doc.settings.symmetry_axes.clone();
            let base: Vec<(FaceId, DAffine3)> = std::iter::once((face, DAffine3::IDENTITY))
                .chain(out.clone())
                .collect();
            for ax in axes {
                let Some(m) = self.mirror_map(body, ax).cloned() else {
                    continue;
                };
                for (f, t) in &base {
                    if let Some((g, r)) = m.get(f) {
                        push(&mut out, *g, *r * *t);
                    }
                }
            }
        }
        out
    }

    /// Copy a body's split lines and rules (SPEC §6.13).
    pub fn copy_rules(&self, body: BodyId) -> Clipboard {
        let Some(c) = self.ctx(body) else {
            return Clipboard {
                source: body,
                rules: vec![],
                split_lines: vec![],
                phase_overrides: vec![],
                edits: 0,
                edit_list: vec![],
            };
        };
        let rules = self
            .doc
            .rules
            .iter()
            .filter(|r| !eval::rule_faces(&r.rule, body, c).is_empty())
            .map(|r| r.rule.clone())
            .collect();
        Clipboard {
            source: body,
            rules,
            split_lines: self.doc.split_lines.iter().filter(|s| s.body == body).cloned().collect(),
            phase_overrides: self
                .doc
                .phase_overrides
                .iter()
                .filter(|(k, _)| matches!(k, crate::doc::PhaseTarget::Face(b, _) | crate::doc::PhaseTarget::Edge(b, _) if *b == body))
                .map(|(k, v)| (*k, *v))
                .collect(),
            edits: self.doc.edits.iter().filter(|e| e.edit.body() == body).count(),
            edit_list: self
                .doc
                .edits
                .iter()
                .filter(|e| e.edit.body() == body)
                .map(|e| e.edit.clone())
                .collect(),
        }
    }

    /// Replace a body by a new version (import "Replace"), carrying rules on
    /// faces that still match. One undo step.
    pub fn replace_body(
        &mut self,
        old: BodyId,
        brep: &[u8],
        attrs: BodyAttributes,
        heal: HealReport,
    ) -> anyhow::Result<BodyId> {
        Ok(self
            .replace_body_with_report(old, brep, attrs, heal)?
            .new_body
            .unwrap_or(old))
    }

    /// [`Scene::replace_body`] with a report of what matched. Refused for a
    /// body with mesh edits (SPEC §9.5.1): replacing changes its CAD stage,
    /// and the edited mesh belongs to the old body.
    pub fn replace_body_with_report(
        &mut self,
        old: BodyId,
        brep: &[u8],
        attrs: BodyAttributes,
        heal: HealReport,
    ) -> anyhow::Result<ReplaceReport> {
        let entry = self.doc.body(old).cloned().context("no such body")?;
        if self.has_mesh_edits(old) {
            anyhow::bail!("{}", self.mesh_lock_refusal(old).reason);
        }
        anyhow::ensure!(self.bodies.contains_key(&old), "body without kernel data");
        let kbody = self
            .kernel
            .load_brep(brep)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        // Until the new body is inserted below the scene does not own it,
        // so every failure from here on unloads it.
        let ctx = match BodyCtx::build(self.kernel.clone(), kbody) {
            Ok(c) => c,
            Err(e) => {
                self.kernel.unload(kbody);
                anyhow::bail!("{e}");
            }
        };
        let old_ctx = &self.bodies[&old].ctx;
        // Face matching by congruence (identity transform preferred).
        let mut report = ReplaceReport::default();
        let src: Vec<FaceId> = old_ctx.topo.face_ids().collect();
        let (map, unmatched) = link::match_bodies(old_ctx, &src, &ctx, &[DAffine3::IDENTITY]);
        report.matched = map.len();
        report.unmatched = unmatched;
        let mut doc = (*self.doc).clone();
        let id = kbody;
        // Split lines first, so rules on patches they bound can be mapped
        // onto the new body's patches (as paste does).
        let splits: Vec<_> = doc
            .split_lines
            .iter()
            .filter(|s| s.body == old)
            .cloned()
            .collect();
        doc.split_lines.retain(|s| s.body != old);
        for s in &splits {
            if let Some(ns) = link::map_split(s, old_ctx, id, &ctx, &map, s.id, s.group) {
                doc.split_lines.push(ns);
            }
        }
        let old_rules: Vec<&Rule> = doc
            .rules
            .iter()
            .filter(|r| !eval::rule_faces(&r.rule, old, old_ctx).is_empty())
            .map(|r| &r.rule)
            .collect();
        let split_patches =
            map_split_patches(old_ctx, old, &old_rules, &splits, &ctx, id, &doc, &map);
        let mp = |p: &PatchId| -> Option<PatchId> {
            if link::is_whole(p) {
                link::map_patch(p, id, &map)
            } else {
                split_patches.get(p).copied()
            }
        };
        let mut new_rules = Vec::new();
        doc.rules.retain(|r| {
            if eval::rule_faces(&r.rule, old, old_ctx).is_empty() {
                return true;
            }
            match link::map_rule_with(&r.rule, old, old_ctx, id, &ctx, &map, &mp) {
                Some(nr) => {
                    new_rules.push(crate::doc::RuleEntry {
                        rule: nr,
                        ..r.clone()
                    });
                    report.rules_kept += 1;
                }
                None => report.rules_dropped += 1,
            }
            false
        });
        doc.rules.extend(new_rules);
        report.edits_dropped = doc.edits.iter().filter(|e| e.edit.body() == old).count();
        doc.edits.retain(|e| e.edit.body() != old);
        link::map_phases(&mut doc, old, old_ctx, id, &ctx, &map);
        let po: Vec<_> = doc.phase_overrides.keys().copied().filter(|k| matches!(k, crate::doc::PhaseTarget::Face(b, _) | crate::doc::PhaseTarget::Edge(b, _) if *b == old)).collect();
        for k in po {
            doc.phase_overrides.remove(&k);
        }
        let pos = doc.bodies.iter().position(|b| b.id == old).unwrap_or(0);
        doc.bodies[pos] = crate::doc::BodyEntry {
            id,
            name: if attrs.name.is_empty() {
                entry.name.clone()
            } else {
                attrs.name.clone()
            },
            color: attrs.color.or(entry.color),
            transform: attrs.transform,
            heal,
            ..entry
        };
        // A rule that also names patches of another body was rewritten
        // above: refused if that body has mesh edits (its CAD stage is
        // locked). The scene does not own the new body yet.
        if let Err(r) = self.check_mesh_locks(&doc) {
            drop(ctx);
            self.kernel.unload(kbody);
            anyhow::bail!("{}", r.reason);
        }
        let edge_polylines = ctx.topo.edge_ids().map(|e| ctx.edge_polyline(e)).collect();
        self.bodies.insert(
            id,
            BodyCache {
                ctx,
                edge_polylines,
            },
        );
        self.commit(doc);
        report.new_body = Some(id);
        Ok(report)
    }

    /// Map a clipboard's rules onto `target` (paste, SPEC §6.13). Split
    /// lines are pasted first so patches they bound can be mapped; spread
    /// rules take their patches over (SPEC §4.3); Join rules and edits are
    /// mapped by geometry on the outputs. Returns a message saying what was
    /// pasted and what could not be mapped; refuses when no face matches.
    pub(crate) fn paste_into(
        &mut self,
        doc: &mut Document,
        clip: &Clipboard,
        target: BodyId,
    ) -> Result<String, crate::ops::Refusal> {
        use crate::doc::SliceScope;
        use crate::ops::Refusal;
        if !self.bodies.contains_key(&clip.source) {
            return Err(Refusal::new("the copied body is no longer in the scene"));
        }
        if !self.bodies.contains_key(&target) {
            return Err(Refusal::new(format!("there is no body {target}")));
        }
        let src = clip.source;
        let has_keyed =
            !clip.edit_list.is_empty() || clip.rules.iter().any(|r| matches!(r, Rule::Join { .. }));
        // Join rules and edits name generated vertices and edges: they are
        // mapped by position between the source output (without its edits)
        // and the target output, and their faces join the face matching.
        let ms = if has_keyed {
            let mut sdoc = (*self.doc).clone();
            sdoc.edits.retain(|e| e.edit.body() != src);
            Some(
                self.eval_doc(&sdoc, src)
                    .ok_or_else(|| Refusal::new("the copied body has no CAD data"))?
                    .mesh,
            )
        } else {
            None
        };
        let (map, unmatched, mut pasted, mut skipped) = {
            let ca = &self.bodies[&src].ctx;
            let cb = &self.bodies[&target].ctx;
            let mut src_faces: Vec<FaceId> = Vec::new();
            for r in &clip.rules {
                if !matches!(r, Rule::Join { .. }) {
                    src_faces.extend(eval::rule_faces(r, src, ca));
                }
            }
            if let Some(ms) = &ms {
                src_faces.extend(keyed_faces(ms, ca, clip));
            }
            src_faces.extend(clip.split_lines.iter().map(|s| s.face));
            src_faces.sort();
            src_faces.dedup();
            // A body transform estimate from the attributes, then per-face matches.
            let wa = doc.body(src).map(|b| b.transform).unwrap_or_default();
            let wb = doc.body(target).map(|b| b.transform).unwrap_or_default();
            let body_xf = DAffine3::from_mat4(wb.inverse() * wa);
            let (map, unmatched_faces) =
                link::match_bodies(ca, &src_faces, cb, &[body_xf, DAffine3::IDENTITY]);
            if map.is_empty() {
                return Err(Refusal::new("no face of the copied body matches this body"));
            }
            // Split lines first: patches bounded by them map onto the
            // target's new patches.
            let group = doc.fresh_id();
            for s in &clip.split_lines {
                let id = crate::keys::SplitId(doc.fresh_id());
                if let Some(ns) = link::map_split(s, ca, target, cb, &map, id, group) {
                    doc.split_lines.push(ns);
                }
            }
            let rules: Vec<&Rule> = clip.rules.iter().collect();
            let split_patches =
                map_split_patches(ca, src, &rules, &clip.split_lines, cb, target, doc, &map);
            let mp = |p: &PatchId| -> Option<PatchId> {
                if link::is_whole(p) {
                    link::map_patch(p, target, &map)
                } else {
                    split_patches.get(p).copied()
                }
            };
            let mut pasted = 0;
            let mut skipped = 0;
            for r in &clip.rules {
                if matches!(r, Rule::Join { .. }) {
                    continue;
                }
                let Some(nr) = link::map_rule_with(r, src, ca, target, cb, &map, &mp) else {
                    skipped += 1;
                    continue;
                };
                // Pasting takes over the patches the new rule owns.
                let owned: Vec<PatchId> = match &nr {
                    Rule::Connect { patches, .. } => patches.clone(),
                    Rule::Grid { patch, .. } => vec![*patch],
                    Rule::Slice {
                        scope: SliceScope::Patches(l),
                        ..
                    } => l.clone(),
                    Rule::Slice {
                        scope: SliceScope::Body(_),
                        ..
                    } => self
                        .patches_cached(doc, target)
                        .iter()
                        .map(|p| p.id)
                        .collect(),
                    _ => vec![],
                };
                if !owned.is_empty() {
                    self.takeover(doc, target, &owned);
                }
                push_rule(doc, nr);
                pasted += 1;
            }
            link::map_phases(doc, src, ca, target, cb, &map);
            (map, unmatched_faces.len(), pasted, skipped)
        };

        // Join rules and edits: their keys name generated vertices and
        // edges, so they are mapped by position between the source output
        // (without its edits) and the target output.
        let joins: Vec<&Rule> = clip
            .rules
            .iter()
            .filter(|r| matches!(r, Rule::Join { .. }))
            .collect();
        let mut pasted_edits = 0;
        if let Some(ms) = ms {
            let tol = 1e-4 * self.bodies[&target].ctx.size.max(1e-9);
            let mut new_joins: Vec<RuleId> = Vec::new();
            if !joins.is_empty() {
                let mt = self
                    .eval_doc(doc, target)
                    .ok_or_else(|| Refusal::new("body has no CAD data"))?
                    .mesh;
                for r in joins {
                    let Rule::Join { a, b, .. } = r else { continue };
                    match (
                        map_vertex_key(&ms, &mt, &map, tol, a),
                        map_vertex_key(&ms, &mt, &map, tol, b),
                    ) {
                        (Some(a), Some(b)) if a != b => {
                            new_joins.push(push_rule(doc, Rule::Join { body: target, a, b }));
                        }
                        _ => skipped += 1,
                    }
                }
            }
            let mut new_edits: Vec<u32> = Vec::new();
            if !clip.edit_list.is_empty() {
                let mt = self
                    .eval_doc(doc, target)
                    .ok_or_else(|| Refusal::new("body has no CAD data"))?
                    .mesh;
                let ca = &self.bodies[&src].ctx;
                let cb = &self.bodies[&target].ctx;
                for e in &clip.edit_list {
                    let ek = |k: &EdgeKey| map_edge_key(&ms, &mt, &map, tol, k);
                    let vk = |k: &VertexKey| map_vertex_key(&ms, &mt, &map, tol, k);
                    let mapped = match e {
                        Edit::RemoveEdge { edge, .. } => {
                            ek(edge).map(|edge| Edit::RemoveEdge { body: target, edge })
                        }
                        Edit::Collapse { edge, .. } => {
                            ek(edge).map(|edge| Edit::Collapse { body: target, edge })
                        }
                        Edit::SetHard { edge, hard, .. } => ek(edge).map(|edge| Edit::SetHard {
                            body: target,
                            edge,
                            hard: *hard,
                        }),
                        Edit::Weld { from, to, .. } => match (vk(from), vk(to)) {
                            (Some(from), Some(to)) if from != to => Some(Edit::Weld {
                                body: target,
                                from,
                                to,
                            }),
                            _ => None,
                        },
                        Edit::RemoveCadEdge { edge, .. } => {
                            let (lo, hi) = ca.topo.edge(*edge).t_range;
                            link::map_edge_point(ca, *edge, 0.5 * (lo + hi), cb, &map)
                                .map(|(edge, _)| Edit::RemoveCadEdge { body: target, edge })
                        }
                    };
                    match mapped {
                        Some(edit) => {
                            let id = doc.fresh_id();
                            doc.edits.push(crate::doc::EditEntry { id, edit });
                            new_edits.push(id);
                        }
                        None => skipped += 1,
                    }
                }
            }
            // Mapped joins that make no line and edits that go dormant on
            // the target are dropped and reported.
            let ev = self
                .eval_doc(doc, target)
                .ok_or_else(|| Refusal::new("body has no CAD data"))?;
            let dead_joins: Vec<RuleId> = new_joins
                .iter()
                .copied()
                .filter(|id| !ev.lines_per_rule.contains_key(id))
                .collect();
            let dead_edits: Vec<u32> = new_edits
                .iter()
                .copied()
                .filter(|id| ev.dormant.contains(id))
                .collect();
            doc.rules.retain(|r| !dead_joins.contains(&r.id));
            doc.edits.retain(|e| !dead_edits.contains(&e.id));
            skipped += dead_joins.len() + dead_edits.len();
            pasted += new_joins.len() - dead_joins.len();
            pasted_edits = new_edits.len() - dead_edits.len();
        }
        let mut msg = format!(
            "pasted {pasted} rule(s) and {pasted_edits} edit(s) onto body {target}; {unmatched} face(s) had no match"
        );
        if skipped > 0 {
            msg.push_str(&format!(
                "; {skipped} rule(s) or edit(s) could not be mapped and were skipped"
            ));
        }
        Ok(msg)
    }
}

fn push_rule(doc: &mut Document, rule: Rule) -> RuleId {
    let id = RuleId(doc.fresh_id());
    let ordinal = doc.rules.iter().map(|r| r.ordinal).max().unwrap_or(0) + 1;
    doc.rules.push(crate::doc::RuleEntry {
        id,
        ordinal,
        rule,
        link_group: None,
    });
    id
}

/// Map the clipboard's patches bounded by split lines onto the target:
/// each source patch's sample point, transformed, lands in one patch of the
/// mapped face (with the pasted split lines already in `doc`).
#[allow(clippy::too_many_arguments)]
fn map_split_patches(
    ca: &BodyCtx,
    src: BodyId,
    rules: &[&Rule],
    src_splits: &[crate::doc::SplitLine],
    cb: &BodyCtx,
    target: BodyId,
    doc: &Document,
    map: &link::FaceMap,
) -> HashMap<PatchId, PatchId> {
    let mut want: Vec<PatchId> = Vec::new();
    for r in rules {
        match r {
            Rule::Connect {
                patches, flipped, ..
            } => want.extend(patches.iter().chain(flipped)),
            Rule::Grid { patch, .. } => want.push(*patch),
            Rule::Slice {
                scope: crate::doc::SliceScope::Patches(l),
                ..
            } => want.extend(l),
            _ => {}
        }
    }
    want.retain(|p| p.body == src && !link::is_whole(p) && map.contains_key(&p.face));
    want.sort();
    want.dedup();
    let mut out = HashMap::new();
    let mut faces: Vec<FaceId> = want.iter().map(|p| p.face).collect();
    faces.dedup();
    for fa in faces {
        let (fb, t) = map[&fa];
        let sa: Vec<_> = src_splits
            .iter()
            .filter(|s| s.body == src && s.face == fa)
            .collect();
        let sb: Vec<_> = doc
            .split_lines
            .iter()
            .filter(|s| s.body == target && s.face == fb)
            .collect();
        let s1a = eval::face::stage1(ca, src, ca.face(fa), &sa);
        let s1b = eval::face::stage1(cb, target, cb.face(fb), &sb);
        for p in want.iter().filter(|p| p.face == fa) {
            // An interior point of the patch (a cell sample; the patch's
            // own `sample` lies on its boundary).
            let Some(inner) = s1a
                .patches
                .iter()
                .find(|r| r.id == *p)
                .and_then(|r| r.cells.first())
                .map(|c| s1a.arr.cells[*c].sample)
            else {
                continue;
            };
            let q = t.transform_point3(ca.point(fa, inner));
            let Ok(v) = cb.kernel.project_to_face(cb.kbody, fb, &[q]) else {
                continue;
            };
            let Some(Some((uv, _))) = v.into_iter().next() else {
                continue;
            };
            if let Some(i) = s1b.locate(cb.face(fb), uv)
                && let Some(pb) = s1b.patches.iter().find(|r| r.index == i)
            {
                out.insert(*p, pb.id);
            }
        }
    }
    out
}

/// Source faces that the clipboard's Join rules and edits lie on (through
/// the source output `ms`).
fn keyed_faces(ms: &OutputMesh, ca: &BodyCtx, clip: &Clipboard) -> Vec<FaceId> {
    let vert = |k: &VertexKey| {
        ms.vertices
            .iter()
            .position(|v| &v.key == k)
            .map(|i| i as u32)
    };
    let edge = |k: &EdgeKey| {
        ms.edges
            .iter()
            .find(|e| e.key.as_ref() == Some(k))
            .map(|e| vec![e.a, e.b])
            .unwrap_or_default()
    };
    let mut verts: Vec<u32> = Vec::new();
    let mut faces: Vec<FaceId> = Vec::new();
    for r in &clip.rules {
        if let Rule::Join { a, b, .. } = r {
            verts.extend(vert(a).into_iter().chain(vert(b)));
        }
    }
    for e in &clip.edit_list {
        match e {
            Edit::RemoveEdge { edge: k, .. }
            | Edit::Collapse { edge: k, .. }
            | Edit::SetHard { edge: k, .. } => verts.extend(edge(k)),
            Edit::Weld { from, to, .. } => verts.extend(vert(from).into_iter().chain(vert(to))),
            Edit::RemoveCadEdge { edge: e, .. } => {
                if e.idx() < ca.topo.edges.len() {
                    faces.extend(ca.topo.edge(*e).faces.iter().copied());
                }
            }
        }
    }
    for v in verts {
        faces.extend(crate::tools::faces_around(ms, &[v]));
    }
    faces
}

/// Transform for a generated vertex: that of the first mapped face its
/// polygons lie on.
fn vertex_transform(m: &OutputMesh, map: &link::FaceMap, v: u32) -> Option<DAffine3> {
    crate::tools::faces_around(m, &[v])
        .iter()
        .find_map(|f| map.get(f).map(|(_, t)| *t))
}

/// Map a vertex key from one output to another by position.
fn map_vertex_key(
    ms: &OutputMesh,
    mt: &OutputMesh,
    map: &link::FaceMap,
    tol: f64,
    k: &VertexKey,
) -> Option<VertexKey> {
    let i = ms.vertices.iter().position(|v| &v.key == k)? as u32;
    let t = vertex_transform(ms, map, i)?;
    let j = crate::tools::nearest_vertex(
        mt,
        t.transform_point3(ms.vertices[i as usize].position),
        tol,
    )?;
    Some(mt.vertices[j as usize].key.clone())
}

/// Map an edge key from one output to another by its ends and middle.
fn map_edge_key(
    ms: &OutputMesh,
    mt: &OutputMesh,
    map: &link::FaceMap,
    tol: f64,
    k: &EdgeKey,
) -> Option<EdgeKey> {
    let e = ms.edges.iter().find(|e| e.key.as_ref() == Some(k))?;
    let t = vertex_transform(ms, map, e.a).or_else(|| vertex_transform(ms, map, e.b))?;
    crate::tools::match_edge(
        mt,
        &t,
        ms.vertices[e.a as usize].position,
        ms.vertices[e.b as usize].position,
        e.path[e.path.len() / 2],
        tol,
    )
}

/// Iso piece → body-space polylines (all trimmed components, or the one
/// nearest the seed).
fn iso_piece_polylines(
    c: &BodyCtx,
    body: BodyId,
    face: FaceId,
    dir: IsoDir,
    value: f64,
    seed: Option<f64>,
    through: bool,
) -> Vec<Vec<DVec3>> {
    let fg = c.face(face);
    let d = if dir == IsoDir::U { 0 } else { 1 };
    let v = eval::lines::iso_abs(fg, d, value);
    let (pts, closed) = eval::lines::iso_line(fg, d, v, None);
    let seed_uv = seed.map(|s| {
        let t = eval::lines::iso_abs(fg, 1 - d, s);
        if d == 0 {
            DVec2::new(v, t)
        } else {
            DVec2::new(t, v)
        }
    });
    let s1 = eval::face::stage1(c, body, fg, &[]);
    let input = eval::clip::ClipInput {
        who: eval::clip::Who::Line(crate::keys::LineKey {
            rule: crate::keys::RuleId(0),
            index: 0,
            sub: 0,
        }),
        pts: &pts,
        closed,
        allowed: None,
        through,
        seed: if through { None } else { seed_uv },
        end_keys: [None, None],
    };
    let mut loc = |uv: DVec2| s1.locate(fg, uv);
    let out = eval::clip::clip(fg, &input, &s1.curves, &mut loc);
    let mut res = Vec::new();
    for cv in &out.curves {
        let mut uv: Vec<DVec2> = Vec::new();
        for w in cv.pts.windows(2) {
            for k in 0..16 {
                uv.push(w[0] + (w[1] - w[0]) * (k as f64 / 16.0));
            }
        }
        if let Some(l) = cv.pts.last() {
            uv.push(*l);
        }
        res.push(c.face_eval(face, &uv).into_iter().map(|e| e.p).collect());
    }
    res
}
