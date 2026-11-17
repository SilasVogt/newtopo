//! The `Scene` facade: document + undo + kernel + evaluation cache. The app,
//! the CLI and tests all drive the core through this type.
//!
//! OWNER: engine. Signatures here are the contract the app and CLI build
//! against; keep them stable (add methods freely).
//!
//! * The document lives behind an `Arc`; every accepted op replaces it with
//!   a new snapshot and pushes the old one on the undo stack (depth 500,
//!   SPEC §4.7). [`Scene::begin_coalesce`] / [`Scene::end_coalesce`] merge a
//!   burst of ops (a wheel gesture) into one undo step.
//! * Per body the scene caches topology, display tessellation and CAD edge
//!   polylines ([`BodyCtx`]) and the last evaluation, keyed by a fingerprint
//!   of the document parts that affect the body (dirty tracking per body).

use crate::doc::{BodyEntry, Document, Layer, LayerId, PatchId, Rule};
use crate::eval::lines::{Bend, Patch, bend_class};
use crate::eval::{self, BodyCtx, BodyEval, Issue};
use crate::keys::{EdgeKey, RuleId, VertexKey};
use crate::mesh::{MeshStats, OutputMesh};
use crate::ops::{Op, OpOutcome, Refusal};
use anyhow::{Context, bail};
use glam::DVec3;
use newtopo_kernel::{
    BodyAttributes, BodyId, BodyTopology, CadKernel, FaceId, FaceMesh, HealReport,
};
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;

/// Undo depth (SPEC §4.7).
pub const UNDO_DEPTH: usize = 500;

struct BodyCache {
    ctx: BodyCtx,
    edge_polylines: Vec<Vec<DVec3>>,
}

pub struct Scene {
    pub kernel: Arc<dyn CadKernel>,
    doc: Arc<Document>,
    undo: Vec<Arc<Document>>,
    redo: Vec<Arc<Document>>,
    saved: Option<Arc<Document>>,
    /// Some(pushed) while coalescing: `pushed` says whether this gesture
    /// already pushed its undo snapshot.
    coalesce: Option<bool>,
    bodies: BTreeMap<BodyId, BodyCache>,
    evals: BTreeMap<BodyId, (u64, BodyEval)>,
    stashed: Option<(u64, BodyId, BodyEval)>,
    outputs: Vec<OutputMesh>,
}

/// A body as returned to the UI for drawing the CAD model.
pub struct BodyView<'a> {
    pub entry: &'a BodyEntry,
    pub topology: &'a BodyTopology,
    /// Display tessellation per face (index = FaceId).
    pub face_meshes: &'a [FaceMesh],
    /// Polyline per CAD edge (index = EdgeId), body coordinates.
    pub edge_polylines: &'a [Vec<glam::DVec3>],
}

/// A patch as shown to the UI (SPEC §4.2, §6.2).
#[derive(Clone, Debug, PartialEq)]
pub struct PatchInfo {
    pub id: PatchId,
    pub face: FaceId,
    pub bend: Bend,
    /// Approximate area (model units²).
    pub area: f64,
    /// The spread rule owning this patch, if any.
    pub spread_rule: Option<RuleId>,
    /// No outer boundary: implicit grid / count clamp apply.
    pub closed: bool,
    pub wraps: [bool; 2],
}

impl Scene {
    pub fn new(kernel: Arc<dyn CadKernel>) -> Self {
        Scene {
            kernel,
            doc: Arc::new(Document::default()),
            undo: vec![],
            redo: vec![],
            saved: None,
            coalesce: None,
            bodies: BTreeMap::new(),
            evals: BTreeMap::new(),
            stashed: None,
            outputs: vec![],
        }
    }

    pub fn doc(&self) -> &Document {
        &self.doc
    }

    /// The shared document snapshot (cheap clone).
    pub fn doc_arc(&self) -> Arc<Document> {
        self.doc.clone()
    }

    /// Add a body from B-rep bytes (from the import helper or a `.ntopo`).
    /// One undo step.
    pub fn add_body(
        &mut self,
        brep: &[u8],
        attrs: BodyAttributes,
        heal: HealReport,
    ) -> anyhow::Result<BodyId> {
        let kbody = self
            .kernel
            .load_brep(brep)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        self.add_kernel_body(kbody, attrs, heal, None)
    }

