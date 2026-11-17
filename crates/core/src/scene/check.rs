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
use std::collections::{HashMap, HashSet};

/// Largest line count a rule may ask for in one direction (the UI's count
/// fields stop at 999).
pub const MAX_RULE_COUNT: u32 = 1000;
/// Largest number of crossings one two-direction rule may produce
/// (count A × count B for Connect, U × V for Grid): bounds the arrangement
/// work and memory for a single patch.
pub const MAX_RULE_CROSSINGS: u64 = 90_000;
/// Largest number of rules, split lines, edits or cut points a document may
/// hold.
pub const MAX_DOC_ITEMS: usize = 100_000;
/// Largest number of generated lines (all rule kinds, split lines and
/// body-wide rules together) one face may receive. Lines cross pairwise, so
/// this bounds a face's arrangement like MAX_RULE_CROSSINGS bounds one rule
/// (at most about 300 x 300 crossings).
pub const MAX_FACE_LINES: u64 = 600;
/// Largest number of generated lines in a whole document.
pub const MAX_DOC_LINES: u64 = 200_000;
/// Normalised iso values and seeds may run past 0..1 a little (through-iso
/// continuation, periodic wrap) but never far.
const ISO_SLACK: f64 = 1.0;

/// Work limits shared by `.ntopo` load and every op (scripts included), so a
/// compact document can't ask the evaluator for billions of crossings.
fn work_budget(doc: &Document, faces_of: impl Fn(BodyId) -> u64) -> Result<()> {
    if doc.rules.len() > MAX_DOC_ITEMS
        || doc.split_lines.len() > MAX_DOC_ITEMS
        || doc.edits.len() > MAX_DOC_ITEMS
    {
        bail!("the scene has more than {MAX_DOC_ITEMS} rules, split lines or edits");
    }
    for r in &doc.rules {
        let what = format!("rule {}", r.id.0);
        let pair = |a: u32, b: u32| -> Result<()> {
            count(a, &what)?;
            count(b, &what)?;
            if u64::from(a.max(1)) * u64::from(b.max(1)) > MAX_RULE_CROSSINGS {
                bail!(
                    "{what} asks for {a} × {b} lines (at most {MAX_RULE_CROSSINGS} crossings per patch)"
                );
            }
            Ok(())
        };
        match &r.rule {
            Rule::Connect {
                count_a, count_b, ..
            } => pair(*count_a, *count_b)?,
            Rule::Grid {
                count_u, count_v, ..
            } => pair(*count_u, *count_v)?,
            Rule::Slice {
                count: c,
                frozen_planes,
                ..
            } => {
                count(*c, &what)?;
                if frozen_planes
                    .as_ref()
                    .is_some_and(|p| p.len() > MAX_RULE_COUNT as usize)
                {
                    bail!("{what} has more than {MAX_RULE_COUNT} planes");
                }
            }
            Rule::Cut { points, .. } if points.len() > MAX_DOC_ITEMS => {
                bail!("{what} has more than {MAX_DOC_ITEMS} cut points");
            }
            Rule::Iso { pieces, .. } if pieces.len() > MAX_DOC_ITEMS => {
                bail!("{what} has more than {MAX_DOC_ITEMS} pieces");
            }
            _ => {}
        }
    }
    spread_ownership(doc)?;

    // Every line a face receives can cross every other one, so bound the
    // total per face across all rule kinds (a single Iso rule can list
    // thousands of pieces on one face).
    let mut per_face: HashMap<(BodyId, FaceId), u64> = HashMap::new();
    let mut per_body: HashMap<BodyId, u64> = HashMap::new();
    // Body-scope Slice planes are evaluated on every face of the body; other
    // body-wide items (snapped cut points, Joins) touch only a few faces.
    let mut body_slices: HashMap<BodyId, u64> = HashMap::new();
    let mut add = |b: BodyId, f: Option<FaceId>, n: u64| match f {
        Some(f) => *per_face.entry((b, f)).or_default() += n,
        None => *per_body.entry(b).or_default() += n,
    };
    for sl in &doc.split_lines {
        add(sl.body, Some(sl.face), 1);
    }
    for r in &doc.rules {
        match &r.rule {
            Rule::Connect {
                patches,
                count_a,
                count_b,
                ..
            } => {
                for p in patches {
                    add(
                        p.body,
                        Some(p.face),
                        u64::from(*count_a) + u64::from(*count_b),
                    );
                }
            }
            Rule::Grid {
                patch,
                count_u,
                count_v,
                ..
            } => {
                add(
                    patch.body,
                    Some(patch.face),
                    u64::from(*count_u) + u64::from(*count_v),
                );
            }
            Rule::Slice {
                scope,
                count: c,
                frozen_planes,
                ..
            } => {
                let n = frozen_planes
                    .as_ref()
                    .map_or(u64::from(*c), |p| p.len() as u64);
                match scope {
                    SliceScope::Body(b) => {
                        add(*b, None, n);
                        *body_slices.entry(*b).or_default() += n;
                    }
                    SliceScope::Patches(l) => l.iter().for_each(|p| add(p.body, Some(p.face), n)),
                }
            }
            Rule::Iso { pieces, .. } => pieces.iter().for_each(|p| add(p.body, Some(p.face), 1)),
            Rule::Cut { points, .. } => {
                for p in points {
                    match p {
                        CutPoint::Face { body, face, .. } => add(*body, Some(*face), 1),
                        CutPoint::CadEdge { body, .. } | CutPoint::Vertex { body, .. } => {
                            add(*body, None, 1)
                        }
                    }
                }
            }
            Rule::Join { body, .. } => add(*body, None, 1),
        }
    }
    // Body-scope Slice planes are evaluated on every face of the body, so
    // they count once per face; other body-wide items count once.
    let total: u64 = per_face.values().sum::<u64>()
        + per_body.values().sum::<u64>()
        + body_slices
            .iter()
            .map(|(b, n)| n.saturating_mul(faces_of(*b).saturating_sub(1)))
            .sum::<u64>();
    if total > MAX_DOC_LINES {
        bail!("the scene asks for {total} lines in total (at most {MAX_DOC_LINES})");
    }
    let body_wide = |b: BodyId| per_body.get(&b).copied().unwrap_or(0);
    for (&(b, f), &n) in &per_face {
        if n + body_wide(b) > MAX_FACE_LINES {
            bail!(
                "face {} of body {} would get {} lines (at most {MAX_FACE_LINES})",
                f.0,
                b.0,
                n + body_wide(b)
            );
        }
    }
    for (&b, &n) in &per_body {
        if n > MAX_FACE_LINES {
            bail!(
                "body {} would get {n} body-wide lines (at most {MAX_FACE_LINES})",
                b.0
            );
        }
    }
    Ok(())
}

