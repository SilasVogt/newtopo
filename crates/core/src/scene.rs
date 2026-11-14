//! The `Scene` facade: document + undo + kernel + evaluation cache. The app,
//! the CLI and tests all drive the core through this type.
//!
//! OWNER: engine. Signatures here are the contract the app and CLI build
//! against; keep them stable (add methods freely).

use crate::doc::{BodyEntry, Document};
use crate::mesh::{MeshStats, OutputMesh};
use crate::ops::{Op, OpOutcome, Refusal};
use newtopo_kernel::{BodyAttributes, BodyId, BodyTopology, CadKernel, FaceMesh, HealReport};
use std::path::Path;
use std::sync::Arc;

pub struct Scene {
    pub kernel: Arc<dyn CadKernel>,
    // engine: private fields (document Arc, undo stack, topology cache,
    // display tessellation cache, evaluation cache, dirty tracking).
    _private: (),
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

impl Scene {
    pub fn new(kernel: Arc<dyn CadKernel>) -> Self {
        let _ = kernel;
        todo!("engine")
    }

    pub fn doc(&self) -> &Document {
        todo!("engine")
    }

    /// Add a body from B-rep bytes (from the import helper or a `.ntopo`).
    pub fn add_body(&mut self, brep: &[u8], attrs: BodyAttributes, heal: HealReport) -> anyhow::Result<BodyId> {
        let _ = (brep, attrs, heal);
        todo!("engine")
    }

    pub fn body_view(&self, body: BodyId) -> Option<BodyView<'_>> {
        let _ = body;
        todo!("engine")
    }

    /// Apply one op as one undo step. On refusal nothing changes.
    pub fn apply(&mut self, op: Op) -> Result<OpOutcome, Refusal> {
        let _ = op;
        todo!("engine")
    }

    pub fn undo(&mut self) -> bool {
        todo!("engine")
    }
    pub fn redo(&mut self) -> bool {
        todo!("engine")
    }
    pub fn can_undo(&self) -> bool {
        todo!("engine")
    }
    pub fn can_redo(&self) -> bool {
        todo!("engine")
    }
    /// True if the document changed since the last save/load.
    pub fn is_modified(&self) -> bool {
        todo!("engine")
    }

    /// Evaluated output for every body (lazy; only dirty bodies re-evaluate).
    pub fn output(&mut self) -> &[OutputMesh] {
        todo!("engine")
    }

    pub fn stats(&mut self) -> MeshStats {
        todo!("engine")
    }

    /// Number of edits whose keys no longer exist (SPEC §4.5).
    pub fn dormant_edits(&mut self) -> Vec<u32> {
        todo!("engine")
    }

    /// Save as `.ntopo` (SPEC §8.2).
    pub fn save(&mut self, path: &Path) -> anyhow::Result<()> {
        let _ = path;
        todo!("engine")
    }

    /// Load a `.ntopo` into a fresh scene.
    pub fn load(kernel: Arc<dyn CadKernel>, path: &Path) -> anyhow::Result<Scene> {
        let _ = (kernel, path);
        todo!("engine")
    }
}
