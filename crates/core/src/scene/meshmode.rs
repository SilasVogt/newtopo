//! Mesh mode on the scene (SPEC §9.5): edited meshes in the document,
//! the CAD lock on edited bodies, and the effective output (the edited mesh
//! where there is one, the evaluated mesh elsewhere).
//!
//! An edited mesh is part of the document, so every mesh edit is an
//! ordinary undo step and undo/redo work the same in CAD and mesh mode.
//! Entering mesh mode changes nothing: [`Scene::editable_mesh`] bakes the
//! evaluated output on demand (cached), and only the first edit writes the
//! mesh into the document, which then locks the body's CAD stage until
//! [`Scene::discard_mesh_edits`].

use super::Scene;
use crate::doc::{Document, MeshRef};
use crate::eval::{self, Issue, IssueKind};
use crate::mesh::OutputMesh;
use crate::meshedit::EditMesh;
use crate::ops::Refusal;
use glam::DVec3;
use newtopo_kernel::{BodyId, FaceId};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

impl Scene {
    /// The body has mesh edits (its CAD stage is locked).
    pub fn has_mesh_edits(&self, body: BodyId) -> bool {
        self.doc.meshes.contains_key(&body)
    }

    /// Bodies with mesh edits.
    pub fn mesh_edited_bodies(&self) -> Vec<BodyId> {
        self.doc.meshes.keys().copied().collect()
    }

    /// The body's edited mesh, if it has mesh edits.
    pub fn mesh_edit(&self, body: BodyId) -> Option<Arc<EditMesh>> {
        self.doc.meshes.get(&body).map(|m| m.0.clone())
    }

    /// The mesh mesh mode edits for this body: its edited mesh, or the
    /// evaluated output baked into an [`EditMesh`] (cached until the
    /// body's rules change). Refused if the body has no valid output.
    pub fn editable_mesh(&mut self, body: BodyId) -> Result<Arc<EditMesh>, Refusal> {
        if let Some(m) = self.doc.meshes.get(&body) {
            return Ok(m.0.clone());
        }
        let fp = self.fingerprint(body);
        if let Some((f, m)) = self.baked.get(&body)
            && *f == fp
        {
            return Ok(m.clone());
        }
        let name = self.body_name(body);
        let out = self
            .eval(body)
            .map(|e| e.mesh.clone())
            .ok_or_else(|| Refusal::new(format!("There is no object {}", body.0)))?;
        if out.polygons.is_empty() {
            return Err(Refusal::new(format!(
                "\"{name}\" has no mesh yet: add lines in CAD mode first"
            )));
        }
        let m = Arc::new(EditMesh::bake(&out)?);
        self.baked.insert(body, (fp, m.clone()));
        Ok(m)
    }

    /// Store edited meshes (`Some`) or drop mesh edits (`None`) as one undo
    /// step. Every new mesh must be valid ([`EditMesh::issues`]) and belong
    /// to its body. Afterwards [`Scene::amend_meshes`] may replace this
    /// step while nothing else has changed (live tool parameters).
    pub fn set_meshes(&mut self, changes: Vec<(BodyId, Option<EditMesh>)>) -> Result<(), Refusal> {
        let doc = self.doc_with_meshes(changes)?;
        if doc == *self.doc {
            return Ok(());
        }
        self.coalesce = None;
        self.commit(doc);
        self.amend_mark = Some(self.doc.clone());
        Ok(())
    }

    /// Replace the last [`Scene::set_meshes`] step without a new undo step
    /// (a tool re-run with new parameters from the same starting mesh).
    /// Refused once anything else changed the document since.
    pub fn amend_meshes(
        &mut self,
        changes: Vec<(BodyId, Option<EditMesh>)>,
    ) -> Result<(), Refusal> {
        if !self.can_amend_meshes() {
            return Err(Refusal::new(
                "The last mesh edit can't be adjusted any more: something changed since",
            ));
        }
        let doc = self.doc_with_meshes(changes)?;
        self.doc = Arc::new(doc);
        self.redo.clear();
        self.amend_mark = Some(self.doc.clone());
        Ok(())
    }

    /// The document is still exactly what the last [`Scene::set_meshes`]
    /// or [`Scene::amend_meshes`] left (so the step can be amended).
    pub fn can_amend_meshes(&self) -> bool {
        self.preview.is_none()
            && !self.undo.is_empty()
            && self
                .amend_mark
                .as_ref()
                .is_some_and(|d| Arc::ptr_eq(d, &self.doc))
    }

    /// Drop the mesh edits of these bodies (one undo step): they return to
    /// their CAD-driven mesh and are CAD-editable again. Returns how many
    /// bodies had edits.
    pub fn discard_mesh_edits(&mut self, bodies: &[BodyId]) -> Result<usize, Refusal> {
        let changes: Vec<(BodyId, Option<EditMesh>)> = bodies
            .iter()
            .filter(|b| self.has_mesh_edits(**b))
            .map(|b| (*b, None))
            .collect();
        let n = changes.len();
        if n > 0 {
            self.set_meshes(changes)?;
        }
        Ok(n)
    }

