//! Reference checks for documents that come from outside (a `.ntopo` file,
//! a script): every body, face, edge and vertex id a document names must
//! exist in the loaded topology, so evaluation can index topology without
//! bounds checks. Ids of rules and split lines inside generated keys are not
//! checked: an edit whose key no longer matches anything is dormant
//! (SPEC §4.5), not malformed.

use crate::doc::{
    CutPoint, Document, Edit, PatchId, PhaseTarget, Rule, SliceOrient, SliceScope, SplitKind,
};
use crate::keys::{EdgeKey, EdgeOrSplit, LoopId, VertexKey};
use anyhow::{Result, bail};
use newtopo_kernel::{BodyId, BodyTopology, EdgeId, FaceId, VertexId};
use std::collections::HashSet;

/// Largest line count a loaded rule may ask for. Op handlers clamp far
/// below this; a larger value can only come from a hand-edited file and
/// would make evaluation run out of memory.
pub const MAX_RULE_COUNT: u32 = 100_000;

struct Topo<'a> {
    body: BodyId,
    t: &'a BodyTopology,
}

impl Topo<'_> {
    fn face(&self, f: FaceId, what: &str) -> Result<()> {
        if f.idx() >= self.t.faces.len() {
            bail!(
                "{what} names face {} of body {}, which has {} faces",
                f.0,
                self.body.0,
                self.t.faces.len()
            );
        }
        Ok(())
    }
    fn edge(&self, e: EdgeId, what: &str) -> Result<()> {
        if e.idx() >= self.t.edges.len() {
            bail!(
                "{what} names CAD edge {} of body {}, which has {} edges",
                e.0,
                self.body.0,
                self.t.edges.len()
            );
        }
        Ok(())
    }
    fn vertex(&self, v: VertexId, what: &str) -> Result<()> {
        if v.idx() >= self.t.vertices.len() {
            bail!(
                "{what} names CAD vertex {} of body {}, which has {} vertices",
                v.0,
                self.body.0,
                self.t.vertices.len()
            );
        }
        Ok(())
    }
    fn on(&self, o: &EdgeOrSplit, what: &str) -> Result<()> {
        match o {
            EdgeOrSplit::Cad(e) => self.edge(*e, what),
            EdgeOrSplit::Seam(f, _) => self.face(*f, what),
            EdgeOrSplit::Split(_) | EdgeOrSplit::Line(_) => Ok(()),
        }
    }
    fn vkey(&self, k: &VertexKey, what: &str) -> Result<()> {
        match k {
            VertexKey::Cad(v) => self.vertex(*v, what),
            VertexKey::Pole(f, _) => self.face(*f, what),
            VertexKey::OnBoundary { on, .. } | VertexKey::SplitEnd { on, .. } => self.on(on, what),
            VertexKey::Placeholder { lp, .. } => match lp {
                LoopId::Cad(es) => es.iter().try_for_each(|e| self.edge(*e, what)),
                LoopId::Seam(f, _) => self.face(*f, what),
                LoopId::Split(_) | LoopId::Line(_) => Ok(()),
            },
            VertexKey::Crossing { .. }
            | VertexKey::CutPoint { .. }
            | VertexKey::SplitCross { .. } => Ok(()),
        }
    }
    fn ekey(&self, k: &EdgeKey, what: &str) -> Result<()> {
        let (a, b) = match k {
            EdgeKey::Line { a, b, .. } | EdgeKey::Split { a, b, .. } => (a, b),
            EdgeKey::Cad { edge, a, b } => {
                self.edge(*edge, what)?;
                (a, b)
            }
        };
        self.vkey(a, what)?;
        self.vkey(b, what)
    }
}

fn finite(x: f64, what: &str) -> Result<()> {
    if !x.is_finite() {
        bail!("{what} has a non-finite number");
    }
    Ok(())
}

fn count(c: u32, what: &str) -> Result<()> {
    if c > MAX_RULE_COUNT {
        bail!("{what} asks for {c} lines (at most {MAX_RULE_COUNT})");
    }
    Ok(())
}

/// Upper bound for `SceneSettings::min_closed_segments` accepted on load.
pub const MAX_CLOSED_SEGMENTS: u32 = 1024;

