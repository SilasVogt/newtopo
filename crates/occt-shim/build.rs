//! Builds the C++ shim and links Open CASCADE (SPEC §3.4).
//!
//! OCCT is located, in order, through:
//! 1. `OCCT_INCLUDE_DIR` + `OCCT_LIB_DIR` (explicit override),
//! 2. `OCCT_DIR` (an install prefix; Unix `include/opencascade` + `lib`/`lib64`,
//!    vcpkg `include/opencascade` + `lib`, or the Windows installer layout
//!    `inc` + `win64/vc14/lib`),
//! 3. common prefixes (`/usr`, `/usr/local`, Homebrew).
//!
//! Library names are detected from what is installed: OCCT 7.8+/8.x ship
//! `TKDESTEP`/`TKDEIGES`, older 7.x ship `TKSTEP*`/`TKIGES`/`TKXDE*`.

use std::env;
use std::path::{Path, PathBuf};

/// Every toolkit the shim may need, in link order (dependents first does not
/// matter for shared libraries). Only the ones present are linked.
const TOOLKITS: &[&str] = &[
    // DataExchange, 7.8+ / 8.x
    "TKDESTEP",
    "TKDEIGES",
    "TKDE",
    // DataExchange, <= 7.7
    "TKXDESTEP",
    "TKXDEIGES",
    "TKSTEP",
    "TKSTEP209",
    "TKSTEPAttr",
    "TKSTEPBase",
    "TKIGES",
    // XCAF / OCAF
    "TKXCAF",
    "TKVCAF",
    "TKV3d",
    "TKService",
    "TKCAF",
    "TKLCAF",
    "TKCDF",
    "TKXSBase",
    // Modelling
    "TKHLR",
    "TKFeat",
    "TKOffset",
    "TKFillet",
    "TKBool",
    "TKBO",
    "TKPrim",
    "TKMesh",
    "TKShHealing",
    "TKTopAlgo",
    "TKGeomAlgo",
    "TKBRep",
    "TKGeomBase",
    "TKG3d",
    "TKG2d",
    "TKMath",
    "TKernel",
];

struct Occt {
    include: PathBuf,
    lib: PathBuf,
    bin: Option<PathBuf>,
}

fn has_headers(dir: &Path) -> bool {
    dir.join("Standard.hxx").is_file()
}

fn lib_file_names(name: &str) -> Vec<String> {
    let target = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    match target.as_str() {
        "windows" => vec![format!("{name}.lib")],
        "macos" => vec![format!("lib{name}.dylib"), format!("lib{name}.a")],
        _ => vec![format!("lib{name}.so"), format!("lib{name}.a")],
    }
}

fn has_lib(dir: &Path, name: &str) -> bool {
    lib_file_names(name).iter().any(|f| dir.join(f).is_file())
}

fn from_prefix(prefix: &Path) -> Option<Occt> {
    let includes = [prefix.join("include/opencascade"), prefix.join("inc"), prefix.join("include")];
    let include = includes.into_iter().find(|d| has_headers(d))?;
    let mut libs = vec![prefix.join("lib"), prefix.join("lib64"), prefix.join("lib/x86_64-linux-gnu")];
    // Windows installer layout: win64/vc14/lib (any compiler dir).
    if let Ok(rd) = std::fs::read_dir(prefix.join("win64")) {
        for e in rd.flatten() {
            libs.push(e.path().join("lib"));
        }
    }
    let lib = libs.into_iter().find(|d| has_lib(d, "TKernel"))?;
    let bin = {
        let b = lib.parent().map(|p| p.join("bin"));
        b.filter(|b| b.is_dir())
    };
    Some(Occt { include, lib, bin })
}

