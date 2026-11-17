//! Planar arrangement of curves in a face's seam-cut UV domain, face
//! extraction by half-edge traversal, and gluing across seams with the
//! topology guard (SPEC §4.1, §4.6 step 3). Own design; standard planar
//! subdivision technique (de Berg et al., "Computational Geometry", ch. 2).
//!
//! Input: curves (polylines with a monotone parameter) and events (points
//! with keys and incidences `(curve, param)`). Events closer than `eps`
//! (metric-scaled UV) are merged. Every curve is split at its incident
//! events into chains; chains between the same two nodes with the same
//! geometry are merged (highest owner precedence wins).

use super::Owner;
use super::ctx::FaceGeom;
use super::geom2::*;
use crate::keys::VertexKey;
use glam::DVec2;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct ACurve {
    pub owner: Owner,
    /// Lies on a seam copy `(dir, copy)`.
    pub seam: Option<(u8, u8)>,
    pub pts: Vec<DVec2>,
    /// Parameter per point, increasing.
    pub par: Vec<f64>,
    /// Closed loop: last point equals first; parameter wraps.
    pub closed: bool,
}

#[derive(Clone, Debug)]
pub struct AEvent {
    pub uv: DVec2,
    pub key: VertexKey,
    pub inc: Vec<(usize, f64)>,
    /// Artificial anchor on a closed curve without real events.
    pub artificial: bool,
    /// Exists because of a seam (a curve crossing a seam).
    pub seam_rel: bool,
}

#[derive(Clone, Debug)]
pub struct Node {
    pub uv: DVec2,
    pub key: VertexKey,
    pub keys: Vec<VertexKey>,
    pub incs: Vec<(usize, f64)>,
    pub artificial: bool,
    pub seam_rel: bool,
    pub out: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct Chain {
    pub curve: usize,
    pub a: usize,
    pub b: usize,
    pub pa: f64,
    pub pb: f64,
    pub pts: Vec<DVec2>,
    /// Curve parameter per point (increasing, may exceed the curve's range
    /// on a wrapping chain of a closed curve).
    pub pars: Vec<f64>,
    /// Lies on a seam copy (inherited from a merged duplicate if needed).
    pub seam: Option<(u8, u8)>,
}

#[derive(Clone, Debug)]
pub struct Cell {
    pub outer: usize,
    pub holes: Vec<usize>,
    pub inside: bool,
    pub sample: DVec2,
}

pub struct Arr {
    pub curves: Vec<ACurve>,
    pub nodes: Vec<Node>,
    pub chains: Vec<Chain>,
    pub angle: Vec<f64>,
    pub next: Vec<usize>,
    pub orbit: Vec<usize>,
    pub orbits: Vec<Vec<usize>>,
    pub area: Vec<f64>,
    pub cell_of_orbit: Vec<Option<usize>>,
    pub cells: Vec<Cell>,
}

pub fn owner_rank(o: &Owner) -> u8 {
    match o {
        Owner::Cad(_) => 0,
        Owner::Pole(_) => 1,
        Owner::Split(_) => 2,
        Owner::Line(_) => 3,
        Owner::Seam(_) => 4,
        Owner::Fill => 5,
    }
}

struct Uf(Vec<usize>);
impl Uf {
    fn new(n: usize) -> Self {
        Uf((0..n).collect())
    }
    fn find(&mut self, x: usize) -> usize {
        let mut r = x;
        while self.0[r] != r {
            r = self.0[r];
        }
        let mut y = x;
        while self.0[y] != r {
            let n = self.0[y];
            self.0[y] = r;
            y = n;
        }
        r
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
            self.0[hi] = lo;
        }
    }
}

fn slice_with_par(c: &ACurve, pa: f64, pb: f64) -> (Vec<DVec2>, Vec<f64>) {
    let mut pts = vec![poly_at(&c.pts, s_of_par(&c.par, pa))];
    let mut pars = vec![pa];
    for i in 0..c.par.len() {
        if c.par[i] > pa + 1e-14 && c.par[i] < pb - 1e-14 {
            pts.push(c.pts[i]);
            pars.push(c.par[i]);
        }
    }
    pts.push(poly_at(&c.pts, s_of_par(&c.par, pb)));
    pars.push(pb);
    (pts, pars)
}

