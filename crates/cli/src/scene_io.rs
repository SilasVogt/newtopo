//! Loading a scene from a CAD file (through the import helper) or a `.ntopo`.

use anyhow::{Context, Result};
use newtopo_core::Scene;
use newtopo_kernel::default_kernel;
use newtopo_kernel::helper::{ImportOptions, run_import};
use std::path::{Path, PathBuf};

#[derive(clap::Args, Debug, Clone, Default)]
pub struct LoadArgs {
    /// Path to the newtopo-import helper (default: next to this executable).
    #[arg(long)]
    pub helper: Option<PathBuf>,
    /// Skip healing on CAD import.
    #[arg(long)]
    pub no_heal: bool,
}

pub fn is_scene(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("ntopo"))
}

/// Open `path` as a scene: `.ntopo` loads directly, CAD files are imported.
pub fn load(path: &Path, args: &LoadArgs) -> Result<Scene> {
    let kernel = default_kernel();
    if is_scene(path) {
        return Scene::load(kernel, path).with_context(|| format!("opening {}", path.display()));
    }
    let opts = ImportOptions {
        heal: !args.no_heal,
        helper: args.helper.clone(),
        ..Default::default()
    };
    let file = run_import(path, &opts, &mut |_, _| true)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .with_context(|| format!("importing {}", path.display()))?;
    for w in &file.warnings {
        eprintln!("warning: {w}");
    }
    let mut scene = Scene::new(kernel);
    for b in file.bodies {
        scene.add_body(&b.brep, b.attrs, b.heal)?;
    }
    Ok(scene)
}
