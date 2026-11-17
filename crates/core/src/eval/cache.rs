//! Incremental evaluation (SPEC §4.6 "Incremental"): content-keyed caches of
//! the per-face arrangement stages and of each region's bridging + fill
//! result. A one-patch change re-runs the arrangement only for faces whose
//! inputs changed, and bridging/fill only for regions whose boundary walks
//! (vertex keys, edge owners, UV paths, neighbouring edges) changed.
//!
//! Keys are hashes of the full inputs, so a hit is always valid; entries not
//! used for a few evaluations are dropped.

use super::Owner;
use super::arr::{Arr, Glued};
use super::assemble::{HE, HMesh, Region};
use super::bridge::BridgeState;
use super::face::Stage1;
use glam::{DVec2, DVec3};
use newtopo_kernel::{EdgeId, FaceId};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// How many evaluations an unused entry survives.
const KEEP: u64 = 4;

#[derive(Clone, Debug)]
pub enum Step {
    /// The boundary half-edge leaving occurrence `k`.
    Boundary(usize),
    /// New chord `j`, forward (`true`) or backward.
    Chord(usize, bool),
}

#[derive(Clone, Debug)]
pub struct Chord {
    pub from_occ: usize,
    pub to_occ: usize,
    pub uv: Vec<DVec2>,
    pub p3: Option<Vec<DVec3>>,
}

/// Bridging + fill result of one region, in occurrence numbering.
#[derive(Clone, Debug)]
pub struct RegionRecord {
    pub chords: Vec<Chord>,
    pub polys: Vec<(Vec<Step>, bool)>,
}

/// Per-body cache.
#[derive(Default)]
pub struct EvalCache {
    generation: u64,
    pub stage1: HashMap<u64, (u64, Arc<Stage1>)>,
    pub stage3: HashMap<u64, (u64, (Arc<Arr>, Arc<Glued>))>,
    pub regions: HashMap<u64, (u64, Arc<RegionRecord>)>,
    /// Statistics of the last evaluation (hits, misses) per level.
    pub last: [(usize, usize); 3],
}

impl EvalCache {
    pub fn begin(&mut self) {
        self.generation += 1;
        self.last = [(0, 0); 3];
    }

    pub fn end(&mut self) {
        let g = self.generation;
        self.stage1.retain(|_, v| g - v.0 < KEEP);
        self.stage3.retain(|_, v| g - v.0 < KEEP);
        self.regions.retain(|_, v| g - v.0 < KEEP);
    }

    pub fn get1(&mut self, k: u64) -> Option<Arc<Stage1>> {
        let g = self.generation;
        let r = self.stage1.get_mut(&k).map(|v| {
            v.0 = g;
            v.1.clone()
        });
        self.count(0, r.is_some());
        r
    }
    /// Lookup without counting (used outside an evaluation).
    pub fn peek1(&mut self, k: u64) -> Option<Arc<Stage1>> {
        let g = self.generation;
        self.stage1.get_mut(&k).map(|v| {
            v.0 = g;
            v.1.clone()
        })
    }
    pub fn put1(&mut self, k: u64, v: Arc<Stage1>) {
        self.stage1.insert(k, (self.generation, v));
    }
    pub fn get3(&mut self, k: u64) -> Option<(Arc<Arr>, Arc<Glued>)> {
        let g = self.generation;
        let r = self.stage3.get_mut(&k).map(|v| {
            v.0 = g;
            v.1.clone()
        });
        self.count(1, r.is_some());
        r
    }
    pub fn put3(&mut self, k: u64, v: (Arc<Arr>, Arc<Glued>)) {
        self.stage3.insert(k, (self.generation, v));
    }
    pub fn get_region(&mut self, k: u64) -> Option<Arc<RegionRecord>> {
        let g = self.generation;
        let r = self.regions.get_mut(&k).map(|v| {
            v.0 = g;
            v.1.clone()
        });
        self.count(2, r.is_some());
        r
    }
    pub fn put_region(&mut self, k: u64, v: RegionRecord) {
        self.regions.insert(k, (self.generation, Arc::new(v)));
    }
    fn count(&mut self, level: usize, hit: bool) {
        if hit {
            self.last[level].0 += 1;
        } else {
            self.last[level].1 += 1;
        }
    }
}

