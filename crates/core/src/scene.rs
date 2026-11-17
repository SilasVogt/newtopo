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

mod check;
mod ext;
pub(crate) use check::work_budget;
pub use check::{MAX_RULE_COUNT, MAX_RULE_CROSSINGS};
pub use ext::{BendArrow, BendInfo, ReplaceReport};

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
    links: HashMap<BodyId, crate::link::LinkSets>,
    mirrors: HashMap<(BodyId, crate::doc::Axis), crate::link::FaceMap>,
    caches: std::sync::Mutex<HashMap<BodyId, eval::cache::EvalCache>>,
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
            links: HashMap::new(),
            mirrors: HashMap::new(),
            caches: std::sync::Mutex::new(HashMap::new()),
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
        let r = self.add_kernel_body(kbody, attrs, heal, None);
        if r.is_err() && !self.bodies.contains_key(&kbody) {
            self.kernel.unload(kbody);
        }
        r
    }

    /// Add a body already loaded into the kernel (tests, import helper
    /// in-process). One undo step. On success the scene owns the kernel
    /// body and unloads it when dropped; on failure the caller still owns
    /// it.
    pub fn add_kernel_body(
        &mut self,
        kbody: BodyId,
        attrs: BodyAttributes,
        heal: HealReport,
        source_sha256: Option<String>,
    ) -> anyhow::Result<BodyId> {
        let ctx = BodyCtx::build(self.kernel.clone(), kbody).map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut doc = (*self.doc).clone();
        // Document body ids equal kernel body ids (the app queries the
        // kernel with them).
        let id = kbody;
        if doc.body(id).is_some() {
            bail!("body id {id} is already in the scene");
        }
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
        self.gate_and_commit(doc)?;
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

    /// Apply several ops as one undo step, all or nothing (the result of the
    /// whole batch must validate).
    pub fn apply_all(&mut self, ops: Vec<Op>) -> Result<Vec<OpOutcome>, Refusal> {
        let orig = self.doc.clone();
        let mut outs = Vec::with_capacity(ops.len());
        for op in &ops {
            match self.run_op(op) {
                Ok((d, o)) => {
                    self.doc = Arc::new(d);
                    outs.push(o);
                }
                Err(e) => {
                    self.doc = orig;
                    self.stashed = None;
                    return Err(e);
                }
            }
        }
        let fin = (*self.doc).clone();
        self.doc = orig;
        self.stashed = None;
        self.gate_and_commit(fin)?;
        Ok(outs)
    }

    /// Validation gate shared by [`Scene::apply`] and [`Scene::apply_all`].
    fn gate_and_commit(&mut self, doc: Document) -> Result<(), Refusal> {
        if doc == *self.doc {
            return Ok(());
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
        Ok(())
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
            let mut caches = self.caches.lock().unwrap();
            let cache = caches.entry(body).or_default();
            let ev = eval::evaluate_with(&c.ctx, &self.doc, body, cache);
            drop(caches);
            self.evals.insert(body, (fp, ev));
        }
        self.evals.get(&body).map(|(_, e)| e)
    }

    /// Evaluate a body for a candidate document (no caching).
    pub(crate) fn eval_doc(&mut self, doc: &Document, body: BodyId) -> Option<BodyEval> {
        let c = self.bodies.get(&body)?;
        let mut caches = self.caches.lock().unwrap();
        let cache = caches.entry(body).or_default();
        Some(eval::evaluate_with(&c.ctx, doc, body, cache))
    }

    /// Patches of a body for a document (stage 1 only, cached).
    pub(crate) fn patches_cached(
        &self,
        doc: &Document,
        body: BodyId,
    ) -> Vec<crate::eval::lines::Patch> {
        let Some(c) = self.bodies.get(&body) else {
            return vec![];
        };
        let mut caches = self.caches.lock().unwrap();
        let cache = caches.entry(body).or_default();
        eval::body_patches_with(&c.ctx, doc, body, cache)
    }

    /// Cache hits / misses of the last evaluation of a body, per level
    /// (face patches, face arrangement, region bridging+fill).
    pub fn cache_stats(&self, body: BodyId) -> Option<[(usize, usize); 3]> {
        self.caches.lock().unwrap().get(&body).map(|c| c.last)
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
        self.write_ntopo(path)?;
        self.saved = Some(self.doc.clone());
        Ok(())
    }

    /// Write a copy (autosave): like [`Scene::save`] but the modified marker
    /// stays as it is.
    pub fn save_copy(&mut self, path: &Path) -> anyhow::Result<()> {
        self.write_ntopo(path)
    }

    /// Write the archive to a temporary sibling of `path`, sync it, then
    /// rename it over `path`, so a failed save never destroys the previous
    /// file. `persist` replaces an existing destination on every platform
    /// (`rename(2)` on Unix, `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` on
    /// Windows). The temporary file is removed if anything fails.
    fn write_ntopo(&self, path: &Path) -> anyhow::Result<()> {
        let dir = match path.parent() {
            Some(d) if !d.as_os_str().is_empty() => d,
            _ => Path::new("."),
        };
        let name = path
            .file_name()
            .with_context(|| format!("{} is not a file path", path.display()))?;
        let mut prefix = std::ffi::OsString::from(".");
        prefix.push(name);
        prefix.push(".");
        let mut builder = tempfile::Builder::new();
        builder.prefix(&prefix).suffix(".tmp");
        // Temp files default to 0600; a saved scene should get the usual
        // permissions: the existing file's mode when replacing it, otherwise
        // 0666 filtered by the process umask (like `File::create`).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(path)
                .map(|m| m.permissions().mode() & 0o7777)
                .unwrap_or(0o666);
            builder.permissions(std::fs::Permissions::from_mode(mode));
        }
        let tmp = builder
            .tempfile_in(dir)
            .with_context(|| format!("create a temporary file in {}", dir.display()))?;
        let file = self.write_zip(tmp.reopen()?)?;
        file.sync_all()
            .with_context(|| format!("sync {}", tmp.path().display()))?;
        drop(file);
        tmp.persist(path)
            .map_err(|e| e.error)
            .with_context(|| format!("replace {}", path.display()))?;
        #[cfg(unix)]
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all();
        }
        Ok(())
    }

    fn write_zip(&self, file: std::fs::File) -> anyhow::Result<std::fs::File> {
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
        Ok(zip.finish()?)
    }

    /// Load a `.ntopo` into a fresh scene. Newer formats are refused, and
    /// so are archives over the default [`LoadLimits`] and scenes whose
    /// references do not match their CAD data.
    pub fn load(kernel: Arc<dyn CadKernel>, path: &Path) -> anyhow::Result<Scene> {
        Self::load_with_limits(kernel, path, &LoadLimits::default())
    }

    /// [`Scene::load`] with explicit size limits for untrusted archives.
    pub fn load_with_limits(
        kernel: Arc<dyn CadKernel>,
        path: &Path,
        limits: &LoadLimits,
    ) -> anyhow::Result<Scene> {
        let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
        let mut zip = zip::ZipArchive::new(file).context("not a .ntopo archive")?;
        if zip.len() > limits.max_entries {
            bail!(
                "the archive has {} entries (at most {} allowed)",
                zip.len(),
                limits.max_entries
            );
        }
        let mut budget = limits.max_total;
        let json = read_entry(&mut zip, "scene.json", limits.max_scene_json, &mut budget)?;
        let json = String::from_utf8(json).context("scene.json is not UTF-8")?;
        let raw: serde_json::Value = serde_json::from_str(&json)?;
        drop(json);
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
        let mut doc: Document = migrate(raw)?;
        let mut scene = Scene::new(kernel);
        let mut remap: BTreeMap<BodyId, BodyId> = BTreeMap::new();
        for b in &doc.bodies {
            if remap.contains_key(&b.id) {
                bail!("body {} is listed twice", b.id.0);
            }
            let bytes = read_entry(
                &mut zip,
                &format!("cad/{}.bbrep", b.id.0),
                limits.max_brep,
                &mut budget,
            )?;
            let kbody = scene
                .kernel
                .load_brep(&bytes)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            drop(bytes);
            // Bodies already in `scene.bodies` are unloaded when `scene`
            // drops on an early return; this one is not there yet.
            let ctx = match BodyCtx::build(scene.kernel.clone(), kbody) {
                Ok(c) => c,
                Err(e) => {
                    scene.kernel.unload(kbody);
                    bail!("{e}");
                }
            };
            let edge_polylines = ctx.topo.edge_ids().map(|e| ctx.edge_polyline(e)).collect();
            remap.insert(b.id, kbody);
            scene.bodies.insert(
                kbody,
                BodyCache {
                    ctx,
                    edge_polylines,
                },
            );
        }
        if remap.iter().any(|(a, b)| a != b) {
            doc.remap_bodies(&remap);
        }
        scene
            .check_document(&doc)
            .context("the scene does not match its CAD data")?;
        scene.doc = Arc::new(doc);
        scene.saved = Some(scene.doc.clone());
        Ok(scene)
    }

    /// Check that every body, face, edge and vertex a document names exists
    /// in this scene's CAD data.
    pub fn check_document(&self, doc: &Document) -> anyhow::Result<()> {
        check::check_document(doc, |b| self.bodies.get(&b).map(|c| &c.ctx.topo))
    }

    /// Replace the document wholesale as one undo step (used by import
    /// replace flows and scripts). Bodies must already be known. Only for
    /// documents built in-process; use [`Scene::try_set_document`] for
    /// anything read from outside.
    pub fn set_document(&mut self, doc: Document) {
        debug_assert!(
            self.check_document(&doc).is_ok(),
            "set_document: {:?}",
            self.check_document(&doc)
        );
        self.commit(doc);
    }

    /// [`Scene::set_document`] for untrusted documents: refuses one whose
    /// references do not match the loaded CAD data.
    pub fn try_set_document(&mut self, doc: Document) -> anyhow::Result<()> {
        self.check_document(&doc)?;
        self.commit(doc);
        Ok(())
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

/// A scene owns the kernel bodies it loaded (or was handed through
/// [`Scene::add_kernel_body`]) and unloads them when dropped, so opening a
/// new scene does not leak the old model's native data. Bodies of other
/// scenes sharing the kernel are untouched.
impl Drop for Scene {
    fn drop(&mut self) {
        for c in self.bodies.values() {
            self.kernel.unload(c.ctx.kbody);
        }
    }
}

/// Size limits for reading a `.ntopo` (an untrusted zip archive).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadLimits {
    /// Entries in the archive.
    pub max_entries: usize,
    /// Decompressed bytes of `scene.json`.
    pub max_scene_json: u64,
    /// Decompressed bytes of each `cad/<body>.bbrep`.
    pub max_brep: u64,
    /// Decompressed bytes read in all.
    pub max_total: u64,
}

