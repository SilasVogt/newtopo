//! Test and bench binaries of this crate link OCCT when the workspace enables
//! `newtopo-kernel/occt`. Add an rpath to `$OCCT_DIR/lib` for dev runs on
//! Linux/macOS (packages bundle the libraries and set their own rpath).
fn main() {
    println!("cargo:rerun-if-env-changed=OCCT_DIR");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if os == "windows" {
        return;
    }
    if let Ok(root) = std::env::var("OCCT_DIR") {
        for sub in ["lib", "lib64"] {
            let dir = std::path::Path::new(&root).join(sub);
            if dir.is_dir() {
                if os == "linux" {
                    println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
                }
                println!("cargo:rustc-link-arg=-Wl,-rpath,{}", dir.display());
                break;
            }
        }
    }
}