/// Check every reference in `doc` against the topology `topo_of` returns.
pub(super) fn check_document<'a>(
    doc: &Document,
    topo_of: impl Fn(BodyId) -> Option<&'a BodyTopology>,
) -> Result<()> {
    if crate::export::unit_metres(&doc.settings.units).is_none() {
        bail!("unknown scene unit `{}`", doc.settings.units);
    }
    // Settings that size allocations or drive geometry must be sane too.
    let m = doc.settings.min_closed_segments;
    if !(3..=MAX_CLOSED_SEGMENTS).contains(&m) {
        bail!("min_closed_segments {m} is outside 3..={MAX_CLOSED_SEGMENTS}");
    }
    let a = doc.settings.sharp_angle_deg;
    if !(a.is_finite() && (0.0..=180.0).contains(&a)) {
        bail!("sharp angle {a} is not between 0 and 180 degrees");
    }
    let mut seen = HashSet::new();
    for b in &doc.bodies {
        if !seen.insert(b.id) {
            bail!("body {} is listed twice", b.id.0);
        }
        if topo_of(b.id).is_none() {
            bail!("body {} has no CAD data", b.id.0);
        }
        if doc.layer(b.layer).is_none() {
            bail!("body {} is on missing layer {}", b.id.0, b.layer.0);
        }
        if !b.transform.is_finite() {
            bail!("body {} has a non-finite transform", b.id.0);
        }
    }
    if doc.layer(doc.active_layer).is_none() {
        bail!("the active layer {} does not exist", doc.active_layer.0);
    }
    let topo = |b: BodyId, what: &str| -> Result<Topo<'a>> {
        if !seen.contains(&b) {
            bail!("{what} names body {}, which is not in the scene", b.0);
        }
        Ok(Topo {
            body: b,
            t: topo_of(b).expect("checked above"),
        })
    };
    let patch = |p: &PatchId, what: &str| -> Result<()> { topo(p.body, what)?.face(p.face, what) };

    for s in &doc.split_lines {
        let what = format!("split line {}", s.id.0);
        topo(s.body, &what)?.face(s.face, &what)?;
        let SplitKind::Iso { value, seed, .. } = s.kind;
        finite(value, &what)?;
        finite(seed.unwrap_or(0.0), &what)?;
    }
    let mut rule_ids = HashSet::new();
    for r in &doc.rules {
        let what = format!("{} rule {}", r.rule.kind_name(), r.id.0);
        if !rule_ids.insert(r.id) {
            bail!("rule {} is listed twice", r.id.0);
        }
        match &r.rule {
            Rule::Connect {
                patches,
                flipped,
                count_a,
                count_b,
                ..
            } => {
                patches
                    .iter()
                    .chain(flipped)
                    .try_for_each(|p| patch(p, &what))?;
                count(*count_a, &what)?;
                count(*count_b, &what)?;
            }
            Rule::Grid {
                patch: p,
                count_u,
                count_v,
                ..
            } => {
                patch(p, &what)?;
                count(*count_u, &what)?;
                count(*count_v, &what)?;
            }
            Rule::Slice {
                scope,
                count: c,
                orient,
                frozen_planes,
                ..
            } => {
                match scope {
                    SliceScope::Body(b) => {
                        topo(*b, &what)?;
                    }
                    SliceScope::Patches(l) => l.iter().try_for_each(|p| patch(p, &what))?,
                }
                if let SliceOrient::GuideEdge(b, e) = orient {
                    topo(*b, &what)?.edge(*e, &what)?;
                }
                count(*c, &what)?;
                for pl in frozen_planes.iter().flatten() {
                    if !pl.origin.is_finite() || !pl.normal.is_finite() {
                        bail!("{what} has a non-finite plane");
                    }
                }
            }
            Rule::Iso { pieces, .. } => {
                for p in pieces {
                    topo(p.body, &what)?.face(p.face, &what)?;
                    finite(p.value, &what)?;
                    finite(p.seed.unwrap_or(0.0), &what)?;
                }
            }
            Rule::Cut { points, .. } => {
                for p in points {
                    match p {
                        CutPoint::Face { body, face, uv } => {
                            topo(*body, &what)?.face(*face, &what)?;
                            if !uv.is_finite() {
                                bail!("{what} has a non-finite cut point");
                            }
                        }
                        CutPoint::CadEdge { body, edge, t } => {
                            topo(*body, &what)?.edge(*edge, &what)?;
                            finite(*t, &what)?;
                        }
                        CutPoint::Vertex { body, key } => topo(*body, &what)?.vkey(key, &what)?,
                    }
                }
            }
            Rule::Join { body, a, b } => {
                let t = topo(*body, &what)?;
                t.vkey(a, &what)?;
                t.vkey(b, &what)?;
            }
        }
    }
    for e in &doc.edits {
        let what = format!("edit {}", e.id);
        let t = topo(e.edit.body(), &what)?;
        match &e.edit {
            Edit::RemoveEdge { edge, .. }
            | Edit::Collapse { edge, .. }
            | Edit::SetHard { edge, .. } => t.ekey(edge, &what)?,
            Edit::RemoveCadEdge { edge, .. } => t.edge(*edge, &what)?,
            Edit::Weld { from, to, .. } => {
                t.vkey(from, &what)?;
                t.vkey(to, &what)?;
            }
        }
    }
    for (k, v) in &doc.phase_overrides {
        let what = "a phase override";
        match k {
            PhaseTarget::Face(b, f) => topo(*b, what)?.face(*f, what)?,
            PhaseTarget::Edge(b, e) => topo(*b, what)?.edge(*e, what)?,
        }
        if !v.is_finite() || v.length_squared() == 0.0 {
            bail!("a phase override has a zero or non-finite direction");
        }
    }
    Ok(())
}
