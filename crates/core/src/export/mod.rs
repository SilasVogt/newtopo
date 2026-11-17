//! OBJ, glTF (.glb) and binary FBX 7.4 writers (SPEC §8.3). OWNER: export.
//!
//! Units: callers pass `scale` = scene unit -> chosen export unit and
//! `unit_metres` = that export unit in metres. OBJ writes coordinates in the
//! export unit (it has no unit metadata); FBX writes them in the export unit
//! and declares it (`UnitScaleFactor` = unit_metres x 100, centimetres per
//! unit); GLB converts to metres, which glTF requires. So a 100 mm part
//! exported in millimetres reads as 100 in OBJ and imports as 0.1 m from
//! FBX and GLB.

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
    /// Multiply positions by this: scene unit -> export unit (e.g. 0.1 for
    /// a millimetre scene exported in centimetres), times any user scale.
    pub scale: f64,
    pub up: ExportUp,
    /// Layers become hierarchy (FBX/glTF parent nodes, OBJ groups).
    pub layers_as_hierarchy: bool,
    /// Materials from CAD colours.
    pub materials: bool,
    /// Face-weighted normals instead of exact CAD normals.
    pub face_weighted_normals: bool,
    /// Size of one exported coordinate unit in metres (0.001 for mm, 0.01
    /// for cm, 1.0 for m, 0.0254 for in). FBX declares it
    /// (`UnitScaleFactor` = centimetres per unit); glTF is metres by
    /// definition, so GLB coordinates are multiplied by it; OBJ has no unit
    /// metadata and keeps coordinates in the export unit.
    #[serde(default = "default_unit_metres")]
    pub unit_metres: f64,
}

fn default_unit_metres() -> f64 {
    1.0
}

/// Metres per unit for the unit names used in settings ("mm", "cm", "m",
/// "in"); `None` for anything else.
pub fn unit_metres(name: &str) -> Option<f64> {
    match name.trim().to_ascii_lowercase().as_str() {
        "mm" => Some(0.001),
        "cm" => Some(0.01),
        "m" => Some(1.0),
        "in" => Some(0.0254),
        _ => None,
    }
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
            unit_metres: 1.0,
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
    anyhow::ensure!(
        opts.unit_metres.is_finite() && opts.unit_metres > 0.0,
        "export unit must be a positive length in metres (got {})",
        opts.unit_metres
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
            // glTF is +Y up and in metres by definition: `opts.up` doesn't
            // apply, and coordinates are converted from the export unit.
            let o = ExportOptions {
                up: ExportUp::Y,
                scale: opts.scale * opts.unit_metres,
                unit_metres: 1.0,
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
                main: fbx::write_fbx(
                    &objs,
                    &mats,
                    opts.up,
                    opts.unit_metres,
                    opts.layers_as_hierarchy,
                ),
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