fn find_occt() -> Option<Occt> {
    if let (Ok(inc), Ok(lib)) = (env::var("OCCT_INCLUDE_DIR"), env::var("OCCT_LIB_DIR")) {
        return Some(Occt { include: inc.into(), lib: lib.into(), bin: None });
    }
    if let Ok(dir) = env::var("OCCT_DIR") {
        let dir = PathBuf::from(dir);
        if let Some(o) = from_prefix(&dir) {
            return Some(o);
        }
        panic!(
            "\n\nOCCT_DIR={} does not look like an Open CASCADE install \
             (expected include/opencascade/Standard.hxx and lib/libTKernel.*).\n\n",
            dir.display()
        );
    }
    let prefixes = ["/usr", "/usr/local", "/opt/homebrew/opt/opencascade", "/usr/local/opt/opencascade", "/opt/opencascade"];
    prefixes.iter().find_map(|p| from_prefix(Path::new(p)))
}

fn main() {
    for v in ["OCCT_DIR", "OCCT_INCLUDE_DIR", "OCCT_LIB_DIR"] {
        println!("cargo:rerun-if-env-changed={v}");
    }
    println!("cargo:rerun-if-changed=cpp");
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=src/testshapes.rs");

    let occt = find_occt().unwrap_or_else(|| {
        panic!(
            "\n\nOpen CASCADE (OCCT 7.8+) was not found.\n\
             Set OCCT_DIR to its install prefix, e.g.\n  \
             OCCT_DIR=$HOME/.cache/newtopo-occt/install cargo build\n\
             or build it with scripts/build-occt.sh (Linux/macOS) / scripts/build-occt.ps1 (Windows).\n\n"
        )
    });

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let testshapes = env::var_os("CARGO_FEATURE_TESTSHAPES").is_some();

    let mut bridges = vec!["src/lib.rs"];
    let mut files = vec!["cpp/shim.cpp", "cpp/io.cpp", "cpp/topo.cpp"];
    if testshapes {
        bridges.push("src/testshapes.rs");
        files.push("cpp/testshapes.cpp");
    }

    let mut build = cxx_build::bridges(bridges);
    build.files(files).include(&occt.include).include("cpp").std("c++17");
    if target_os == "windows" {
        build.flag_if_supported("/bigobj").flag_if_supported("/EHsc").define("_USE_MATH_DEFINES", None);
    } else {
        // Lets OCC_CATCH_SIGNALS turn signals into Standard_Failure in the
        // import helper (the only process that calls OSD::SetSignal).
        build.define("OCC_CONVERT_SIGNALS", None);
        build
            .flag_if_supported("-Wno-deprecated-declarations")
            .flag_if_supported("-Wno-unused-parameter")
            // False positives in cxx-generated Vec constructors (GCC 13+).
            .flag_if_supported("-Wno-maybe-uninitialized");
    }
    build.compile("newtopo-occt-shim");

    println!("cargo:rustc-link-search=native={}", occt.lib.display());
    let mut linked = Vec::new();
    for tk in TOOLKITS {
        if has_lib(&occt.lib, tk) {
            println!("cargo:rustc-link-lib=dylib={tk}");
            linked.push(*tk);
        }
    }
    let step = linked.contains(&"TKDESTEP") || linked.contains(&"TKXDESTEP");
    let iges = linked.contains(&"TKDEIGES") || linked.contains(&"TKXDEIGES");
    if !step || !iges || !linked.contains(&"TKXCAF") {
        panic!(
            "\n\nOCCT at {} lacks the DataExchange/XCAF toolkits (found: {:?}).\n\
             Build OCCT with BUILD_MODULE_DataExchange=ON and BUILD_MODULE_ApplicationFramework=ON.\n\n",
            occt.lib.display(),
            linked
        );
    }

    // Exposed to dependents' build scripts as DEP_OCCT_SHIM_* so binaries can
    // add an rpath (Linux/macOS) or know where the DLLs live (Windows).
    println!("cargo:lib_dir={}", occt.lib.display());
    println!("cargo:include_dir={}", occt.include.display());
    if let Some(bin) = &occt.bin {
        println!("cargo:bin_dir={}", bin.display());
    }
    if target_os == "linux" {
        // DT_RPATH (not RUNPATH) so OCCT's transitive dependencies resolve too.
        println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
    }
    if target_os != "windows" {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", occt.lib.display());
    }
}