/// Points and parameters of a curve between parameters `pa < pb`
/// (wrapping through the start of a closed curve when `pb` exceeds its end).
pub fn curve_slice(c: &ACurve, pa: f64, pb: f64) -> (Vec<DVec2>, Vec<f64>) {
    let end = c.par[c.par.len() - 1];
    let total = end - c.par[0];
    if c.closed && pb > end + 1e-15 {
        let (mut a, mut ap) = slice_with_par(c, pa, end);
        let (b, bp) = slice_with_par(c, c.par[0], pb - total);
        a.extend(b.into_iter().skip(1));
        ap.extend(bp.into_iter().skip(1).map(|x| x + total));
        return (a, ap);
    }
    slice_with_par(c, pa, pb)
}

impl Arr {
    pub fn he_origin(&self, h: usize) -> usize {
        let c = &self.chains[h / 2];
        if h.is_multiple_of(2) { c.a } else { c.b }
    }
    pub fn he_dest(&self, h: usize) -> usize {
        let c = &self.chains[h / 2];
        if h.is_multiple_of(2) { c.b } else { c.a }
    }
    pub fn he_pts(&self, h: usize) -> Vec<DVec2> {
        let c = &self.chains[h / 2];
        if h.is_multiple_of(2) {
            c.pts.clone()
        } else {
            c.pts.iter().rev().copied().collect()
        }
    }
    pub fn orbit_poly(&self, o: usize) -> Vec<DVec2> {
        let mut out = Vec::new();
        for &h in &self.orbits[o] {
            let p = self.he_pts(h);
            out.extend_from_slice(&p[..p.len() - 1]);
        }
        out
    }