pub fn hash_str(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// A region's canonical walks (each rotated to a canonical start; walks
/// sorted) and its signature.
pub struct RegionSig {
    pub key: u64,
    /// Boundary half-edges in occurrence order.
    pub occ: Vec<usize>,
}

pub fn region_sig(
    mesh: &HMesh,
    st: &BridgeState,
    pole_target: &HashMap<EdgeId, usize>,
    r: usize,
    autofill: bool,
) -> RegionSig {
    let walks = st.walks(mesh, r);
    let desc = |h: usize| -> String {
        let e = &mesh.he[h];
        let fv = mesh.final_v(e.from, pole_target);
        format!(
            "{:?}|{:?}|{:?}|{}|{:?}|{:?}",
            mesh.verts[fv].key, mesh.verts[e.from].key, e.owner, e.face.0, e.uv, e.p3
        )
    };
    let mut ws: Vec<(String, Vec<usize>)> = walks
        .iter()
        .map(|w| {
            let ds: Vec<String> = w.iter().map(|&h| desc(h)).collect();
            let start = (0..w.len())
                .min_by(|&a, &b| ds[a].cmp(&ds[b]).then(a.cmp(&b)))
                .unwrap_or(0);
            let n = w.len();
            let rot: Vec<usize> = (0..n).map(|k| w[(start + k) % n]).collect();
            let s: String = (0..n)
                .map(|k| ds[(start + k) % n].as_str())
                .collect::<Vec<_>>()
                .join(";");
            (s, rot)
        })
        .collect();
    ws.sort_by(|a, b| a.0.cmp(&b.0));
    // Edges joining vertices of this region (they constrain chords).
    let verts: HashSet<usize> = walks
        .iter()
        .flatten()
        .map(|&h| mesh.final_v(mesh.he[h].from, pole_target))
        .collect();
    let mut pairs: Vec<String> = Vec::new();
    for &v in &verts {
        if let Some(l) = st.adj.get(&v) {
            for &x in l {
                if verts.contains(&x) {
                    pairs.push(format!("{:?}-{:?}", mesh.verts[v].key, mesh.verts[x].key));
                }
            }
        }
    }
    pairs.sort();
    pairs.dedup();
    let reg = &mesh.regions[r];
    let full = format!(
        "{}|{}|{}|{}||{}",
        reg.face.0,
        reg.multi,
        autofill,
        ws.iter()
            .map(|w| w.0.as_str())
            .collect::<Vec<_>>()
            .join("#"),
        pairs.join(",")
    );
    RegionSig {
        key: hash_str(&full),
        occ: ws.into_iter().flat_map(|w| w.1).collect(),
    }
}

/// Record the decomposition of a region after bridging and fill.
pub fn record(
    mesh: &HMesh,
    st: &BridgeState,
    occ: &[usize],
    he_before: usize,
    regions: &[usize],
    fan: &HashSet<usize>,
) -> Option<RegionRecord> {
    let occ_of: HashMap<usize, usize> = occ.iter().enumerate().map(|(k, &h)| (h, k)).collect();
    let vert_occ = |v: usize| occ.iter().position(|&h| mesh.he[h].from == v);
    let mut chord_of: HashMap<usize, (usize, bool)> = HashMap::new();
    let mut chords: Vec<Chord> = Vec::new();
    let mut polys = Vec::new();
    for &r in regions {
        if !mesh.regions[r].alive {
            continue;
        }
        for w in st.walks(mesh, r) {
            let mut steps = Vec::with_capacity(w.len());
            for h in w {
                if let Some(&k) = occ_of.get(&h) {
                    steps.push(Step::Boundary(k));
                    continue;
                }
                if h < he_before {
                    return None; // a half-edge from outside the region
                }
                let step = match chord_of.get(&h) {
                    Some(&(j, fwd)) => Step::Chord(j, fwd),
                    None => {
                        let e = &mesh.he[h];
                        let j = chords.len();
                        chords.push(Chord {
                            from_occ: vert_occ(e.from)?,
                            to_occ: vert_occ(e.to)?,
                            uv: e.uv.clone(),
                            p3: e.p3.clone(),
                        });
                        chord_of.insert(h, (j, true));
                        if let Some(t) = e.twin {
                            chord_of.insert(t, (j, false));
                        }
                        Step::Chord(j, true)
                    }
                };
                steps.push(step);
            }
            polys.push((steps, fan.contains(&r)));
        }
    }
    Some(RegionRecord { chords, polys })
}

/// Replay a recorded decomposition onto region `r` (whose canonical walks
/// are `occ`). Returns the regions produced and those fan-filled.
pub fn replay(
    mesh: &mut HMesh,
    st: &mut BridgeState,
    pole_target: &HashMap<EdgeId, usize>,
    r: usize,
    occ: &[usize],
    rec: &RegionRecord,
) -> (Vec<usize>, Vec<usize>) {
    let face: FaceId = mesh.regions[r].face;
    let base = mesh.he.len();
    for c in &rec.chords {
        let (a, b) = (mesh.he[occ[c.from_occ]].from, mesh.he[occ[c.to_occ]].from);
        let h = mesh.he.len();
        let mut ruv = c.uv.clone();
        ruv.reverse();
        let rp3 = c.p3.as_ref().map(|v| v.iter().rev().copied().collect());
        mesh.he.push(HE {
            from: a,
            to: b,
            twin: Some(h + 1),
            next: h,
            region: r,
            owner: Owner::Fill,
            face,
            uv: c.uv.clone(),
            p3: c.p3.clone(),
            alive: true,
            src: vec![],
        });
        mesh.he.push(HE {
            from: b,
            to: a,
            twin: Some(h),
            next: h + 1,
            region: r,
            owner: Owner::Fill,
            face,
            uv: ruv,
            p3: rp3,
            alive: true,
            src: vec![],
        });
        let (fa, fb) = (mesh.final_v(a, pole_target), mesh.final_v(b, pole_target));
        st.adj.entry(fa).or_default().push(fb);
        st.adj.entry(fb).or_default().push(fa);
    }
    let he_of = |s: &Step| match *s {
        Step::Boundary(k) => occ[k],
        Step::Chord(j, fwd) => base + 2 * j + usize::from(!fwd),
    };
    let mut produced = Vec::new();
    let mut fan = Vec::new();
    let mut all: Vec<usize> = st.reg_he.remove(&r).unwrap_or_default();
    all.extend(base..mesh.he.len());
    for (i, (steps, is_fan)) in rec.polys.iter().enumerate() {
        let region = if i == 0 {
            r
        } else {
            let reg: Region = mesh.regions[r].clone();
            mesh.regions.push(Region { alive: true, ..reg });
            mesh.regions.len() - 1
        };
        let hs: Vec<usize> = steps.iter().map(he_of).collect();
        for k in 0..hs.len() {
            mesh.he[hs[k]].next = hs[(k + 1) % hs.len()];
            mesh.he[hs[k]].region = region;
        }
        produced.push(region);
        if *is_fan {
            fan.push(region);
        }
    }
    for h in all {
        st.reg_he.entry(mesh.he[h].region).or_default().push(h);
    }
    (produced, fan)
}
