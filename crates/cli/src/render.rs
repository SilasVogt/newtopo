//! `newtopo-cli render in.ntopo --view iso [--preview] -o shot.png` (headless wgpu).

use crate::scene_io::{self, LoadArgs};
use anyhow::{Result, anyhow};
use clap::Args;
use newtopo_render::camera::{NamedView, UpAxis};
use newtopo_render::core_scene::{CoreDisplay, render_core_scene_to_png};
use std::path::PathBuf;

#[derive(Args, Debug)]
pub struct RenderArgs {
    /// Scene (.ntopo) or CAD model.
    pub input: PathBuf,
    /// front, back, left, right, top, bottom or iso.
    #[arg(long, default_value = "iso")]
    pub view: String,
    /// Preview mode: output mesh only.
    #[arg(long)]
    pub preview: bool,
    /// Hide the wireframe in preview mode.
    #[arg(long)]
    pub no_wireframe: bool,
    #[arg(long, default_value_t = 1600)]
    pub width: u32,
    #[arg(long, default_value_t = 1000)]
    pub height: u32,
    #[arg(short, long)]
    pub output: PathBuf,
    #[command(flatten)]
    pub load: LoadArgs,
}

pub fn run(a: &RenderArgs) -> Result<()> {
    let view = NamedView::parse(&a.view).ok_or_else(|| anyhow!("unknown view `{}`", a.view))?;
    let mut scene = scene_io::load(&a.input, &a.load)?;
    let up = match scene.doc().settings.up {
        newtopo_core::doc::UpAxis::Y => UpAxis::Y,
        newtopo_core::doc::UpAxis::Z => UpAxis::Z,
    };
    let display = CoreDisplay {
        preview: a.preview,
        wireframe: !a.no_wireframe,
        ..Default::default()
    };
    render_core_scene_to_png(
        &mut scene,
        view,
        &display,
        up,
        [a.width, a.height],
        &a.output,
    )?;
    println!("wrote {}", a.output.display());
    Ok(())
}
