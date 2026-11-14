//! Polygon normalisation by bridging (SPEC §4.6 step 6) and auto-fill
//! driving (SPEC §7). Own design following the spec's chord rules:
//! loops are joined first, then repeated vertices are separated; the
//! shortest valid chord by 3D surface-path length wins, ties by vertex key.

use super::Owner;
use super::assemble::HMesh;
use super::ctx::{BodyCtx, FaceGeom};
use super::geom2::*;
use crate::fill::{FillInput, fill_polygon};
use crate::keys::VertexKey;
use glam::{DVec2, DVec3};
use newtopo_kernel::{EdgeId, FaceId};
use std::collections::{HashMap, HashSet};

/// Coordinates used for a region: one face's metric-scaled UV (lifted, u
/// mirrored for reversed faces so walks are counter-clockwise), or a best-fit
/// plane for regions spanning faces.
pub enum Chart<'a> {
    Face { fg: &'a FaceGeom, sign: f64 },
    Plane { o: DVec3, e1: DVec3, e2: DVec3 },
}

impl Chart<'_> {
    fn d(&self, raw: DVec2) -> DVec2 {
        match self {
            Chart::Face { fg, sign } => DVec2::new(sign * raw.x * fg.metric.x, raw.y * fg.metric.y),
            Chart::Plane { .. } => raw,
        }
    }
    fn shifts(&self) -> Vec<DVec2> {
        match self {
            Chart::Face { fg, sign } => {
                let p = fg.periods();
                let ku: Vec<i32> = if p.x > 0.0 { vec![-2, -1, 0, 1, 2] } else { vec![0] };
                let kv: Vec<i32> = if p.y > 0.0 { vec![-2, -1, 0, 1, 2] } else { vec![0] };
                let mut out = Vec::new();
                for &a in &ku {
                    for &b in &kv {
                        out.push(DVec2::new(sign * a as f64 * p.x * fg.metric.x, b as f64 * p.y * fg.metric.y));
                    }
                }
                out
            }
            Chart::Plane { .. } => vec![DVec2::ZERO],
        }
    }
    /// Raw shift for a domain shift.
    fn raw_shift(&self, d: DVec2) -> DVec2 {
        match self {
            Chart::Face { fg, sign } => DVec2::new(d.x / (sign * fg.metric.x), d.y / fg.metric.y),
            Chart::Plane { .. } => DVec2::ZERO,
        }
    }
}

pub struct Occ {
    pub v: usize,
    pub fid: usize,
    pub hout: usize,
    pub raw: DVec2,
    pub d: DVec2,
    pub dout: DVec2,
    pub din_rev: DVec2,
}

pub struct WalkGeom {
    pub occ: Vec<Occ>,
    /// Domain path of the half-edge leaving occurrence i.
    pub paths: Vec<Vec<DVec2>>,
    pub winding: DVec2,
}

pub fn he_path3(ctx: &BodyCtx, mesh: &HMesh, h: usize) -> Vec<DVec3> {
    let he = &mesh.he[h];
    if let Some(p) = &he.p3 {
        return p.clone();
    }
    if he.uv.is_empty() {
        return vec![mesh.verts[he.from].pos, mesh.verts[he.to].pos];
    }
    let mut uv = he.uv.clone();
    if uv.len() == 2 {
        let (a, b) = (uv[0], uv[1]);
        uv = (0..=12).map(|i| a + (b - a) * (i as f64 / 12.0)).collect();
    }
    let ev = ctx.face_eval(he.face, &uv);
    let mut out: Vec<DVec3> = ev.iter().map(|e| e.p).collect();
    if let (Some(f), Some(l)) = (out.first_mut(), Some(mesh.verts[he.from].pos)) {
        *f = l;
    }
    let to = mesh.verts[he.to].pos;
    if let Some(l) = out.last_mut() {
        *l = to;
    }
    out
}

fn first_dir(path: &[DVec2]) -> DVec2 {
    let o = path[0];
    let scale = path.iter().fold(0.0f64, |m, p| m.max((*p - o).length()));
    for p in &path[1..] {
        if (*p - o).length() > scale * 1e-6 {
            return (*p - o).normalize_or_zero();
        }
    }
    DVec2::X
}