    /// Add a body already loaded into the kernel (tests, import helper
    /// in-process). One undo step.
    pub fn add_kernel_body(
        &mut self,
        kbody: BodyId,
        attrs: BodyAttributes,
        heal: HealReport,
        source_sha256: Option<String>,
    ) -> anyhow::Result<BodyId> {
        let ctx = BodyCtx::build(self.kernel.clone(), kbody).map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut doc = (*self.doc).clone();
        let id = BodyId(doc.fresh_id());
        let layer = match &attrs.layer {
            Some(name) => match doc.layers.iter().find(|l| &l.name == name) {
                Some(l) => l.id,
                None => {
                    let lid = LayerId(doc.fresh_id());
                    doc.layers.push(Layer {
                        id: lid,
                        name: name.clone(),
                        visible: true,
                        locked: false,
                    });
                    lid
                }
            },
            None => doc.active_layer,
        };
        doc.bodies.push(BodyEntry {
            id,
            name: if attrs.name.is_empty() {
                format!("Body {}", id.0)
            } else {
                attrs.name.clone()
            },
            layer,
            color: attrs.color,
            transform: attrs.transform,
            visible: true,
            locked: false,
            heal,
            source_sha256,
        });
        let edge_polylines = ctx.topo.edge_ids().map(|e| ctx.edge_polyline(e)).collect();
        self.bodies.insert(
            id,
            BodyCache {
                ctx,
                edge_polylines,
            },
        );
        self.commit(doc);
        Ok(id)
    }

