//! `newtopo-import`: the import helper process (SPEC §3.4, §8.1).
//!
//! Reads STEP/IGES/BREP, heals, and writes `body-<n>.bbrep` plus
//! `sidecar.json` into `--out-dir`. Progress goes to stdout as
//! `progress <0..1> <message>`; errors to stderr with a non-zero exit. This is
//! the only process that installs OCCT's signal handlers, so a crash inside
//! the CAD readers becomes an error message here and never reaches the app.

use anyhow::{Result, bail};
use clap::Parser;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser, Debug)]
#[command(
    name = "newtopo-import",
    version,
    about = "Reads a CAD file into OCCT B-rep bodies plus a JSON sidecar"
)]
struct Args {
    /// Input file (.step .stp .iges .igs .brep .bbrep).
    input: PathBuf,
    /// Output directory (created if missing).
    #[arg(long)]
    out_dir: PathBuf,
    /// Skip ShapeFix and sewing.
    #[arg(long)]
    no_heal: bool,
    /// Sewing tolerance in target units.
    #[arg(long, default_value_t = 1e-3)]
    sewing_tol: f64,
    /// Target length unit: mm, cm, m, in, ft.
    #[arg(long, default_value = "mm")]
    units: String,
}

#[cfg(feature = "occt")]
fn progress(f: f32, msg: &str) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "progress {f:.3} {msg}");
    let _ = out.flush();
}

#[cfg(feature = "occt")]
fn run(a: &Args) -> Result<()> {
    use anyhow::Context;
    use newtopo_kernel::helper::{SIDECAR_FILE, SIDECAR_VERSION, Sidecar, SidecarBody};
    use newtopo_kernel::occt::{CadFormat, HealSettings, OcctKernel, unit_in_metres};
    use newtopo_kernel::{CadKernel, HealReport};
    use sha2::{Digest, Sha256};

    let format = CadFormat::from_path(&a.input).with_context(|| {
        format!(
            "unsupported file type {} (expected .step .stp .iges .igs .brep)",
            a.input.display()
        )
    })?;
    let unit_m = unit_in_metres(&a.units).with_context(|| format!("unknown unit {:?}", a.units))?;
    if a.sewing_tol.is_nan() || a.sewing_tol <= 0.0 {
        bail!("sewing tolerance must be positive");
    }
    OcctKernel::enable_signal_handlers()?;

    progress(0.0, "hashing");
    let bytes =
        std::fs::read(&a.input).with_context(|| format!("cannot read {}", a.input.display()))?;
    let sha: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    drop(bytes);

    progress(0.05, "reading");
    let k = OcctKernel::new();
    let file = k.import_file(&a.input, format, unit_m)?;
    std::fs::create_dir_all(&a.out_dir)
        .with_context(|| format!("cannot create {}", a.out_dir.display()))?;

    let n = file.bodies.len();
    let settings = HealSettings {
        fix: !a.no_heal,
        sew: !a.no_heal,
        sewing_tolerance: a.sewing_tol,
    };
    let mut bodies = Vec::with_capacity(n);
    for (i, b) in file.bodies.iter().enumerate() {
        progress(
            0.5 + 0.5 * i as f32 / n as f32,
            &format!(
                "{} {}/{}: {}",
                if a.no_heal { "checking" } else { "healing" },
                i + 1,
                n,
                b.attrs.name
            ),
        );
        let heal = match k.heal(b.body, settings) {
            Ok(r) => r,
            Err(e) => HealReport {
                fixes: vec![format!("healing failed: {e}")],
                ..Default::default()
            },
        };
        let brep = k.save_brep(b.body)?;
        let name = format!("body-{i}.bbrep");
        std::fs::write(a.out_dir.join(&name), &brep)
            .with_context(|| format!("cannot write {name}"))?;
        k.unload(b.body);
        bodies.push(SidecarBody {
            file: name,
            attrs: b.attrs.clone(),
            heal,
        });
    }
    let side = Sidecar {
        version: SIDECAR_VERSION,
        source: a
            .input
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        source_sha256: sha,
        units: a.units.to_ascii_lowercase(),
        kernel: k.name(),
        bodies,
        warnings: file.warnings,
    };
    let tmp = a.out_dir.join(format!("{SIDECAR_FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec_pretty(&side)?)?;
    std::fs::rename(&tmp, a.out_dir.join(SIDECAR_FILE))?;
    progress(1.0, "done");
    Ok(())
}

#[cfg(not(feature = "occt"))]
fn run(_: &Args) -> Result<()> {
    bail!("newtopo-import was built without the `occt` feature")
}

fn main() -> ExitCode {
    let args = Args::parse();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e:#}");
            ExitCode::FAILURE
        }
    }
}
