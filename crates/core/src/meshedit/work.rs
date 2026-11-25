//! Mutable working copy shared by the mesh tools (own design).
//!
//! Polygons are vertex cycles with a vertex → polygon index; every local
//! change is planned first ([`Work::plan_merge`]), checked for topology
//! (Euler characteristic unchanged, manifold edges and fans, no duplicate
//! faces) and geometry (no flipped, degenerate or concave results, optional
//! deviation bound), then applied. Vertex positions are tracked by
//! provenance relative to the input mesh so per-corner normals and UVs can
//! be carried over exactly (kept vertices) or interpolated (vertices that
//! slid along an input edge).

use super::mesh::{EdgeFlags, EditMesh, EditPolygon, FlaggedEdge, area_vec, ekey, single_fan};
use super::refill::quad_quality;
use glam::{DVec2, DVec3};
use newtopo_kernel::FaceId;
use std::collections::{BTreeSet, HashMap, HashSet};

/// Internal edge bits: the three [`EdgeFlags`] plus the tool selection.
pub(crate) const F_CAD: u8 = 1;
pub(crate) const F_HARD: u8 = 2;
pub(crate) const F_SEAM: u8 = 4;
pub(crate) const F_SEL: u8 = 8;

/// Where a vertex position comes from, relative to the input mesh.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Prov {
    Orig(u32),
    /// Affine combination of input vertices (weights sum to 1), for
    /// vertices that slid along edges.
    Mix(Vec<(u32, f64)>),
}

impl Prov {
    fn terms(&self) -> Vec<(u32, f64)> {
        match self {
            Prov::Orig(s) => vec![(*s, 1.0)],
            Prov::Mix(m) => m.clone(),
        }
    }

    /// Equal-weight mean of several provenances.
    pub fn mean(all: &[Prov]) -> Prov {
        let mut acc = all.first().cloned().unwrap_or(Prov::Orig(0));
        for (k, p) in all.iter().enumerate().skip(1) {
            // Running mean: the k+1-th term gets weight 1/(k+1).
            acc = Prov::lerp(&acc, p, 1.0 / (k as f64 + 1.0));
        }
        acc
    }