pub fn chart_for<'a>(ctx: &'a BodyCtx, mesh: &HMesh, r: usize, walks: &[Vec<usize>]) -> Chart<'a> {
    let reg = &mesh.regions[r];
    let single = !reg.multi && walks.iter().flatten().all(|&h| mesh.he[h].face == reg.face && (!mesh.he[h].uv.is_empty() || mesh.he[h].owner == Owner::Fill));
    if single {
        let fg = ctx.face(reg.face);
        return Chart::Face { fg, sign: if fg.reversed { -1.0 } else { 1.0 } };
    }
    // Best-fit plane through all vertex positions; normal from CAD normals.
    let mut pts = Vec::new();
    let mut nrm = DVec3::ZERO;
    for &h in walks.iter().flatten() {
        let he = &mesh.he[h];
        pts.push(mesh.verts[he.from].pos);
        if let Some(uv) = he.uv.first() {
            if let Some(e) = ctx.face_eval(he.face, &[*uv]).first() {
                nrm += e.n;
            }
        }
    }
    let o = pts.iter().copied().sum::<DVec3>() / pts.len().max(1) as f64;
    let n = if nrm.length() > 1e-9 { nrm.normalize() } else { DVec3::Z };
    let e1 = n.any_orthonormal_vector();
    let e2 = n.cross(e1);
    Chart::Plane { o, e1, e2 }
}

pub fn walk_geom(ctx: &BodyCtx, mesh: &HMesh, chart: &Chart, walk: &[usize], fid: &dyn Fn(usize) -> usize) -> WalkGeom {
    let mut occ = Vec::with_capacity(walk.len());
    let mut paths = Vec::with_capacity(walk.len());
    let mut raw_cur: Option<DVec2> = None;
    let mut start_d = DVec2::ZERO;
    let mut end_d = DVec2::ZERO;
    for (k, &h) in walk.iter().enumerate() {
        let he = &mesh.he[h];
        let (raw0, dpath) = match chart {
            Chart::Face { fg, .. } => {
                let mut uv = he.uv.clone();
                if uv.is_empty() {
                    uv = vec![raw_cur.unwrap_or(DVec2::ZERO), raw_cur.unwrap_or(DVec2::ZERO)];
                }
                let s = match raw_cur {
                    Some(c) => fg.lift_near(uv[0], c) - uv[0],
                    None => DVec2::ZERO,
                };
                let lifted: Vec<DVec2> = uv.iter().map(|p| *p + s).collect();
                raw_cur = Some(*lifted.last().unwrap());
                (lifted[0], lifted.iter().map(|p| chart.d(*p)).collect::<Vec<_>>())
            }
            Chart::Plane { o, e1, e2 } => {
                let p3 = he_path3(ctx, mesh, h);
                let d: Vec<DVec2> = p3.iter().map(|p| DVec2::new((*p - *o).dot(*e1), (*p - *o).dot(*e2))).collect();
                (d[0], d)
            }
        };
        if k == 0 {
            start_d = dpath[0];
        }
        end_d = *dpath.last().unwrap();
        occ.push(Occ { v: he.from, fid: fid(he.from), hout: h, raw: raw0, d: dpath[0], dout: first_dir(&dpath), din_rev: DVec2::ZERO });
        paths.push(dpath);
    }
    let n = occ.len();
    for i in 0..n {
        let prev = &paths[(i + n - 1) % n];
        let rev: Vec<DVec2> = prev.iter().rev().copied().collect();
        occ[i].din_rev = first_dir(&rev);
    }
    WalkGeom { occ, paths, winding: end_d - start_d }
}

/// Final vertices that occur in more than one separate run of a walk
/// (consecutive repeats are one pole run and collapse later).
fn repeated_fids(g: &WalkGeom) -> Vec<usize> {
    let n = g.occ.len();
    if n == 0 {
        return vec![];
    }
    let mut runs: HashMap<usize, usize> = HashMap::new();
    for k in 0..n {
        let prev = g.occ[(k + n - 1) % n].fid;
        if g.occ[k].fid != prev || n == 1 {
            *runs.entry(g.occ[k].fid).or_default() += 1;
        }
    }
    // A walk made of a single fid has no run starts.
    let mut out: Vec<usize> = runs.into_iter().filter(|(_, c)| *c > 1).map(|(f, _)| f).collect();
    out.sort();
    out
}

fn in_sector(o: &Occ, dir: DVec2) -> bool {
    let a = ccw_angle(o.dout, dir);
    let lim = ccw_angle(o.dout, o.din_rev);
    a > 1e-9 && a < lim - 1e-9
}

fn chord_clear(geoms: &[WalkGeom], shifts: &[DVec2], a: DVec2, b: DVec2, eps: f64) -> bool {
    let lo = a.min(b) - DVec2::splat(eps);
    let hi = a.max(b) + DVec2::splat(eps);
    for g in geoms {
        for path in &g.paths {
            for s in shifts {
                for w in path.windows(2) {
                    let (c, d) = (w[0] + *s, w[1] + *s);
                    if c.x.max(d.x) < lo.x || c.x.min(d.x) > hi.x || c.y.max(d.y) < lo.y || c.y.min(d.y) > hi.y {
                        continue;
                    }
                    for (sa, _) in seg_hits(a, b, c, d, eps) {
                        let p = a + (b - a) * sa;
                        if (p - a).length() <= eps || (p - b).length() <= eps {
                            continue;
                        }
                        return false;
                    }
                }
            }
        }
    }
    true
}

