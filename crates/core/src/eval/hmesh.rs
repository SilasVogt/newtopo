//! Half-edge operations for edits (SPEC §4.5, §6.7–§6.10): remove edge,
//! collapse, weld, chord insertion. Standard half-edge surgery (own code).

use super::Owner;
use super::arr::owner_rank;
use super::assemble::{HE, HMesh, Region};
use crate::keys::{EdgeKey, VertexKey};
use crate::mesh::Anchor;
use glam::DVec2;
use std::collections::{HashMap, HashSet};

/// `uv` shifted by whole periods (`per`, 0 = not periodic) to be nearest
/// to `near`.
fn lift(uv: DVec2, near: DVec2, per: DVec2) -> DVec2 {
    let mut out = uv;
    if per.x > 0.0 {
        out.x += ((near.x - out.x) / per.x).round() * per.x;
    }
    if per.y > 0.0 {
        out.y += ((near.y - out.y) / per.y).round() * per.y;
    }
    out
}

impl HMesh {
    pub fn alive_he(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.he.len()).filter(|&h| self.he[h].alive)
    }

    pub fn orbit(&self, h: usize) -> Vec<usize> {
        let mut out = vec![h];
        let mut x = self.he[h].next;
        let mut guard = 0;
        while x != h && guard < self.he.len() + 1 {
            out.push(x);
            x = self.he[x].next;
            guard += 1;
        }
        out
    }

    pub fn prev(&self, h: usize) -> usize {
        let o = self.orbit(h);
        *o.last().unwrap()
    }

    /// Walks (orbits) of a region.
    pub fn region_walks(&self, r: usize) -> Vec<Vec<usize>> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for h in self.alive_he() {
            if self.he[h].region != r || seen.contains(&h) {
                continue;
            }
            let o = self.orbit(h);
            for &x in &o {
                seen.insert(x);
            }
            out.push(o);
        }
        out
    }

    /// Region → walks for all alive regions (one pass).
    pub fn all_region_walks(&self) -> HashMap<usize, Vec<Vec<usize>>> {
        let mut seen = vec![false; self.he.len()];
        let mut out: HashMap<usize, Vec<Vec<usize>>> = HashMap::new();
        for h in 0..self.he.len() {
            if !self.he[h].alive || seen[h] {
                continue;
            }
            let o = self.orbit(h);
            for &x in &o {
                seen[x] = true;
            }
            out.entry(self.he[h].region).or_default().push(o);
        }
        out
    }

    /// Vertex a pole vertex collapses to (itself otherwise).
    pub fn final_v(&self, v: usize, pole_target: &HashMap<newtopo_kernel::EdgeId, usize>) -> usize {
        match self.verts[v].pole {
            Some(e) => pole_target.get(&e).copied().unwrap_or(v),
            None => v,
        }
    }

    pub fn edge_key(
        &self,
        h: usize,
        pole_target: &HashMap<newtopo_kernel::EdgeId, usize>,
    ) -> Option<EdgeKey> {
        let he = &self.he[h];
        let a = self.verts[self.final_v(he.from, pole_target)].key.clone();
        let b = self.verts[self.final_v(he.to, pole_target)].key.clone();
        match he.owner {
            Owner::Line(k) => Some(EdgeKey::line(k, a, b)),
            Owner::Cad(e) => Some(EdgeKey::cad(e, a, b)),
            Owner::Split(s) => Some(EdgeKey::split(s, a, b)),
            _ => None,
        }
    }

    pub fn find_edge(
        &self,
        key: &EdgeKey,
        pole_target: &HashMap<newtopo_kernel::EdgeId, usize>,
    ) -> Option<usize> {
        // Fast path by owner, then compare keys (with aliases).
        let (a, b) = key.ends();
        let va = self.key_index.get(a).copied();
        let vb = self.key_index.get(b).copied();
        let (va, vb) = (va?, vb?);
        let owner = match key {
            EdgeKey::Line { line, .. } => Owner::Line(*line),
            EdgeKey::Cad { edge, .. } => Owner::Cad(*edge),
            EdgeKey::Split { split, .. } => Owner::Split(*split),
        };
        let fa = self.final_v(va, pole_target);
        let fb = self.final_v(vb, pole_target);
        self.alive_he().find(|&h| {
            let he = &self.he[h];
            he.owner == owner && {
                let (x, y) = (
                    self.final_v(he.from, pole_target),
                    self.final_v(he.to, pole_target),
                );
                (x == fa && y == fb) || (x == fb && y == fa)
            }
        })
    }

    fn neighbours(&self, v: usize) -> HashSet<usize> {
        let mut s = HashSet::new();
        for h in self.alive_he() {
            let he = &self.he[h];
            if he.from == v {
                s.insert(he.to);
            }
            if he.to == v {
                s.insert(he.from);
            }
        }
        s.remove(&v);
        s
    }

    fn kill_pair(&mut self, h: usize) {
        self.he[h].alive = false;
        if let Some(t) = self.he[h].twin {
            self.he[t].alive = false;
        }
    }

    /// Remove an edge, merging the polygons on its two sides.
    pub fn remove_edge(&mut self, h: usize) -> Result<(), String> {
        let Some(t) = self.he[h].twin else {
            return Err("cannot remove a boundary edge".into());
        };
        let (ph, pt) = (self.prev(h), self.prev(t));
        let (nh, nt) = (self.he[h].next, self.he[t].next);
        if nh == t && nt == h {
            // Isolated edge.
        } else if nh == t {
            self.he[ph].next = nt;
        } else if nt == h {
            self.he[pt].next = nh;
        } else {
            self.he[ph].next = nt;
            self.he[pt].next = nh;
        }
        self.kill_pair(h);
        let (rh, rt) = (self.he[h].region, self.he[t].region);
        if rh != rt {
            let multi = self
                .he
                .iter()
                .any(|e| e.alive && e.region == rt && e.face != self.regions[rh].face)
                || self.regions[rt].face != self.regions[rh].face;
            for e in &mut self.he {
                if e.region == rt {
                    e.region = rh;
                }
            }
            self.regions[rt].alive = false;
            if multi {
                self.regions[rh].multi = true;
            }
        }
        Ok(())
    }

    /// Collapse half-edge `h` (a→b) merging `a` into `b` if `keep_to`, else
    /// `b` into `a`. `anchor` overrides the kept vertex's anchor.
    pub fn collapse(
        &mut self,
        h: usize,
        keep_to: bool,
        anchor: Option<(Anchor, glam::DVec3)>,
    ) -> Result<usize, String> {
        let (a, b) = (self.he[h].from, self.he[h].to);
        if a == b {
            return Err("edge is a loop".into());
        }
        let (keep, rem) = if keep_to { (b, a) } else { (a, b) };
        // Link condition.
        let common: HashSet<usize> = self
            .neighbours(a)
            .intersection(&self.neighbours(b))
            .copied()
            .collect();
        let mut allowed = HashSet::new();
        let t = self.he[h].twin;
        for side in [Some(h), t].into_iter().flatten() {
            let o = self.orbit(side);
            if o.len() == 3 {
                allowed.insert(self.he[self.he[side].next].to);
            }
        }
        if !common.is_subset(&allowed) {
            return Err("collapse would make the mesh non-manifold".into());
        }
        let ph = self.prev(h);
        let nh = self.he[h].next;
        match t {
            Some(t) => {
                let pt = self.prev(t);
                let nt = self.he[t].next;
                if nh == t && nt == h {
                    // isolated
                } else if nh == t {
                    self.he[ph].next = nt;
                } else if nt == h {
                    self.he[pt].next = nh;
                } else {
                    self.he[ph].next = nh;
                    self.he[pt].next = nt;
                }
            }
            None => {
                self.he[ph].next = nh;
            }
        }
        self.kill_pair(h);
        // UV of the kept vertex per face, before re-pointing.
        let mut keep_uv: Vec<(newtopo_kernel::FaceId, DVec2)> = Vec::new();
        if let Some((Anchor::Face { face, uv }, _)) = &anchor {
            keep_uv.push((*face, *uv));
        }
        for e in self.he.iter().filter(|e| e.alive && !e.uv.is_empty()) {
            if keep_uv.iter().any(|(f, _)| *f == e.face) {
                continue;
            }
            if e.from == keep {
                keep_uv.push((e.face, e.uv[0]));
            } else if e.to == keep {
                keep_uv.push((e.face, *e.uv.last().unwrap()));
            }
        }
        if keep_uv.is_empty() {
            let e = &self.he[h];
            if !e.uv.is_empty() {
                keep_uv.push((
                    e.face,
                    if keep_to {
                        *e.uv.last().unwrap()
                    } else {
                        e.uv[0]
                    },
                ));
            }
        }
        let moved = anchor.is_some();
        let periods = self.periods.clone();
        for e in &mut self.he {
            let start = e.from == rem || (moved && e.from == keep);
            let end = e.to == rem || (moved && e.to == keep);
            if e.from == rem {
                e.from = keep;
            }
            if e.to == rem {
                e.to = keep;
            }
            if !e.alive || e.uv.is_empty() {
                continue;
            }
            if let Some(&(_, kuv)) = keep_uv.iter().find(|(f, _)| *f == e.face) {
                let per = periods.get(e.face.idx()).copied().unwrap_or(DVec2::ZERO);
                if start {
                    let near = e.uv[0];
                    e.uv[0] = lift(kuv, near, per);
                }
                if end {
                    let n = e.uv.len() - 1;
                    let near = e.uv[n];
                    e.uv[n] = lift(kuv, near, per);
                }
                if start || end {
                    e.p3 = None;
                }
            }
        }
        let rem_keys = self.verts[rem].keys.clone();
        self.verts[keep].keys.extend(rem_keys);
        self.verts[rem].alive = false;
        if let Some((anc, pos)) = anchor {
            self.verts[keep].anchor = anc;
            self.verts[keep].pos = pos;
        }
        self.cleanup_degenerate(keep);
        self.rebuild_key_index();
        Ok(keep)
    }

    /// Remove 2-gons and loops around `v` after a collapse.
    fn cleanup_degenerate(&mut self, v: usize) {
        loop {
            let mut changed = false;
            let hs: Vec<usize> = self
                .alive_he()
                .filter(|&h| self.he[h].from == v || self.he[h].to == v)
                .collect();
            for h in hs {
                if !self.he[h].alive {
                    continue;
                }
                let o = self.orbit(h);
                if o.len() == 1 {
                    self.he[h].alive = false;
                    changed = true;
                } else if o.len() == 2 {
                    let (e1, e2) = (o[0], o[1]);
                    let (t1, t2) = (self.he[e1].twin, self.he[e2].twin);
                    self.he[e1].alive = false;
                    self.he[e2].alive = false;
                    let r = self.he[e1].region;
                    if !self.he.iter().any(|e| e.alive && e.region == r) {
                        self.regions[r].alive = false;
                    }
                    match (t1, t2) {
                        (Some(x), Some(y)) => {
                            self.he[x].twin = Some(y);
                            self.he[y].twin = Some(x);
                            let ox = self.he[x].owner;
                            let oy = self.he[y].owner;
                            let o = if owner_rank(&ox) <= owner_rank(&oy) {
                                ox
                            } else {
                                oy
                            };
                            self.he[x].owner = o;
                            self.he[y].owner = o;
                        }
                        (Some(x), None) => self.he[x].twin = None,
                        (None, Some(y)) => self.he[y].twin = None,
                        _ => {}
                    }
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// Insert a chord between the corner of `x` before half-edge `hx`
    /// (leaving x) and the corner of `y` before `hy` (leaving y), both in
    /// the same region. Returns the chord half-edge x→y.
    pub fn insert_chord(
        &mut self,
        hx: usize,
        hy: usize,
        uv: Vec<DVec2>,
        p3: Option<Vec<glam::DVec3>>,
    ) -> usize {
        let (px, py) = (self.prev(hx), self.prev(hy));
        let (x, y) = (self.he[hx].from, self.he[hy].from);
        let r = self.he[hx].region;
        let face = self.he[hx].face;
        let c = self.he.len();
        let cr = c + 1;
        let mut ruv = uv.clone();
        ruv.reverse();
        let rp3 = p3.as_ref().map(|v| v.iter().rev().copied().collect());
        self.he.push(HE {
            from: x,
            to: y,
            twin: Some(cr),
            next: hy,
            region: r,
            owner: Owner::Fill,
            face,
            uv,
            p3,
            alive: true,
            src: vec![],
        });
        self.he.push(HE {
            from: y,
            to: x,
            twin: Some(c),
            next: hx,
            region: r,
            owner: Owner::Fill,
            face,
            uv: ruv,
            p3: rp3,
            alive: true,
            src: vec![],
        });
        self.he[px].next = c;
        self.he[py].next = cr;
        c
    }

    /// Weld vertex `s` into `t` (target keeps its anchor).
    pub fn weld(&mut self, s: usize, t: usize) -> Result<usize, String> {
        if s == t {
            return Ok(t);
        }
        let fwd = self
            .alive_he()
            .find(|&h| self.he[h].from == s && self.he[h].to == t);
        if let Some(h) = fwd {
            return self.collapse(h, true, None);
        }
        let bwd = self
            .alive_he()
            .find(|&h| self.he[h].from == t && self.he[h].to == s);
        if let Some(h) = bwd {
            return self.collapse(h, false, None);
        }
        // Common polygon.
        for hs in self
            .alive_he()
            .filter(|&h| self.he[h].from == s)
            .collect::<Vec<_>>()
        {
            let o = self.orbit(hs);
            if let Some(&ht) = o.iter().find(|&&h| self.he[h].from == t) {
                let uv = vec![];
                let c = self.insert_chord(hs, ht, uv, None);
                let side: Vec<usize> = self
                    .orbit(c + 1)
                    .into_iter()
                    .filter(|&x| x != c + 1)
                    .collect();
                let res = match self.collapse(c, true, None) {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = self.remove_edge(c);
                        return Err(e);
                    }
                };
                // The two sides are separate polygons now.
                let live: Vec<usize> = side.into_iter().filter(|&x| self.he[x].alive).collect();
                if !live.is_empty() {
                    let r = self.new_region(self.he[live[0]].region);
                    for x in self.orbit(live[0]) {
                        self.he[x].region = r;
                    }
                }
                return Ok(res);
            }
        }
        Err("the two vertices don't share a polygon".into())
    }

    pub fn new_region(&mut self, like: usize) -> usize {
        let r = self.regions[like].clone();
        self.regions.push(Region { alive: true, ..r });
        self.regions.len() - 1
    }

    /// Alive vertex for a key (aliases included).
    pub fn vertex_of(&self, k: &VertexKey) -> Option<usize> {
        self.key_index
            .get(k)
            .copied()
            .filter(|&v| self.verts[v].alive)
    }
}