    /// `(1 - t) a + t b`, merged by input vertex, tiny weights dropped.
    pub fn lerp(a: &Prov, b: &Prov, t: f64) -> Prov {
        if t <= 0.0 {
            return a.clone();
        }
        if t >= 1.0 {
            return b.clone();
        }
        let mut m: Vec<(u32, f64)> = Vec::new();
        for (s, w) in a
            .terms()
            .into_iter()
            .map(|(s, w)| (s, w * (1.0 - t)))
            .chain(b.terms().into_iter().map(|(s, w)| (s, w * t)))
        {
            match m.iter_mut().find(|e| e.0 == s) {
                Some(e) => e.1 += w,
                None => m.push((s, w)),
            }
        }
        m.retain(|e| e.1 > 1e-12);
        let total: f64 = m.iter().map(|e| e.1).sum();
        for e in &mut m {
            e.1 /= total;
        }
        m.sort_by_key(|e| e.0);
        if m.len() == 1 {
            Prov::Orig(m[0].0)
        } else {
            Prov::Mix(m)
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct WPoly {
    pub verts: Vec<u32>,
    pub group: FaceId,
    pub chart: u32,
    /// Input polygon this one descends from (attribute lookup).
    pub src: u32,
    pub alive: bool,
    pub touched: bool,
}

/// Vertex role for the feature rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VClass {
    /// No feature edge.
    Free,
    /// Exactly two feature edges of the same kind: may move along them.
    Curve([u32; 2]),
    /// Pinned, a feature end or junction, or two different feature kinds.
    Corner,
}

/// A planned merge of `u` into `v`.
#[derive(Clone, Debug)]
pub(crate) struct MergePlan {
    pub u: u32,
    pub v: u32,
    /// Affected polygons (all polygons of `u`), sorted.
    pub affected: Vec<u32>,
    /// Replacement pieces per affected polygon (same order).
    pub pieces: Vec<Vec<Vec<u32>>>,
    /// Vertices other than `u` left without polygons.
    pub dead: Vec<u32>,
}

impl MergePlan {
    pub fn kept_pieces(&self) -> impl Iterator<Item = &Vec<u32>> {
        self.pieces.iter().flatten()
    }
    pub fn quads_before(&self, w: &Work) -> usize {
        self.affected
            .iter()
            .filter(|&&p| w.polys[p as usize].verts.len() == 4)
            .count()
    }
    pub fn quads_after(&self) -> usize {
        self.kept_pieces().filter(|p| p.len() == 4).count()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Work<'a> {
    pub src: &'a EditMesh,
    pub diag: f64,
    pub pos: Vec<DVec3>,
    pub prov: Vec<Prov>,
    pub valive: Vec<bool>,
    pub pinned: Vec<bool>,
    pub polys: Vec<WPoly>,
    pub vpolys: Vec<Vec<u32>>,
    pub flags: HashMap<(u32, u32), u8>,
    /// Bits that count as feature edges (boundary always does).
    pub feature_mask: u8,
    /// Per polygon: input vertices that were removed or moved and whose
    /// original position must stay within `max_dev` of this polygon.
    pub witnesses: Option<Vec<Vec<u32>>>,
    pub max_dev: f64,
    /// Smallest cosine between a changed polygon's normal and its CAD
    /// reference normal.
    pub min_cos: f64,
    /// Shape guard (0 = off): a change may not leave its worst polygon
    /// (by [`quad_quality`]) worse than both the worst before and this.
    pub min_quality: f64,
    /// Input vertices merged into a working vertex by Merge by Distance.
    /// A polygon corner that was one of them takes its exact normal and UV
    /// from that input corner.
    pub aliases: HashMap<u32, Vec<u32>>,
    src_normals: HashMap<(u32, FaceId), DVec3>,
    /// Per input vertex: the distinct (chart, uv) pairs of its corners.
    src_uvs: HashMap<u32, Vec<(u32, DVec2)>>,
}

impl<'a> Work<'a> {
    /// `src` must have passed [`EditMesh::issues`].
    pub fn new(src: &'a EditMesh, feature_mask: u8) -> Work<'a> {
        let nv = src.positions.len();
        let mut flags: HashMap<(u32, u32), u8> = HashMap::new();
        for e in &src.flagged {
            if e.a != e.b {
                *flags.entry(ekey(e.a, e.b)).or_default() |= e.flags.bits();
            }
        }
        let mut vpolys = vec![Vec::new(); nv];
        for (i, p) in src.polygons.iter().enumerate() {
            for &v in &p.verts {
                vpolys[v as usize].push(i as u32);
            }
        }
        // UV seams: corners on the two sides of an edge disagree.
        let has_uv = src.has_uvs();
        if has_uv {
            let mut side: HashMap<(u32, u32), (DVec2, DVec2)> = HashMap::new();
            for p in &src.polygons {
                let uv = p.uvs.as_ref().expect("checked by issues()");
                let n = p.verts.len();
                for i in 0..n {
                    let j = (i + 1) % n;
                    let (a, b) = (p.verts[i], p.verts[j]);
                    let (ua, ub) = (uv[i], uv[j]);
                    let k = ekey(a, b);
                    let val = if a < b { (ua, ub) } else { (ub, ua) };
                    if let Some(o) = side.get(&k) {
                        if o.0.distance(val.0) > 1e-9 || o.1.distance(val.1) > 1e-9 {
                            *flags.entry(k).or_default() |= F_SEAM;
                        }
                    } else {
                        side.insert(k, val);
                    }
                }
            }
        }
        let charts = charts(src, &flags);
        let mut src_normals = HashMap::new();
        let mut src_uvs = HashMap::new();
        for (i, p) in src.polygons.iter().enumerate() {
            for (k, &v) in p.verts.iter().enumerate() {
                if let Some(n) = p.normals.get(k) {
                    src_normals.entry((v, p.group)).or_insert(*n);
                }
                if let Some(uv) = &p.uvs {
                    let list: &mut Vec<(u32, DVec2)> = src_uvs.entry(v).or_default();
                    if !list
                        .iter()
                        .any(|(c, u)| *c == charts[i] && u.distance(uv[k]) <= 1e-12)
                    {
                        list.push((charts[i], uv[k]));
                    }
                }
            }
        }
        let polys = src
            .polygons
            .iter()
            .enumerate()
            .map(|(i, p)| WPoly {
                verts: p.verts.clone(),
                group: p.group,
                chart: charts[i],
                src: i as u32,
                alive: true,
                touched: false,
            })
            .collect();
        let mut pinned = src.pinned.clone();
        pinned.resize(nv, false);
        Work {
            src,
            diag: src.diagonal().max(1e-300),
            pos: src.positions.clone(),
            prov: (0..nv as u32).map(Prov::Orig).collect(),
            valive: (0..nv).map(|v| !vpolys[v].is_empty()).collect(),
            pinned,
            polys,
            vpolys,
            flags,
            feature_mask: feature_mask | F_SEAM,
            witnesses: None,
            max_dev: f64::INFINITY,
            min_cos: 0.5,
            min_quality: 0.0,
            aliases: HashMap::new(),
            src_normals,
            src_uvs,
        }
    }

    pub fn track_deviation(&mut self, max_dev: f64) {
        self.max_dev = max_dev;
        self.witnesses = Some(vec![Vec::new(); self.polys.len()]);
    }

    // ---------------------------------------------------------------- queries

    pub fn flag(&self, a: u32, b: u32) -> u8 {
        self.flags.get(&ekey(a, b)).copied().unwrap_or(0)
    }

    /// Polygons that contain the edge `a`-`b` (as consecutive corners).
    pub fn edge_polys(&self, a: u32, b: u32) -> Vec<u32> {
        self.vpolys[a as usize]
            .iter()
            .copied()
            .filter(|&p| has_edge(&self.polys[p as usize].verts, a, b))
            .collect()
    }

    pub fn is_boundary(&self, a: u32, b: u32) -> bool {
        self.edge_polys(a, b).len() == 1
    }

    /// Feature bits of an edge, with bit 7 for boundary.
    pub fn feature_bits(&self, a: u32, b: u32) -> u8 {
        let mut f = self.flag(a, b) & self.feature_mask;
        if self.is_boundary(a, b) {
            f |= 0x80;
        }
        f
    }

    pub fn is_feature(&self, a: u32, b: u32) -> bool {
        self.feature_bits(a, b) != 0
    }

    /// Edge neighbours, sorted.
    pub fn neighbors(&self, x: u32) -> Vec<u32> {
        let mut s = BTreeSet::new();
        for &p in &self.vpolys[x as usize] {
            let v = &self.polys[p as usize].verts;
            let n = v.len();
            if let Some(i) = v.iter().position(|&y| y == x) {
                s.insert(v[(i + 1) % n]);
                s.insert(v[(i + n - 1) % n]);
            }
        }
        s.into_iter().collect()
    }

    /// Opposite corners of `x` in its quads, sorted.
    pub fn diagonal_neighbors(&self, x: u32) -> Vec<u32> {
        let mut s = BTreeSet::new();
        for &p in &self.vpolys[x as usize] {
            let v = &self.polys[p as usize].verts;
            if v.len() == 4
                && let Some(i) = v.iter().position(|&y| y == x)
            {
                s.insert(v[(i + 2) % 4]);
            }
        }
        s.into_iter().collect()
    }

    pub fn feature_neighbors(&self, x: u32) -> Vec<u32> {
        self.neighbors(x)
            .into_iter()
            .filter(|&y| self.is_feature(x, y))
            .collect()
    }

    pub fn vclass(&self, x: u32) -> VClass {
        if self.pinned[x as usize] {
            return VClass::Corner;
        }
        let f = self.feature_neighbors(x);
        match f.len() {
            0 => VClass::Free,
            2 if self.feature_bits(x, f[0]) == self.feature_bits(x, f[1]) => {
                VClass::Curve([f[0], f[1]])
            }
            _ => VClass::Corner,
        }
    }

    /// Feature rule shared by every tool: may `u` be merged into its
    /// neighbour `v` without bending a feature curve or losing a corner?
    pub fn merge_allowed(&self, u: u32, v: u32) -> bool {
        if u == v || !self.valive[u as usize] || !self.valive[v as usize] {
            return false;
        }
        match self.vclass(u) {
            VClass::Free => true,
            VClass::Corner => false,
            VClass::Curve([p, q]) => {
                if v != p && v != q {
                    return false;
                }
                let other = if v == p { q } else { p };
                // A feature loop must keep at least three vertices.
                !(other == v || self.neighbors(other).contains(&v) && self.is_feature(other, v))
            }
        }
    }

    pub fn poly_pos(&self, verts: &[u32]) -> Vec<DVec3> {
        verts.iter().map(|&v| self.pos[v as usize]).collect()
    }

    pub fn tri_count(&self) -> usize {
        self.polys
            .iter()
            .filter(|p| p.alive)
            .map(|p| p.verts.len() - 2)
            .sum()
    }

    pub fn alive_polys(&self) -> impl Iterator<Item = u32> + '_ {
        (0..self.polys.len() as u32).filter(|&p| self.polys[p as usize].alive)
    }

    /// Corner links `(prev, next)` of `x` over the given polygons.
    fn links(polys: &[&[u32]], x: u32) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        for v in polys {
            let n = v.len();
            for i in 0..n {
                if v[i] == x {
                    out.push((v[(i + n - 1) % n], v[(i + 1) % n]));
                }
            }
        }
        out
    }

    // ------------------------------------------------------------------ merge

    /// Plan merging `u` into `v` (any two vertices sharing a polygon).
    /// `None` if the result would change the topology or be non-manifold.
    pub fn plan_merge(&self, u: u32, v: u32) -> Option<MergePlan> {
        if u == v {
            return None;
        }
        let affected: Vec<u32> = self.vpolys[u as usize].clone();
        if !affected
            .iter()
            .any(|&p| self.polys[p as usize].verts.contains(&v))
        {
            return None;
        }
        let aset: HashSet<u32> = affected.iter().copied().collect();
        let mut pieces = Vec::with_capacity(affected.len());
        for &p in &affected {
            pieces.push(merge_pieces(&self.polys[p as usize].verts, u, v));
        }
        let kept: Vec<&Vec<u32>> = pieces.iter().flatten().collect();

        // Vertices touched (all corners of affected polygons except u).
        let mut touched: BTreeSet<u32> = BTreeSet::new();
        for &p in &affected {
            for &x in &self.polys[p as usize].verts {
                if x != u {
                    touched.insert(x);
                }
            }
        }
        let others = |x: u32| -> Vec<u32> {
            self.vpolys[x as usize]
                .iter()
                .copied()
                .filter(|p| !aset.contains(p))
                .collect()
        };
        let in_unaffected = |a: u32, b: u32| -> bool {
            self.vpolys[a as usize]
                .iter()
                .any(|p| !aset.contains(p) && has_edge(&self.polys[*p as usize].verts, a, b))
        };

        // Dead vertices.
        let mut dead = Vec::new();
        for &x in &touched {
            if others(x).is_empty() && !kept.iter().any(|k| k.contains(&x)) {
                dead.push(x);
            }
        }

        // Euler characteristic.
        let mut ea: BTreeSet<(u32, u32)> = BTreeSet::new();
        for &p in &affected {
            let vv = &self.polys[p as usize].verts;
            for i in 0..vv.len() {
                ea.insert(ekey(vv[i], vv[(i + 1) % vv.len()]));
            }
        }
        let mut ep: BTreeSet<(u32, u32)> = BTreeSet::new();
        for k in &kept {
            for i in 0..k.len() {
                ep.insert(ekey(k[i], k[(i + 1) % k.len()]));
            }
        }
        let de = ep.iter().filter(|e| !in_unaffected(e.0, e.1)).count() as i64
            - ea.iter().filter(|e| !in_unaffected(e.0, e.1)).count() as i64;
        let df = kept.len() as i64 - affected.len() as i64;
        let dv = -1 - dead.len() as i64;
        if dv - de + df != 0 {
            return None;
        }

        // Directed edges unique, undirected used at most twice.
        let mut dir: HashMap<(u32, u32), u32> = HashMap::new();
        for k in &kept {
            for i in 0..k.len() {
                *dir.entry((k[i], k[(i + 1) % k.len()])).or_default() += 1;
            }
        }
        for (&(a, b), &c) in &dir {
            let mut same = c;
            let mut any = c + dir.get(&(b, a)).copied().unwrap_or(0);
            for &p in &self.vpolys[a as usize] {
                if aset.contains(&p) {
                    continue;
                }
                let vv = &self.polys[p as usize].verts;
                if has_directed(vv, a, b) {
                    same += 1;
                    any += 1;
                } else if has_directed(vv, b, a) {
                    any += 1;
                }
            }
            if same > 1 || any > 2 {
                return None;
            }
        }

        // Manifold fans and no duplicate faces.
        for &x in &touched {
            if dead.contains(&x) {
                continue;
            }
            let oth = others(x);
            let mut ps: Vec<&[u32]> = oth
                .iter()
                .map(|&p| self.polys[p as usize].verts.as_slice())
                .collect();
            for k in &kept {
                if k.contains(&x) {
                    ps.push(k.as_slice());
                }
            }
            if !single_fan(&Self::links(&ps, x)) {
                return None;
            }
            let mut seen: HashSet<Vec<u32>> = HashSet::new();
            for p in &ps {
                let mut s = p.to_vec();
                s.sort_unstable();
                if !seen.insert(s) {
                    return None;
                }
            }
        }
        Some(MergePlan {
            u,
            v,
            affected,
            pieces,
            dead,
        })
    }

    /// Geometric check of a planned merge: every piece keeps a non-zero
    /// area, its normal stays close to the CAD normal and to the polygon's
    /// previous normal, and quads stay convex.
    pub fn merge_geometry_ok(&self, plan: &MergePlan) -> bool {
        for (i, &p) in plan.affected.iter().enumerate() {
            let old = area_vec(&self.poly_pos(&self.polys[p as usize].verts)).normalize_or_zero();
            for piece in &plan.pieces[i] {
                if !self.piece_ok(piece, &self.polys[p as usize], old, None) {
                    return false;
                }
            }
        }
        if self.min_quality > 0.0 {
            let (before, after) = self.merge_quality(plan);
            if after < before.min(self.min_quality) - 1e-12 {
                return false;
            }
        }
        true
    }

    /// Worst polygon shape among the affected polygons before and the
    /// pieces after a planned merge.
    pub fn merge_quality(&self, plan: &MergePlan) -> (f64, f64) {
        let before = plan
            .affected
            .iter()
            .map(|&p| quad_quality(&self.poly_pos(&self.polys[p as usize].verts)))
            .fold(1.0, f64::min);
        let after = plan
            .kept_pieces()
            .map(|k| quad_quality(&self.poly_pos(k)))
            .fold(1.0, f64::min);
        (before, after)
    }

    /// Check one polygon (vertex cycle) using the current positions, with
    /// `moved` optionally overriding one vertex.
    pub fn piece_ok(
        &self,
        verts: &[u32],
        parent: &WPoly,
        old_normal: DVec3,
        moved: Option<(u32, DVec3)>,
    ) -> bool {
        let pos: Vec<DVec3> = verts
            .iter()
            .map(|&v| match moved {
                Some((m, p)) if m == v => p,
                _ => self.pos[v as usize],
            })
            .collect();
        polygon_shape_ok(&pos, self.diag) && {
            let n = area_vec(&pos).normalize_or_zero();
            let r = self.reference_normal(verts, parent);
            n.dot(r) >= self.min_cos && (old_normal == DVec3::ZERO || n.dot(old_normal) > 0.0)
        }
    }

    /// Mean CAD normal at the corners of a polygon (from the input
    /// attributes), falling back to its geometric normal.
    pub fn reference_normal(&self, verts: &[u32], parent: &WPoly) -> DVec3 {
        let mut s = DVec3::ZERO;
        for &v in verts {
            if let Some(n) = self.corner_normal(v, parent) {
                s += n;
            }
        }
        let s = s.normalize_or_zero();
        if s == DVec3::ZERO {
            area_vec(&self.poly_pos(verts)).normalize_or_zero()
        } else {
            s
        }
    }

    /// Deviation bound for a planned merge: every witness of the affected
    /// polygons, and `u` itself, stays within `max_dev` of the new
    /// polygons around `v`. Returns the redistribution on success.
    pub fn merge_deviation(&self, plan: &MergePlan) -> Option<Vec<(u32, usize)>> {
        let Some(wit) = &self.witnesses else {
            return Some(Vec::new());
        };
        let mut w: Vec<u32> = Vec::new();
        for &p in &plan.affected {
            w.extend_from_slice(&wit[p as usize]);
        }
        if (plan.u as usize) < self.src.positions.len() {
            w.push(plan.u);
        }
        for &d in &plan.dead {
            if (d as usize) < self.src.positions.len() {
                w.push(d);
            }
        }
        // Candidate surfaces: the kept pieces and the other polygons of v.
        let mut surf: Vec<Vec<DVec3>> = plan.kept_pieces().map(|k| self.poly_pos(k)).collect();
        let aset: HashSet<u32> = plan.affected.iter().copied().collect();
        for &p in &self.vpolys[plan.v as usize] {
            if !aset.contains(&p) {
                surf.push(self.poly_pos(&self.polys[p as usize].verts));
            }
        }
        assign_witnesses(&w, &surf, &self.src.positions, self.max_dev)
    }

    /// Apply a planned merge. Returns the ids of the polygons that now
    /// replace the affected ones.
    pub fn apply_merge(&mut self, plan: &MergePlan, wit: Vec<(u32, usize)>) -> Vec<u32> {
        let (u, v) = (plan.u, plan.v);
        // Detach affected polygons.
        for &p in &plan.affected {
            for &x in &self.polys[p as usize].verts.clone() {
                self.vpolys[x as usize].retain(|&q| q != p);
            }
        }
        let mut new_ids = Vec::new();
        for (i, &p) in plan.affected.iter().enumerate() {
            let parent = self.polys[p as usize].clone();
            let mut first = true;
            for piece in &plan.pieces[i] {
                let id = if first {
                    first = false;
                    let w = &mut self.polys[p as usize];
                    w.verts = piece.clone();
                    w.touched = true;
                    p
                } else {
                    self.polys.push(WPoly {
                        verts: piece.clone(),
                        touched: true,
                        ..parent.clone()
                    });
                    if let Some(ws) = &mut self.witnesses {
                        ws.push(Vec::new());
                    }
                    (self.polys.len() - 1) as u32
                };
                new_ids.push(id);
            }
            if first {
                self.polys[p as usize].alive = false;
            }
            if let Some(ws) = &mut self.witnesses {
                ws[p as usize].clear();
            }
        }
        for &id in &new_ids {
            for &x in &self.polys[id as usize].verts.clone() {
                let vp = &mut self.vpolys[x as usize];
                if let Err(at) = vp.binary_search(&id) {
                    vp.insert(at, id);
                }
            }
        }
        // Witnesses: index into [kept pieces in order..., other polys of v].
        if let Some(ws) = &mut self.witnesses {
            let aset: HashSet<u32> = plan.affected.iter().copied().collect();
            let mut targets: Vec<u32> = new_ids.clone();
            for &p in &self.vpolys[v as usize] {
                if !aset.contains(&p) && !new_ids.contains(&p) {
                    targets.push(p);
                }
            }
            // `merge_deviation` enumerated v's other polygons before the
            // merge; their order is unchanged (sorted vpolys minus the new).
            for (w, slot) in wit {
                if let Some(&t) = targets.get(slot) {
                    ws[t as usize].push(w);
                }
            }
        }
        // Flags: edges of u move to v.
        let moved: Vec<((u32, u32), u8)> = self
            .flags
            .iter()
            .filter(|(k, _)| k.0 == u || k.1 == u)
            .map(|(k, f)| (*k, *f))
            .collect();
        let mut moved = moved;
        moved.sort();
        for (k, f) in moved {
            self.flags.remove(&k);
            let w = if k.0 == u { k.1 } else { k.0 };
            if w != v {
                *self.flags.entry(ekey(v, w)).or_default() |= f;
            }
        }
        self.valive[u as usize] = false;
        self.vpolys[u as usize].clear();
        for &d in &plan.dead {
            self.valive[d as usize] = false;
        }
        new_ids
    }

    /// Plan + check + apply. Returns false (and changes nothing) if any
    /// check fails.
    pub fn try_merge(&mut self, u: u32, v: u32) -> bool {
        if !self.merge_allowed(u, v) {
            return false;
        }
        let Some(plan) = self.plan_merge(u, v) else {
            return false;
        };
        if !self.merge_geometry_ok(&plan) {
            return false;
        }
        let Some(wit) = self.merge_deviation(&plan) else {
            return false;
        };
        self.apply_merge(&plan, wit);
        true
    }

    // ------------------------------------------------------------ move/split

    /// Check moving `x` to `p`: every polygon around it keeps its shape and
    /// orientation, and the deviation bound holds.
    pub fn move_ok(&self, x: u32, p: DVec3) -> bool {
        let (mut before, mut after) = (1.0f64, 1.0f64);
        for &q in &self.vpolys[x as usize] {
            let w = &self.polys[q as usize];
            let pos = self.poly_pos(&w.verts);
            let old = area_vec(&pos).normalize_or_zero();
            if !self.piece_ok(&w.verts, w, old, Some((x, p))) {
                return false;
            }
            if self.min_quality > 0.0 {
                before = before.min(quad_quality(&pos));
                let moved: Vec<DVec3> = w
                    .verts
                    .iter()
                    .map(|&v| if v == x { p } else { self.pos[v as usize] })
                    .collect();
                after = after.min(quad_quality(&moved));
            }
        }
        if self.min_quality > 0.0 && after < before.min(self.min_quality) - 1e-12 {
            return false;
        }
        self.move_deviation(x, p).is_some()
    }

    pub fn move_deviation(&self, x: u32, p: DVec3) -> Option<Vec<(u32, usize)>> {
        let Some(wit) = &self.witnesses else {
            return Some(Vec::new());
        };
        let around = &self.vpolys[x as usize];
        let mut w: Vec<u32> = Vec::new();
        for &q in around {
            w.extend_from_slice(&wit[q as usize]);
        }
        if (x as usize) < self.src.positions.len() {
            w.push(x);
        }
        let surf: Vec<Vec<DVec3>> = around
            .iter()
            .map(|&q| {
                self.polys[q as usize]
                    .verts
                    .iter()
                    .map(|&v| if v == x { p } else { self.pos[v as usize] })
                    .collect()
            })
            .collect();
        assign_witnesses(&w, &surf, &self.src.positions, self.max_dev)
    }

    /// Move `x` (unchecked) and record its provenance.
    pub fn set_position(&mut self, x: u32, p: DVec3, prov: Prov) {
        self.pos[x as usize] = p;
        self.prov[x as usize] = prov;
        for &q in &self.vpolys[x as usize].clone() {
            self.polys[q as usize].touched = true;
        }
    }

    /// Checked move with witness redistribution.
    pub fn try_move(&mut self, x: u32, p: DVec3, prov: Prov) -> bool {
        if !self.move_ok(x, p) {
            return false;
        }
        let wit = self.move_deviation(x, p).unwrap_or_default();
        let around = self.vpolys[x as usize].clone();
        if let Some(ws) = &mut self.witnesses {
            for &q in &around {
                ws[q as usize].clear();
            }
            for (w, slot) in wit {
                ws[around[slot] as usize].push(w);
            }
        }
        self.set_position(x, p, prov);
        true
    }

    /// Insert new vertices along the edge `a`→`b` (in that order) in every
    /// polygon that uses the edge. Returns their ids.
    pub fn split_edge(&mut self, a: u32, b: u32, pts: &[(DVec3, Prov)]) -> Vec<u32> {
        let mut ids = Vec::new();
        for (p, pr) in pts {
            self.pos.push(*p);
            self.prov.push(pr.clone());
            self.valive.push(true);
            self.pinned.push(false);
            self.vpolys.push(Vec::new());
            ids.push((self.pos.len() - 1) as u32);
        }
        for q in self.edge_polys(a, b) {
            let w = &mut self.polys[q as usize];
            let n = w.verts.len();
            let i = (0..n)
                .find(|&i| {
                    let (x, y) = (w.verts[i], w.verts[(i + 1) % n]);
                    (x, y) == (a, b) || (x, y) == (b, a)
                })
                .expect("edge_polys");
            let ins: Vec<u32> = if w.verts[i] == a {
                ids.clone()
            } else {
                ids.iter().rev().copied().collect()
            };
            let at = i + 1;
            w.verts.splice(at..at, ins);
            w.touched = true;
            for &id in &ids {
                self.vpolys[id as usize].push(q);
            }
        }
        for &id in &ids {
            self.vpolys[id as usize].sort_unstable();
        }
        if let Some(f) = self.flags.remove(&ekey(a, b)) {
            let mut chain = vec![a];
            chain.extend_from_slice(&ids);
            chain.push(b);
            for w in chain.windows(2) {
                *self.flags.entry(ekey(w[0], w[1])).or_default() |= f;
            }
        }
        ids
    }

    /// Replace polygon `q` with `pieces` (same orientation, same vertices).
    pub fn replace_polygon(&mut self, q: u32, pieces: Vec<Vec<u32>>) {
        let parent = self.polys[q as usize].clone();
        for &x in &parent.verts {
            self.vpolys[x as usize].retain(|&p| p != q);
        }
        let wit = self
            .witnesses
            .as_mut()
            .map(|w| std::mem::take(&mut w[q as usize]));
        let mut ids = Vec::new();
        for (i, piece) in pieces.into_iter().enumerate() {
            let id = if i == 0 {
                let w = &mut self.polys[q as usize];
                w.verts = piece;
                w.touched = true;
                q
            } else {
                self.polys.push(WPoly {
                    verts: piece,
                    touched: true,
                    ..parent.clone()
                });
                if let Some(ws) = &mut self.witnesses {
                    ws.push(Vec::new());
                }
                (self.polys.len() - 1) as u32
            };
            ids.push(id);
        }
        for &id in &ids {
            for &x in &self.polys[id as usize].verts.clone() {
                let vp = &mut self.vpolys[x as usize];
                if let Err(at) = vp.binary_search(&id) {
                    vp.insert(at, id);
                }
            }
        }
        if let (Some(ws), Some(w)) = (&mut self.witnesses, wit) {
            ws[ids[0] as usize] = w;
        }
    }

    // ------------------------------------------------------------- attributes

    fn src_corner<T: Copy>(
        &self,
        s: u32,
        parent: &WPoly,
        get: impl Fn(&EditPolygon, usize) -> Option<T>,
    ) -> Option<T> {
        let sp = &self.src.polygons[parent.src as usize];
        sp.verts
            .iter()
            .position(|&v| v == s)
            .and_then(|k| get(sp, k))
    }

    fn normal_at_src(&self, s: u32, parent: &WPoly) -> Option<DVec3> {
        self.src_corner(s, parent, |p, k| p.normals.get(k).copied())
            .or_else(|| self.src_normals.get(&(s, parent.group)).copied())
            .or_else(|| {
                // Mean over the vertex's other groups.
                let mut acc = DVec3::ZERO;
                for &q in &self.src_vpolys(s) {
                    let sp = &self.src.polygons[q as usize];
                    if let Some(k) = sp.verts.iter().position(|&v| v == s)
                        && let Some(n) = sp.normals.get(k)
                    {
                        acc += *n;
                    }
                }
                let acc = acc.normalize_or_zero();
                (acc != DVec3::ZERO).then_some(acc)
            })
    }

    fn src_vpolys(&self, s: u32) -> Vec<u32> {
        (0..self.src.polygons.len() as u32)
            .filter(|&q| self.src.polygons[q as usize].verts.contains(&s))
            .collect()
    }

    /// UV of input vertex `s` seen from a polygon descending from `parent`:
    /// exact if the parent input polygon has the corner, otherwise the
    /// vertex's UV in the same chart nearest to `near` (a chart can meet
    /// itself along a seam, so one vertex may have several UVs in it).
    fn uv_at_src(&self, s: u32, parent: &WPoly, near: Option<DVec2>) -> Option<DVec2> {
        if let Some(uv) = self.src_corner(s, parent, |p, k| p.uvs.as_ref().map(|u| u[k])) {
            return Some(uv);
        }
        let cands = self.src_uvs.get(&s)?;
        let mut it = cands
            .iter()
            .filter(|(c, _)| *c == parent.chart)
            .map(|e| e.1);
        match near {
            None => it.next(),
            Some(r) => it.min_by(|a, b| a.distance(r).total_cmp(&b.distance(r))),
        }
    }

    /// UV of a corner if every term of its provenance is a corner of the
    /// parent input polygon.
    fn exact_uv(&self, v: u32, parent: &WPoly) -> Option<DVec2> {
        let get = |s: u32| self.src_corner(s, parent, |p, k| p.uvs.as_ref().map(|u| u[k]));
        match &self.prov[v as usize] {
            Prov::Orig(s) => get(*s),
            Prov::Mix(m) => {
                let mut uv = DVec2::ZERO;
                for &(s, w) in m {
                    uv += get(s)? * w;
                }
                Some(uv)
            }
        }
    }

    /// Exact attribute of an input vertex merged into `v`, if it is a
    /// corner of the parent input polygon.
    fn alias_corner<T: Copy>(
        &self,
        v: u32,
        parent: &WPoly,
        get: impl Fn(&EditPolygon, usize) -> Option<T> + Copy,
    ) -> Option<T> {
        self.aliases
            .get(&v)?
            .iter()
            .find_map(|&a| self.src_corner(a, parent, get))
    }

    /// Input-attribute normal at the corner `v` of a polygon descending
    /// from `parent`.
    pub fn corner_normal(&self, v: u32, parent: &WPoly) -> Option<DVec3> {
        let exact = |p: &EditPolygon, k: usize| p.normals.get(k).copied();
        match &self.prov[v as usize] {
            // The vertex's own normal in this polygon or CAD face first; a
            // vertex merged into it only if it has none there (welded across
            // a CAD face border).
            Prov::Orig(s) => self
                .src_corner(*s, parent, exact)
                .or_else(|| self.src_normals.get(&(*s, parent.group)).copied())
                .or_else(|| self.alias_corner(v, parent, exact))
                .or_else(|| self.normal_at_src(*s, parent)),
            Prov::Mix(_) => self.mixed_normal(v, parent),
        }
    }

    fn mixed_normal(&self, v: u32, parent: &WPoly) -> Option<DVec3> {
        match &self.prov[v as usize] {
            Prov::Orig(s) => self.normal_at_src(*s, parent),
            Prov::Mix(m) => {
                let mut n = DVec3::ZERO;
                for &(s, w) in m {
                    n += self.normal_at_src(s, parent)? * w;
                }
                let n = n.normalize_or_zero();
                (n != DVec3::ZERO).then_some(n)
            }
        }
    }

    pub fn corner_uv(&self, v: u32, parent: &WPoly, near: Option<DVec2>) -> Option<DVec2> {
        match &self.prov[v as usize] {
            Prov::Orig(s) => self.uv_at_src(*s, parent, near),
            Prov::Mix(m) => {
                let mut uv = DVec2::ZERO;
                for &(s, w) in m {
                    uv += self.uv_at_src(s, parent, near)? * w;
                }
                Some(uv)
            }
        }
    }

    /// UVs of a polygon's corners. Corners that are corners of the parent
    /// input polygon, or whose vertex has a single UV in the chart, are
    /// certain; the others (vertices on a seam where the chart meets
    /// itself) take their UV nearest to the mean of the certain ones, or of
    /// the parent input polygon if none is certain.
    pub fn polygon_uvs(&self, p: &WPoly) -> Vec<DVec2> {
        let unique = |v: u32| -> Option<DVec2> {
            let one = |s: u32| -> Option<DVec2> {
                let mut it = self
                    .src_uvs
                    .get(&s)?
                    .iter()
                    .filter(|(c, _)| *c == p.chart)
                    .map(|e| e.1);
                let first = it.next()?;
                it.next().is_none().then_some(first)
            };
            match &self.prov[v as usize] {
                Prov::Orig(s) => one(*s),
                Prov::Mix(m) => {
                    let mut uv = DVec2::ZERO;
                    for &(s, w) in m {
                        uv += one(s)? * w;
                    }
                    Some(uv)
                }
            }
        };
        // Last resort for merged vertices (Merge by Distance) that have no
        // UV of their own in this chart (welded across charts): the UV of
        // the input corner merged into them, or the mean of the provenance
        // terms that have a UV in the chart.
        let alias_uv = |v: u32| -> Option<DVec2> {
            if !self.aliases.contains_key(&v) {
                return None;
            }
            if let Some(uv) = self.alias_corner(v, p, |q, k| q.uvs.as_ref().map(|u| u[k])) {
                return Some(uv);
            }
            let Prov::Mix(m) = &self.prov[v as usize] else {
                return None;
            };
            let (mut uv, mut total) = (DVec2::ZERO, 0.0);
            for &(s, w) in m {
                let mut it = self
                    .src_uvs
                    .get(&s)
                    .into_iter()
                    .flatten()
                    .filter(|(c, _)| *c == p.chart);
                if let (Some(&(_, u)), None) = (it.next(), it.next()) {
                    uv += u * w;
                    total += w;
                }
            }
            (total > 0.0).then(|| uv / total)
        };
        let certain: Vec<Option<DVec2>> = p
            .verts
            .iter()
            .map(|&v| self.exact_uv(v, p).or_else(|| unique(v)))
            .collect();
        let known: Vec<DVec2> = certain.iter().flatten().copied().collect();
        let near = if known.is_empty() {
            self.src.polygons[p.src as usize]
                .uvs
                .as_ref()
                .map(|u| u.iter().copied().sum::<DVec2>() / u.len().max(1) as f64)
        } else {
            Some(known.iter().copied().sum::<DVec2>() / known.len() as f64)
        };
        p.verts
            .iter()
            .zip(&certain)
            .map(|(&v, e)| {
                e.or_else(|| self.corner_uv(v, p, near))
                    .or_else(|| alias_uv(v))
                    .unwrap_or(DVec2::ZERO)
            })
            .collect()
    }

    // ----------------------------------------------------------------- output

    /// Compact into an [`EditMesh`]. Returns the mesh and the map from
    /// working vertex ids (input ids first) to output ids.
    pub fn finish(&self) -> (EditMesh, Vec<Option<u32>>) {
        let n = self.pos.len();
        let mut used = vec![false; n];
        for p in self.polys.iter().filter(|p| p.alive) {
            for &v in &p.verts {
                used[v as usize] = true;
            }
        }
        let mut map = vec![None; n];
        let mut positions = Vec::new();
        let mut pinned = Vec::new();
        for v in 0..n {
            if used[v] {
                map[v] = Some(positions.len() as u32);
                positions.push(self.pos[v]);
                pinned.push(self.pinned[v]);
            }
        }
        let has_uv = self.src.has_uvs();
        let mut polygons = Vec::new();
        let mut edges: BTreeSet<(u32, u32)> = BTreeSet::new();
        for p in self.polys.iter().filter(|p| p.alive) {
            let verts: Vec<u32> = p.verts.iter().map(|&v| map[v as usize].unwrap()).collect();
            for i in 0..p.verts.len() {
                edges.insert(ekey(p.verts[i], p.verts[(i + 1) % p.verts.len()]));
            }
            let sp = &self.src.polygons[p.src as usize];
            let (normals, uvs) = if !p.touched && sp.verts == p.verts {
                (sp.normals.clone(), sp.uvs.clone())
            } else {
                let geo = area_vec(&self.poly_pos(&p.verts)).normalize_or_zero();
                let normals = p
                    .verts
                    .iter()
                    .map(|&v| {
                        self.corner_normal(v, p)
                            .filter(|n| n.dot(geo) > 0.0)
                            .unwrap_or(geo)
                    })
                    .collect();
                let uvs = has_uv.then(|| self.polygon_uvs(p));
                (normals, uvs)
            };
            polygons.push(EditPolygon {
                verts,
                group: p.group,
                normals,
                uvs,
            });
        }
        let mut flagged: Vec<FlaggedEdge> = edges
            .iter()
            .filter_map(|&(a, b)| {
                let f = self.flag(a, b) & (F_CAD | F_HARD | F_SEAM);
                if f == 0 {
                    return None;
                }
                let (a, b) = (map[a as usize]?, map[b as usize]?);
                let (a, b) = if a < b { (a, b) } else { (b, a) };
                Some(FlaggedEdge {
                    a,
                    b,
                    flags: EdgeFlags::from_bits(f),
                })
            })
            .collect();
        flagged.sort_by_key(|e| (e.a, e.b));
        (
            EditMesh {
                body: self.src.body,
                positions,
                pinned,
                polygons,
                flagged,
            },
            map,
        )
    }

    /// Selected edges (F_SEL) in output ids.
    pub fn selected_edges(&self, map: &[Option<u32>]) -> Vec<[u32; 2]> {
        let mut out: Vec<[u32; 2]> = self
            .flags
            .iter()
            .filter(|(k, f)| {
                **f & F_SEL != 0
                    && self.valive[k.0 as usize]
                    && self.valive[k.1 as usize]
                    && !self.edge_polys(k.0, k.1).is_empty()
            })
            .filter_map(|(k, _)| {
                let (a, b) = (map[k.0 as usize]?, map[k.1 as usize]?);
                Some(if a < b { [a, b] } else { [b, a] })
            })
            .collect();
        out.sort_unstable();
        out
    }
}

/// UV charts: polygons connected across non-seam edges.
fn charts(src: &EditMesh, flags: &HashMap<(u32, u32), u8>) -> Vec<u32> {
    let n = src.polygons.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], x: usize) -> usize {
        let mut r = x;
        while p[r] != r {
            r = p[r];
        }
        let mut y = x;
        while p[y] != r {
            let nx = p[y];
            p[y] = r;
            y = nx;
        }
        r
    }
    let mut first: HashMap<(u32, u32), usize> = HashMap::new();
    for (i, p) in src.polygons.iter().enumerate() {
        let m = p.verts.len();
        for k in 0..m {
            let e = ekey(p.verts[k], p.verts[(k + 1) % m]);
            if flags.get(&e).copied().unwrap_or(0) & F_SEAM != 0 {
                continue;
            }
            if let Some(&j) = first.get(&e) {
                let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                if a != b {
                    parent[a.max(b)] = a.min(b);
                }
            } else {
                first.insert(e, i);
            }
        }
    }
    (0..n).map(|i| find(&mut parent, i) as u32).collect()
}