/// Uniform grid over all boundary path segments (with period shifts) for
/// chord crossing tests.
struct SegIndex {
    segs: Vec<(DVec2, DVec2)>,
    cell: f64,
    lo: DVec2,
    grid: HashMap<(i64, i64), Vec<usize>>,
}

impl SegIndex {
    fn new(geoms: &[WalkGeom], shifts: &[DVec2]) -> Self {
        let mut segs = Vec::new();
        for g in geoms {
            for path in &g.paths {
                for w in path.windows(2) {
                    for s in shifts {
                        segs.push((w[0] + *s, w[1] + *s));
                    }
                }
            }
        }
        let mut lo = DVec2::splat(f64::INFINITY);
        let mut hi = DVec2::splat(f64::NEG_INFINITY);
        for (a, b) in &segs {
            lo = lo.min(a.min(*b));
            hi = hi.max(a.max(*b));
        }
        let ext = (hi - lo).max_element().max(1e-12);
        let n = ((segs.len() as f64).sqrt().ceil()).clamp(1.0, 256.0);
        let cell = ext / n;
        let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (i, (a, b)) in segs.iter().enumerate() {
            let (x0, y0) = (((a.x.min(b.x) - lo.x) / cell).floor() as i64, ((a.y.min(b.y) - lo.y) / cell).floor() as i64);
            let (x1, y1) = (((a.x.max(b.x) - lo.x) / cell).floor() as i64, ((a.y.max(b.y) - lo.y) / cell).floor() as i64);
            for x in x0..=x1 {
                for y in y0..=y1 {
                    grid.entry((x, y)).or_default().push(i);
                }
            }
        }
        SegIndex { segs, cell, lo, grid }
    }

    fn clear(&self, a: DVec2, b: DVec2, eps: f64) -> bool {
        let (x0, y0) = (((a.x.min(b.x) - eps - self.lo.x) / self.cell).floor() as i64, ((a.y.min(b.y) - eps - self.lo.y) / self.cell).floor() as i64);
        let (x1, y1) = (((a.x.max(b.x) + eps - self.lo.x) / self.cell).floor() as i64, ((a.y.max(b.y) + eps - self.lo.y) / self.cell).floor() as i64);
        let mut seen = std::collections::HashSet::new();
        let cells = ((x1 - x0 + 1) * (y1 - y0 + 1)) as usize;
        let cand: Box<dyn Iterator<Item = usize>> = if cells > self.segs.len() {
            Box::new(0..self.segs.len())
        } else {
            Box::new((x0..=x1).flat_map(move |x| (y0..=y1).map(move |y| (x, y))).filter_map(|k| self.grid.get(&k)).flatten().copied())
        };
        for i in cand {
            if !seen.insert(i) {
                continue;
            }
            let (c, d) = self.segs[i];
            for (sa, _) in seg_hits(a, b, c, d, eps) {
                let p = a + (b - a) * sa;
                if (p - a).length() <= eps || (p - b).length() <= eps {
                    continue;
                }
                return false;
            }
        }
        true
    }
}

struct Cand {
    wi: usize,
    i: usize,
    wj: usize,
    j: usize,
    shift: DVec2,
    len: f64,
    keys: (VertexKey, VertexKey),
}

