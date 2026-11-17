//! `newtopo-cli run script.json -o out.ntopo`: apply a JSON op script (SPEC §10).

use crate::scene_io::{self, LoadArgs};
use anyhow::{Context, Result, bail};
use clap::Args;
use newtopo_core::{Op, Script};
use std::path::PathBuf;

#[derive(Args, Debug)]
pub struct RunArgs {
    /// Op script (JSON).
    pub script: PathBuf,
    /// Output scene (.ntopo).
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Input model or scene; overrides the script's `input`.
    #[arg(long)]
    pub input: Option<PathBuf>,
    /// Continue past refused ops (they are reported; exit code is still 1).
    #[arg(long)]
    pub keep_going: bool,
    #[command(flatten)]
    pub load: LoadArgs,
}

pub fn run(a: &RunArgs) -> Result<()> {
    let text = std::fs::read_to_string(&a.script)
        .with_context(|| format!("reading {}", a.script.display()))?;
    let script: Script = serde_json::from_str(&text).context("parsing the op script")?;
    if script.version != 1 {
        bail!(
            "unsupported script version {} (this build reads version 1)",
            script.version
        );
    }
    let input = match (&a.input, &script.input) {
        (Some(p), _) => p.clone(),
        (None, Some(rel)) => a
            .script
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join(rel),
        (None, None) => bail!("the script has no `input`; pass --input"),
    };
    let mut scene = scene_io::load(&input, &a.load)?;

    let mut ops = Vec::new();
    if let Some(s) = &script.settings {
        if let Some(on) = s.autofill {
            ops.push(Op::SetAutofill { on });
        }
        if let Some(on) = s.link {
            ops.push(Op::SetLink { on });
        }
        if let Some(axes) = &s.symmetry {
            ops.push(Op::SetSymmetry {
                on: !axes.is_empty(),
                axes: axes.clone(),
            });
        }
    }
    ops.extend(script.ops.iter().cloned());

    let mut refused = 0;
    for (i, op) in ops.into_iter().enumerate() {
        match scene.apply(op) {
            Ok(out) => {
                if let Some(m) = out.message {
                    println!("op {i}: {m}");
                }
            }
            Err(r) => {
                refused += 1;
                eprintln!("op {i} refused: {}", r.reason);
                if !a.keep_going {
                    bail!("op {i} refused: {}", r.reason);
                }
            }
        }
    }
    let stats = scene.stats();
    println!(
        "output: {} verts, {} faces ({} quads, {} tris, {} n-gons)",
        stats.verts, stats.faces, stats.quads, stats.tris, stats.ngons
    );
    if let Some(out) = &a.output {
        scene
            .save(out)
            .with_context(|| format!("saving {}", out.display()))?;
        println!("saved {}", out.display());
    }
    if refused > 0 {
        bail!("{refused} op(s) refused");
    }
    Ok(())
}