    pub fn body_view(&self, body: BodyId) -> Option<BodyView<'_>> {
        let entry = self.doc.body(body)?;
        let c = self.bodies.get(&body)?;
        Some(BodyView {
            entry,
            topology: &c.ctx.topo,
            face_meshes: &c.ctx.face_meshes,
            edge_polylines: &c.edge_polylines,
        })
    }

    /// Static evaluation context of a body.
    pub fn ctx(&self, body: BodyId) -> Option<&BodyCtx> {
        self.bodies.get(&body).map(|c| &c.ctx)
    }

    fn commit(&mut self, doc: Document) {
        let old = std::mem::replace(&mut self.doc, Arc::new(doc));
        match self.coalesce {
            Some(true) => {}
            Some(false) => {
                self.undo.push(old);
                self.coalesce = Some(true);
            }
            None => self.undo.push(old),
        }
        if self.undo.len() > UNDO_DEPTH {
            let n = self.undo.len() - UNDO_DEPTH;
            self.undo.drain(0..n);
        }
        self.redo.clear();
    }

    /// Apply one op as one undo step. On refusal nothing changes. Every op
    /// that changes a body's evaluation is refused if the output would gain
    /// validation problems (SPEC §11: either the output validates or the op
    /// is refused with a reason).
    pub fn apply(&mut self, op: Op) -> Result<OpOutcome, Refusal> {
        let (doc, outcome) = self.run_op(&op)?;
        if doc == *self.doc {
            return Ok(outcome);
        }
        let ids: Vec<BodyId> = doc
            .bodies
            .iter()
            .map(|b| b.id)
            .filter(|b| self.bodies.contains_key(b))
            .collect();
        let mut fresh: Vec<(BodyId, u64, BodyEval)> = Vec::new();
        for b in ids {
            let fp_new = self.fingerprint_of(&doc, b);
            if self.evals.get(&b).is_some_and(|(f, _)| *f == fp_new) {
                continue;
            }
            let before = self.blocking_count(b);
            let ev = match self
                .stashed
                .take()
                .filter(|(f, bb, _)| *f == fp_new && *bb == b)
            {
                Some((_, _, ev)) => ev,
                None => match self.eval_doc(&doc, b) {
                    Some(e) => e,
                    None => continue,
                },
            };
            let issues: Vec<Issue> = eval::validate_body(&self.bodies[&b].ctx, &ev)
                .into_iter()
                .filter(|i| i.kind != eval::IssueKind::FanFallback)
                .collect();
            if issues.len() > before {
                return Err(Refusal::new(format!(
                    "can't do that: {}",
                    issue_reason(&issues[0])
                )));
            }
            fresh.push((b, fp_new, ev));
        }
        self.commit(doc);
        for (b, fp, ev) in fresh {
            self.evals.insert(b, (fp, ev));
        }
        Ok(outcome)
    }

    /// Apply an op without the validation gate (debugging, scripts that
    /// want to inspect an invalid result). Still one undo step.
    pub fn apply_unchecked(&mut self, op: Op) -> Result<OpOutcome, Refusal> {
        let (doc, outcome) = self.run_op(&op)?;
        if doc != *self.doc {
            self.commit(doc);
        }
        Ok(outcome)
    }

    fn blocking_count(&mut self, b: BodyId) -> usize {
        self.validate(b)
            .into_iter()
            .filter(|i| i.kind != eval::IssueKind::FanFallback)
            .count()
    }

    /// Keep an evaluation computed for a candidate document (saves the
    /// second evaluation in `apply`).
    pub(crate) fn stash_eval(&mut self, doc: &Document, body: BodyId, ev: BodyEval) {
        let fp = self.fingerprint_of(doc, body);
        self.stashed = Some((fp, body, ev));
    }

    /// Start merging subsequent ops into one undo step (a wheel gesture).
    pub fn begin_coalesce(&mut self) {
        if self.coalesce.is_none() {
            self.coalesce = Some(false);
        }
    }

    /// End the current coalesced gesture (call once the wheel has been idle
    /// for 400 ms, SPEC §4.7).
    pub fn end_coalesce(&mut self) {
        self.coalesce = None;
    }

    /// Apply an op as part of the current gesture (starts one if needed).
    pub fn apply_coalesced(&mut self, op: Op) -> Result<OpOutcome, Refusal> {
        self.begin_coalesce();
        self.apply(op)
    }

    pub fn undo(&mut self) -> bool {
        self.coalesce = None;
        match self.undo.pop() {
            Some(d) => {
                let cur = std::mem::replace(&mut self.doc, d);
                self.redo.push(cur);
                true
            }
            None => false,
        }
    }
    pub fn redo(&mut self) -> bool {
        self.coalesce = None;
        match self.redo.pop() {
            Some(d) => {
                let cur = std::mem::replace(&mut self.doc, d);
                self.undo.push(cur);
                true
            }
            None => false,
        }
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    /// True if the document changed since the last save/load.
    pub fn is_modified(&self) -> bool {
        match &self.saved {
            Some(s) => !Arc::ptr_eq(s, &self.doc) && **s != *self.doc,
            None => self.can_undo() || !self.doc.bodies.is_empty(),
        }
    }

    fn fingerprint(&self, body: BodyId) -> u64 {
        self.fingerprint_of(&self.doc, body)
    }

    fn fingerprint_of(&self, d: &Document, body: BodyId) -> u64 {
        let Some(c) = self.bodies.get(&body) else {
            return 0;
        };
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let rules: Vec<&crate::doc::RuleEntry> = d
            .rules
            .iter()
            .filter(|r| !eval::rule_faces(&r.rule, body, &c.ctx).is_empty())
            .collect();
        let splits: Vec<&crate::doc::SplitLine> =
            d.split_lines.iter().filter(|s| s.body == body).collect();
        let edits: Vec<&crate::doc::EditEntry> =
            d.edits.iter().filter(|e| e.edit.body() == body).collect();
        let phases: Vec<_> = d
            .phase_overrides
            .iter()
            .filter(|(k, _)| matches!(k, crate::doc::PhaseTarget::Face(b, _) | crate::doc::PhaseTarget::Edge(b, _) if *b == body))
            .collect();
        serde_json::to_string(&(
            rules,
            splits,
            edits,
            phases,
            d.settings.autofill,
            d.settings.sharp_angle_deg,
            d.settings.min_closed_segments,
        ))
        .unwrap_or_default()
        .hash(&mut h);
        h.finish()
    }

    /// Evaluate one body (cached).
    pub fn eval(&mut self, body: BodyId) -> Option<&BodyEval> {
        let fp = self.fingerprint(body);
        let fresh = self.evals.get(&body).is_some_and(|(f, _)| *f == fp);
        if !fresh {
            let c = self.bodies.get(&body)?;
            let ev = eval::evaluate(&c.ctx, &self.doc, body);
            self.evals.insert(body, (fp, ev));
        }
        self.evals.get(&body).map(|(_, e)| e)
    }

    /// Evaluate a body for a candidate document (no caching).
    pub(crate) fn eval_doc(&self, doc: &Document, body: BodyId) -> Option<BodyEval> {
        let c = self.bodies.get(&body)?;
        Some(eval::evaluate(&c.ctx, doc, body))
    }

    /// Evaluated output for every body (lazy; only dirty bodies re-evaluate).
    pub fn output(&mut self) -> &[OutputMesh] {
        let ids: Vec<BodyId> = self.doc.bodies.iter().map(|b| b.id).collect();
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(ev) = self.eval(id) {
                out.push(ev.mesh.clone());
            }
        }
        self.outputs = out;
        &self.outputs
    }

    /// Output of one body.
    pub fn output_of(&mut self, body: BodyId) -> Option<&OutputMesh> {
        self.eval(body).map(|e| &e.mesh)
    }

    pub fn stats(&mut self) -> MeshStats {
        let mut s = MeshStats::default();
        for m in self.output() {
            s += m.stats();
        }
        s
    }

    /// Edits whose keys no longer exist (SPEC §4.5), by edit id.
    pub fn dormant_edits(&mut self) -> Vec<u32> {
        let ids: Vec<BodyId> = self.doc.bodies.iter().map(|b| b.id).collect();
        let mut out = Vec::new();
        for id in ids {
            if let Some(ev) = self.eval(id) {
                out.extend(ev.dormant.iter().copied());
            }
        }
        out.sort();
        out
    }

    /// Dormant edits with the reason each one doesn't apply.
    pub fn dormant_reasons(&mut self) -> Vec<(u32, String)> {
        let ids: Vec<BodyId> = self.doc.bodies.iter().map(|b| b.id).collect();
        let mut out = Vec::new();
        for id in ids {
            if let Some(ev) = self.eval(id) {
                out.extend(ev.dormant_reasons.iter().cloned());
            }
        }
        out
    }

    /// Validation issues of a body's output (SPEC §4.6 step 8).
    pub fn validate(&mut self, body: BodyId) -> Vec<Issue> {
        let Some(ev) = self.eval(body).cloned() else {
            return vec![];
        };
        let c = &self.bodies[&body];
        eval::validate_body(&c.ctx, &ev)
    }

    /// Validation issues of every body, excluding informational ones.
    pub fn validate_all(&mut self) -> Vec<(BodyId, Issue)> {
        let ids: Vec<BodyId> = self.doc.bodies.iter().map(|b| b.id).collect();
        let mut out = Vec::new();
        for id in ids {
            for i in self.validate(id) {
                if i.kind != eval::IssueKind::FanFallback {
                    out.push((id, i));
                }
            }
        }
        out
    }

    /// Patches of a body with bend class and owning spread rule.
    pub fn patches(&mut self, body: BodyId) -> Vec<PatchInfo> {
        let Some(ev) = self.eval(body).cloned() else {
            return vec![];
        };
        let c = &self.bodies[&body];
        ev.patches
            .iter()
            .map(|p| PatchInfo {
                id: p.id,
                face: p.face,
                bend: bend_class(&c.ctx, c.ctx.face(p.face)),
                area: p.area,
                spread_rule: self.rule_on_patch(&p.id),
                closed: p.closed_patch(c.ctx.face(p.face)),
                wraps: p.wraps,
            })
            .collect()
    }

    /// Raw patch records (with UV extents) of a body.
    pub fn patch_records(&mut self, body: BodyId) -> Vec<Patch> {
        self.eval(body)
            .map(|e| e.patches.clone())
            .unwrap_or_default()
    }

    /// The spread rule that owns a patch (SPEC §4.3).
    pub fn rule_on_patch(&self, p: &PatchId) -> Option<RuleId> {
        self.doc
            .rules
            .iter()
            .find(|r| rule_owns(&r.rule, p))
            .map(|r| r.id)
    }

    /// Nearest output vertex of a body to a point (body coordinates).
    pub fn nearest_output_vertex(&mut self, body: BodyId, p: DVec3) -> Option<(u32, VertexKey)> {
        let m = self.output_of(body)?;
        m.vertices
            .iter()
            .enumerate()
            .min_by(|a, b| {
                (a.1.position - p)
                    .length()
                    .total_cmp(&(b.1.position - p).length())
            })
            .map(|(i, v)| (i as u32, v.key.clone()))
    }

    /// Nearest output edge (index into `OutputMesh::edges`, and its key).
    pub fn nearest_output_edge(
        &mut self,
        body: BodyId,
        p: DVec3,
    ) -> Option<(usize, Option<EdgeKey>)> {
        let m = self.output_of(body)?;
        let mut best: Option<(f64, usize)> = None;
        for (i, e) in m.edges.iter().enumerate() {
            for w in e.path.windows(2) {
                let (a, b) = (w[0], w[1]);
                let ab = b - a;
                let t = if ab.length_squared() > 0.0 {
                    ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let d = (a + ab * t - p).length();
                if best.is_none_or(|(bd, _)| d < bd) {
                    best = Some((d, i));
                }
            }
        }
        best.map(|(_, i)| (i, m.edges[i].key.clone()))
    }

    /// Save as `.ntopo` (SPEC §8.2): zip with `scene.json`,
    /// `cad/<body>.bbrep` and `cad/source.sha256`.
    pub fn save(&mut self, path: &Path) -> anyhow::Result<()> {
        let file =
            std::fs::File::create(path).with_context(|| format!("create {}", path.display()))?;
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("scene.json", opts)?;
        zip.write_all(serde_json::to_string_pretty(&*self.doc)?.as_bytes())?;
        let mut shas = String::new();
        for b in &self.doc.bodies {
            let c = self.bodies.get(&b.id).context("body without kernel data")?;
            let bytes = self
                .kernel
                .save_brep(c.ctx.kbody)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            zip.start_file(format!("cad/{}.bbrep", b.id.0), opts)?;
            zip.write_all(&bytes)?;
            if let Some(s) = &b.source_sha256 {
                shas.push_str(&format!("{} {}\n", b.id.0, s));
            }
        }
        zip.start_file("cad/source.sha256", opts)?;
        zip.write_all(shas.as_bytes())?;
        zip.finish()?;
        self.saved = Some(self.doc.clone());
        Ok(())
    }

    /// Load a `.ntopo` into a fresh scene. Newer formats are refused.
    pub fn load(kernel: Arc<dyn CadKernel>, path: &Path) -> anyhow::Result<Scene> {
        let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
        let mut zip = zip::ZipArchive::new(file).context("not a .ntopo archive")?;
        let mut json = String::new();
        zip.by_name("scene.json")
            .context("scene.json missing")?
            .read_to_string(&mut json)?;
        let raw: serde_json::Value = serde_json::from_str(&json)?;
        let ver = raw
            .get("format_version")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        if ver > crate::doc::FORMAT_VERSION {
            bail!(
                "this scene was saved by a newer version of newtopo (format {ver}, this build reads up to {}); please update the app",
                crate::doc::FORMAT_VERSION
            );
        }
        let doc: Document = migrate(raw)?;
        let mut scene = Scene::new(kernel);
        for b in &doc.bodies {
            let mut bytes = Vec::new();
            zip.by_name(&format!("cad/{}.bbrep", b.id.0))
                .with_context(|| format!("cad/{}.bbrep missing", b.id.0))?
                .read_to_end(&mut bytes)?;
            let kbody = scene
                .kernel
                .load_brep(&bytes)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let ctx =
                BodyCtx::build(scene.kernel.clone(), kbody).map_err(|e| anyhow::anyhow!("{e}"))?;
            let edge_polylines = ctx.topo.edge_ids().map(|e| ctx.edge_polyline(e)).collect();
            scene.bodies.insert(
                b.id,
                BodyCache {
                    ctx,
                    edge_polylines,
                },
            );
        }
        scene.doc = Arc::new(doc);
        scene.saved = Some(scene.doc.clone());
        Ok(scene)
    }

    /// Replace the document wholesale as one undo step (used by import
    /// replace flows and scripts). Bodies must already be known.
    pub fn set_document(&mut self, doc: Document) {
        self.commit(doc);
    }

    /// Bodies known to the scene (document order).
    pub fn body_ids(&self) -> Vec<BodyId> {
        self.doc.bodies.iter().map(|b| b.id).collect()
    }

    /// Per-rule generated line counts of a body (pieces), for panels.
    pub fn lines_per_rule(&mut self, body: BodyId) -> HashMap<RuleId, u32> {
        self.eval(body)
            .map(|e| e.lines_per_rule.clone())
            .unwrap_or_default()
    }
}

