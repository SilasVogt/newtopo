//! Reference models (SPEC §11), generated in code through the OCCT test-shape
//! builder and written as `.step`. Feature `occt` enables everything; without
//! it the crate builds empty.

#[cfg(feature = "occt")]
mod models;
#[cfg(feature = "occt")]
pub use models::*;