/// True if `a` and `b` are the same vertex cycle up to rotation.
pub(crate) fn same_cycle(a: &[u32], b: &[u32]) -> bool {
    if a.len() != b.len() || a.is_empty() {
        return false;
    }
    let Some(k) = b.iter().position(|&x| x == a[0]) else {
        return false;
    };
    (0..a.len()).all(|i| a[i] == b[(k + i) % b.len()])
}

pub(crate) fn has_edge(v: &[u32], a: u32, b: u32) -> bool {
    has_directed(v, a, b) || has_directed(v, b, a)
}

pub(crate) fn has_directed(v: &[u32], a: u32, b: u32) -> bool {
    let n = v.len();
    (0..n).any(|i| v[i] == a && v[(i + 1) % n] == b)
}

/// Pieces of polygon `verts` after `u` is merged into `v`.
pub(crate) fn merge_pieces(verts: &[u32], u: u32, v: u32) -> Vec<Vec<u32>> {
    let n = verts.len();
    let Some(iu) = verts.iter().position(|&x| x == u) else {
        return vec![verts.to_vec()];
    };
    let rot: Vec<u32> = (0..n).map(|k| verts[(iu + k) % n]).collect();
    let Some(j) = rot.iter().position(|&x| x == v) else {
        let mut r = verts.to_vec();
        r[iu] = v;
        return vec![r];
    };
    let mut a = vec![v];
    a.extend_from_slice(&rot[1..j]);
    let mut b = vec![v];
    b.extend_from_slice(&rot[j + 1..]);
    [a, b].into_iter().filter(|p| p.len() >= 3).collect()
}