/// Bridge every region of the mesh. Returns regions that could not be made
/// simple.
pub fn bridge_all(ctx: &BodyCtx, mesh: &mut HMesh, pole_target: &HashMap<EdgeId, usize>) -> Vec<usize> {
    let mut failed = Vec::new();
    let mut queue: Vec<usize> = (0..mesh.regions.len()).filter(|&r| mesh.regions[r].alive).collect();
    let fid_of = |mesh: &HMesh, v: usize| mesh.final_v(v, pole_target);
    // Final-vertex adjacency and per-region half-edge lists, kept up to date
    // as chords are inserted.
    let mut adj: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut reg_he: HashMap<usize, Vec<usize>> = HashMap::new();
    for h in mesh.alive_he() {
        let (a, b) = (fid_of(mesh, mesh.he[h].from), fid_of(mesh, mesh.he[h].to));
        adj.entry(a).or_default().push(b);
        reg_he.entry(mesh.he[h].region).or_default().push(h);
    }
    let mut guard = 0usize;
    let timing = std::env::var("NT_TIME").is_ok();
    let mut tt = [std::time::Duration::ZERO; 5];
    let mut iters = 0usize;
    while let Some(r) = queue.pop() {
        loop {
            iters += 1;
            let t0 = std::time::Instant::now();
            guard += 1;
            if guard > 200_000 {
                failed.push(r);
                break;
            }
            let walks = {
                let hs = reg_he.get(&r).cloned().unwrap_or_default();
                let mut seen = HashSet::new();
                let mut out = Vec::new();
                for h in hs {
                    if !mesh.he[h].alive || mesh.he[h].region != r || seen.contains(&h) {
                        continue;
                    }
                    let o = mesh.orbit(h);
                    for &x in &o {
                        seen.insert(x);
                    }
                    out.push(o);
                }
                out
            };
            if walks.is_empty() {
                break;
            }
            let chart = chart_for(ctx, mesh, r, &walks);
            let fidf = |v: usize| fid_of(mesh, v);
            let geoms: Vec<WalkGeom> = walks.iter().map(|w| walk_geom(ctx, mesh, &chart, w, &fidf)).collect();
            let eps = ctx.size * 1e-9;
            tt[0] += t0.elapsed();
            let t1 = std::time::Instant::now();
            if walks.len() == 1 {
                let g = &geoms[0];
                let repeated = !repeated_fids(g).is_empty();
                let distinct: HashSet<usize> = g.occ.iter().map(|o| o.fid).collect();
                if !repeated && g.winding.length() <= eps * 1e3 && distinct.len() >= 3 {
                    break;
                }
            }
            // Existing edges between final vertices.
            let joined_fn = |a: usize, b: usize| adj.get(&a).is_some_and(|l| l.contains(&b)) || adj.get(&b).is_some_and(|l| l.contains(&a));
            let is_pole = |v: usize| mesh.verts[v].pole.is_some();
            let shifts = chart.shifts();
            let mut cands: Vec<Cand> = Vec::new();
            let push = |cands: &mut Vec<Cand>, wi: usize, i: usize, wj: usize, j: usize, shift: DVec2| {
                let (oa, ob) = (&geoms[wi].occ[i], &geoms[wj].occ[j]);
                if oa.fid == ob.fid || joined_fn(oa.fid, ob.fid) {
                    return;
                }
                if is_pole(oa.v) && is_pole(ob.v) && mesh.verts[oa.v].pole == mesh.verts[ob.v].pole {
                    return;
                }
                let (ka, kb) = (mesh.verts[oa.fid].key.clone(), mesh.verts[ob.fid].key.clone());
                let keys = if ka <= kb { (ka, kb) } else { (kb, ka) };
                cands.push(Cand { wi, i, wj, j, shift, len: 0.0, keys });
            };
            // Candidate partners: the K nearest occurrences (domain distance,
            // with period shifts) of each occurrence.
            const K: usize = 12;
            let occs: Vec<(usize, usize)> = (0..geoms.len()).flat_map(|w| (0..geoms[w].occ.len()).map(move |i| (w, i))).collect();
            let single = walks.len() == 1;
            let (rep_pos, wind) = if single {
                let g = &geoms[0];
                let mut pos: HashMap<usize, Vec<usize>> = HashMap::new();
                for (k, o) in g.occ.iter().enumerate() {
                    pos.entry(o.fid).or_default().push(k);
                }
                let repf = repeated_fids(g);
                let rp: Vec<Vec<usize>> = repf.iter().filter_map(|f| pos.get(f).cloned()).collect();
                (rp, g.winding.length() > eps * 1e3)
            } else {
                (vec![], false)
            };
            let separates = |i: usize, j: usize| -> bool {
                let (i, j) = (i.min(j), i.max(j));
                rep_pos.iter().any(|ps| {
                    let inside = ps.iter().any(|&p| p > i && p < j);
                    let outside = ps.iter().any(|&p| p < i || p > j);
                    inside && outside
                })
            };
            let mut seen_pairs: std::collections::HashSet<(usize, usize, usize, usize, i64, i64)> = Default::default();
            for &(wa, ia) in &occs {
                let oa = &geoms[wa].occ[ia];
                let mut near: Vec<(f64, usize, usize, DVec2)> = Vec::new();
                for &(wb, ib) in &occs {
                    if single {
                        let n = geoms[0].occ.len();
                        let (lo, hi) = (ia.min(ib), ia.max(ib));
                        if hi - lo < 2 || (lo == 0 && hi == n - 1) {
                            continue;
                        }
                    } else if wb == wa {
                        continue;
                    }
                    let ob = &geoms[wb].occ[ib];
                    if oa.fid == ob.fid {
                        continue;
                    }
                    for s in &shifts {
                        if single && !wind && *s != DVec2::ZERO {
                            continue;
                        }
                        let d = (ob.d + *s - oa.d).length();
                        if near.len() < K || d < near[near.len() - 1].0 {
                            let pos = near.partition_point(|x| x.0 <= d);
                            near.insert(pos, (d, wb, ib, *s));
                            if near.len() > K {
                                near.pop();
                            }
                        }
                    }
                }
                for (_, wb, ib, sh) in near {
                    let (wi, i, wj, j, shift) = if (wa, ia) <= (wb, ib) { (wa, ia, wb, ib, sh) } else { (wb, ib, wa, ia, -sh) };
                    let key = (wi, i, wj, j, (shift.x * 1e6).round() as i64, (shift.y * 1e6).round() as i64);
                    if !seen_pairs.insert(key) {
                        continue;
                    }
                    if single && !wind && !separates(i, j) {
                        continue;
                    }
                    push(&mut cands, wi, i, wj, j, shift);
                }
            }
            tt[1] += t1.elapsed();
            let t2 = std::time::Instant::now();
            if cands.is_empty() {
                if std::env::var("NT_DEBUG_BRIDGE").is_ok() {
                    eprintln!("bridge: no candidates in region {r}: {} walks", walks.len());
                    for (wi, g) in geoms.iter().enumerate() {
                        eprintln!("  walk {wi} winding {:?}", g.winding);
                        for o in &g.occ {
                            eprintln!("    v{} fid{} d {:?} key {:?}", o.v, o.fid, o.d, mesh.verts[o.v].key);
                        }
                    }
                }
                failed.push(r);
                break;
            }
            // Rank by domain distance (≈ 3D length in metric-scaled UV),
            // then take exact 3D surface-path lengths for the best batch and
            // test validity in exact order (SPEC §4.6 step 6).
            for c in &mut cands {
                let (oa, ob) = (&geoms[c.wi].occ[c.i], &geoms[c.wj].occ[c.j]);
                c.len = (ob.d + c.shift - oa.d).length();
            }
            cands.sort_by(|a, b| a.len.total_cmp(&b.len).then_with(|| a.keys.cmp(&b.keys)));
            let index = SegIndex::new(&geoms, &shifts);
            let q = ctx.size * 1e-9;
            // Exact 3D lengths of every candidate's surface path.
            match &chart {
                Chart::Face { fg, .. } => {
                    let k = 8;
                    for chunk in (0..cands.len()).collect::<Vec<_>>().chunks(4096) {
                        let mut uvs = Vec::with_capacity(chunk.len() * (k + 1));
                        for &ci in chunk {
                            let c = &cands[ci];
                            let a = geoms[c.wi].occ[c.i].raw;
                            let b = geoms[c.wj].occ[c.j].raw + chart.raw_shift(c.shift);
                            for t in 0..=k {
                                uvs.push(a + (b - a) * (t as f64 / k as f64));
                            }
                        }
                        let ev = ctx.face_eval(fg.id, &uvs);
                        for (n, &ci) in chunk.iter().enumerate() {
                            let mut l = 0.0;
                            for t in 0..k {
                                if let (Some(p), Some(q)) = (ev.get(n * (k + 1) + t), ev.get(n * (k + 1) + t + 1)) {
                                    l += (q.p - p.p).length();
                                }
                            }
                            cands[ci].len = l;
                        }
                    }
                }
                Chart::Plane { .. } => {
                    for ci in 0..cands.len() {
                        let c = &cands[ci];
                        cands[ci].len = (mesh.verts[geoms[c.wi].occ[c.i].v].pos - mesh.verts[geoms[c.wj].occ[c.j].v].pos).length();
                    }
                }
            }
            cands.sort_by(|a, b| {
                let (la, lb) = ((a.len / q).round(), (b.len / q).round());
                la.total_cmp(&lb).then_with(|| a.keys.cmp(&b.keys)).then(a.i.cmp(&b.i)).then(a.j.cmp(&b.j))
            });
            // Batch selection: shortest valid chords that join different
            // loops (Kruskal) or separate repeated vertices, never crossing
            // each other and never sharing an occurrence.
            let mut accepted: Vec<usize> = Vec::new();
            let mut acc_segs: Vec<(DVec2, DVec2)> = Vec::new();
            let mut used: HashSet<(usize, usize)> = HashSet::new();
            let mut wuf: Vec<usize> = (0..geoms.len()).collect();
            fn find(u: &mut [usize], x: usize) -> usize {
                let mut r = x;
                while u[r] != r {
                    r = u[r];
                }
                u[x] = r;
                r
            }
            let mut separated: HashSet<usize> = HashSet::new();
            for (ci, c) in cands.iter().enumerate() {
                if used.contains(&(c.wi, c.i)) || used.contains(&(c.wj, c.j)) {
                    continue;
                }
                let (oa, ob) = (&geoms[c.wi].occ[c.i], &geoms[c.wj].occ[c.j]);
                let (a, b) = (oa.d, ob.d + c.shift);
                if (b - a).length() <= eps {
                    continue;
                }
                let mut sep_set: Vec<usize> = Vec::new();
                if walks.len() > 1 {
                    if find(&mut wuf, c.wi) == find(&mut wuf, c.wj) {
                        continue;
                    }
                } else if !wind {
                    let (i, j) = (c.i.min(c.j), c.i.max(c.j));
                    for (ps, f) in rep_pos.iter().zip(repeated_fids(&geoms[0])) {
                        let inside = ps.iter().any(|&p| p > i && p < j);
                        let outside = ps.iter().any(|&p| p < i || p > j);
                        if inside && outside {
                            sep_set.push(f);
                        }
                    }
                    if sep_set.iter().all(|f| separated.contains(f)) {
                        continue;
                    }
                }
                if !in_sector(oa, (b - a).normalize()) || !in_sector(ob, (a - b).normalize()) {
                    continue;
                }
                if !index.clear(a, b, eps * 10.0) {
                    continue;
                }
                let crosses = acc_segs.iter().any(|&(c0, c1)| {
                    shifts.iter().any(|s| {
                        seg_hits(a, b, c0 + *s, c1 + *s, eps * 10.0).iter().any(|&(sa, _)| {
                            let p = a + (b - a) * sa;
                            (p - a).length() > eps * 10.0 && (p - b).length() > eps * 10.0
                        })
                    })
                });
                if crosses {
                    continue;
                }
                if walks.len() > 1 {
                    let (x, y) = (find(&mut wuf, c.wi), find(&mut wuf, c.wj));
                    wuf[x] = y;
                } else {
                    separated.extend(sep_set);
                }
                used.insert((c.wi, c.i));
                used.insert((c.wj, c.j));
                acc_segs.push((a, b));
                accepted.push(ci);
                if walks.len() == 1 && wind {
                    break;
                }
            }
            let chosen = accepted.first().copied();
            let chosen = chosen.map(|ci| &cands[ci]);
            tt[2] += t2.elapsed();
            let Some(c) = chosen else {
                if std::env::var("NT_DEBUG_BRIDGE").is_ok() {
                    eprintln!("bridge fail region {r}: {} walks, {} cands", walks.len(), cands.len());
                    for (wi, g) in geoms.iter().enumerate() {
                        eprintln!("  walk {wi} winding {:?}", g.winding);
                        for o in &g.occ {
                            eprintln!("    v{} fid{} d {:?} dout {:?} din {:?} key {:?}", o.v, o.fid, o.d, o.dout, o.din_rev, mesh.verts[o.v].key);
                        }
                    }
                    for c in cands.iter().take(6) {
                        let (oa, ob) = (&geoms[c.wi].occ[c.i], &geoms[c.wj].occ[c.j]);
                        let (a, b) = (oa.d, ob.d + c.shift);
                        eprintln!("  cand v{}-v{} len {} sector {} {} clear {}", oa.v, ob.v, c.len, in_sector(oa, (b - a).normalize()), in_sector(ob, (a - b).normalize()), chord_clear(&geoms, &shifts, a, b, eps * 10.0));
                    }
                }
                failed.push(r);
                break;
            };
            let _ = c;
            for &ci in &accepted {
                let c = &cands[ci];
                let (oa, ob) = (&geoms[c.wi].occ[c.i], &geoms[c.wj].occ[c.j]);
                let (hx, hy) = (oa.hout, ob.hout);
                let (uv, p3) = match &chart {
                    Chart::Face { .. } => (vec![oa.raw, ob.raw + chart.raw_shift(c.shift)], None),
                    Chart::Plane { .. } => (vec![], Some(vec![mesh.verts[oa.v].pos, mesh.verts[ob.v].pos])),
                };
                let (fa, fb) = (oa.fid, ob.fid);
                let ch = mesh.insert_chord(hx, hy, uv, p3);
                adj.entry(fa).or_default().push(fb);
                adj.entry(fb).or_default().push(fa);
                reg_he.entry(r).or_default().extend([ch, ch + 1]);
            }
            if walks.len() > 1 {
                continue;
            }
            // The walk split into pieces: each extra orbit becomes a region.
            let hs: Vec<usize> = reg_he.get(&r).cloned().unwrap_or_default();
            let mut seen = HashSet::new();
            let mut first = true;
            for h in hs {
                if !mesh.he[h].alive || mesh.he[h].region != r || seen.contains(&h) {
                    continue;
                }
                let o = mesh.orbit(h);
                for &x in &o {
                    seen.insert(x);
                }
                if first {
                    first = false;
                    reg_he.insert(r, o);
                    continue;
                }
                let nr = mesh.new_region(r);
                for &x in &o {
                    mesh.he[x].region = nr;
                }
                reg_he.insert(nr, o);
                queue.push(nr);
            }
        }
    }
    if timing {
        eprintln!("  bridge: {iters} iterations; walks {:?} cands {:?} choose {:?}", tt[0], tt[1], tt[2]);
    }
    failed
}

