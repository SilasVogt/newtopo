//! `newtopo-cli validate in.ntopo`: invariants + stats; non-zero exit on failure.

use crate::scene_io::{self, LoadArgs};
use anyhow::{Result, bail};
use clap::Args;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Args, Debug)]
pub struct ValidateArgs {
    /// Scene (.ntopo) or CAD model.
    pub input: PathBuf,
    #[arg(long)]
    pub json: bool,
    #[command(flatten)]
    pub load: LoadArgs,
}

#[derive(Serialize)]
struct BodyReport {
    body: u32,
    name: String,
    stats: newtopo_core::MeshStats,
    dormant_edits: usize,
    issues: Vec<String>,
}

pub fn run(a: &ValidateArgs) -> Result<()> {
    let mut scene = scene_io::load(&a.input, &a.load)?;
    let dormant = scene.dormant_edits().len();
    let mut reports = Vec::new();
    for id in scene.body_ids() {
        let name = scene
            .doc()
            .body(id)
            .map(|b| b.name.clone())
            .unwrap_or_default();
        let stats = scene.output_of(id).map(|m| m.stats()).unwrap_or_default();
        let issues = scene.validate(id).into_iter().map(|i| i.message).collect();
        reports.push(BodyReport {
            body: id.0,
            name,
            stats,
            dormant_edits: 0,
            issues,
        });
    }
    if let Some(first) = reports.first_mut() {
        first.dormant_edits = dormant;
    }
    let failures: usize = reports.iter().map(|r| r.issues.len()).sum();
    if a.json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({ "ok": failures == 0, "dormant_edits": dormant, "bodies": reports })
            )?
        );
    } else {
        for r in &reports {
            let s = r.stats;
            println!(
                "{} ({}): {} verts, {} edges, {} faces = {} quads + {} tris + {} n-gons{}",
                r.name,
                r.body,
                s.verts,
                s.edges,
                s.faces,
                s.quads,
                s.tris,
                s.ngons,
                if r.issues.is_empty() {
                    String::from(", valid")
                } else {
                    format!(", {} issue(s)", r.issues.len())
                }
            );
            for i in &r.issues {
                println!("  - {i}");
            }
        }
        println!("dormant edits: {dormant}");
    }
    if failures > 0 {
        bail!("validation failed with {failures} issue(s)");
    }
    Ok(())
}
