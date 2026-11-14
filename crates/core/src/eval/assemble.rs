//! Global assembly across faces (SPEC §4.1, §4.6 steps 3–4): vertices on
//! CAD edges are shared between the faces using the edge (registry keyed
//! by (edge, t)), seam-only vertices are dropped per vertex, short closed
//! loops get placeholders, and region walks become a half-edge mesh whose
//! half-edges all run counter-clockwise seen from outside.

use super::Owner;
use super::arr::{Arr, Glued};
use super::ctx::BodyCtx;
use super::lines::default_phase_dir;
use crate::doc::{Document, PhaseTarget};
use crate::keys::{LoopId, VertexKey};
use crate::mesh::Anchor;
use glam::{DVec2, DVec3};
use newtopo_kernel::{BodyId, EdgeId, FaceId};
use std::collections::{BTreeMap, HashMap};

pub struct FaceRes {
    pub face: FaceId,
    pub arr: Arr,
    pub glued: Glued,
}

#[derive(Clone, Debug)]
pub struct GVert {
    pub key: VertexKey,
    pub keys: Vec<VertexKey>,
    pub anchor: Anchor,
    /// On a pole line: collapses to this vertex at emission.
    pub pole: Option<EdgeId>,
    pub pos: DVec3,
    pub alive: bool,
}

#[derive(Clone, Debug)]
pub struct HE {
    pub from: usize,
    pub to: usize,
    pub twin: Option<usize>,
    pub next: usize,
    pub region: usize,
    pub owner: Owner,
    pub face: FaceId,
    /// Path in the face's UV chart (raw UV, locally continuous).
    pub uv: Vec<DVec2>,
    /// Explicit 3D path (chords across faces); else derived from `uv`.
    pub p3: Option<Vec<DVec3>>,
    pub alive: bool,
}

#[derive(Clone, Debug)]
pub struct Region {
    pub face: FaceId,
    pub alive: bool,
    pub multi: bool,
}

#[derive(Default)]
pub struct HMesh {
    pub verts: Vec<GVert>,
    pub he: Vec<HE>,
    pub regions: Vec<Region>,
    pub key_index: HashMap<VertexKey, usize>,
}

