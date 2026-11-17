//! `newtopo-cli export in.ntopo --format fbx -o out.fbx [--triangulate] [--up y]`.

use crate::scene_io::{self, LoadArgs};
use anyhow::{Context, Result};
use clap::{Args, ValueEnum};
use newtopo_core::export::{ExportObject, ExportOptions, ExportUp, Format, export};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum FormatArg {
    Fbx,
    Obj,
    Glb,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum UpArg {
    Y,
    Z,
}

#[derive(Args, Debug)]
pub struct ExportArgs {
    /// Scene (.ntopo) or CAD model.
    pub input: PathBuf,
    #[arg(long, value_enum)]
    pub format: Option<FormatArg>,
    #[arg(short, long)]
    pub output: PathBuf,
    #[arg(long)]
    pub triangulate: bool,
    #[arg(long, value_enum, default_value = "y")]
    pub up: UpArg,
    #[arg(long, default_value_t = 1.0)]
    pub scale: f64,
    /// Don't turn layers into hierarchy.
    #[arg(long)]
    pub flat: bool,
    #[command(flatten)]
    pub load: LoadArgs,
}

pub fn run(a: &ExportArgs) -> Result<()> {
    let format = match a.format {
        Some(FormatArg::Fbx) => Format::Fbx,
        Some(FormatArg::Obj) => Format::Obj,
        Some(FormatArg::Glb) => Format::Glb,
        None => match a
            .output
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref()
        {
            Some("obj") => Format::Obj,
            Some("glb") | Some("gltf") => Format::Glb,
            _ => Format::Fbx,
        },
    };
    let mut scene = scene_io::load(&a.input, &a.load)?;
    let meshes = scene.output().to_vec();
    let doc = scene.doc();
    let mut objects = Vec::new();
    for m in &meshes {
        let Some(b) = doc.body(m.body) else { continue };
        let layer = doc
            .layer(b.layer)
            .map(|l| l.name.as_str())
            .unwrap_or("Default");
        objects.push(ExportObject {
            name: &b.name,
            layer,
            color: b.color,
            transform: b.transform,
            mesh: m,
        });
    }
    let opts = ExportOptions {
        triangulate: a.triangulate,
        scale: a.scale,
        up: match a.up {
            UpArg::Y => ExportUp::Y,
            UpArg::Z => ExportUp::Z,
        },
        layers_as_hierarchy: !a.flat,
        ..Default::default()
    };
    let fans: usize = meshes
        .iter()
        .flat_map(|m| &m.polygons)
        .filter(|p| p.fan_fallback)
        .count();
    if fans > 0 {
        eprintln!("note: {fans} polygon(s) could only be filled with a triangle fan");
    }
    let files = export(format, &objects, &opts)?;
    files
        .save(&a.output)
        .with_context(|| format!("writing {}", a.output.display()))?;
    println!("wrote {} ({} object(s))", a.output.display(), objects.len());
    Ok(())
}