    pub fn build(
        curves: Vec<ACurve>,
        events: Vec<AEvent>,
        fg: &FaceGeom,
        classify: &mut dyn FnMut(&[DVec2]) -> Vec<bool>,
    ) -> Arr {
        let eps = fg.eps;
        // 1. Cluster events.
        let n = events.len();
        let mut uf = Uf::new(n);
        let cell = eps * 4.0;
        let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (i, e) in events.iter().enumerate() {
            let m = fg.m(e.uv);
            let (cx, cy) = ((m.x / cell).floor() as i64, (m.y / cell).floor() as i64);
            for dx in -1..=1 {
                for dy in -1..=1 {
                    if let Some(v) = grid.get(&(cx + dx, cy + dy)) {
                        for &j in v {
                            if (fg.m(events[j].uv) - m).length() <= eps {
                                uf.union(i, j);
                            }
                        }
                    }
                }
            }
            grid.entry((cx, cy)).or_default().push(i);
        }
        let mut root_node: HashMap<usize, usize> = HashMap::new();
        let mut nodes: Vec<Node> = Vec::new();
        let mut members: Vec<Vec<usize>> = Vec::new();
        for i in 0..n {
            let r = uf.find(i);
            let id = *root_node.entry(r).or_insert_with(|| {
                members.push(vec![]);
                members.len() - 1
            });
            members[id].push(i);
        }
        for mem in &members {
            let real: Vec<usize> = mem
                .iter()
                .copied()
                .filter(|&i| !events[i].artificial)
                .collect();
            let pool = if real.is_empty() { mem.clone() } else { real };
            let mut keys: Vec<VertexKey> = pool.iter().map(|&i| events[i].key.clone()).collect();
            keys.sort();
            keys.dedup();
            let canon = VertexKey::canonical(&keys).cloned().unwrap();
            let rep = *pool.iter().find(|&&i| events[i].key == canon).unwrap();
            let mut incs = Vec::new();
            for &i in mem {
                incs.extend(events[i].inc.iter().copied());
            }
            nodes.push(Node {
                uv: events[rep].uv,
                key: canon,
                keys,
                incs,
                artificial: mem.iter().all(|&i| events[i].artificial),
                seam_rel: mem.iter().any(|&i| events[i].seam_rel),
                out: vec![],
            });
        }
        // 2. Chains.
        let mut per_curve: Vec<Vec<(f64, usize)>> = vec![vec![]; curves.len()];
        for (ni, nd) in nodes.iter().enumerate() {
            for &(c, p) in &nd.incs {
                per_curve[c].push((p, ni));
            }
        }
        let mut chains: Vec<Chain> = Vec::new();
        for (ci, list) in per_curve.iter_mut().enumerate() {
            let c = &curves[ci];
            list.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            list.dedup_by(|b, a| a.1 == b.1 && (b.0 - a.0).abs() < 1e-12 * (1.0 + a.0.abs()));
            let mut l: Vec<(f64, usize)> = Vec::new();
            let span = (c.par[c.par.len() - 1] - c.par[0]).abs().max(1e-300);
            for &x in list.iter() {
                if l.last()
                    .is_some_and(|y: &(f64, usize)| y.1 == x.1 && (x.0 - y.0).abs() <= 1e-9 * span)
                {
                    continue;
                }
                l.push(x);
            }
            if c.closed {
                let total = c.par[c.par.len() - 1] - c.par[0];
                if l.len() > 1
                    && l[0].1 == l[l.len() - 1].1
                    && (l[l.len() - 1].0 - l[0].0 - total).abs() <= 1e-9 * span
                {
                    l.pop();
                }
                if l.is_empty() {
                    continue;
                }
                for i in 0..l.len() {
                    let (pa, a) = l[i];
                    let (mut pb, b) = l[(i + 1) % l.len()];
                    if i + 1 == l.len() {
                        pb += total;
                    }
                    let (pts, pars) = curve_slice(c, pa, pb);
                    chains.push(Chain {
                        curve: ci,
                        a,
                        b,
                        pa,
                        pb,
                        pts,
                        pars,
                        seam: c.seam,
                    });
                }
            } else {
                for w in l.windows(2) {
                    let ((pa, a), (pb, b)) = (w[0], w[1]);
                    if pb - pa <= 0.0 {
                        continue;
                    }
                    let (pts, pars) = curve_slice(c, pa, pb);
                    chains.push(Chain {
                        curve: ci,
                        a,
                        b,
                        pa,
                        pb,
                        pts,
                        pars,
                        seam: c.seam,
                    });
                }
            }
        }
        for ch in &mut chains {
            let (ua, ub) = (nodes[ch.a].uv, nodes[ch.b].uv);
            let last = ch.pts.len() - 1;
            ch.pts[0] = ua;
            ch.pts[last] = ub;
        }
        // Drop degenerate chains (a == b, no length) and duplicates.
        chains.retain(|ch| {
            !(ch.a == ch.b && poly_len(&ch.pts.iter().map(|p| fg.m(*p)).collect::<Vec<_>>()) <= eps)
        });
        let mut by_pair: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
        for (i, ch) in chains.iter().enumerate() {
            by_pair
                .entry((ch.a.min(ch.b), ch.a.max(ch.b)))
                .or_default()
                .push(i);
        }
        let mut dead = vec![false; chains.len()];
        let mut inherit: Vec<(usize, Option<(u8, u8)>)> = Vec::new();
        let mut pairs: Vec<&Vec<usize>> = by_pair.values().collect();
        pairs.sort();
        for list in pairs {
            for x in 0..list.len() {
                for y in x + 1..list.len() {
                    let (i, j) = (list[x], list[y]);
                    if dead[i] || dead[j] {
                        continue;
                    }
                    let (ci, cj) = (&chains[i], &chains[j]);
                    let mi = poly_at(&ci.pts, (ci.pts.len() - 1) as f64 * 0.5);
                    let mj = if ci.a == cj.a {
                        poly_at(&cj.pts, (cj.pts.len() - 1) as f64 * 0.5)
                    } else {
                        let r: Vec<DVec2> = cj.pts.iter().rev().copied().collect();
                        poly_at(&r, (r.len() - 1) as f64 * 0.5)
                    };
                    // Compare midpoints by arc length rather than index.
                    let li = poly_len(&ci.pts);
                    let lj = poly_len(&cj.pts);
                    let close = (fg.m(mi) - fg.m(mj)).length()
                        <= eps * 100.0 + 1e-6 * fg.m(DVec2::splat(li.max(lj))).length()
                        && (li - lj).abs() <= 1e-3 * li.max(lj) + eps;
                    let _ = (mi, mj);
                    if close && same_geometry(fg, &ci.pts, &cj.pts, ci.a == cj.a, eps) {
                        let (ri, rj) = (
                            owner_rank(&curves[ci.curve].owner),
                            owner_rank(&curves[cj.curve].owner),
                        );
                        let (kill, keep) = if rj < ri { (i, j) } else { (j, i) };
                        dead[kill] = true;
                        if chains[keep].seam.is_none() {
                            inherit.push((keep, chains[kill].seam));
                        }
                    }
                }
            }
        }
        for (k, sm) in inherit {
            if chains[k].seam.is_none() {
                chains[k].seam = sm;
            }
        }
        let chains: Vec<Chain> = chains
            .into_iter()
            .zip(dead)
            .filter(|(_, d)| !d)
            .map(|(c, _)| c)
            .collect();
        // 3. Half-edges and angles.
        let nh = chains.len() * 2;
        let mut angle = vec![0.0; nh];
        let tiny = eps * 0.1;
        for (ci, ch) in chains.iter().enumerate() {
            for d in 0..2 {
                let h = ci * 2 + d;
                let pts: Vec<DVec2> = if d == 0 {
                    ch.pts.clone()
                } else {
                    ch.pts.iter().rev().copied().collect()
                };
                let o = fg.m(pts[0]);
                let mut dir = fg.m(pts[pts.len() - 1]) - o;
                for p in &pts[1..] {
                    let v = fg.m(*p) - o;
                    if v.length() > tiny {
                        dir = v;
                        break;
                    }
                }
                angle[h] = dir.y.atan2(dir.x);
                let origin = if d == 0 { ch.a } else { ch.b };
                nodes[origin].out.push(h);
            }
        }
        for nd in &mut nodes {
            nd.out
                .sort_by(|&a, &b| angle[a].total_cmp(&angle[b]).then(a.cmp(&b)));
        }
        let mut pos = vec![0usize; nh];
        for nd in &nodes {
            for (i, &h) in nd.out.iter().enumerate() {
                pos[h] = i;
            }
        }
        let mut next = vec![0usize; nh];
        for h in 0..nh {
            let ch = &chains[h / 2];
            let dest = if h % 2 == 0 { ch.b } else { ch.a };
            let twin = h ^ 1;
            let out = &nodes[dest].out;
            let p = pos[twin];
            next[h] = out[(p + out.len() - 1) % out.len()];
        }
        // 4. Orbits.
        let mut orbit = vec![usize::MAX; nh];
        let mut orbits: Vec<Vec<usize>> = Vec::new();
        for h0 in 0..nh {
            if orbit[h0] != usize::MAX {
                continue;
            }
            let id = orbits.len();
            let mut cyc = Vec::new();
            let mut h = h0;
            let mut guard = 0;
            while orbit[h] == usize::MAX && guard <= nh {
                orbit[h] = id;
                cyc.push(h);
                h = next[h];
                guard += 1;
            }
            orbits.push(cyc);
        }
        let mut arr = Arr {
            curves,
            nodes,
            chains,
            angle,
            next,
            orbit,
            orbits,
            area: vec![],
            cell_of_orbit: vec![],
            cells: vec![],
        };
        arr.area = (0..arr.orbits.len())
            .map(|o| signed_area(&arr.orbit_poly(o)))
            .collect();
        // 5. Cells and holes.
        let mut cell_of_orbit = vec![None; arr.orbits.len()];
        let mut cells: Vec<Cell> = Vec::new();
        let polys: Vec<Vec<DVec2>> = (0..arr.orbits.len()).map(|o| arr.orbit_poly(o)).collect();
        for o in 0..arr.orbits.len() {
            if arr.area[o] > 0.0 {
                cell_of_orbit[o] = Some(cells.len());
                cells.push(Cell {
                    outer: o,
                    holes: vec![],
                    inside: false,
                    sample: DVec2::ZERO,
                });
            }
        }
        for o in 0..arr.orbits.len() {
            if arr.area[o] > 0.0 {
                continue;
            }
            let Some(p) = left_sample(&polys[o], fg, None) else {
                continue;
            };
            let mut best: Option<(f64, usize)> = None;
            for (ci, c) in cells.iter().enumerate() {
                if point_in_poly(p, &polys[c.outer]) {
                    let a = arr.area[c.outer];
                    if best.is_none_or(|(ba, _)| a < ba) {
                        best = Some((a, ci));
                    }
                }
            }
            if let Some((_, ci)) = best {
                cells[ci].holes.push(o);
                cell_of_orbit[o] = Some(ci);
            }
        }
        let mut samples = Vec::with_capacity(cells.len());
        for c in &cells {
            let holes: Vec<&Vec<DVec2>> = c.holes.iter().map(|&h| &polys[h]).collect();
            let s = left_sample(&polys[c.outer], fg, Some(&holes)).unwrap_or(polys[c.outer][0]);
            samples.push(s);
        }
        let ins = if samples.is_empty() {
            vec![]
        } else {
            classify(&samples)
        };
        for (i, c) in cells.iter_mut().enumerate() {
            c.sample = samples[i];
            c.inside = ins[i];
        }
        arr.cell_of_orbit = cell_of_orbit;
        arr.cells = cells;
        arr
    }

