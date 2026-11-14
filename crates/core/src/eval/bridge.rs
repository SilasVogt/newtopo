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
    let mut guard = 0usize;
    while let Some(r) = queue.pop() {
        loop {
            guard += 1;
            if guard > 200_000 {
                failed.push(r);
                break;
            }
            let walks = mesh.region_walks(r);
            if walks.is_empty() {
                break;
            }
            let chart = chart_for(ctx, mesh, r, &walks);
            let fidf = |v: usize| fid_of(mesh, v);
            let geoms: Vec<WalkGeom> = walks.iter().map(|w| walk_geom(ctx, mesh, &chart, w, &fidf)).collect();
            let eps = ctx.size * 1e-9;
            if walks.len() == 1 {
                let g = &geoms[0];
                let repeated = !repeated_fids(g).is_empty();
                let distinct: HashSet<usize> = g.occ.iter().map(|o| o.fid).collect();
                if !repeated && g.winding.length() <= eps * 1e3 && distinct.len() >= 3 {
                    break;
                }
            }
            // Existing edges between final vertices.
            let mut joined: HashSet<(usize, usize)> = HashSet::new();
            for h in mesh.alive_he() {
                let (a, b) = (fid_of(mesh, mesh.he[h].from), fid_of(mesh, mesh.he[h].to));
                joined.insert((a.min(b), a.max(b)));
            }
            let is_pole = |v: usize| mesh.verts[v].pole.is_some();
            let shifts = chart.shifts();
            let mut cands: Vec<Cand> = Vec::new();
            let push = |cands: &mut Vec<Cand>, wi: usize, i: usize, wj: usize, j: usize, shift: DVec2| {
                let (oa, ob) = (&geoms[wi].occ[i], &geoms[wj].occ[j]);
                if oa.fid == ob.fid || joined.contains(&(oa.fid.min(ob.fid), oa.fid.max(ob.fid))) {
                    return;
                }
                if is_pole(oa.v) && is_pole(ob.v) && mesh.verts[oa.v].pole == mesh.verts[ob.v].pole {
                    return;
                }
                let (ka, kb) = (mesh.verts[oa.fid].key.clone(), mesh.verts[ob.fid].key.clone());
                let keys = if ka <= kb { (ka, kb) } else { (kb, ka) };
                cands.push(Cand { wi, i, wj, j, shift, len: 0.0, keys });
            };
            if walks.len() > 1 {
                for wi in 0..walks.len() {
                    for wj in wi + 1..walks.len() {
                        for i in 0..geoms[wi].occ.len() {
                            for j in 0..geoms[wj].occ.len() {
                                for s in &shifts {
                                    // Keep only shifts that make the chord short-ish.
                                    let d = geoms[wj].occ[j].d + *s - geoms[wi].occ[i].d;
                                    let shortest = shifts.iter().map(|t| (geoms[wj].occ[j].d + *t - geoms[wi].occ[i].d).length()).fold(f64::INFINITY, f64::min);
                                    if d.length() > shortest * 1.0001 + eps && *s != DVec2::ZERO {
                                        continue;
                                    }
                                    push(&mut cands, wi, i, wj, j, *s);
                                }
                            }
                        }
                    }
                }
            } else {
                let g = &geoms[0];
                let n = g.occ.len();
                let mut pos: HashMap<usize, Vec<usize>> = HashMap::new();
                for (k, o) in g.occ.iter().enumerate() {
                    pos.entry(o.fid).or_default().push(k);
                }
                let repf = repeated_fids(g);
                let rep: Vec<&Vec<usize>> = repf.iter().filter_map(|f| pos.get(f)).collect();
                let wind = g.winding.length() > eps * 1e3;
                for i in 0..n {
                    for j in i + 2..n {
                        if i == 0 && j == n - 1 {
                            continue;
                        }
                        let separates = rep.iter().any(|ps| {
                            let inside = ps.iter().any(|&p| p > i && p < j);
                            let outside = ps.iter().any(|&p| p < i || p > j);
                            inside && outside
                        });
                        if !separates && !wind {
                            continue;
                        }
                        if wind {
                            for s in &shifts {
                                push(&mut cands, 0, i, 0, j, *s);
                            }
                        } else {
                            push(&mut cands, 0, i, 0, j, DVec2::ZERO);
                        }
                    }
                }
            }
            if cands.is_empty() {
                failed.push(r);
                break;
            }
            // Exact 3D lengths of the chords' surface paths.
            match &chart {
                Chart::Face { fg, .. } => {
                    let k = 8;
                    let mut uvs = Vec::with_capacity(cands.len() * (k + 1));
                    for c in &cands {
                        let a = geoms[c.wi].occ[c.i].raw;
                        let b = geoms[c.wj].occ[c.j].raw + chart.raw_shift(c.shift);
                        for t in 0..=k {
                            uvs.push(a + (b - a) * (t as f64 / k as f64));
                        }
                    }
                    let ev = ctx.face_eval(fg.id, &uvs);
                    for (ci, c) in cands.iter_mut().enumerate() {
                        let mut l = 0.0;
                        for t in 0..k {
                            let (p, q) = (ev.get(ci * (k + 1) + t), ev.get(ci * (k + 1) + t + 1));
                            if let (Some(p), Some(q)) = (p, q) {
                                l += (q.p - p.p).length();
                            }
                        }
                        c.len = l;
                    }
                }
                Chart::Plane { .. } => {
                    for c in &mut cands {
                        c.len = (mesh.verts[geoms[c.wi].occ[c.i].v].pos - mesh.verts[geoms[c.wj].occ[c.j].v].pos).length();
                    }
                }
            }
            let q = ctx.size * 1e-9;
            cands.sort_by(|a, b| {
                let (la, lb) = ((a.len / q).round(), (b.len / q).round());
                la.total_cmp(&lb).then_with(|| a.keys.cmp(&b.keys)).then(a.i.cmp(&b.i)).then(a.j.cmp(&b.j))
            });
            let mut chosen = None;
            for c in &cands {
                let (oa, ob) = (&geoms[c.wi].occ[c.i], &geoms[c.wj].occ[c.j]);
                let (a, b) = (oa.d, ob.d + c.shift);
                if (b - a).length() <= eps {
                    continue;
                }
                if !in_sector(oa, (b - a).normalize()) || !in_sector(ob, (a - b).normalize()) {
                    continue;
                }
                if !chord_clear(&geoms, &shifts, a, b, eps * 10.0) {
                    continue;
                }
                chosen = Some(c);
                break;
            }
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
            let (oa, ob) = (&geoms[c.wi].occ[c.i], &geoms[c.wj].occ[c.j]);
            let (hx, hy) = (oa.hout, ob.hout);
            let (uv, p3) = match &chart {
                Chart::Face { .. } => (vec![oa.raw, ob.raw + chart.raw_shift(c.shift)], None),
                Chart::Plane { .. } => (vec![], Some(vec![mesh.verts[oa.v].pos, mesh.verts[ob.v].pos])),
            };
            let same = c.wi == c.wj;
            let ch = mesh.insert_chord(hx, hy, uv, p3);
            if !same {
                continue;
            }
            // Same walk split into two: the reverse side becomes a new region.
            let nr = mesh.new_region(r);
            for x in mesh.orbit(ch + 1) {
                mesh.he[x].region = nr;
            }
            queue.push(nr);
        }
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
