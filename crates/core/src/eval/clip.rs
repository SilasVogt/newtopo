//! Clipping generated lines and split lines to a face (SPEC §4.2, §4.6
//! step 2): a polyline given in the periodic cover of the face's UV domain
//! is cut at seams, intersected with the stoppers (CAD boundary curves,
//! pole lines and split lines), its pieces are classified into patches, and
//! kept runs become components with deterministic event keys.

use super::arr::{ACurve, AEvent};
use super::ctx::FaceGeom;
use super::geom2::*;
use super::Owner;
use crate::keys::{EdgeOrSplit, LineKey, SplitId, VertexKey};
use glam::DVec2;

/// What is being clipped: a generated line or a split line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Who {
    Line(LineKey),
    Split(SplitId),
}

impl Who {
    pub fn owner(&self) -> Owner {
        match *self {
            Who::Line(k) => Owner::Line(k),
            Who::Split(s) => Owner::Split(s),
        }
    }
    fn with_sub(&self, comp: u32) -> Who {
        match *self {
            Who::Line(k) => Who::Line(LineKey { sub: k.sub * 64 + comp.min(63), ..k }),
            s => s,
        }
    }
    fn key(&self, on: EdgeOrSplit, end: u32) -> VertexKey {
        match *self {
            Who::Line(line) => VertexKey::OnBoundary { line, on, end },
            Who::Split(split) => VertexKey::SplitEnd { split, on, end },
        }
    }
    fn self_ref(&self) -> EdgeOrSplit {
        match *self {
            Who::Line(k) => EdgeOrSplit::Line(k),
            Who::Split(s) => EdgeOrSplit::Split(s),
        }
    }
}

pub struct ClipInput<'a> {
    pub who: Who,
    pub pts: &'a [DVec2],
    pub closed: bool,
    /// Patch filter: `None` keeps every piece inside the face.
    pub allowed: Option<&'a [usize]>,
    pub through: bool,
    pub seed: Option<DVec2>,
    /// Keys for the free start/end of an open line (cut points).
    pub end_keys: [Option<VertexKey>; 2],
}

/// Result: curves (line pieces in the domain) and events with incidences
/// indexed into `curves` (local) or the stopper list (given offsets).
pub struct ClipOut {
    pub curves: Vec<ACurve>,
    /// Events; incidences `(curve, par)` where curve < `curves.len()` refers
    /// to `curves` and `STOP + i` to stopper `i`.
    pub events: Vec<AEvent>,
}

pub const STOP: usize = 1 << 30;

#[derive(Clone, Debug)]
struct Piece {
    pts: Vec<DVec2>,
    s: Vec<f64>,
    seam: Option<(u8, u8)>,
}

#[derive(Clone, Debug)]
struct Inc {
    uv: DVec2,
    /// Stopper (index, param) or seam crossing (dir) or nothing (free end).
    stop: Option<(usize, f64)>,
    seam: Option<u8>,
}

#[derive(Clone, Debug)]
struct LEv {
    s: f64,
    incs: Vec<Inc>,
}

fn on_ref(fg: &FaceGeom, o: Owner) -> EdgeOrSplit {
    match o {
        Owner::Cad(e) | Owner::Pole(e) => EdgeOrSplit::Cad(e),
        Owner::Split(s) => EdgeOrSplit::Split(s),
        Owner::Seam(d) => EdgeOrSplit::Seam(fg.id, d),
        Owner::Line(k) => EdgeOrSplit::Line(k),
        Owner::Fill => EdgeOrSplit::Seam(fg.id, 9),
    }
}

/// Parameter of the nearest point on a curve.
pub fn project_par(c: &ACurve, uv: DVec2) -> f64 {
    let mut best = (f64::INFINITY, 0.0);
    for i in 0..c.pts.len().saturating_sub(1) {
        let (d, t) = dist_point_seg(uv, c.pts[i], c.pts[i + 1]);
        if d < best.0 {
            best = (d, c.par[i] + (c.par[i + 1] - c.par[i]) * t);
        }
    }
    if c.pts.len() == 1 {
        best.1 = c.par[0];
    }
    best.1
}

