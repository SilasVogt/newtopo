//! Adds an rpath to the OCCT libraries (Linux/macOS dev runs) when the
//! `occt` feature pulls in `newtopo-occt-shim`, which exports its lib dir.
fn main() {
    println!("cargo:rerun-if-env-changed=DEP_OCCT_SHIM_LIB_DIR");
    windows_icon();
    git_commit();
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if let Ok(dir) = std::env::var("DEP_OCCT_SHIM_LIB_DIR")
        && os != "windows"
    {
        // OCCT's own libraries carry no RUNPATH, so on Linux use DT_RPATH,
        // which also applies to their transitive dependencies.
        if os == "linux" {
            println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
        }
        println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
    }
}

/// The commit this build is made from, for problem reports
/// (`NEWTOPO_COMMIT`): from `NEWTOPO_COMMIT` or `GITHUB_SHA` when set (CI,
/// packagers), else from git. Builds outside a git checkout have none.
fn git_commit() {
    println!("cargo:rerun-if-env-changed=NEWTOPO_COMMIT");
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
    let git = |args: &[&str]| -> Option<String> {
        let out = std::process::Command::new("git").args(args).output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let sha = std::env::var("NEWTOPO_COMMIT")
        .ok()
        .or_else(|| std::env::var("GITHUB_SHA").ok())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            // Rebuild when HEAD moves (a checkout or a new commit).
            for p in [
                git(&["rev-parse", "--git-path", "HEAD"]),
                git(&["symbolic-ref", "-q", "HEAD"])
                    .and_then(|r| git(&["rev-parse", "--git-path", &r])),
            ]
            .into_iter()
            .flatten()
            {
                if std::path::Path::new(&p).exists() {
                    println!("cargo:rerun-if-changed={p}");
                }
            }
            git(&["rev-parse", "HEAD"])
        });
    if let Some(sha) = sha.filter(|s| s.len() >= 7 && s.bytes().all(|b| b.is_ascii_hexdigit())) {
        println!("cargo:rustc-env=NEWTOPO_COMMIT={sha}");
    }
}

/// Embed `packaging/icons/newtopo.ico` as the executable icon on Windows.
#[cfg(windows)]
fn windows_icon() {
    println!("cargo:rerun-if-changed=newtopo.rc");
    println!("cargo:rerun-if-changed=../../packaging/icons/newtopo.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // Missing resource compiler: warn, don't fail the build.
        if let Err(e) =
            embed_resource::compile("newtopo.rc", embed_resource::NONE).manifest_optional()
        {
            println!("cargo:warning=app icon not embedded: {e}");
        }
    }
}

#[cfg(not(windows))]
fn windows_icon() {}
