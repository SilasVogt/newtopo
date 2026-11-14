//! `newtopo-testmodels <out-dir>`: writes every reference model as `.step`.

#[cfg(feature = "occt")]
fn main() -> anyhow::Result<()> {
    let dir = std::env::args_os().nth(1).ok_or_else(|| anyhow::anyhow!("usage: newtopo-testmodels <out-dir>"))?;
    let paths = newtopo_testmodels::write_all(std::path::Path::new(&dir))?;
    for p in paths {
        println!("{}", p.display());
    }
    Ok(())
}

#[cfg(not(feature = "occt"))]
fn main() {
    eprintln!("newtopo-testmodels was built without the `occt` feature");
    std::process::exit(2);
}