/// Non-zero area, no zero-length edge, triangles and quads strictly convex.
/// Larger polygons may be concave (they are filled or exported as n-gons).
pub(crate) fn polygon_shape_ok(pos: &[DVec3], diag: f64) -> bool {
    let n = pos.len();
    let av = area_vec(pos);
    let len = av.length();
    if n < 3 || len <= 1e-12 * diag * diag {
        return false;
    }
    let nrm = av / len;
    for i in 0..n {
        let a = pos[(i + n - 1) % n];
        let b = pos[i];
        let c = pos[(i + 1) % n];
        let (e1, e2) = (b - a, c - b);
        if e2.length() <= 1e-12 * diag {
            return false;
        }
        let turn = e1.cross(e2).dot(nrm);
        if n <= 4 && turn <= 1e-9 * e1.length() * e2.length() {
            return false;
        }
    }
    true
}

/// For each witness (input vertex id), the nearest surface polygon if the
/// distance is within `max_dev`; `None` if any witness is too far.
pub(crate) fn assign_witnesses(
    w: &[u32],
    surf: &[Vec<DVec3>],
    src_pos: &[DVec3],
    max_dev: f64,
) -> Option<Vec<(u32, usize)>> {
    if !max_dev.is_finite() {
        return Some(Vec::new());
    }
    let mut out = Vec::with_capacity(w.len());
    for &x in w {
        let p = src_pos[x as usize];
        let mut best = (f64::INFINITY, 0usize);
        for (i, poly) in surf.iter().enumerate() {
            let d = polygon_distance(p, poly);
            if d < best.0 {
                best = (d, i);
            }
        }
        if best.0 > max_dev {
            return None;
        }
        out.push((x, best.1));
    }
    Some(out)
}

