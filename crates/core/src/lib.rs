//! newtopo core: no UI, no GPU; everything unit-testable (SPEC §3.3).

pub mod bridge;
pub mod doc;
pub mod eval;
pub mod export;
pub mod fill;
pub mod keys;
pub mod mesh;
pub mod ops;
pub mod rename;
pub mod scene;
pub mod select;
pub mod tools;

pub use doc::Document;
pub use mesh::{MeshStats, OutputMesh};
pub use ops::{Op, OpOutcome, Refusal, Script};
pub use scene::Scene;