    /// Inside cell to the left of half-edge `h`, if any.
    pub fn left_cell(&self, h: usize) -> Option<usize> {
        self.cell_of_orbit[self.orbit[h]].filter(|&c| self.cells[c].inside)
    }

    /// Cell containing a domain point (outer polygon, not in holes).
    pub fn locate(&self, p: DVec2) -> Option<usize> {
        let mut best: Option<(f64, usize)> = None;
        for (ci, c) in self.cells.iter().enumerate() {
            let poly = self.orbit_poly(c.outer);
            if !point_in_poly(p, &poly) {
                continue;
            }
            if c.holes
                .iter()
                .any(|&h| point_in_poly(p, &self.orbit_poly(h)) && self.area[h].abs() > 0.0)
            {
                continue;
            }
            let a = self.area[c.outer];
            if best.is_none_or(|(ba, _)| a < ba) {
                best = Some((a, ci));
            }
        }
        best.map(|(_, c)| c)
    }
}

fn same_geometry(fg: &FaceGeom, a: &[DVec2], b: &[DVec2], same_dir: bool, eps: f64) -> bool {
    let bb: Vec<DVec2> = if same_dir {
        b.to_vec()
    } else {
        b.iter().rev().copied().collect()
    };
    let la = poly_len(a);
    let lb = poly_len(&bb);
    if la <= 0.0 || lb <= 0.0 {
        return false;
    }
    let tol = eps * 100.0 + 1e-6 * fg.m(DVec2::splat(la)).length();
    for k in 1..4 {
        let f = k as f64 / 4.0;
        let pa = at_len(a, la * f);
        let pb = at_len(&bb, lb * f);
        if (fg.m(pa) - fg.m(pb)).length() > tol {
            return false;
        }
    }
    true
}