/// Auto-fill every polygon with more than four sides (SPEC §7). Returns
/// regions filled only by the fan fallback.
pub fn autofill_all(ctx: &BodyCtx, mesh: &mut HMesh, pole_target: &HashMap<EdgeId, usize>) -> HashSet<usize> {
    let mut fan = HashSet::new();
    let regions: Vec<usize> = (0..mesh.regions.len()).filter(|&r| mesh.regions[r].alive).collect();
    let all = mesh.all_region_walks();
    for r in regions {
        let Some(walks) = all.get(&r) else { continue };
        if walks.len() != 1 || walks[0].len() <= 4 {
            continue;
        }
        let walk = walks[0].clone();
        let chart = chart_for(ctx, mesh, r, &walks.clone());
        let fidf = |v: usize| mesh.final_v(v, pole_target);
        let g = walk_geom(ctx, mesh, &chart, &walk, &fidf);
        let uv: Vec<DVec2> = g.occ.iter().map(|o| o.d).collect();
        let pos: Vec<DVec3> = g.occ.iter().map(|o| mesh.verts[o.v].pos).collect();
        let mut order: Vec<usize> = (0..g.occ.len()).collect();
        order.sort_by(|&a, &b| mesh.verts[g.occ[a].fid].key.cmp(&mesh.verts[g.occ[b].fid].key).then(a.cmp(&b)));
        let mut rank = vec![0u32; order.len()];
        for (k, &i) in order.iter().enumerate() {
            rank[i] = k as u32;
        }
        let iso = match &chart {
            Chart::Face { sign, .. } => [DVec2::new(*sign, 0.0), DVec2::new(0.0, 1.0)],
            Chart::Plane { .. } => [DVec2::X, DVec2::Y],
        };
        let mut res = fill_polygon(&FillInput { uv: &uv, pos: &pos, rank: &rank, iso_dirs: iso });
        // Diagonals must not duplicate an edge that already joins the same
        // two vertices elsewhere (that would be non-manifold).
        let n = g.occ.len();
        let joined: HashSet<(usize, usize)> = mesh
            .alive_he()
            .map(|h| {
                let (a, b) = (mesh.final_v(mesh.he[h].from, pole_target), mesh.final_v(mesh.he[h].to, pole_target));
                (a.min(b), a.max(b))
            })
            .collect();
        let bad = |polys: &[Vec<u32>]| {
            polys.iter().any(|p| {
                (0..p.len()).any(|k| {
                    let (a, b) = (p[k] as usize, p[(k + 1) % p.len()] as usize);
                    if b == (a + 1) % n || a == (b + 1) % n {
                        return false;
                    }
                    let (fa, fb) = (g.occ[a].fid, g.occ[b].fid);
                    fa == fb || joined.contains(&(fa.min(fb), fa.max(fb)))
                })
            })
        };
        if bad(&res.polys) {
            let forbid: HashSet<(usize, usize)> = (0..n)
                .flat_map(|a| (0..n).map(move |b| (a, b)))
                .filter(|&(a, b)| a < b && (g.occ[a].fid == g.occ[b].fid || joined.contains(&(g.occ[a].fid.min(g.occ[b].fid), g.occ[a].fid.max(g.occ[b].fid)))))
                .collect();
            match ear_fill(&uv, &forbid) {
                Some(p) => {
                    res.polys = p;
                    res.method = "ear-uv";
                }
                None => continue,
            }
        }
        if res.method == "fan" {
            fan.insert(r);
        }
        if res.polys.len() <= 1 {
            continue;
        }
        apply_fill(mesh, r, &walk, &g, &chart, &res.polys);
        let _ = FaceId(0);
    }
    fan
}

