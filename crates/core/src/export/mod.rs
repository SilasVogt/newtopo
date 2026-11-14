//! OBJ, glTF (.glb) and binary FBX 7.4 writers (SPEC §8.3). OWNER: export.
//!
//! Units: written coordinates are scene units times `ExportOptions::scale`,
//! and every format declares them as metres (glTF and OBJ by convention, FBX
//! through `UnitScaleFactor = 100`), so the app picks `scale` (e.g. 0.001
//! for a millimetre scene) and all three formats import at the same size.

pub mod fbx;
pub mod glb;
pub mod obj;
pub mod prep;
#[doc(hidden)]
pub mod samples;

use crate::mesh::OutputMesh;
use glam::DMat4;
use prep::TransformMode;
use std::path::Path;

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
    anyhow::ensure!(
        opts.scale.is_finite() && opts.scale > 0.0,
        "export scale must be a positive number (got {})",
        opts.scale
    );
    for o in objects {
        anyhow::ensure!(
            o.transform.is_finite(),
            "object {:?} has a non-finite transform",
            o.name
        );
    }
    Ok(match format {
        Format::Obj => {
            let (objs, mats) =
                prep::prepare_all(objects, opts, TransformMode::Bake, opts.triangulate);
            let (main, mtl) = obj::write_obj(&objs, &mats, opts.layers_as_hierarchy);
            let side = if mats.is_empty() {
                Vec::new()
            } else {
                vec![(".mtl".to_string(), mtl)]
            };
            ExportFiles { main, side }
        }
        Format::Glb => {
            // glTF is +Y up by definition; `opts.up` doesn't apply.
            let o = ExportOptions {
                up: ExportUp::Y,
                ..opts.clone()
            };
            let (objs, mats) = prep::prepare_all(objects, &o, TransformMode::Node, true);
            ExportFiles {
                main: glb::write_glb(&objs, &mats, opts.layers_as_hierarchy),
                side: Vec::new(),
            }
        }
        Format::Fbx => {
            let (objs, mats) =
                prep::prepare_all(objects, opts, TransformMode::Node, opts.triangulate);
            ExportFiles {
                main: fbx::write_fbx(&objs, &mats, opts.up, opts.layers_as_hierarchy),
                side: Vec::new(),
            }
        }
    })
}

impl Format {
    /// File extension without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Format::Obj => "obj",
            Format::Glb => "glb",
            Format::Fbx => "fbx",
        }
    }
}

/// Output files: the main file plus side files (OBJ's `.mtl`).
pub struct ExportFiles {
    pub main: Vec<u8>,
    /// (file-name suffix such as ".mtl", bytes)
    pub side: Vec<(String, Vec<u8>)>,
}

impl ExportFiles {
    /// Write the main file to `path` and each side file next to it as
    /// `<stem><suffix>`. For OBJ the `mtllib` line is rewritten to the actual
    /// MTL name (the in-memory output references `newtopo.mtl`).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("export");
        let dir = path.parent().unwrap_or(Path::new(""));
        let mut main = std::borrow::Cow::Borrowed(&self.main[..]);
        for (suffix, bytes) in &self.side {
            let name = format!("{stem}{suffix}");
            if suffix == ".mtl" {
                let from = format!("mtllib {}\n", obj::DEFAULT_MTL);
                let text =
                    String::from_utf8_lossy(&main).replacen(&from, &format!("mtllib {name}\n"), 1);
                main = std::borrow::Cow::Owned(text.into_bytes());
            }
            std::fs::write(dir.join(&name), bytes)?;
        }
        std::fs::write(path, &main)
    }
}

#[cfg(test)]
mod tests;