fn at_len(p: &[DVec2], l: f64) -> DVec2 {
    let mut acc = 0.0;
    for w in p.windows(2) {
        let d = (w[1] - w[0]).length();
        if acc + d >= l && d > 0.0 {
            return w[0] + (w[1] - w[0]) * ((l - acc) / d);
        }
        acc += d;
    }
    *p.last().unwrap()
}

/// A point to the left of a closed polyline (the side its face is on). With
/// `holes` given, the point must lie inside `poly` and outside the holes,
/// and among candidates the one farthest from the boundary is chosen (so a
/// kernel classifying against its own approximation agrees).
fn left_sample(poly: &[DVec2], fg: &FaceGeom, holes: Option<&[&Vec<DVec2>]>) -> Option<DVec2> {
    let n = poly.len();
    if n < 2 {
        return None;
    }
    let mp: Vec<DVec2> = poly.iter().map(|p| fg.m(*p)).collect();
    let mut lo = DVec2::splat(f64::INFINITY);
    let mut hi = DVec2::splat(f64::NEG_INFINITY);
    for p in &mp {
        lo = lo.min(*p);
        hi = hi.max(*p);
    }
    let diag = (hi - lo).length().max(fg.eps * 10.0);
    let mut segs: Vec<(f64, usize)> = (0..n)
        .map(|i| ((mp[(i + 1) % n] - mp[i]).length(), i))
        .collect();
    segs.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let hole_m: Vec<Vec<DVec2>> = holes
        .map(|hs| {
            hs.iter()
                .map(|h| h.iter().map(|p| fg.m(*p)).collect())
                .collect()
        })
        .unwrap_or_default();
    let clearance = |q: DVec2| -> f64 {
        let mut d = f64::INFINITY;
        for ring in std::iter::once(&mp).chain(hole_m.iter()) {
            let k = ring.len();
            for i in 0..k {
                d = d.min(dist_point_seg(q, ring[i], ring[(i + 1) % k]).0);
            }
        }
        d
    };
    let mut best: Option<(f64, DVec2)> = None;
    for &(len, i) in segs.iter().take(12) {
        if len <= 0.0 {
            break;
        }
        let a = mp[i];
        let b = mp[(i + 1) % n];
        let left = (b - a).perp().normalize();
        for f in [0.5, 0.25, 0.75] {
            for off in [1e-4, 1e-3, 1e-2, 0.05, 0.15, 0.3] {
                let m = a + (b - a) * f;
                let q = m + left * (diag * off).max(fg.eps * 10.0);
                let s = q / fg.metric;
                match holes {
                    None => return Some(s),
                    Some(hs) => {
                        if !point_in_poly(s, poly) || hs.iter().any(|h| point_in_poly(s, h)) {
                            continue;
                        }
                        let c = clearance(q);
                        if best.is_none_or(|(bc, _)| c > bc) {
                            best = Some((c, s));
                        }
                    }
                }
            }
        }
    }
    best.map(|(_, s)| s)
}

// ---------------------------------------------------------------------------
// Gluing across seams.

