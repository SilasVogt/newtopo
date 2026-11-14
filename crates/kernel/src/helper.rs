//! Runs the `newtopo-import` helper process (SPEC §3.4, §8.1) and reads its
//! output. A crash or hang in the CAD readers kills the helper, never the app.
//! OWNER: occt. Signatures are a contract used by the app and CLI.

use crate::{BodyAttributes, HealReport};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImportOptions {
    pub heal: bool,
    /// Sewing tolerance in model units.
    pub sewing_tolerance: f64,
    /// Target units ("mm", "cm", "m", "in"); the helper converts.
    pub units: String,
    pub timeout: Duration,
    /// Explicit helper path; default: next to the current executable.
    pub helper: Option<PathBuf>,
}

impl Default for ImportOptions {
    fn default() -> Self {
        ImportOptions {
            heal: true,
            sewing_tolerance: 1e-3,
            units: "mm".into(),
            timeout: Duration::from_secs(300),
            helper: None,
        }
    }
}

/// One body read from a CAD file.
#[derive(Clone, Debug)]
pub struct ImportedBody {
    /// OCCT binary B-rep bytes, loadable with `CadKernel::load_brep`.
    pub brep: Vec<u8>,
    pub attrs: BodyAttributes,
    pub heal: HealReport,
}

#[derive(Clone, Debug)]
pub struct ImportedFile {
    pub bodies: Vec<ImportedBody>,
    /// SHA-256 of the source file, hex.
    pub source_sha256: String,
    /// Helper's warnings (stderr lines worth showing).
    pub warnings: Vec<String>,
}

/// Progress callback: fraction 0..1 and a message. Return false to cancel.
pub type Progress<'a> = &'a mut dyn FnMut(f32, &str) -> bool;

pub fn run_import(
    path: &Path,
    opts: &ImportOptions,
    progress: Progress,
) -> anyhow_like::Result<ImportedFile> {
    let _ = (path, opts, progress);
    Err(anyhow_like::Error(
        "import helper not implemented yet".into(),
    ))
}

/// Minimal error type so the kernel crate needs no anyhow.
pub mod anyhow_like {
    #[derive(Debug, Clone)]
    pub struct Error(pub String);
    impl std::fmt::Display for Error {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.0)
        }
    }
    impl std::error::Error for Error {}
    pub type Result<T> = std::result::Result<T, Error>;
}