fn apply_fill(mesh: &mut HMesh, r: usize, walk: &[usize], g: &WalkGeom, chart: &Chart, polys: &[Vec<u32>]) {
    let n = walk.len();
    let mut diag: HashMap<(usize, usize), usize> = HashMap::new();
    let face = mesh.regions[r].face;
    let mut new_next: Vec<(usize, usize)> = Vec::new();
    for (pi, poly) in polys.iter().enumerate() {
        let region = if pi == 0 { r } else { mesh.new_region(r) };
        let m = poly.len();
        let mut hes = Vec::with_capacity(m);
        for k in 0..m {
            let (a, b) = (poly[k] as usize, poly[(k + 1) % m] as usize);
            let h = if b == (a + 1) % n {
                walk[a]
            } else if let Some(&h) = diag.get(&(a, b)) {
                h
            } else {
                let (oa, ob) = (&g.occ[a], &g.occ[b]);
                let (uv, p3) = match chart {
                    Chart::Face { .. } => (vec![oa.raw, ob.raw], None),
                    Chart::Plane { .. } => (vec![], Some(vec![mesh.verts[oa.v].pos, mesh.verts[ob.v].pos])),
                };
                let mut ruv = uv.clone();
                ruv.reverse();
                let rp3 = p3.as_ref().map(|v: &Vec<DVec3>| v.iter().rev().copied().collect());
                let h = mesh.he.len();
                mesh.he.push(super::assemble::HE { from: oa.v, to: ob.v, twin: Some(h + 1), next: h, region, owner: Owner::Fill, face, uv, p3, alive: true, src: vec![] });
                mesh.he.push(super::assemble::HE { from: ob.v, to: oa.v, twin: Some(h), next: h + 1, region, owner: Owner::Fill, face, uv: ruv, p3: rp3, alive: true, src: vec![] });
                diag.insert((a, b), h);
                diag.insert((b, a), h + 1);
                h
            };
            mesh.he[h].region = region;
            hes.push(h);
        }
        for k in 0..m {
            new_next.push((hes[k], hes[(k + 1) % m]));
        }
    }
    for (a, b) in new_next {
        mesh.he[a].next = b;
    }
}