#[derive(Clone, Debug)]
pub struct GNode {
    pub key: VertexKey,
    pub keys: Vec<VertexKey>,
    pub uv: DVec2,
    pub incs: Vec<(usize, f64)>,
    pub seam_rel: bool,
    pub artificial: bool,
    pub nodes: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct GEdge {
    pub owner: Owner,
    pub curve: usize,
    pub a: usize,
    pub b: usize,
    pub pa: f64,
    pub pb: f64,
    pub chains: Vec<usize>,
    pub pts: Vec<DVec2>,
    pub pars: Vec<f64>,
}

pub struct Glued {
    pub gnodes: Vec<GNode>,
    pub node_g: Vec<usize>,
    pub gedges: Vec<GEdge>,
    /// Planar half-edge → glued half-edge.
    pub he_g: Vec<usize>,
    pub rot: Vec<Vec<usize>>,
    pub rot_pos: Vec<usize>,
    pub left: Vec<Option<usize>>,
    pub dissolved: Vec<bool>,
    /// Region id per cell (inside cells only; others usize::MAX).
    pub region_of_cell: Vec<usize>,
    /// Per region: walks (cycles of glued half-edges).
    pub regions: Vec<Vec<Vec<usize>>>,
}

impl Glued {
    pub fn g_origin(&self, gh: usize) -> usize {
        let e = &self.gedges[gh / 2];
        if gh.is_multiple_of(2) { e.a } else { e.b }
    }
    pub fn g_dest(&self, gh: usize) -> usize {
        let e = &self.gedges[gh / 2];
        if gh.is_multiple_of(2) { e.b } else { e.a }
    }
    pub fn g_pts(&self, gh: usize) -> Vec<DVec2> {
        let e = &self.gedges[gh / 2];
        if gh.is_multiple_of(2) {
            e.pts.clone()
        } else {
            e.pts.iter().rev().copied().collect()
        }
    }

    fn next(&self, h: usize, dissolved: &[bool]) -> usize {
        let d = self.g_dest(h);
        let rot = &self.rot[d];
        let mut p = self.rot_pos[h ^ 1];
        for _ in 0..rot.len() {
            p = (p + rot.len() - 1) % rot.len();
            let c = rot[p];
            if !dissolved[c / 2] {
                return c;
            }
        }
        h ^ 1
    }

    fn trace(&self, dissolved: &[bool]) -> Vec<Vec<usize>> {
        let nh = self.gedges.len() * 2;
        let mut seen = vec![false; nh];
        let mut walks = Vec::new();
        for h0 in 0..nh {
            if seen[h0] || dissolved[h0 / 2] || self.left[h0].is_none() {
                continue;
            }
            let mut w = Vec::new();
            let mut h = h0;
            let mut guard = 0;
            while !seen[h] && guard <= nh {
                seen[h] = true;
                w.push(h);
                h = self.next(h, dissolved);
                guard += 1;
            }
            walks.push(w);
        }
        walks
    }

    /// Glue an arrangement. `guard`: apply the topology guard (output
    /// stage); without it every seam between inside cells dissolves
    /// (patch identity). `forced`: glued edges removed by edits (always
    /// dissolved; indices from a previous build of the same arrangement).
    pub fn build(arr: &Arr, guard: bool) -> Glued {
        Self::build_with(arr, guard, &[])
    }

