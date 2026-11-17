//! Tool operations. One `Op` = one undoable user action. The same enum is the
//! JSON op-script format of `newtopo-cli run` and of "record ops" in the UI
//! (SPEC §10), so every tool is testable headless.

use crate::doc::{Axis, CutPoint, IsoDir, LayerId, PatchId, Spacing};
use crate::keys::{EdgeKey, RuleId, VertexKey};
use newtopo_kernel::{BodyId, EdgeId};
use serde::{Deserialize, Serialize};

/// Patch selection in scripts: whole faces of one body (`body` defaults to
/// the first body in the scene), plus explicit patch ids.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PatchSel {
    #[serde(default)]
    pub body: Option<BodyId>,
    #[serde(default)]
    pub faces: Vec<u32>,
    #[serde(default)]
    pub patches: Vec<PatchId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// SPEC §6.2. `a`/`b` are the A and B direction counts.
    Connect {
        patches: PatchSel,
        #[serde(default)]
        a: u32,
        #[serde(default)]
        b: u32,
        #[serde(default)]
        start_on_seam: bool,
    },
    /// SPEC §6.3.
    Grid {
        patches: PatchSel,
        u: u32,
        v: u32,
        #[serde(default)]
        start_on_seam: bool,
        #[serde(default = "default_spacing")]
        spacing: Spacing,
    },
    /// SPEC §6.4. `body` set → body scope; otherwise `patches` scope.
    Slice {
        #[serde(default)]
        body: Option<BodyId>,
        #[serde(default)]
        patches: Option<PatchSel>,
        count: u32,
        #[serde(default)]
        through: bool,
        #[serde(default)]
        guide_edge: Option<EdgeId>,
    },
    /// SPEC §6.5. `split: true` adds split lines instead of an Iso rule.
    Iso {
        #[serde(default)]
        body: Option<BodyId>,
        face: u32,
        dir: IsoDir,
        value: f64,
        #[serde(default)]
        seed: Option<f64>,
        #[serde(default)]
        split: bool,
        #[serde(default)]
        through: bool,
    },
    /// SPEC §6.6.
    Cut {
        points: Vec<CutPoint>,
        #[serde(default)]
        closed: bool,
    },
    /// SPEC §6.2 "connect vertices".
    Join {
        #[serde(default)]
        body: Option<BodyId>,
        a: VertexKey,
        b: VertexKey,
    },
    /// Remove a spread rule's lines from patches (count 0, SPEC §6.1).
    ClearPatches {
        patches: PatchSel,
    },
    DeleteRule {
        rule: RuleId,
    },
    /// SPEC §6.7.
    RemoveEdge {
        #[serde(default)]
        body: Option<BodyId>,
        edge: EdgeKey,
    },
    /// Remove several generated edges as one step (a selection + Delete,
    /// SPEC §6.7); only the final result has to validate.
    RemoveEdges {
        #[serde(default)]
        body: Option<BodyId>,
        edges: Vec<EdgeKey>,
    },
    /// SPEC §6.10. `confirmed` must be true (the UI asks first).
    RemoveCadEdge {
        #[serde(default)]
        body: Option<BodyId>,
        edge: EdgeId,
        #[serde(default)]
        confirmed: bool,
    },
    /// SPEC §6.8.
    Collapse {
        #[serde(default)]
        body: Option<BodyId>,
        edge: EdgeKey,
        #[serde(default)]
        confirmed: bool,
    },
    /// SPEC §6.9.
    Weld {
        #[serde(default)]
        body: Option<BodyId>,
        from: VertexKey,
        to: VertexKey,
        #[serde(default)]
        confirmed: bool,
    },
    SetHard {
        #[serde(default)]
        body: Option<BodyId>,
        edge: EdgeKey,
        hard: bool,
    },
    PurgeDormant,
    SetAutofill {
        on: bool,
    },
    SetLink {
        on: bool,
    },
    SetSymmetry {
        on: bool,
        #[serde(default)]
        axes: Vec<Axis>,
    },
    RenameBody {
        body: BodyId,
        name: String,
    },
    SetBodyLocked {
        body: BodyId,
        locked: bool,
    },
    SetLayerVisible {
        layer: LayerId,
        visible: bool,
    },
    SetLayerLocked {
        layer: LayerId,
        locked: bool,
    },
    /// The new layer's id is in `OpOutcome::created`.
    CreateLayer {
        name: String,
    },
    /// Bodies on the layer move to the active layer (or the first other layer).
    DeleteLayer {
        layer: LayerId,
    },
    RenameLayer {
        layer: LayerId,
        name: String,
    },
    MoveBodyToLayer {
        body: BodyId,
        layer: LayerId,
    },
    SetActiveLayer {
        layer: LayerId,
    },
    /// Toggle the A/B assignment of a patch in its Connect rule (§6.2 Tab).
    FlipConnect {
        patch: PatchId,
    },
    /// Delete one edit (History panel).
    DeleteEdit {
        edit: u32,
    },
    /// SPEC §6.13: apply copied rules to other bodies through face congruence.
    PasteRules {
        clipboard: crate::link::Clipboard,
        targets: Vec<BodyId>,
    },
    SetBodyVisible {
        body: BodyId,
        visible: bool,
    },
}

fn default_spacing() -> Spacing {
    Spacing::ArcLength
}

/// The JSON op script (SPEC §10).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Script {
    pub version: u32,
    /// Input CAD/brep/ntopo path, relative to the script file.
    #[serde(default)]
    pub input: Option<String>,
    #[serde(default)]
    pub settings: Option<ScriptSettings>,
    pub ops: Vec<Op>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ScriptSettings {
    #[serde(default)]
    pub autofill: Option<bool>,
    #[serde(default)]
    pub link: Option<bool>,
    #[serde(default)]
    pub symmetry: Option<Vec<Axis>>,
}

/// A tool refused the step; nothing changed. Shown as a toast (SPEC §5.9).
#[derive(Clone, Debug, PartialEq, thiserror::Error, Serialize, Deserialize)]
#[error("{reason}")]
pub struct Refusal {
    pub reason: String,
    /// The op would be accepted with `confirmed: true` (UI shows a dialog).
    pub needs_confirmation: bool,
}

impl Refusal {
    pub fn new(reason: impl Into<String>) -> Self {
        Refusal {
            reason: reason.into(),
            needs_confirmation: false,
        }
    }
    pub fn confirm(reason: impl Into<String>) -> Self {
        Refusal {
            reason: reason.into(),
            needs_confirmation: true,
        }
    }
}

/// What an accepted op did, for the status bar / toast.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OpOutcome {
    pub message: Option<String>,
    pub lines_added: u32,
    /// Id of something the op created (`CreateLayer` → the layer id).
    #[serde(default)]
    pub created: Option<u32>,
}
