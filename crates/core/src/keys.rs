//! Stable keys for generated vertices and edges (SPEC §4.5).
//!
//! Keys come from provenance, never from evaluation order, so edits stored
//! against them survive re-evaluation. The derived `Ord` is the tie-break
//! order used by bridging and auto-fill: key kinds order as
//! `Cad < OnBoundary < Crossing < CutPoint < Placeholder` (SPEC §4.6 step 6),
//! then by their fields. Pole vertices sort with `Cad`.

use newtopo_kernel::{EdgeId, FaceId, VertexId};
use serde::{Deserialize, Serialize};

/// Id of a rule in the document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RuleId(pub u32);

/// Id of a split line in the document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SplitId(pub u32);

/// One generated line: the `index`-th line of a rule. For rules that produce
/// lines per face (Connect over a chain, Grid), `index` is unique within the
/// rule and stable for a given count. `sub` distinguishes pieces of the same
/// logical line on different faces or trimmed components (0 when unused).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LineKey {
    pub rule: RuleId,
    pub index: u32,
    pub sub: u32,
}

/// What a line ended on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum EdgeOrSplit {
    Cad(EdgeId),
    Split(SplitId),
    /// A periodic seam (only used transiently; seam-only vertices are dropped).
    Seam(FaceId, u8),
    Line(LineKey),
}

/// Canonical id of a closed loop for placeholder keys (SPEC §4.1).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum LoopId {
    /// Sorted ids of the CAD edges forming the loop.
    Cad(Vec<EdgeId>),
    Split(SplitId),
    Line(LineKey),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum VertexKey {
    /// A CAD vertex, or a pole/apex vertex (`Pole(face, which)`).
    Cad(VertexId),
    Pole(FaceId, u8),
    /// Where a line ends on (or crosses) a CAD edge, split line, seam or
    /// another line it does not continue through. `end` is 0 for the line's
    /// start, 1 for its end, and counts up for interior boundary crossings
    /// of through-lines.
    OnBoundary { line: LineKey, on: EdgeOrSplit, end: u32 },
    /// Crossing of two lines; `a < b` canonical; `occurrence` counts
    /// crossings of the same two lines in order along `a`.
    Crossing { a: LineKey, b: LineKey, occurrence: u32 },
    CutPoint { rule: RuleId, i: u32 },
    Placeholder { lp: LoopId, i: u32 },
}

impl VertexKey {
    /// True for keys anchored to CAD topology (used by collapse priority).
    pub fn is_cad(&self) -> bool {
        matches!(self, VertexKey::Cad(_) | VertexKey::Pole(..))
    }
}

/// Key of a generated output edge: the line it lies on plus its two end keys
/// (sorted), or the CAD edge / split line segment between two vertices.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum EdgeKey {
    Line { line: LineKey, a: VertexKey, b: VertexKey },
    Cad { edge: EdgeId, a: VertexKey, b: VertexKey },
    Split { split: SplitId, a: VertexKey, b: VertexKey },
}

impl EdgeKey {
    pub fn line(line: LineKey, a: VertexKey, b: VertexKey) -> Self {
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        EdgeKey::Line { line, a, b }
    }
    pub fn cad(edge: EdgeId, a: VertexKey, b: VertexKey) -> Self {
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        EdgeKey::Cad { edge, a, b }
    }
    pub fn split(split: SplitId, a: VertexKey, b: VertexKey) -> Self {
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        EdgeKey::Split { split, a, b }
    }
    pub fn ends(&self) -> (&VertexKey, &VertexKey) {
        match self {
            EdgeKey::Line { a, b, .. } | EdgeKey::Cad { a, b, .. } | EdgeKey::Split { a, b, .. } => (a, b),
        }
    }
}