/// Spread rules own patches exclusively (SPEC §4.3): ops keep that through
/// takeover, so a document where two spread rules (or one rule twice) name
/// the same patch, or a body-wide Slice shares its body with another spread
/// rule, didn't come from the app and would multiply evaluation work.
fn spread_ownership(doc: &Document) -> Result<()> {
    let mut owner: HashMap<PatchId, u32> = HashMap::new();
    let mut body_slice: HashMap<BodyId, u32> = HashMap::new();
    let mut claim = |p: &PatchId, rule: u32| -> Result<()> {
        if let Some(prev) = owner.insert(*p, rule) {
            bail!(
                "face {} of body {} is claimed by spread rules {prev} and {rule} (or twice by one rule)",
                p.face.0,
                p.body.0
            );
        }
        Ok(())
    };
    for r in &doc.rules {
        let id = r.id.0;
        match &r.rule {
            Rule::Connect {
                patches, flipped, ..
            } => {
                patches.iter().try_for_each(|p| claim(p, id))?;
                let mut seen = HashSet::new();
                for f in flipped {
                    if !seen.insert(*f) || !patches.contains(f) {
                        bail!("Connect rule {id} flips a patch it doesn't own, or one twice");
                    }
                }
            }
            Rule::Grid { patch, .. } => claim(patch, id)?,
            Rule::Slice {
                scope: SliceScope::Patches(l),
                ..
            } => l.iter().try_for_each(|p| claim(p, id))?,
            Rule::Slice {
                scope: SliceScope::Body(b),
                ..
            } => {
                if let Some(prev) = body_slice.insert(*b, id) {
                    bail!(
                        "body {} has two body-wide Slice rules ({prev} and {id})",
                        b.0
                    );
                }
            }
            _ => {}
        }
    }
    if let Some((p, rule)) = owner.iter().find(|(p, _)| body_slice.contains_key(&p.body)) {
        bail!(
            "body {} has a body-wide Slice and spread rule {rule} on face {}",
            p.body.0,
            p.face.0
        );
    }
    Ok(())
}

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

/// A normalised (0..1) iso value or seed, with a little slack.
pub(crate) fn norm(x: f64, what: &str) -> Result<()> {
    if !(x.is_finite() && (-ISO_SLACK..=1.0 + ISO_SLACK).contains(&x)) {
        bail!("{what} has an iso value {x} outside the face");
    }
    Ok(())
}

/// A parameter that must lie on (or just past) its range: within one range
/// width of either end.
pub(crate) fn near_range(x: f64, (lo, hi): (f64, f64), what: &str) -> Result<()> {
    let w = (hi - lo).abs().max(1e-9);
    if !(x.is_finite() && x >= lo.min(hi) - w && x <= lo.max(hi) + w) {
        bail!("{what} has a parameter {x} outside its range {lo}..{hi}");
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
    work_budget(doc, |b| topo_of(b).map_or(0, |t| t.faces.len() as u64))?;
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
        norm(value, &what)?;
        norm(seed.unwrap_or(0.0), &what)?;
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
                    norm(p.value, &what)?;
                    norm(p.seed.unwrap_or(0.0), &what)?;
                }
            }
            Rule::Cut { points, .. } => {
                for p in points {
                    match p {
                        CutPoint::Face { body, face, uv } => {
                            let tp = topo(*body, &what)?;
                            tp.face(*face, &what)?;
                            let sf = &tp.t.face(*face).surface;
                            near_range(uv.x, sf.u_range, &what)?;
                            near_range(uv.y, sf.v_range, &what)?;
                        }
                        CutPoint::CadEdge { body, edge, t } => {
                            let tp = topo(*body, &what)?;
                            tp.edge(*edge, &what)?;
                            near_range(*t, tp.t.edge(*edge).t_range, &what)?;
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