/// Rotate a closed cover polyline to start at parameter `s0`.
fn rotate_closed(pts: &[DVec2], s0: f64) -> Vec<DVec2> {
    let n = pts.len() - 1;
    let shift = pts[n] - pts[0];
    let mut out = poly_slice(pts, s0, n as f64);
    let tail = poly_slice(pts, 0.0, s0);
    out.extend(tail.into_iter().skip(1).map(|p| p + shift));
    out
}

/// Split a cover polyline into domain pieces at seam lines.
fn domain_pieces(fg: &FaceGeom, pts: &[DVec2]) -> (Vec<Piece>, Vec<(f64, u8)>) {
    let per = fg.periods();
    let lo = fg.lo;
    let tiny = 1e-12 * fg.size_uv();
    // Cut parameters.
    let mut cuts: Vec<(f64, u8)> = Vec::new();
    for i in 0..pts.len() - 1 {
        let (a, b) = (pts[i], pts[i + 1]);
        for dir in 0..2u8 {
            let p = if dir == 0 { per.x } else { per.y };
            if p <= 0.0 {
                continue;
            }
            let (ca, cb) = if dir == 0 { (a.x, b.x) } else { (a.y, b.y) };
            let o = if dir == 0 { lo.x } else { lo.y };
            let (k0, k1) = (((ca - o) / p), ((cb - o) / p));
            let (kmin, kmax) = (k0.min(k1), k0.max(k1));
            let mut k = kmin.ceil();
            while k <= kmax + 1e-12 {
                let c = o + k * p;
                if (cb - ca).abs() > tiny {
                    let f = ((c - ca) / (cb - ca)).clamp(0.0, 1.0);
                    cuts.push((i as f64 + f, dir));
                } else if (ca - c).abs() <= tiny {
                    // Segment lies on the seam; no crossing.
                }
                k += 1.0;
            }
        }
    }
    cuts.sort_by(|a, b| a.0.total_cmp(&b.0));
    cuts.dedup_by(|b, a| (b.0 - a.0).abs() < 1e-12 && a.1 == b.1);
    let n = (pts.len() - 1) as f64;
    let mut bounds: Vec<f64> = vec![0.0];
    for &(s, _) in &cuts {
        if s > 1e-12 && s < n - 1e-12 && s - bounds.last().unwrap() > 1e-12 {
            bounds.push(s);
        }
    }
    bounds.push(n);
    let mut pieces = Vec::new();
    for w in bounds.windows(2) {
        let (s0, s1) = (w[0], w[1]);
        if s1 - s0 <= 1e-12 {
            continue;
        }
        let poly = poly_slice(pts, s0, s1);
        let mut ss = vec![s0];
        let mut i = s0.floor() as usize + 1;
        while (i as f64) < s1 {
            if (i as f64) > s0 {
                ss.push(i as f64);
            }
            i += 1;
        }
        ss.push(s1);
        // Shift into the domain using the midpoint.
        let mid = poly_at(pts, 0.5 * (s0 + s1));
        let w = fg.wrap(mid);
        let shift = w - mid;
        let mut pp: Vec<DVec2> = poly.iter().map(|p| *p + shift).collect();
        // Detect a piece lying on a seam.
        let mut seam = None;
        for dir in 0..2u8 {
            let p = if dir == 0 { per.x } else { per.y };
            if p <= 0.0 {
                continue;
            }
            let o = if dir == 0 { lo.x } else { lo.y };
            let c0 = if dir == 0 { pp[0].x } else { pp[0].y };
            let snap = fg.eps / if dir == 0 { fg.metric.x } else { fg.metric.y };
            let on = ((c0 - o) / p - ((c0 - o) / p).round()).abs() * p <= snap
                && pp.iter().all(|q| {
                    let c = if dir == 0 { q.x } else { q.y };
                    (c - c0).abs() <= snap
                });
            if on {
                for q in &mut pp {
                    if dir == 0 {
                        q.x = o;
                    } else {
                        q.y = o;
                    }
                }
                seam = Some((dir, 0u8));
            }
        }
        // Snap ends at cut points exactly onto the seam lines.
        for idx in [0, pp.len() - 1] {
            for dir in 0..2u8 {
                let p = if dir == 0 { per.x } else { per.y };
                if p <= 0.0 {
                    continue;
                }
                let o = if dir == 0 { lo.x } else { lo.y };
                let c = if dir == 0 { pp[idx].x } else { pp[idx].y };
                let snap = fg.eps / if dir == 0 { fg.metric.x } else { fg.metric.y };
                for target in [o, o + p] {
                    if (c - target).abs() <= snap {
                        if dir == 0 {
                            pp[idx].x = target;
                        } else {
                            pp[idx].y = target;
                        }
                    }
                }
            }
        }
        match seam {
            Some((dir, _)) => {
                let p = if dir == 0 { DVec2::new(per.x, 0.0) } else { DVec2::new(0.0, per.y) };
                pieces.push(Piece { pts: pp.clone(), s: ss.clone(), seam: Some((dir, 0)) });
                pieces.push(Piece { pts: pp.iter().map(|q| *q + p).collect(), s: ss, seam: Some((dir, 1)) });
            }
            None => pieces.push(Piece { pts: pp, s: ss, seam: None }),
        }
    }
    (pieces, cuts)
}