    fn doc_with_meshes(
        &self,
        changes: Vec<(BodyId, Option<EditMesh>)>,
    ) -> Result<Document, Refusal> {
        if self.preview.is_some() {
            return Err(Refusal::new("Apply or discard the preview first"));
        }
        let mut doc = (*self.doc).clone();
        for (body, mesh) in changes {
            let Some(c) = self.bodies.get(&body) else {
                return Err(Refusal::new(format!("There is no object {}", body.0)));
            };
            match mesh {
                None => {
                    doc.meshes.remove(&body);
                }
                Some(mut m) => {
                    m.body = body;
                    check_mesh(&m, c.ctx.topo.faces.len())
                        .map_err(|e| Refusal::new(format!("Can't keep that mesh: {e}")))?;
                    doc.meshes.insert(body, MeshRef::new(m));
                }
            }
        }
        Ok(doc)
    }

    /// The CAD lock (SPEC §9.5): an op may not change the rules, split
    /// lines, edits or phase overrides of a body with mesh edits.
    pub(crate) fn check_mesh_locks(&self, new: &Document) -> Result<(), Refusal> {
        for &b in self.doc.meshes.keys() {
            if self.cad_fingerprint(&self.doc, b) != self.cad_fingerprint(new, b) {
                return Err(self.mesh_lock_refusal(b));
            }
        }
        Ok(())
    }

    /// The refusal for an op that would change the CAD stage of `body`,
    /// which has mesh edits.
    pub(crate) fn mesh_lock_refusal(&self, body: BodyId) -> Refusal {
        Refusal::new(format!(
            "\"{}\" has mesh edits, so its CAD lines are locked. Run \"Discard mesh edits\" to edit it in CAD mode again.",
            self.body_name(body)
        ))
    }

    /// Hash of the document parts that make up one body's CAD stage
    /// (rules on its faces, split lines, edits, phase overrides).
    fn cad_fingerprint(&self, d: &Document, body: BodyId) -> u64 {
        let Some(c) = self.bodies.get(&body) else {
            return 0;
        };
        let rules: Vec<&crate::doc::RuleEntry> = d
            .rules
            .iter()
            .filter(|r| !eval::rule_faces(&r.rule, body, &c.ctx).is_empty())
            .collect();
        let splits: Vec<_> = d.split_lines.iter().filter(|s| s.body == body).collect();
        let edits: Vec<_> = d.edits.iter().filter(|e| e.edit.body() == body).collect();
        let phases: Vec<_> = d
            .phase_overrides
            .iter()
            .filter(|(k, _)| matches!(k, crate::doc::PhaseTarget::Face(b, _) | crate::doc::PhaseTarget::Edge(b, _) if *b == body))
            .collect();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        serde_json::to_string(&(rules, splits, edits, phases))
            .unwrap_or_default()
            .hash(&mut h);
        h.finish()
    }

    pub(crate) fn body_name(&self, body: BodyId) -> String {
        self.doc
            .body(body)
            .map(|b| b.name.clone())
            .unwrap_or_else(|| format!("Body {}", body.0))
    }

    /// The edited mesh as output (cached per mesh).
    pub(crate) fn mesh_output(&mut self, body: BodyId) -> Option<OutputMesh> {
        let m = self.doc.meshes.get(&body)?.0.clone();
        if let Some((k, o)) = self.mesh_outputs.get(&body)
            && Arc::ptr_eq(k, &m)
        {
            return Some(o.clone());
        }
        let o = m.to_output();
        self.mesh_outputs.insert(body, (m, o.clone()));
        Some(o)
    }

    /// What the body exports: its edited mesh if it has one, otherwise its
    /// evaluated output.
    pub fn effective_output_of(&mut self, body: BodyId) -> Option<OutputMesh> {
        if self.has_mesh_edits(body) {
            return self.mesh_output(body);
        }
        self.eval(body).map(|e| e.mesh.clone())
    }

    /// Validation issues of an edited mesh, in the evaluator's terms.
    pub(crate) fn mesh_issues(&self, body: BodyId) -> Vec<Issue> {
        self.doc
            .meshes
            .get(&body)
            .map(|m| {
                m.issues()
                    .into_iter()
                    .map(|message| Issue {
                        kind: IssueKind::NonManifold,
                        message,
                        polygon: None,
                        vertices: vec![],
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Closest points on a CAD face (body coordinates), for mesh tools'
    /// "Stay on CAD surface". `None` where the kernel finds none.
    pub fn project_to_cad(&self, body: BodyId, face: FaceId, pts: &[DVec3]) -> Vec<Option<DVec3>> {
        let Some(c) = self.bodies.get(&body) else {
            return vec![None; pts.len()];
        };
        if face.idx() >= c.ctx.topo.faces.len() {
            return vec![None; pts.len()];
        }
        match c.ctx.kernel.project_to_face(c.ctx.kbody, face, pts) {
            Ok(v) if v.len() == pts.len() => v.into_iter().map(|o| o.map(|(_, p)| p)).collect(),
            _ => vec![None; pts.len()],
        }
    }
}

/// Checks for an edited mesh from outside the tools (a loaded file, a
/// caller-built mesh): structurally valid and every group a face of the
/// body.
pub(crate) fn check_mesh(m: &EditMesh, faces: usize) -> anyhow::Result<()> {
    if m.polygons.is_empty() {
        anyhow::bail!("the mesh is empty");
    }
    if let Some(p) = m.polygons.iter().find(|p| p.group.idx() >= faces) {
        anyhow::bail!(
            "a polygon names CAD face {}, which the body doesn't have",
            p.group.0
        );
    }
    if let Some(first) = m.issues().first() {
        anyhow::bail!("{first}");
    }
    Ok(())
}
