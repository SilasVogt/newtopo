//! Adds an rpath to the OCCT libraries (Linux/macOS dev runs) when the
//! `occt` feature pulls in `newtopo-occt-shim`, which exports its lib dir.
fn main() {
    println!("cargo:rerun-if-env-changed=DEP_OCCT_SHIM_LIB_DIR");
    windows_icon();
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