impl Default for LoadLimits {
    fn default() -> Self {
        LoadLimits {
            max_entries: 4096,
            max_scene_json: 64 << 20,
            max_brep: 1 << 30,
            max_total: 4 << 30,
        }
    }
}

/// Read one archive entry, refusing it once it decompresses past `limit` or
/// past what is left of the shared `budget`. The size the archive declares
/// is only used to refuse early; the read itself is bounded.
fn read_entry(
    zip: &mut zip::ZipArchive<std::fs::File>,
    name: &str,
    limit: u64,
    budget: &mut u64,
) -> anyhow::Result<Vec<u8>> {
    let cap = limit.min(*budget);
    let entry = zip
        .by_name(name)
        .with_context(|| format!("{name} missing"))?;
    let too_big = || {
        if cap < limit {
            anyhow::anyhow!("the archive decompresses to more than {} bytes", *budget)
        } else {
            anyhow::anyhow!("{name} is larger than {limit} bytes")
        }
    };
    if entry.size() > cap {
        return Err(too_big());
    }
    let mut out = Vec::with_capacity(entry.size().min(64 << 20) as usize);
    entry
        .take(cap + 1)
        .read_to_end(&mut out)
        .with_context(|| format!("read {name}"))?;
    if out.len() as u64 > cap {
        return Err(too_big());
    }
    *budget -= out.len() as u64;
    Ok(out)
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