/// Older formats → current (SPEC §8.2). Format 1 is current.
fn migrate(raw: serde_json::Value) -> anyhow::Result<Document> {
    Ok(serde_json::from_value(raw)?)
}

/// Does a spread rule own this patch?
pub fn rule_owns(r: &Rule, p: &PatchId) -> bool {
    match r {
        Rule::Connect { patches, .. } => patches.contains(p),
        Rule::Grid { patch, .. } => patch == p,
        Rule::Slice {
            scope: crate::doc::SliceScope::Body(b),
            ..
        } => *b == p.body,
        Rule::Slice {
            scope: crate::doc::SliceScope::Patches(list),
            ..
        } => list.contains(p),
        _ => false,
    }
}

/// Toast text for a validation issue that blocks an op.
pub fn issue_reason(i: &Issue) -> &'static str {
    use eval::IssueKind::*;
    match i.kind {
        Flipped => "a face would flip against the CAD surface",
        NonManifold => "the mesh would become non-manifold",
        Degenerate | TooFewVertices => "a face would become degenerate",
        ZeroLengthEdge => "an edge would get zero length",
        NotSimple | RepeatedVertex => "a polygon could not be made simple",
        OpenBoundary | MissingEdge => "the mesh would get a hole",
        AnchorOff => "a vertex would leave its CAD anchor",
        FanFallback => "a polygon could only be fan-filled",
    }
}