/// Clip one line. `stoppers`: curves that end non-through lines (with
/// their owners); `locate`: domain point → patch index (None = outside).
pub fn clip(
    fg: &FaceGeom,
    input: &ClipInput,
    stoppers: &[ACurve],
    locate: &mut dyn FnMut(DVec2) -> Option<usize>,
) -> ClipOut {
    let mut out = ClipOut { curves: vec![], events: vec![] };
    if input.pts.len() < 2 {
        return out;
    }
    let mut pts: Vec<DVec2> = input.pts.to_vec();
    let closed = input.closed;
    // For closed lines, start at a seam crossing if there is one.
    if closed {
        let (_, cuts) = domain_pieces(fg, &pts);
        if let Some(&(s, _)) = cuts.first() {
            if s > 1e-12 && s < (pts.len() - 1) as f64 - 1e-12 {
                pts = rotate_closed(&pts, s);
            }
        }
    }
    let n = (pts.len() - 1) as f64;
    let (pieces, cuts) = domain_pieces(fg, &pts);
    let eps_uv = fg.eps / fg.metric.max_element();
    // Logical events by s.
    let mut evs: Vec<LEv> = Vec::new();
    let mut add = |s: f64, inc: Inc| {
        // On a closed line the end is the start.
        let s = if closed && s >= n - 1e-9 { 0.0 } else { s };
        if let Some(e) = evs.iter_mut().find(|e| (e.s - s).abs() < 1e-9) {
            if !e.incs.iter().any(|i| (i.uv - inc.uv).length() <= 1e-12 * (1.0 + i.uv.length()) && i.stop.map(|x| x.0) == inc.stop.map(|x| x.0)) {
                e.incs.push(inc);
            }
        } else {
            evs.push(LEv { s, incs: vec![inc] });
        }
    };
    // Seam crossings (including closed-line start/end on a seam).
    let per = fg.periods();
    for &(s, dir) in &cuts {
        let p = poly_at(&pts, s);
        let w = fg.wrap(p);
        let (a, b) = if dir == 0 { (DVec2::new(fg.lo.x, w.y), DVec2::new(fg.lo.x + per.x, w.y)) } else { (DVec2::new(w.x, fg.lo.y), DVec2::new(w.x, fg.lo.y + per.y)) };
        add(s, Inc { uv: a, stop: None, seam: Some(dir) });
        add(s, Inc { uv: b, stop: None, seam: Some(dir) });
    }
    // Stopper hits per piece.
    for pc in &pieces {
        let mut lo = DVec2::splat(f64::INFINITY);
        let mut hi = DVec2::splat(f64::NEG_INFINITY);
        for p in &pc.pts {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        for (si, st) in stoppers.iter().enumerate() {
            if matches!(st.owner, Owner::Seam(_)) {
                continue;
            }
            for j in 0..st.pts.len() - 1 {
                let (c, d) = (st.pts[j], st.pts[j + 1]);
                if c.x.max(d.x) < lo.x - eps_uv || c.x.min(d.x) > hi.x + eps_uv || c.y.max(d.y) < lo.y - eps_uv || c.y.min(d.y) > hi.y + eps_uv {
                    continue;
                }
                for i in 0..pc.pts.len() - 1 {
                    for (fa, fb) in seg_hits(pc.pts[i], pc.pts[i + 1], c, d, eps_uv) {
                        let s = pc.s[i] + (pc.s[i + 1] - pc.s[i]) * fa;
                        let tp = st.par[j] + (st.par[j + 1] - st.par[j]) * fb;
                        let uv = c + (d - c) * fb;
                        add(s, Inc { uv, stop: Some((si, tp)), seam: None });
                    }
                }
            }
        }
    }
    evs.sort_by(|a, b| a.s.total_cmp(&b.s));
    // Interval boundaries.
    let mut bnds: Vec<f64> = vec![0.0];
    for e in &evs {
        if e.s > 1e-9 && e.s < n - 1e-9 {
            bnds.push(e.s);
        }
    }
    bnds.push(n);
    bnds.dedup_by(|b, a| (*b - *a).abs() < 1e-9);
    // Classify intervals.
    let ni = bnds.len() - 1;
    let mut keep = vec![false; ni];
    for k in 0..ni {
        let sm = 0.5 * (bnds[k] + bnds[k + 1]);
        let Some(pc) = pieces.iter().find(|p| p.s[0] <= sm + 1e-12 && sm <= p.s[p.s.len() - 1] + 1e-12) else { continue };
        let mut q = poly_at(&pc.pts, s_of_par(&pc.s, sm));
        if let Some((dir, _)) = pc.seam {
            // Nudge off the seam into the domain.
            let off = 1e-6 * fg.size_uv();
            let mut found = None;
            for sign in [1.0, -1.0] {
                let mut r = q;
                if dir == 0 {
                    r.x = fg.lo.x + if sign > 0.0 { off } else { per.x - off };
                } else {
                    r.y = fg.lo.y + if sign > 0.0 { off } else { per.y - off };
                }
                if let Some(pidx) = locate(r)
                    && input.allowed.is_none_or(|a| a.contains(&pidx))
                {
                    found = Some(pidx);
                }
            }
            keep[k] = found.is_some();
            continue;
        }
        q = fg.wrap(q);
        if let Some(pidx) = locate(q) {
            keep[k] = input.allowed.is_none_or(|a| a.contains(&pidx));
        }
    }
    // Is an event at interval boundary k (between interval k-1 and k) a stop?
    let ev_at = |s: f64| evs.iter().find(|e| (e.s - s).abs() < 1e-9);
    let is_stop = |s: f64| ev_at(s).is_some_and(|e| e.incs.iter().any(|i| i.stop.is_some()));
    // Components: runs of kept intervals.
    let mut comps: Vec<(usize, usize)> = Vec::new(); // inclusive interval ranges
    let mut k = 0;
    while k < ni {
        if !keep[k] {
            k += 1;
            continue;
        }
        let start = k;
        while k + 1 < ni && keep[k + 1] && !is_stop(bnds[k + 1]) {
            k += 1;
        }
        comps.push((start, k));
        k += 1;
    }
    // Closed line: merge the last and first runs across the wrap point.
    let mut wrap_merge = false;
    if closed && comps.len() >= 2 {
        let (f0, _) = comps[0];
        let (_, l1) = comps[comps.len() - 1];
        if f0 == 0 && l1 == ni - 1 && !is_stop(0.0) && !is_stop(n) {
            wrap_merge = true;
        }
    }
    if input.through && !comps.is_empty() {
        let first = comps[0].0;
        let last = comps[comps.len() - 1].1;
        comps = vec![(first, last)];
        wrap_merge = false;
    }
    // Seed selection.
    if let Some(seed) = input.seed
        && comps.len() > 1
    {
        let seed_m = fg.m(seed);
        let mut best = (f64::INFINITY, 0usize);
        for (ci, &(a, b)) in comps.iter().enumerate() {
            for pc in &pieces {
                for i in 0..pc.pts.len() - 1 {
                    let sm = 0.5 * (pc.s[i] + pc.s[i + 1]);
                    if sm < bnds[a] || sm > bnds[b + 1] {
                        continue;
                    }
                    for cand in [seed, fg.lift_near(seed, pc.pts[i])] {
                        let (d, _) = dist_point_seg(fg.m(cand), fg.m(pc.pts[i]), fg.m(pc.pts[i + 1]));
                        if d < best.0 {
                            best = (d, ci);
                        }
                    }
                }
            }
            let _ = seed_m;
        }
        comps = vec![comps[best.1]];
        wrap_merge = false;
    }
    // Emit components.
    for (ci, &(a, b)) in comps.iter().enumerate() {
        if wrap_merge && ci == comps.len() - 1 {
            break;
        }
        let who = input.who.with_sub(ci as u32);
        let owner = who.owner();
        // s-ranges covered: [bnds[a], bnds[b+1]] (+ wrap part).
        let mut ranges = vec![(bnds[a], bnds[b + 1])];
        if wrap_merge && ci == 0 {
            let (la, _) = comps[comps.len() - 1];
            ranges.push((bnds[la], n));
        }
        let any_stop = evs.iter().any(|e| e.incs.iter().any(|i| i.stop.is_some()));
        let is_closed_loop = closed && a == 0 && b == ni - 1 && !any_stop && !wrap_merge;
        // Curves.
        let curve_base = out.curves.len();
        for &(r0, r1) in &ranges {
            for pc in &pieces {
                let (p0, p1) = (pc.s[0], pc.s[pc.s.len() - 1]);
                let (c0, c1) = (r0.max(p0), r1.min(p1));
                if c1 - c0 <= 1e-12 {
                    continue;
                }
                let sp = poly_slice(&pc.pts, s_of_par(&pc.s, c0), s_of_par(&pc.s, c1));
                let mut par = vec![c0];
                for s in &pc.s {
                    if *s > c0 + 1e-12 && *s < c1 - 1e-12 {
                        par.push(*s);
                    }
                }
                par.push(c1);
                let (sp, par) = if sp.len() == par.len() { (sp, par) } else { resample(&pc.pts, &pc.s, c0, c1) };
                out.curves.push(ACurve { owner, seam: pc.seam, pts: sp, par, closed: false });
            }
        }
        let curves_here: Vec<usize> = (curve_base..out.curves.len()).collect();
        // Which local curves contain parameter s (and at which uv)?
        let incs_at = |out: &ClipOut, s: f64, uv: DVec2| -> Vec<(usize, f64)> {
            let mut v = Vec::new();
            // On a closed line the start parameter is also the end.
            let cands: Vec<f64> = if closed && s.abs() < 1e-9 { vec![s, n] } else { vec![s] };
            for &c in &curves_here {
                let cv = &out.curves[c];
                for &s in &cands {
                    if s >= cv.par[0] - 1e-9 && s <= cv.par[cv.par.len() - 1] + 1e-9 {
                        let p = poly_at(&cv.pts, s_of_par(&cv.par, s));
                        if (fg.m(p) - fg.m(uv)).length() <= fg.eps * 10.0 + 1e-9 * fg.size_uv() {
                            v.push((c, s.clamp(cv.par[0], cv.par[cv.par.len() - 1])));
                        }
                    }
                }
            }
            v
        };
        // Events of this component in order.
        let mut comp_evs: Vec<&LEv> = Vec::new();
        for e in &evs {
            let inside = ranges.iter().any(|&(r0, r1)| e.s >= r0 - 1e-9 && e.s <= r1 + 1e-9);
            if inside {
                comp_evs.push(e);
            }
        }
        if wrap_merge && ci == 0 {
            // Order: the wrapped part first.
            let (la, _) = comps[comps.len() - 1];
            comp_evs.sort_by(|x, y| {
                let kx = if x.s >= bnds[la] - 1e-9 { x.s - n } else { x.s };
                let ky = if y.s >= bnds[la] - 1e-9 { y.s - n } else { y.s };
                kx.total_cmp(&ky)
            });
        }
        let s_start = ranges[ranges.len() - 1].0.min(ranges[0].0);
        let s_first = if wrap_merge && ci == 0 { ranges[1].0 } else { s_start };
        let s_last = ranges[0].1;
        let mut counter = 2u32;
        let mut any_event = false;
        for e in &comp_evs {
            let is_first = !is_closed_loop && (e.s - s_first).abs() < 1e-9;
            let is_last = !is_closed_loop && (e.s - s_last).abs() < 1e-9;
            // Closed loops starting at a seam: the seam event is both ends.
            let end = if is_first {
                0
            } else if is_last {
                1
            } else {
                let c = counter;
                counter += 1;
                c
            };
            // Key: from the first incidence with a stopper, else seam.
            let on = e
                .incs
                .iter()
                .find_map(|i| i.stop.map(|(si, _)| on_ref(fg, stoppers[si].owner)))
                .or_else(|| e.incs.iter().find_map(|i| i.seam.map(|d| EdgeOrSplit::Seam(fg.id, d))))
                .unwrap_or(who.self_ref());
            let key = who.key(on, end);
            let seam_rel = e.incs.iter().all(|i| i.seam.is_some());
            for inc in &e.incs {
                let mut v = incs_at(&out, e.s, inc.uv);
                if v.is_empty() {
                    continue;
                }
                if let Some((si, tp)) = inc.stop {
                    v.push((STOP + si, tp));
                }
                any_event = true;
                out.events.push(AEvent { uv: inc.uv, key: key.clone(), inc: v, artificial: false, seam_rel });
            }
        }
        // Free ends.
        if !is_closed_loop {
            for (which, s) in [(0usize, s_first), (1usize, s_last)] {
                let has = comp_evs.iter().any(|e| (e.s - s).abs() < 1e-9);
                if has {
                    continue;
                }
                let uv = curves_here
                    .iter()
                    .find_map(|&c| {
                        let cv = &out.curves[c];
                        if (cv.par[0] - s).abs() < 1e-9 {
                            Some(cv.pts[0])
                        } else if (cv.par[cv.par.len() - 1] - s).abs() < 1e-9 {
                            Some(cv.pts[cv.pts.len() - 1])
                        } else {
                            None
                        }
                    })
                    .unwrap_or_else(|| fg.wrap(poly_at(&pts, s)));
                let key = if (which == 0 && s < 1e-9) || (which == 1 && s > n - 1e-9) {
                    input.end_keys[which].clone().unwrap_or_else(|| who.key(who.self_ref(), which as u32))
                } else {
                    who.key(who.self_ref(), which as u32)
                };
                let v = incs_at(&out, s, uv);
                out.events.push(AEvent { uv, key, inc: v, artificial: false, seam_rel: false });
                any_event = true;
            }
        } else if !any_event {
            // Closed loop with no events at all: artificial anchor.
            if let Some(&c) = curves_here.first() {
                let cv = &mut out.curves[c];
                cv.closed = true;
                let uv = cv.pts[0];
                let p0 = cv.par[0];
                out.events.push(AEvent { uv, key: who.key(who.self_ref(), 0), inc: vec![(c, p0)], artificial: true, seam_rel: true });
            }
        }
    }
    out
}

fn resample(pts: &[DVec2], s: &[f64], c0: f64, c1: f64) -> (Vec<DVec2>, Vec<f64>) {
    let mut par = vec![c0];
    for x in s {
        if *x > c0 + 1e-12 && *x < c1 - 1e-12 {
            par.push(*x);
        }
    }
    par.push(c1);
    let p = par.iter().map(|&x| poly_at(pts, s_of_par(s, x))).collect();
    (p, par)
}