/// Ear clipping in the domain that never uses a forbidden diagonal, then
/// greedy pairing of adjacent triangles into strictly convex quads.
fn ear_fill(uv: &[DVec2], forbid: &HashSet<(usize, usize)>) -> Option<Vec<Vec<u32>>> {
    let n = uv.len();
    let ok = |a: usize, b: usize| !forbid.contains(&(a.min(b), a.max(b)));
    let orient = |a: usize, b: usize, c: usize| super::geom2::orient(uv[a], uv[b], uv[c]);
    let mut idx: Vec<usize> = (0..n).collect();
    let mut tris: Vec<[usize; 3]> = Vec::new();
    while idx.len() > 3 {
        let m = idx.len();
        let mut best: Option<(f64, usize)> = None;
        for k in 0..m {
            let (a, b, c) = (idx[(k + m - 1) % m], idx[k], idx[(k + 1) % m]);
            if orient(a, b, c) <= 0.0 || !ok(a, c) {
                continue;
            }
            let ear = idx.iter().all(|&q| q == a || q == b || q == c || !(orient(a, b, q) >= 0.0 && orient(b, c, q) >= 0.0 && orient(c, a, q) >= 0.0));
            if !ear {
                continue;
            }
            if m == 4 {
                let rest: Vec<usize> = idx.iter().copied().filter(|&q| q != b).collect();
                if orient(rest[0], rest[1], rest[2]) <= 0.0 {
                    continue;
                }
            }
            let q = orient(a, b, c) / ((uv[b] - uv[a]).length_squared() + (uv[c] - uv[b]).length_squared() + (uv[a] - uv[c]).length_squared()).max(1e-300);
            if best.is_none_or(|(bq, _)| q > bq) {
                best = Some((q, k));
            }
        }
        let (_, k) = best?;
        let m = idx.len();
        tris.push([idx[(k + m - 1) % m], idx[k], idx[(k + 1) % m]]);
        idx.remove(k);
    }
    tris.push([idx[0], idx[1], idx[2]]);
    let mut used = vec![false; tris.len()];
    let mut out: Vec<Vec<u32>> = Vec::new();
    for i in 0..tris.len() {
        if used[i] {
            continue;
        }
        used[i] = true;
        let mut quad = None;
        for j in i + 1..tris.len() {
            if used[j] {
                continue;
            }
            let (t1, t2) = (tris[i], tris[j]);
            for e in 0..3 {
                let (a, b) = (t1[e], t1[(e + 1) % 3]);
                if let Some(f) = (0..3).find(|&f| t2[f] == b && t2[(f + 1) % 3] == a) {
                    let q = [a, t2[(f + 2) % 3], b, t1[(e + 2) % 3]];
                    if (0..4).all(|k| orient(q[k], q[(k + 1) % 4], q[(k + 2) % 4]) > 0.0) {
                        quad = Some((j, q));
                    }
                }
            }
            if quad.is_some() {
                break;
            }
        }
        match quad {
            Some((j, q)) => {
                used[j] = true;
                out.push(q.iter().map(|&x| x as u32).collect());
            }
            None => out.push(tris[i].iter().map(|&x| x as u32).collect()),
        }
    }
    Some(out)
}
