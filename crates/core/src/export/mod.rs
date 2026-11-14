//! OBJ, glTF (.glb) and binary FBX 7.4 writers (SPEC §8.3). OWNER: export.

use crate::mesh::OutputMesh;
use glam::DMat4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Format {
    Obj,
    Glb,
    Fbx,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ExportUp {
    Y,
    Z,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ExportOptions {
    /// Triangulate (FBX/OBJ; glTF is always triangulated).
    pub triangulate: bool,
    /// Multiply positions by this (unit conversion; 1.0 = scene units).
    pub scale: f64,
    pub up: ExportUp,
    /// Layers become hierarchy (FBX/glTF parent nodes, OBJ groups).
    pub layers_as_hierarchy: bool,
    /// Materials from CAD colours.
    pub materials: bool,
    /// Face-weighted normals instead of exact CAD normals.
    pub face_weighted_normals: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        ExportOptions {
            triangulate: false,
            scale: 1.0,
            up: ExportUp::Y,
            layers_as_hierarchy: true,
            materials: true,
            face_weighted_normals: false,
        }
    }
}

/// One object to export. The source scene is Z-up (CAD convention); the
/// writer converts to `ExportOptions::up`.
pub struct ExportObject<'a> {
    pub name: &'a str,
    pub layer: &'a str,
    pub color: Option<[f32; 3]>,
    /// Body → world (scene) transform.
    pub transform: DMat4,
    pub mesh: &'a OutputMesh,
}

pub fn export(
    format: Format,
    objects: &[ExportObject],
    opts: &ExportOptions,
) -> anyhow::Result<ExportFiles> {
    let _ = (format, objects, opts);
    todo!("export")
}

/// Output files: the main file plus side files (OBJ's `.mtl`).
pub struct ExportFiles {
    pub main: Vec<u8>,
    /// (file-name suffix such as ".mtl", bytes)
    pub side: Vec<(String, Vec<u8>)>,
}