struct Uf(Vec<usize>);
impl Uf {
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

fn interp_pars(pts: &[DVec2], pars: &[f64], p: f64) -> DVec2 {
    let n = pars.len();
    if n == 1 {
        return pts[0];
    }
    let inc = pars[n - 1] >= pars[0];
    for i in 0..n - 1 {
        let (a, b) = (pars[i], pars[i + 1]);
        let inside = if inc { p >= a && p <= b } else { p <= a && p >= b };
        if inside {
            let f = if (b - a).abs() > 0.0 { (p - a) / (b - a) } else { 0.0 };
            return pts[i] + (pts[i + 1] - pts[i]) * f;
        }
    }
    if (p - pars[0]).abs() < (p - pars[n - 1]).abs() { pts[0] } else { pts[n - 1] }
}

fn sub_path(pts: &[DVec2], pars: &[f64], p0: f64, p1: f64) -> Vec<DVec2> {
    // pars increasing; p0 < p1 (caller reverses for backward traversal).
    let mut out = vec![interp_pars(pts, pars, p0)];
    for i in 0..pars.len() {
        if pars[i] > p0 + 1e-14 && pars[i] < p1 - 1e-14 {
            out.push(pts[i]);
        }
    }
    out.push(interp_pars(pts, pars, p1));
    out
}

fn newell(pts: &[DVec3]) -> DVec3 {
    let mut n = DVec3::ZERO;
    for i in 0..pts.len() {
        let a = pts[i];
        let b = pts[(i + 1) % pts.len()];
        n.x += (a.y - b.y) * (a.z + b.z);
        n.y += (a.z - b.z) * (a.x + b.x);
        n.z += (a.x - b.x) * (a.y + b.y);
    }
    n
}

/// Placeholder arc positions on a loop of length `total` (SPEC §4.1).
fn placeholder_positions(total: f64, existing: &[f64], start: f64, n: u32) -> Vec<(u32, f64)> {
    let n = n.max(3);
    let sp = total / n as f64;
    let mut out = Vec::new();
    for k in 0..n {
        let s = (start + sp * k as f64).rem_euclid(total);
        let near = existing.iter().any(|&e| {
            let d = (e - s).rem_euclid(total);
            d.min(total - d) < sp * 0.5 - 1e-12 * total
        });
        if !near {
            out.push((k, s));
        }
    }
    out
}

/// Phase start on a sampled 3D loop (cumulative arc `cum`).
fn phase_start(pts: &[DVec3], cum: &[f64], rf: Option<DVec3>) -> f64 {
    let c = pts.iter().copied().sum::<DVec3>() / pts.len() as f64;
    let nrm = newell(pts).normalize_or_zero();
    let rf = rf.unwrap_or_else(|| default_phase_dir(if nrm.length() > 0.5 { nrm } else { DVec3::Z }));
    let rf = (rf - nrm * nrm.dot(rf)).normalize_or_zero();
    let mut best = (f64::NEG_INFINITY, 0usize);
    for (i, p) in pts.iter().enumerate() {
        let d = (*p - c - nrm * nrm.dot(*p - c)).normalize_or_zero().dot(rf);
        if d > best.0 + 1e-12 {
            best = (d, i);
        }
    }
    // Parabolic refinement between neighbours.
    let i = best.1;
    let n = pts.len();
    if n >= 3 {
        let f = |k: usize| (pts[k] - c - nrm * nrm.dot(pts[k] - c)).normalize_or_zero().dot(rf);
        let (im, ip) = ((i + n - 1) % n, (i + 1) % n);
        let (ym, y0, yp) = (f(im), f(i), f(ip));
        let den = ym - 2.0 * y0 + yp;
        if den.abs() > 1e-300 && i > 0 && i + 1 < n {
            let off = (0.5 * (ym - yp) / den).clamp(-0.5, 0.5);
            let s0 = cum[i];
            let s1 = if off > 0.0 { cum[ip] } else { cum[im] };
            return s0 + (s1 - s0) * off.abs();
        }
    }
    cum[i]
}

/// Assemble the half-edge mesh of a body.
pub fn assemble(ctx: &BodyCtx, doc: &Document, body: BodyId, faces: &[FaceRes]) -> HMesh {
    let tol = 1e-6 * ctx.size;
    // 1. Item ids.
    let mut offset = Vec::with_capacity(faces.len());
    let mut total = 0;
    for fr in faces {
        offset.push(total);
        total += fr.glued.gnodes.len();
    }
    let mut uf = Uf((0..total).collect());
    let mut by_key: HashMap<VertexKey, usize> = HashMap::new();
    // (edge) -> [(t, item)]
    let mut reg: BTreeMap<EdgeId, Vec<(f64, usize)>> = BTreeMap::new();
    let mut pole_of: HashMap<usize, (EdgeId, f64)> = HashMap::new();
    for (fi, fr) in faces.iter().enumerate() {
        for (g, gn) in fr.glued.gnodes.iter().enumerate() {
            let id = offset[fi] + g;
            for k in &gn.keys {
                match by_key.get(k) {
                    Some(&o) => uf.union(o, id),
                    None => {
                        by_key.insert(k.clone(), id);
                    }
                }
            }
            for &(c, p) in &gn.incs {
                match fr.arr.curves[c].owner {
                    Owner::Cad(e) => reg.entry(e).or_default().push((p, id)),
                    Owner::Pole(e) => {
                        pole_of.insert(id, (e, p));
                    }
                    _ => {}
                }
            }
        }
    }
    for (e, list) in reg.iter_mut() {
        list.sort_by(|a, b| a.0.total_cmp(&b.0));
        let ts: Vec<f64> = list.iter().map(|x| x.0).collect();
        let ps = ctx.kernel.edge_eval(ctx.kbody, *e, &ts).unwrap_or_default();
        if ps.len() != ts.len() {
            continue;
        }
        for i in 1..list.len() {
            if (ps[i] - ps[i - 1]).length() <= tol {
                uf.union(list[i].1, list[i - 1].1);
            }
        }
    }
    // 2. Global vertices.
    let mut root_v: HashMap<usize, usize> = HashMap::new();
    let mut item_v = vec![0usize; total];
    let mut members: Vec<Vec<usize>> = Vec::new();
    for i in 0..total {
        let r = uf.find(i);
        let v = *root_v.entry(r).or_insert_with(|| {
            members.push(vec![]);
            members.len() - 1
        });
        item_v[i] = v;
        members[v].push(i);
    }
    let item_face = |i: usize| -> (usize, usize) {
        let fi = offset.partition_point(|&o| o <= i) - 1;
        (fi, i - offset[fi])
    };
    let mut verts: Vec<GVert> = Vec::with_capacity(members.len());
    for mem in &members {
        let mut keys: Vec<VertexKey> = Vec::new();
        for &i in mem {
            let (fi, g) = item_face(i);
            keys.extend(faces[fi].glued.gnodes[g].keys.iter().cloned());
        }
        keys.sort();
        keys.dedup();
        // Prefer non-artificial keys.
        let mut anchor = None;
        for k in &keys {
            if let VertexKey::Cad(v) = k {
                anchor = Some(Anchor::CadVertex { vertex: *v });
                break;
            }
        }
        let mut pole = None;
        if anchor.is_none() {
            let mut best: Option<(EdgeId, f64)> = None;
            for (e, list) in &reg {
                if let Some((t, _)) = list.iter().find(|(_, it)| mem.contains(it)) {
                    if best.is_none_or(|(be, _)| *e < be) {
                        best = Some((*e, *t));
                    }
                }
            }
            if let Some((e, t)) = best {
                anchor = Some(Anchor::CadEdge { edge: e, t });
            }
        }
        if let Some(&(e, t)) = mem.iter().find_map(|i| pole_of.get(i)) {
            pole = Some(e);
            if anchor.is_none() {
                anchor = Some(Anchor::CadEdge { edge: e, t });
            }
        }
        let anchor = anchor.unwrap_or_else(|| {
            let (fi, g) = item_face(mem[0]);
            let fg = ctx.face(faces[fi].face);
            Anchor::Face { face: faces[fi].face, uv: fg.wrap(faces[fi].glued.gnodes[g].uv) }
        });
        verts.push(GVert { key: keys[0].clone(), keys, anchor, pole, pos: DVec3::ZERO, alive: true });
    }
    // 3. Seam-only drops.
    let mut seam_rel = vec![false; verts.len()];
    let mut shape_ok = vec![true; verts.len()];
    for (fi, fr) in faces.iter().enumerate() {
        for (g, gn) in fr.glued.gnodes.iter().enumerate() {
            let v = item_v[offset[fi] + g];
            if gn.seam_rel || gn.artificial {
                seam_rel[v] = true;
            }
            let es = fr.glued.degree_edges(g);
            let ok = match es.len() {
                0 => true,
                2 => {
                    let (a, b) = (fr.glued.gedges[es[0]].owner, fr.glued.gedges[es[1]].owner);
                    a == b && !matches!(a, Owner::Seam(_) | Owner::Fill)
                }
                _ => false,
            };
            if !ok {
                shape_ok[v] = false;
            }
        }
    }
    let dropped: Vec<bool> = (0..verts.len()).map(|v| seam_rel[v] && shape_ok[v]).collect();
    // Edge registry in global ids (dedup), sorted by t.
    let mut ereg: BTreeMap<EdgeId, Vec<(f64, usize)>> = BTreeMap::new();
    for (e, list) in &reg {
        let mut l: Vec<(f64, usize)> = Vec::new();
        for &(t, it) in list {
            let v = item_v[it];
            if !l.iter().any(|x| x.1 == v) {
                l.push((t, v));
            }
        }
        ereg.insert(*e, l);
    }
    // 4a. Placeholders on CAD loops.
    let nmin = doc.settings.min_closed_segments;
    for lp in &ctx.cad_loops {
        let mut existing: Vec<usize> = Vec::new();
        for e in lp {
            if let Some(l) = ereg.get(e) {
                for &(_, v) in l {
                    if !dropped[v] && !existing.contains(&v) {
                        existing.push(v);
                    }
                }
            }
        }
        if existing.len() >= 3 {
            continue;
        }
        // Order the loop's edges.
        let mut order: Vec<(EdgeId, bool)> = vec![(lp[0], false)];
        let mut cur_end = ctx.topo.edge(lp[0]).v1;
        while order.len() < lp.len() {
            let Some(&e) = lp.iter().find(|e| !order.iter().any(|(o, _)| o == *e) && {
                let et = ctx.topo.edge(**e);
                et.v0 == cur_end || et.v1 == cur_end
            }) else {
                break;
            };
            let et = ctx.topo.edge(e);
            let rev = et.v0 != cur_end;
            cur_end = if rev { et.v0 } else { et.v1 };
            order.push((e, rev));
        }
        let mut pts: Vec<DVec3> = Vec::new();
        let mut cum: Vec<f64> = Vec::new();
        let mut map: Vec<(EdgeId, f64)> = Vec::new();
        let mut acc = 0.0;
        for &(e, rev) in &order {
            let mut s = ctx.edge_polys[e.idx()].clone();
            if rev {
                s.reverse();
            }
            for (k, (t, p)) in s.iter().enumerate() {
                if k == s.len() - 1 {
                    continue;
                }
                if let Some(&last) = pts.last() {
                    acc += (*p - last).length();
                }
                pts.push(*p);
                cum.push(acc);
                map.push((e, *t));
            }
            let _ = s;
        }
        if pts.len() < 3 {
            continue;
        }
        let total_len = acc + (pts[0] - *pts.last().unwrap()).length();
        let arc_of = |e: EdgeId, t: f64| -> f64 {
            // Nearest sample by (edge, t).
            let mut best = (f64::INFINITY, 0.0);
            for (i, &(me, mt)) in map.iter().enumerate() {
                if me == e && (mt - t).abs() < best.0 {
                    best = ((mt - t).abs(), cum[i]);
                }
            }
            best.1
        };
        let mut ex_arcs: Vec<(VertexKey, f64)> = Vec::new();
        for &v in &existing {
            for e in lp {
                if let Some(l) = ereg.get(e)
                    && let Some(&(t, _)) = l.iter().find(|x| x.1 == v)
                {
                    ex_arcs.push((verts[v].key.clone(), arc_of(*e, t)));
                    break;
                }
            }
        }
        ex_arcs.sort_by(|a, b| a.0.cmp(&b.0));
        let start = match ex_arcs.first() {
            Some((_, s)) => *s,
            None => {
                let rf = if lp.len() == 1 { doc.phase_overrides.get(&PhaseTarget::Edge(body, lp[0])).copied() } else { None };
                phase_start(&pts, &cum, rf)
            }
        };
        let arcs: Vec<f64> = ex_arcs.iter().map(|x| x.1).collect();
        let lid = LoopId::Cad(lp.clone());
        for (k, s) in placeholder_positions(total_len, &arcs, start, nmin) {
            // Arc → (edge, t).
            let mut i = 0;
            while i + 1 < cum.len() && cum[i + 1] <= s {
                i += 1;
            }
            let (e, t) = if i + 1 < cum.len() && map[i + 1].0 == map[i].0 {
                let f = (s - cum[i]) / (cum[i + 1] - cum[i]).max(1e-300);
                (map[i].0, map[i].1 + (map[i + 1].1 - map[i].1) * f)
            } else {
                let et = ctx.topo.edge(map[i].0);
                let end_t = if order.iter().find(|o| o.0 == map[i].0).is_some_and(|o| o.1) { et.t_range.0 } else { et.t_range.1 };
                let next_cum = if i + 1 < cum.len() { cum[i + 1] } else { total_len };
                let f = (s - cum[i]) / (next_cum - cum[i]).max(1e-300);
                (map[i].0, map[i].1 + (end_t - map[i].1) * f)
            };
            let t = ctx.kernel.project_to_edge(ctx.kbody, e, &[ctx.edge_point(e, t)]).ok().and_then(|v| v[0]).unwrap_or(t);
            let key = VertexKey::Placeholder { lp: lid.clone(), i: k };
            let v = verts.len();
            verts.push(GVert { key: key.clone(), keys: vec![key], anchor: Anchor::CadEdge { edge: e, t }, pole: None, pos: DVec3::ZERO, alive: true });
            let l = ereg.entry(e).or_default();
            l.push((t, v));
            l.sort_by(|a, b| a.0.total_cmp(&b.0));
        }
    }
    let mut dropped = dropped;
    dropped.resize(verts.len(), false);
    // 4b. Face-local loops (split lines, generated lines, kept seams, poles).
    // (face index, owner) -> [(par, vertex)]
    let mut lreg: HashMap<(usize, Owner), Vec<(f64, usize)>> = HashMap::new();
    for (fi, fr) in faces.iter().enumerate() {
        let gl = &fr.glued;
        for (g, gn) in gl.gnodes.iter().enumerate() {
            let v = item_v[offset[fi] + g];
            for &(c, p) in &gn.incs {
                let o = fr.arr.curves[c].owner;
                if matches!(o, Owner::Line(_) | Owner::Split(_) | Owner::Seam(_) | Owner::Pole(_)) {
                    let l = lreg.entry((fi, o)).or_default();
                    if !l.iter().any(|x| x.1 == v && (x.0 - p).abs() < 1e-12) {
                        l.push((p, v));
                    }
                }
            }
        }
        // Group non-dissolved gedges by owner.
        let mut by_owner: BTreeMap<Owner, Vec<usize>> = BTreeMap::new();
        for (e, ge) in gl.gedges.iter().enumerate() {
            if gl.dissolved[e] {
                continue;
            }
            if matches!(ge.owner, Owner::Line(_) | Owner::Split(_) | Owner::Seam(_) | Owner::Pole(_)) {
                by_owner.entry(ge.owner).or_default().push(e);
            }
        }
        let fg = ctx.face(fr.face);
        for (owner, es) in by_owner {
            // Cycle check: every node has exactly two incidences.
            let mut cnt: HashMap<usize, u32> = HashMap::new();
            for &e in &es {
                *cnt.entry(gl.gedges[e].a).or_default() += 1;
                *cnt.entry(gl.gedges[e].b).or_default() += 1;
            }
            if cnt.values().any(|&c| c != 2) {
                continue;
            }
            // Order edges into a cycle.
            let mut used = vec![false; es.len()];
            let mut seq: Vec<(usize, bool)> = vec![(es[0], true)];
            used[0] = true;
            let mut cur = gl.gedges[es[0]].b;
            let start_node = gl.gedges[es[0]].a;
            let mut ok = true;
            while cur != start_node || seq.len() < es.len() {
                let Some(k) = (0..es.len()).find(|&k| !used[k] && (gl.gedges[es[k]].a == cur || gl.gedges[es[k]].b == cur)) else {
                    ok = cur == start_node;
                    break;
                };
                used[k] = true;
                let ge = &gl.gedges[es[k]];
                let fwd = ge.a == cur;
                cur = if fwd { ge.b } else { ge.a };
                seq.push((es[k], fwd));
            }
            if !ok || used.iter().any(|u| !u) {
                continue;
            }
            let nodes: Vec<usize> = seq.iter().map(|&(e, f)| if f { gl.gedges[e].a } else { gl.gedges[e].b }).collect();
            let existing: Vec<usize> = nodes.iter().map(|&g| item_v[offset[fi] + g]).filter(|v| !dropped[*v]).collect();
            let mut ex_unique = existing.clone();
            ex_unique.sort();
            ex_unique.dedup();
            if ex_unique.len() >= 3 {
                continue;
            }
            // Sample the loop as one continuous (lifted) polyline that ends
            // where it started (possibly one period further).
            let mut poly: Vec<DVec2> = Vec::new();
            let mut seg_tag: Vec<usize> = Vec::new();
            let mut node_at: Vec<(usize, usize)> = Vec::new();
            for &(e, fwd) in &seq {
                let ge = &gl.gedges[e];
                let mut pts: Vec<DVec2> = ge.pts.clone();
                if !fwd {
                    pts.reverse();
                }
                let shift = match poly.last() {
                    Some(&last) => fg.lift_near(pts[0], last) - pts[0],
                    None => DVec2::ZERO,
                };
                node_at.push((poly.len().saturating_sub(1), if fwd { ge.a } else { ge.b }));
                let skip = if poly.is_empty() { 0 } else { 1 };
                for p in pts.iter().skip(skip) {
                    poly.push(*p + shift);
                    if poly.len() > 1 {
                        seg_tag.push(e);
                    }
                }
            }
            if poly.len() < 2 {
                continue;
            }
            // Densify for 3D length.
            let k = 8;
            let mut d_uv: Vec<DVec2> = Vec::new();
            let mut d_tag: Vec<usize> = Vec::new();
            for i in 0..poly.len() - 1 {
                let (a, b) = (poly[i], poly[i + 1]);
                for j in 0..k {
                    d_uv.push(a + (b - a) * (j as f64 / k as f64));
                    d_tag.push(seg_tag[i]);
                }
            }
            d_uv.push(*poly.last().unwrap());
            d_tag.push(*seg_tag.last().unwrap());
            let d_src = |poly_idx: usize| poly_idx * k;
            let ev = ctx.face_eval(fr.face, &d_uv);
            if ev.len() != d_uv.len() {
                continue;
            }
            let is_pole = matches!(owner, Owner::Pole(_));
            let pts3: Vec<DVec3> = ev.iter().map(|e| e.p).collect();
            let mut cum = vec![0.0];
            for i in 1..d_uv.len() {
                let d = if is_pole { (fg.m(d_uv[i]) - fg.m(d_uv[i - 1])).length() } else { (pts3[i] - pts3[i - 1]).length() };
                cum.push(cum[i - 1] + d);
            }
            let total_len = cum[cum.len() - 1];
            if total_len <= 0.0 {
                continue;
            }
            let mut ex: Vec<(VertexKey, f64)> = Vec::new();
            for &(idx, g) in &node_at {
                let v = item_v[offset[fi] + g];
                if !dropped[v] {
                    ex.push((verts[v].key.clone(), cum[d_src(idx).min(cum.len() - 1)]));
                }
            }
            ex.sort_by(|a, b| a.0.cmp(&b.0));
            let start = match ex.first() {
                Some((_, s)) => *s,
                None if is_pole => 0.0,
                None => {
                    let rf = doc.phase_overrides.get(&PhaseTarget::Face(body, fr.face)).copied();
                    let rf = rf.or_else(|| fg.info.axis_dir.map(default_phase_dir));
                    phase_start(&pts3[..pts3.len() - 1], &cum[..cum.len() - 1], rf)
                }
            };
            let arcs: Vec<f64> = ex.iter().map(|x| x.1).collect();
            let lid = match owner {
                Owner::Line(k) => LoopId::Line(k),
                Owner::Split(s) => LoopId::Split(s),
                Owner::Seam(d) => LoopId::Seam(fr.face, d),
                Owner::Pole(e) => LoopId::Cad(vec![e]),
                _ => continue,
            };
            for (k, s) in placeholder_positions(total_len, &arcs, start, nmin) {
                let mut i = 0;
                while i + 1 < cum.len() && cum[i + 1] <= s {
                    i += 1;
                }
                let i = i.min(d_uv.len() - 2);
                let (uv0, uv1) = (d_uv[i], d_uv[i + 1]);
                let next_c = cum[i + 1];
                let f = ((s - cum[i]) / (next_c - cum[i]).max(1e-300)).clamp(0.0, 1.0);
                let uv = fg.wrap(uv0 + (uv1 - uv0) * f);
                // Parameter on the owner curve: project onto the gedge.
                let ge_i = d_tag[i];
                let ge = &gl.gedges[ge_i];
                let mut best = (f64::INFINITY, ge.pa);
                for j in 0..ge.pts.len().saturating_sub(1) {
                    let (a, b) = (ge.pts[j], ge.pts[j + 1]);
                    let q = fg.lift_near(uv, a);
                    let (d, t) = super::geom2::dist_point_seg(q, a, b);
                    if d < best.0 {
                        best = (d, ge.pars[j] + (ge.pars[j + 1] - ge.pars[j]) * t);
                    }
                }
                let key = VertexKey::Placeholder { lp: lid.clone(), i: k };
                let v = verts.len();
                let (anchor, pole) = match owner {
                    Owner::Pole(e) => (Anchor::CadEdge { edge: e, t: best.1 }, Some(e)),
                    _ => (Anchor::Face { face: fr.face, uv }, None),
                };
                verts.push(GVert { key: key.clone(), keys: vec![key], anchor, pole, pos: DVec3::ZERO, alive: true });
                dropped.push(false);
                lreg.entry((fi, owner)).or_default().push((best.1, v));
            }
        }
    }
    for l in lreg.values_mut() {
        l.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
    // 5. Walks → half-edges.
    let mut mesh = HMesh { verts, he: vec![], regions: vec![], key_index: HashMap::new() };
    for (fi, fr) in faces.iter().enumerate() {
        let gl = &fr.glued;
        let fg = ctx.face(fr.face);
        for walks in &gl.regions {
            let rid = mesh.regions.len();
            mesh.regions.push(Region { face: fr.face, alive: true, multi: false });
            for w in walks {
                // Items: (vertex, uv) and segments between them.
                let mut segs: Vec<(usize, usize, Owner, Vec<DVec2>)> = Vec::new();
                for &gh in w {
                    let ge = &gl.gedges[gh / 2];
                    let fwd = gh % 2 == 0;
                    let va = item_v[offset[fi] + ge.a];
                    let vb = item_v[offset[fi] + ge.b];
                    let (plo, phi) = (ge.pars[0], ge.pars[ge.pars.len() - 1]);
                    // Registry entries strictly inside.
                    let mut inner: Vec<(f64, usize)> = Vec::new();
                    let span = (phi - plo).abs();
                    let margin = 1e-9 * span.max(1e-12);
                    match ge.owner {
                        Owner::Cad(e) => {
                            if let Some(l) = ereg.get(&e) {
                                for &(t, v) in l {
                                    if t > plo + margin && t < phi - margin && v != va && v != vb {
                                        inner.push((t, v));
                                    }
                                }
                            }
                        }
                        o => {
                            if let Some(l) = lreg.get(&(fi, o)) {
                                let curve = &fr.arr.curves[ge.curve];
                                let total = curve.par[curve.par.len() - 1] - curve.par[0];
                                let shifts: &[f64] = if curve.closed { &[0.0, 1.0] } else { &[0.0] };
                                for &(t, v) in l {
                                    for tt in shifts.iter().map(|k| t + k * total) {
                                        if tt > plo + margin && tt < phi - margin && v != va && v != vb && !inner.iter().any(|x| x.1 == v) {
                                            inner.push((tt, v));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    inner.sort_by(|a, b| a.0.total_cmp(&b.0));
                    // A dropped end vertex is replaced by a registry vertex
                    // sitting at the same parameter (e.g. a placeholder at
                    // the phase reference on the seam).
                    let period = match ge.owner {
                        Owner::Cad(e) => {
                            let et = ctx.topo.edge(e);
                            if et.closed { Some(et.t_range.1 - et.t_range.0) } else { None }
                        }
                        _ => {
                            let curve = &fr.arr.curves[ge.curve];
                            curve.closed.then(|| curve.par[curve.par.len() - 1] - curve.par[0])
                        }
                    };
                    let reg_list: Option<&Vec<(f64, usize)>> = match ge.owner {
                        Owner::Cad(e) => ereg.get(&e),
                        o => lreg.get(&(fi, o)),
                    };
                    let sub = |v: usize, at: f64| -> usize {
                        if !dropped[v] {
                            return v;
                        }
                        let Some(l) = reg_list else { return v };
                        for &(t, w) in l {
                            if dropped[w] {
                                continue;
                            }
                            let mut d = (t - at).abs();
                            if let Some(p) = period
                                && p > 0.0
                            {
                                d = d.min((t - at + p).abs()).min((t - at - p).abs());
                            }
                            if d <= margin.max(1e-9 * span.max(1e-12)) {
                                return w;
                            }
                        }
                        v
                    };
                    let (va, vb) = (sub(va, plo), sub(vb, phi));
                    inner.retain(|x| x.1 != va && x.1 != vb);
                    let mut stops: Vec<(f64, usize)> = vec![(plo, va)];
                    stops.extend(inner);
                    stops.push((phi, vb));
                    let mut pieces: Vec<(usize, usize, Vec<DVec2>)> = Vec::new();
                    for k in 0..stops.len() - 1 {
                        let (p0, v0) = stops[k];
                        let (p1, v1) = stops[k + 1];
                        let path = sub_path(&ge.pts, &ge.pars, p0, p1);
                        pieces.push((v0, v1, path));
                    }
                    if !fwd {
                        pieces.reverse();
                        for p in &mut pieces {
                            std::mem::swap(&mut p.0, &mut p.1);
                            p.2.reverse();
                        }
                    }
                    for (a, b, path) in pieces {
                        segs.push((a, b, ge.owner, path));
                    }
                }
                // Merge across dropped vertices.
                if segs.is_empty() {
                    continue;
                }
                // Rotate so the walk starts at a kept vertex.
                if let Some(k) = segs.iter().position(|s| !dropped[s.0]) {
                    segs.rotate_left(k);
                } else {
                    continue;
                }
                let mut merged: Vec<(usize, usize, Owner, Vec<DVec2>)> = Vec::new();
                for (a, b, o, path) in segs {
                    if let Some(last) = merged.last_mut()
                        && dropped[a]
                    {
                        let end = *last.3.last().unwrap();
                        let shifted: Vec<DVec2> = {
                            let s = fg.lift_near(path[0], end) - path[0];
                            path.iter().map(|p| *p + s).collect()
                        };
                        last.3.extend(shifted.into_iter().skip(1));
                        last.1 = b;
                        continue;
                    }
                    merged.push((a, b, o, path));
                }
                // Drop zero-length self segments.
                merged.retain(|s| !(s.0 == s.1 && super::geom2::poly_len(&s.3.iter().map(|p| fg.m(*p)).collect::<Vec<_>>()) <= fg.eps));
                if merged.is_empty() {
                    continue;
                }
                // Orient CCW seen from outside.
                if fg.reversed {
                    merged.reverse();
                    for s in &mut merged {
                        std::mem::swap(&mut s.0, &mut s.1);
                        s.3.reverse();
                    }
                }
                let base = mesh.he.len();
                let n = merged.len();
                for (k, (a, b, o, path)) in merged.into_iter().enumerate() {
                    mesh.he.push(HE {
                        from: a,
                        to: b,
                        twin: None,
                        next: base + (k + 1) % n,
                        region: rid,
                        owner: o,
                        face: fr.face,
                        uv: path,
                        p3: None,
                        alive: true,
                    });
                }
            }
        }
    }
    // Remove dropped vertices from the table (mark dead).
    for (v, d) in dropped.iter().enumerate() {
        if *d {
            mesh.verts[v].alive = false;
        }
    }
    // 6. Twins.
    let mut map: HashMap<(usize, usize, Owner), Vec<usize>> = HashMap::new();
    for (h, he) in mesh.he.iter().enumerate() {
        map.entry((he.from, he.to, he.owner)).or_default().push(h);
    }
    for h in 0..mesh.he.len() {
        if mesh.he[h].twin.is_some() {
            continue;
        }
        let he = &mesh.he[h];
        let Some(c) = map.get(&(he.to, he.from, he.owner)) else { continue };
        let cands: Vec<usize> = c.iter().copied().filter(|&x| mesh.he[x].twin.is_none() && x != h).collect();
        if cands.is_empty() {
            continue;
        }
        let pick = if cands.len() == 1 {
            cands[0]
        } else {
            let mid = |x: usize| {
                let e = &mesh.he[x];
                ctx.point(e.face, e.uv[e.uv.len() / 2])
            };
            let m = mid(h);
            *cands.iter().min_by(|&&a, &&b| (mid(a) - m).length().total_cmp(&(mid(b) - m).length())).unwrap()
        };
        mesh.he[h].twin = Some(pick);
        mesh.he[pick].twin = Some(h);
    }
    // 7. Positions and key index.
    compute_positions(ctx, &mut mesh.verts);
    mesh.rebuild_key_index();
    mesh
}

pub fn compute_positions(ctx: &BodyCtx, verts: &mut [GVert]) {
    let mut by_face: HashMap<FaceId, Vec<usize>> = HashMap::new();
    let mut by_edge: HashMap<EdgeId, Vec<usize>> = HashMap::new();
    for (i, v) in verts.iter().enumerate() {
        match v.anchor {
            Anchor::CadVertex { .. } => {}
            Anchor::CadEdge { edge, .. } => by_edge.entry(edge).or_default().push(i),
            Anchor::Face { face, .. } => by_face.entry(face).or_default().push(i),
        }
    }
    for v in verts.iter_mut() {
        if let Anchor::CadVertex { vertex } = v.anchor {
            v.pos = ctx.topo.vertex(vertex).point;
        }
    }
    for (e, list) in by_edge {
        let ts: Vec<f64> = list.iter().map(|&i| if let Anchor::CadEdge { t, .. } = verts[i].anchor { t } else { 0.0 }).collect();
        if let Ok(ps) = ctx.kernel.edge_eval(ctx.kbody, e, &ts) {
            for (k, &i) in list.iter().enumerate() {
                verts[i].pos = ps[k];
            }
        }
    }
    for (f, list) in by_face {
        let uvs: Vec<DVec2> = list.iter().map(|&i| if let Anchor::Face { uv, .. } = verts[i].anchor { uv } else { DVec2::ZERO }).collect();
        let ev = ctx.face_eval(f, &uvs);
        for (k, &i) in list.iter().enumerate() {
            if let Some(e) = ev.get(k) {
                verts[i].pos = e.p;
            }
        }
    }
}

impl HMesh {
    pub fn rebuild_key_index(&mut self) {
        self.key_index.clear();
        for (i, v) in self.verts.iter().enumerate() {
            if !v.alive {
                continue;
            }
            for k in &v.keys {
                self.key_index.entry(k.clone()).or_insert(i);
            }
        }
    }
}