    pub fn build_with(arr: &Arr, guard: bool, forced: &[usize]) -> Glued {
        // Nodes sharing any key are the same glued node (seam copies).
        let mut uf = Uf::new(arr.nodes.len());
        let mut first: HashMap<&VertexKey, usize> = HashMap::new();
        for (i, nd) in arr.nodes.iter().enumerate() {
            for k in &nd.keys {
                match first.get(k) {
                    Some(&j) => uf.union(i, j),
                    None => {
                        first.insert(k, i);
                    }
                }
            }
        }
        let mut root_g: HashMap<usize, usize> = HashMap::new();
        let mut gnodes: Vec<GNode> = Vec::new();
        let mut node_g = vec![0usize; arr.nodes.len()];
        for (i, nd) in arr.nodes.iter().enumerate() {
            let r = uf.find(i);
            let g = *root_g.entry(r).or_insert_with(|| {
                gnodes.push(GNode {
                    key: nd.key.clone(),
                    keys: vec![],
                    uv: nd.uv,
                    incs: vec![],
                    seam_rel: false,
                    artificial: true,
                    nodes: vec![],
                });
                gnodes.len() - 1
            });
            node_g[i] = g;
            let gn = &mut gnodes[g];
            gn.nodes.push(i);
            gn.keys.extend(nd.keys.iter().cloned());
            gn.incs.extend(nd.incs.iter().copied());
            gn.seam_rel |= nd.seam_rel || nd.out.iter().any(|&h| arr.chains[h / 2].seam.is_some());
            gn.artificial &= nd.artificial;
        }
        for g in &mut gnodes {
            g.keys.sort();
            g.keys.dedup();
            if let Some(k) = VertexKey::canonical(&g.keys) {
                g.key = k.clone();
            }
            if g.nodes.len() > 1 {
                g.seam_rel = true;
            }
        }
        // Edges, identifying seam copies.
        let mut gedges: Vec<GEdge> = Vec::new();
        let mut chain_g: Vec<(usize, bool)> = vec![(0, true); arr.chains.len()];
        let mut seam_map: HashMap<(u8, usize, usize), usize> = HashMap::new();
        for (ci, ch) in arr.chains.iter().enumerate() {
            let cv = &arr.curves[ch.curve];
            let (ga, gb) = (node_g[ch.a], node_g[ch.b]);
            if let Some((dir, _copy)) = ch.seam {
                let k = (dir, ga, gb);
                if let Some(&ge) = seam_map.get(&k) {
                    gedges[ge].chains.push(ci);
                    chain_g[ci] = (ge, true);
                    if owner_rank(&cv.owner) < owner_rank(&gedges[ge].owner) {
                        gedges[ge].owner = cv.owner;
                        gedges[ge].curve = ch.curve;
                        gedges[ge].pa = ch.pa;
                        gedges[ge].pb = ch.pb;
                        gedges[ge].pars = ch.pars.clone();
                    }
                    continue;
                }
                seam_map.insert(k, gedges.len());
            }
            chain_g[ci] = (gedges.len(), true);
            gedges.push(GEdge {
                owner: cv.owner,
                curve: ch.curve,
                a: ga,
                b: gb,
                pa: ch.pa,
                pb: ch.pb,
                chains: vec![ci],
                pts: ch.pts.clone(),
                pars: ch.pars.clone(),
            });
        }
        let nh = gedges.len() * 2;
        let mut he_g = vec![0usize; arr.chains.len() * 2];
        for ci in 0..arr.chains.len() {
            let (ge, same) = chain_g[ci];
            he_g[ci * 2] = ge * 2 + if same { 0 } else { 1 };
            he_g[ci * 2 + 1] = ge * 2 + if same { 1 } else { 0 };
        }
        let mut left = vec![None; nh];
        for h in 0..arr.chains.len() * 2 {
            if let Some(c) = arr.left_cell(h) {
                left[he_g[h]] = Some(c);
            }
        }
        // Prefer the copy whose left side is inside for the drawn geometry.
        for ge in &mut gedges {
            if ge.chains.len() > 1 {
                for &ci in &ge.chains {
                    if arr.left_cell(ci * 2).is_some() || arr.left_cell(ci * 2 + 1).is_some() {
                        ge.pts = arr.chains[ci].pts.clone();
                    }
                }
            }
        }
        let mut rot: Vec<Vec<usize>> = vec![vec![]; gnodes.len()];
        for (g, gn) in gnodes.iter().enumerate() {
            let mut list: Vec<(f64, usize)> = Vec::new();
            for &n in &gn.nodes {
                for &h in &arr.nodes[n].out {
                    let gh = he_g[h];
                    if !list.iter().any(|x| x.1 == gh) {
                        list.push((arr.angle[h], gh));
                    }
                }
            }
            list.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            rot[g] = list.into_iter().map(|x| x.1).collect();
        }
        let mut rot_pos = vec![0usize; nh];
        for r in &rot {
            for (i, &h) in r.iter().enumerate() {
                rot_pos[h] = i;
            }
        }
        let mut gl = Glued {
            gnodes,
            node_g,
            gedges,
            he_g,
            rot,
            rot_pos,
            left,
            dissolved: vec![],
            region_of_cell: vec![],
            regions: vec![],
        };
        let ne = gl.gedges.len();
        let ncell = arr.cells.len();
        let cand = |gl: &Glued, dir: Option<u8>| -> Vec<usize> {
            (0..ne)
                .filter(|&e| match gl.gedges[e].owner {
                    Owner::Seam(d) => {
                        dir.is_none_or(|x| x == d)
                            && gl.left[e * 2].is_some()
                            && gl.left[e * 2 + 1].is_some()
                    }
                    _ => false,
                })
                .collect()
        };
        let comps = |gl: &Glued, dis: &[bool]| -> Uf {
            let mut uf = Uf::new(ncell);
            for e in 0..ne {
                if dis[e]
                    && let (Some(a), Some(b)) = (gl.left[e * 2], gl.left[e * 2 + 1])
                {
                    uf.union(a, b);
                }
            }
            uf
        };
        let mut dissolved = vec![false; ne];
        for &e in forced {
            if e >= ne {
                continue;
            }
            // A removed line lying on a seam leaves the seam behind, which
            // the guard then dissolves or keeps.
            let seam = gl.gedges[e].chains.iter().find_map(|&c| arr.chains[c].seam);
            match seam {
                Some((dir, _)) => gl.gedges[e].owner = Owner::Seam(dir),
                None => dissolved[e] = true,
            }
        }
        if !guard {
            for e in cand(&gl, None) {
                dissolved[e] = true;
            }
        } else {
            for dir in [0u8, 1u8] {
                let c = cand(&gl, Some(dir));
                if c.is_empty() {
                    continue;
                }
                let mut tent = dissolved.clone();
                for &e in &c {
                    tent[e] = true;
                }
                let mut uf = comps(&gl, &tent);
                let walks = gl.trace(&tent);
                let mut chi: HashMap<usize, i64> = HashMap::new();
                let mut b: HashMap<usize, i64> = HashMap::new();
                for (ci, cell) in arr.cells.iter().enumerate() {
                    if cell.inside {
                        *chi.entry(uf.find(ci)).or_default() += 1 - cell.holes.len() as i64;
                    }
                }
                for e in 0..ne {
                    if tent[e]
                        && let Some(a) = gl.left[e * 2]
                    {
                        *chi.entry(uf.find(a)).or_default() -= 1;
                    }
                }
                for (g, r) in gl.rot.iter().enumerate() {
                    let _ = g;
                    if !r.is_empty()
                        && r.iter().all(|&h| tent[h / 2])
                        && let Some(c) = r.iter().find_map(|&h| gl.left[h])
                    {
                        *chi.entry(uf.find(c)).or_default() += 1;
                    }
                }
                for w in &walks {
                    if let Some(c) = gl.left[w[0]] {
                        *b.entry(uf.find(c)).or_default() += 1;
                    }
                }
                if std::env::var("NT_DEBUG_GUARD").is_ok() {
                    let mut roots: Vec<_> = chi.keys().copied().collect();
                    roots.sort();
                    for r in roots {
                        eprintln!(
                            "guard dir {dir}: comp {r} chi {} b {}",
                            chi[&r],
                            b.get(&r).copied().unwrap_or(0)
                        );
                    }
                }
                for &e in &c {
                    let Some(a) = gl.left[e * 2] else { continue };
                    let r = uf.find(a);
                    let x = chi.get(&r).copied().unwrap_or(0);
                    let bb = b.get(&r).copied().unwrap_or(0);
                    if x == 2 - bb {
                        dissolved[e] = true;
                    }
                }
            }
        }
        if guard {
            // A region bordering both sides of a kept seam would wrap a full
            // period across it, leaving only polygons that circle the
            // surface; keep the other seam through such a region too.
            let mut uf = comps(&gl, &dissolved);
            let mut wrap_dirs: HashMap<usize, Vec<u8>> = HashMap::new();
            for e in 0..ne {
                if dissolved[e] {
                    continue;
                }
                if let (Owner::Seam(d), Some(a), Some(b)) =
                    (gl.gedges[e].owner, gl.left[e * 2], gl.left[e * 2 + 1])
                    && uf.find(a) == uf.find(b)
                {
                    wrap_dirs.entry(uf.find(a)).or_default().push(d);
                }
            }
            for e in 0..ne {
                if !dissolved[e] {
                    continue;
                }
                if let (Owner::Seam(d), Some(a)) = (gl.gedges[e].owner, gl.left[e * 2])
                    && wrap_dirs
                        .get(&uf.find(a))
                        .is_some_and(|ds| ds.iter().any(|x| *x != d))
                {
                    dissolved[e] = false;
                }
            }
        }
        let mut uf = comps(&gl, &dissolved);
        let mut region_of_cell = vec![usize::MAX; ncell];
        let mut rid: HashMap<usize, usize> = HashMap::new();
        for (ci, cell) in arr.cells.iter().enumerate() {
            if cell.inside {
                let r = uf.find(ci);
                let n = rid.len();
                region_of_cell[ci] = *rid.entry(r).or_insert(n);
            }
        }
        let walks = gl.trace(&dissolved);
        let mut regions: Vec<Vec<Vec<usize>>> = vec![vec![]; rid.len()];
        for w in walks {
            if let Some(c) = gl.left[w[0]] {
                regions[region_of_cell[c]].push(w);
            }
        }
        gl.dissolved = dissolved;
        gl.region_of_cell = region_of_cell;
        gl.regions = regions;
        gl
    }

    /// Non-dissolved incident glued edges of a node (self-loops twice).
    pub fn degree_edges(&self, g: usize) -> Vec<usize> {
        self.rot[g]
            .iter()
            .filter(|&&h| !self.dissolved[h / 2])
            .map(|&h| h / 2)
            .collect()
    }
}
