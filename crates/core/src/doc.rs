//! The procedural document (SPEC §4): B-rep references, split lines, rules
//! and edits. The output mesh is a cache computed by [`crate::eval`]; it is
//! never stored.
//!
//! The document is a plain value type. Undo (SPEC §4.7) snapshots it behind
//! an `Arc`; mutations clone-on-write via `Arc::make_mut`.

use crate::keys::{EdgeKey, LineKey, RuleId, SplitId, VertexKey};
use glam::{DMat4, DVec3};
use newtopo_kernel::{BodyId, EdgeId, FaceId, HealReport};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const FORMAT_VERSION: u32 = 1;

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct LayerId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Axis {
    X,
    Y,
    Z,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UpAxis {
    Y,
    Z,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneSettings {
    pub autofill: bool,
    pub link: bool,
    pub symmetry: bool,
    pub symmetry_axes: Vec<Axis>,
    /// Sharp threshold in degrees (SPEC §2, default 30°).
    pub sharp_angle_deg: f64,
    /// Placeholder count for short closed loops (SPEC §4.1, default 8).
    pub min_closed_segments: u32,
    pub units: String,
    pub up: UpAxis,
}

impl Default for SceneSettings {
    fn default() -> Self {
        SceneSettings {
            autofill: false,
            link: false,
            symmetry: false,
            symmetry_axes: vec![],
            sharp_angle_deg: 30.0,
            min_closed_segments: 8,
            units: "mm".into(),
            up: UpAxis::Z,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    pub visible: bool,
    pub locked: bool,
}

/// A body in the scene. Geometry lives in the kernel (and in `cad/<body>.bbrep`
/// inside a `.ntopo`); the document stores attributes and the kernel handle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BodyEntry {
    pub id: BodyId,
    pub name: String,
    pub layer: LayerId,
    pub color: Option<[f32; 3]>,
    pub transform: DMat4,
    pub visible: bool,
    pub locked: bool,
    /// Heal report from import (kept in the scene, SPEC §8.2).
    pub heal: HealReport,
    /// SHA-256 of the source CAD file, hex.
    pub source_sha256: Option<String>,
}

/// Which bound of a patch. `FaceBoundary` = the face's own UV bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum PatchBound {
    FaceBoundary,
    Split(SplitId),
}

/// Stable patch identity (SPEC §4.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PatchId {
    pub body: BodyId,
    pub face: FaceId,
    pub u_lo: PatchBound,
    pub u_hi: PatchBound,
    pub v_lo: PatchBound,
    pub v_hi: PatchBound,
    pub region: u32,
}

impl PatchId {
    /// The patch covering a whole unsplit face.
    pub fn whole(body: BodyId, face: FaceId) -> Self {
        PatchId {
            body,
            face,
            u_lo: PatchBound::FaceBoundary,
            u_hi: PatchBound::FaceBoundary,
            v_lo: PatchBound::FaceBoundary,
            v_hi: PatchBound::FaceBoundary,
            region: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IsoDir {
    U,
    V,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SplitKind {
    Iso {
        dir: IsoDir,
        value: f64,
        seed: Option<f64>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SplitLine {
    pub id: SplitId,
    pub group: u32,
    pub body: BodyId,
    pub face: FaceId,
    pub kind: SplitKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Spacing {
    ArcLength,
    Parametric,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Plane {
    pub origin: DVec3,
    pub normal: DVec3,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SliceScope {
    Body(BodyId),
    Patches(Vec<PatchId>),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum SliceOrient {
    Auto,
    GuideEdge(BodyId, EdgeId),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct IsoPiece {
    pub body: BodyId,
    pub face: FaceId,
    pub dir: IsoDir,
    pub value: f64,
    pub seed: Option<f64>,
}

/// Where a cut point sits.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CutPoint {
    Face {
        body: BodyId,
        face: FaceId,
        uv: glam::DVec2,
    },
    CadEdge {
        body: BodyId,
        edge: EdgeId,
        t: f64,
    },
    Vertex {
        body: BodyId,
        key: VertexKey,
    },
}

/// SPEC §4.3.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Rule {
    Connect {
        patches: Vec<PatchId>,
        count_a: u32,
        count_b: u32,
        start_on_seam: bool,
        flipped: Vec<PatchId>,
    },
    Grid {
        patch: PatchId,
        count_u: u32,
        count_v: u32,
        start_on_seam: bool,
        spacing: Spacing,
    },
    Slice {
        scope: SliceScope,
        count: u32,
        orient: SliceOrient,
        through: bool,
        frozen_planes: Option<Vec<Plane>>,
    },
    Iso {
        pieces: Vec<IsoPiece>,
        through: bool,
    },
    Cut {
        points: Vec<CutPoint>,
        closed: bool,
    },
    Join {
        body: BodyId,
        a: VertexKey,
        b: VertexKey,
    },
}

impl Rule {
    /// Spread rules own patches (SPEC §4.3).
    pub fn is_spread(&self) -> bool {
        matches!(
            self,
            Rule::Connect { .. } | Rule::Grid { .. } | Rule::Slice { .. }
        )
    }
    pub fn kind_name(&self) -> &'static str {
        match self {
            Rule::Connect { .. } => "Connect",
            Rule::Grid { .. } => "Surface Grid",
            Rule::Slice { .. } => "Slice",
            Rule::Iso { .. } => "Iso",
            Rule::Cut { .. } => "Cut",
            Rule::Join { .. } => "Join",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuleEntry {
    pub id: RuleId,
    /// Deterministic evaluation order.
    pub ordinal: u32,
    pub rule: Rule,
    /// Rules created through Link/symmetry share a group so "unlink" can split them.
    pub link_group: Option<u32>,
}

/// SPEC §4.5.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Edit {
    RemoveEdge {
        body: BodyId,
        edge: EdgeKey,
    },
    RemoveCadEdge {
        body: BodyId,
        edge: EdgeId,
    },
    Collapse {
        body: BodyId,
        edge: EdgeKey,
    },
    Weld {
        body: BodyId,
        from: VertexKey,
        to: VertexKey,
    },
    SetHard {
        body: BodyId,
        edge: EdgeKey,
        hard: bool,
    },
}

impl Edit {
    pub fn body(&self) -> BodyId {
        match self {
            Edit::RemoveEdge { body, .. }
            | Edit::RemoveCadEdge { body, .. }
            | Edit::Collapse { body, .. }
            | Edit::Weld { body, .. }
            | Edit::SetHard { body, .. } => *body,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EditEntry {
    pub id: u32,
    pub edit: Edit,
}

/// Phase-reference override for a periodic face or closed edge (SPEC §4.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum PhaseTarget {
    Face(BodyId, FaceId),
    Edge(BodyId, EdgeId),
}

/// Serde adapter: `BTreeMap<PhaseTarget, DVec3>` as a list of records.
/// Reading also accepts a JSON object, which is how an empty map was written
/// before this adapter existed.
mod phase_records {
    use super::PhaseTarget;
    use glam::DVec3;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::BTreeMap;

    pub fn serialize<S: Serializer>(
        m: &BTreeMap<PhaseTarget, DVec3>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        m.iter().collect::<Vec<_>>().serialize(s)
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        List(Vec<(PhaseTarget, DVec3)>),
        Empty(BTreeMap<String, serde::de::IgnoredAny>),
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<BTreeMap<PhaseTarget, DVec3>, D::Error> {
        match Repr::deserialize(d)? {
            Repr::List(l) => Ok(l.into_iter().collect()),
            Repr::Empty(m) if m.is_empty() => Ok(BTreeMap::new()),
            Repr::Empty(_) => Err(serde::de::Error::custom(
                "phase_overrides must be a list of [target, point] records",
            )),
        }
    }
}

/// Viewport camera saved with the scene.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Camera {
    pub target: DVec3,
    pub yaw: f64,
    pub pitch: f64,
    pub distance: f64,
    pub ortho: bool,
}

impl Default for Camera {
    fn default() -> Self {
        Camera {
            target: DVec3::ZERO,
            yaw: 0.6,
            pitch: 0.5,
            distance: 100.0,
            ortho: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub format_version: u32,
    pub settings: SceneSettings,
    pub layers: Vec<Layer>,
    pub active_layer: LayerId,
    pub bodies: Vec<BodyEntry>,
    pub split_lines: Vec<SplitLine>,
    pub rules: Vec<RuleEntry>,
    pub edits: Vec<EditEntry>,
    /// Stored in `scene.json` as a list of `[target, point]` records: JSON
    /// object keys must be strings and `PhaseTarget` is a tuple enum.
    #[serde(with = "phase_records")]
    pub phase_overrides: BTreeMap<PhaseTarget, DVec3>,
    pub camera: Camera,
    pub next_id: u32,
}

impl Default for Document {
    fn default() -> Self {
        Document {
            format_version: FORMAT_VERSION,
            settings: SceneSettings::default(),
            layers: vec![Layer {
                id: LayerId(0),
                name: "Default".into(),
                visible: true,
                locked: false,
            }],
            active_layer: LayerId(0),
            bodies: vec![],
            split_lines: vec![],
            rules: vec![],
            edits: vec![],
            phase_overrides: BTreeMap::new(),
            camera: Camera::default(),
            next_id: 1,
        }
    }
}

impl Document {
    pub fn fresh_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
    pub fn body(&self, id: BodyId) -> Option<&BodyEntry> {
        self.bodies.iter().find(|b| b.id == id)
    }
    pub fn body_mut(&mut self, id: BodyId) -> Option<&mut BodyEntry> {
        self.bodies.iter_mut().find(|b| b.id == id)
    }
    pub fn rule(&self, id: RuleId) -> Option<&RuleEntry> {
        self.rules.iter().find(|r| r.id == id)
    }
    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        self.layers.iter().find(|l| l.id == id)
    }
}

/// Re-exported for convenience in rule code.
pub type Line = LineKey;

impl Document {
    /// Rename body ids everywhere in the document (rules, splits, edits,
    /// phase overrides). Used to keep document ids equal to kernel ids after
    /// a load or a body replacement.
    pub fn remap_bodies(&mut self, map: &BTreeMap<BodyId, BodyId>) {
        let m = |b: &mut BodyId| {
            if let Some(n) = map.get(b) {
                *b = *n;
            }
        };
        let mp = |p: &mut PatchId| {
            if let Some(n) = map.get(&p.body) {
                p.body = *n;
            }
        };
        for b in &mut self.bodies {
            m(&mut b.id);
        }
        for s in &mut self.split_lines {
            m(&mut s.body);
        }
        for r in &mut self.rules {
            match &mut r.rule {
                Rule::Connect {
                    patches, flipped, ..
                } => {
                    patches.iter_mut().for_each(mp);
                    flipped.iter_mut().for_each(mp);
                }
                Rule::Grid { patch, .. } => mp(patch),
                Rule::Slice { scope, orient, .. } => {
                    match scope {
                        SliceScope::Body(b) => m(b),
                        SliceScope::Patches(l) => l.iter_mut().for_each(mp),
                    }
                    if let SliceOrient::GuideEdge(b, _) = orient {
                        m(b);
                    }
                }
                Rule::Iso { pieces, .. } => pieces.iter_mut().for_each(|p| m(&mut p.body)),
                Rule::Cut { points, .. } => {
                    for p in points {
                        match p {
                            CutPoint::Face { body, .. }
                            | CutPoint::CadEdge { body, .. }
                            | CutPoint::Vertex { body, .. } => m(body),
                        }
                    }
                }
                Rule::Join { body, .. } => m(body),
            }
        }
        for e in &mut self.edits {
            match &mut e.edit {
                Edit::RemoveEdge { body, .. }
                | Edit::RemoveCadEdge { body, .. }
                | Edit::Collapse { body, .. }
                | Edit::Weld { body, .. }
                | Edit::SetHard { body, .. } => m(body),
            }
        }
        let po = std::mem::take(&mut self.phase_overrides);
        for (k, v) in po {
            let k = match k {
                PhaseTarget::Face(b, f) => PhaseTarget::Face(*map.get(&b).unwrap_or(&b), f),
                PhaseTarget::Edge(b, e) => PhaseTarget::Edge(*map.get(&b).unwrap_or(&b), e),
            };
            self.phase_overrides.insert(k, v);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn new_scenes_start_with_autofill_off() {
        // SPEC §13.6: the app never adds edges the user didn't ask for.
        assert!(!super::SceneSettings::default().autofill);
    }
}
