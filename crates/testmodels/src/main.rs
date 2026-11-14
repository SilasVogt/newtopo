//! `newtopo-testmodels <out-dir> [--brep]`: writes every reference model as
//! `.step` (and, with `--brep`, its first body as text `.brep`).

#[cfg(feature = "occt")]
fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let brep = args.iter().any(|a| a == "--brep");
    let dir = args
        .iter()
        .find(|a| *a != "--brep")
        .ok_or_else(|| anyhow::anyhow!("usage: newtopo-testmodels <out-dir> [--brep]"))?;
    let dir = std::path::Path::new(dir);
    for p in newtopo_testmodels::write_all(dir)? {
        println!("{}", p.display());
    }
    if brep {
        // Also the first body of every model as OCCT text B-rep.
        for m in newtopo_testmodels::MODELS {
            let p = dir.join(format!("{}.brep", m.name));
            newtopo_testmodels::write_model_brep(m, &p)?;
            println!("{}", p.display());
        }
    }
    Ok(())
}

#[cfg(not(feature = "occt"))]
fn main() {
    eprintln!("newtopo-testmodels was built without the `occt` feature");
    std::process::exit(2);
}