/// Distance from `p` to a polygon as it will be exported. A quad may be
/// split along either diagonal, so it counts the worse of the two
/// triangulations; larger polygons use a fan from the centroid.
pub(crate) fn polygon_distance(p: DVec3, poly: &[DVec3]) -> f64 {
    let tri = |a: usize, b: usize, c: usize| point_triangle_distance(p, poly[a], poly[b], poly[c]);
    match poly.len() {
        3 => tri(0, 1, 2),
        4 => tri(0, 1, 2)
            .min(tri(0, 2, 3))
            .max(tri(0, 1, 3).min(tri(1, 2, 3))),
        _ => fan_tris(poly)
            .iter()
            .map(|t| point_triangle_distance(p, t[0], t[1], t[2]))
            .fold(f64::INFINITY, f64::min),
    }
}

/// Triangles of a polygon for distance queries (fan from the centroid,
/// which covers concave n-gons well enough for a bound).
pub(crate) fn fan_tris(p: &[DVec3]) -> Vec<[DVec3; 3]> {
    if p.len() == 3 {
        return vec![[p[0], p[1], p[2]]];
    }
    if p.len() == 4 {
        return vec![
            [p[0], p[1], p[2]],
            [p[0], p[2], p[3]],
            [p[0], p[1], p[3]],
            [p[1], p[2], p[3]],
        ];
    }
    let c = p.iter().copied().sum::<DVec3>() / p.len() as f64;
    (0..p.len())
        .map(|i| [c, p[i], p[(i + 1) % p.len()]])
        .collect()
}

/// Euclidean distance from `p` to triangle `abc` (Ericson, *Real-Time
/// Collision Detection*, §5.1.5 closest point on triangle).
pub(crate) fn point_triangle_distance(p: DVec3, a: DVec3, b: DVec3, c: DVec3) -> f64 {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return p.distance(a);
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return p.distance(b);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return p.distance(a + ab * v);
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return p.distance(c);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return p.distance(a + ac * w);
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return p.distance(b + (c - b) * w);
    }
    let denom = 1.0 / (va + vb + vc);
    if !denom.is_finite() {
        // Degenerate triangle: nearest of its edges.
        return [(a, b), (b, c), (c, a)]
            .iter()
            .map(|&(x, y)| point_segment_distance(p, x, y))
            .fold(f64::INFINITY, f64::min);
    }
    let v = vb * denom;
    let w = vc * denom;
    p.distance(a + ab * v + ac * w)
}

pub(crate) fn point_segment_distance(p: DVec3, a: DVec3, b: DVec3) -> f64 {
    let d = b - a;
    let l2 = d.length_squared();
    if l2 <= 0.0 {
        return p.distance(a);
    }
    let t = ((p - a).dot(d) / l2).clamp(0.0, 1.0);
    p.distance(a + d * t)
}
