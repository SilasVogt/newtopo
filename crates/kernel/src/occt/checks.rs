//! Topology and geometry invariants of a kernel body, used by the OCCT tests
//! on every reference model. Kernel-agnostic: it only uses [`CadKernel`].
//!
//! Checked:
//! * every edge has 1–2 faces; seam and degenerate edges have exactly one;
//! * every face has wires, exactly one outer wire, listed first;
//! * closed (seamed) faces actually use a seam edge, twice;
//! * coedges chain: each coedge ends where the next one starts (3D);
//! * pcurve endpoints evaluated through `face_eval` match the 3D curve
//!   endpoints (`edge_eval`) for every coedge;
//! * tessellation triangles wind with the `face_eval` normal;
//! * tessellation UVs classify as inside (or on) their face;
//! * vertices pushed off the surface project back onto it;
//! * points on an edge project back onto that edge.

use crate::{BodyId, BodyTopology, CadKernel, EdgeId};
use glam::{DVec2, DVec3};

/// Thresholds for [`check_body`].
#[derive(Clone, Copy, Debug)]
pub struct CheckOptions {
    /// Absolute distance tolerance in model units.
    pub tol: f64,
    /// Tessellation deflection used for the mesh checks.
    pub mesh_tol: f64,
    /// Minimum fraction of triangles that must agree with the face normal.
    pub min_normal_agreement: f64,
}

impl Default for CheckOptions {
    fn default() -> Self {
        CheckOptions {
            tol: 1e-3,
            mesh_tol: 0.2,
            min_normal_agreement: 0.97,
        }
    }
}

/// `d <= tol`, false for NaN.
fn within(d: f64, tol: f64) -> bool {
    d <= tol
}

fn coedge_ends(t: &BodyTopology, e: EdgeId, reversed: bool) -> (f64, f64) {
    let (a, b) = t.edge(e).t_range;
    if reversed { (b, a) } else { (a, b) }
}

/// Returns every violated invariant (empty = body is fine).
pub fn check_body(k: &dyn CadKernel, body: BodyId, o: CheckOptions) -> Vec<String> {
    let mut errs = Vec::new();
    let t = match k.topology(body) {
        Ok(t) => t,
        Err(e) => return vec![format!("topology: {e}")],
    };
    macro_rules! bad {
        ($($a:tt)*) => { errs.push(format!($($a)*)) };
    }

    for (i, e) in t.edges.iter().enumerate() {
        let n = e.faces.len();
        if !(1..=2).contains(&n) {
            bad!("edge {i}: {n} faces");
        }
        if (e.seam || e.degenerate) && n != 1 {
            bad!("edge {i}: seam/degenerate edge with {n} faces");
        }
        if e.seam && e.degenerate {
            bad!("edge {i}: both seam and degenerate");
        }
        if e.degenerate && e.length > o.tol {
            bad!("edge {i}: degenerate edge of length {}", e.length);
        }
    }

    for fid in t.face_ids() {
        let f = t.face(fid);
        if f.wires.is_empty() {
            bad!("face {fid}: no wires");
            continue;
        }
        let outers = f.wires.iter().filter(|w| w.outer).count();
        if outers != 1 || !f.wires[0].outer {
            bad!(
                "face {fid}: {outers} outer wires (first outer: {})",
                f.wires[0].outer
            );
        }
        let seam_uses = f
            .wires
            .iter()
            .flat_map(|w| &w.coedges)
            .filter(|c| t.edge(c.edge).seam)
            .count();
        let closed_dirs = f.surface.u_closed as usize + f.surface.v_closed as usize;
        if closed_dirs > 0 && seam_uses < 2 * closed_dirs {
            bad!("face {fid}: closed in {closed_dirs} directions but {seam_uses} seam coedges");
        }
        for (wi, w) in f.wires.iter().enumerate() {
            for c in &w.coedges {
                if !t.edge(c.edge).faces.contains(&fid) {
                    bad!(
                        "face {fid} wire {wi}: edge {} does not list the face",
                        c.edge
                    );
                }
            }
            // Chain continuity in 3D.
            let n = w.coedges.len();
            let mut ends = Vec::with_capacity(n);
            for c in &w.coedges {
                let (s, e) = coedge_ends(&t, c.edge, c.reversed);
                match k.edge_eval(body, c.edge, &[s, e]) {
                    Ok(p) => ends.push((p[0], p[1])),
                    Err(e) => {
                        bad!("face {fid}: edge_eval({}) failed: {e}", c.edge);
                        ends.push((DVec3::NAN, DVec3::NAN));
                    }
                }
            }
            for i in 0..n {
                let gap = ends[i].1.distance(ends[(i + 1) % n].0);
                if !within(gap, o.tol) {
                    bad!(
                        "face {fid} wire {wi}: coedge {i} ends {gap:.2e} away from the next start"
                    );
                }
            }
            // Pcurve endpoints through the surface match the 3D endpoints.
            for (ci, c) in w.coedges.iter().enumerate() {
                let (s, e) = coedge_ends(&t, c.edge, c.reversed);
                let uv = match k.edge_pcurve_eval(body, c.edge, fid, c.reversed, &[s, e]) {
                    Ok(uv) => uv,
                    Err(err) => {
                        bad!("face {fid}: pcurve of edge {} failed: {err}", c.edge);
                        continue;
                    }
                };
                let on = match k.face_eval(body, fid, &uv) {
                    Ok(v) => v,
                    Err(err) => {
                        bad!("face {fid}: face_eval failed: {err}");
                        continue;
                    }
                };
                for (j, (ev, end)) in on.iter().zip([ends[ci].0, ends[ci].1]).enumerate() {
                    let d = ev.p.distance(end);
                    if !within(d, o.tol) {
                        bad!(
                            "face {fid} edge {}: pcurve end {j} is {d:.2e} off the 3D curve",
                            c.edge
                        );
                    }
                }
            }
        }

        // Mesh checks.
        let mesh = match k.tessellate(body, fid, o.mesh_tol) {
            Ok(m) => m,
            Err(e) => {
                bad!("face {fid}: tessellate failed: {e}");
                continue;
            }
        };
        if mesh.triangles.is_empty() {
            bad!("face {fid}: empty tessellation");
            continue;
        }
        let cent: Vec<DVec2> = mesh
            .triangles
            .iter()
            .map(|tr| tr.iter().map(|&i| mesh.uvs[i as usize]).sum::<DVec2>() / 3.0)
            .collect();
        let evals = k.face_eval(body, fid, &cent).unwrap_or_default();
        let mut agree = 0usize;
        let mut counted = 0usize;
        for (tr, ev) in mesh.triangles.iter().zip(&evals) {
            let [a, b, c] = tr.map(|i| mesh.positions[i as usize]);
            let gn = (b - a).cross(c - a);
            if gn.length() < 1e-12 || ev.n.length() < 0.5 {
                continue;
            }
            counted += 1;
            agree += (gn.normalize().dot(ev.n) > 0.0) as usize;
        }
        if counted > 0 && (agree as f64) < o.min_normal_agreement * counted as f64 {
            bad!("face {fid}: only {agree}/{counted} triangles wind with the face normal");
        }
        for (i, (n, uv)) in mesh.normals.iter().zip(&mesh.uvs).enumerate().take(64) {
            if let Ok(ev) = k.face_eval(body, fid, &[*uv])
                && n.dot(ev[0].n) < 0.99
            {
                bad!("face {fid}: mesh normal {i} disagrees with face_eval");
            }
        }
        if let Ok(cls) = k.face_classify_uv(body, fid, &cent) {
            let out = cls.iter().filter(|c| **c == crate::UvClass::Out).count();
            if out * 20 > cls.len() {
                bad!(
                    "face {fid}: {out}/{} triangle centroids classify as outside",
                    cls.len()
                );
            }
        } else {
            bad!("face {fid}: classify failed");
        }
        // Mesh vertices lie on the face: offset along the normal, project back.
        let step = (mesh.positions.len() / 24).max(1);
        // Skip singular points (apex, poles): their normal is only a limit.
        let all: Vec<usize> = (0..mesh.positions.len()).step_by(step).collect();
        let sing = k
            .face_eval(
                body,
                fid,
                &all.iter().map(|&i| mesh.uvs[i]).collect::<Vec<_>>(),
            )
            .unwrap_or_default();
        let idx: Vec<usize> = all
            .iter()
            .zip(&sing)
            .filter(|(_, e)| {
                e.du.cross(e.dv).length() > 1e-9 * (1.0 + e.du.length() * e.dv.length())
            })
            .map(|(&i, _)| i)
            .collect();
        let pushed: Vec<DVec3> = idx
            .iter()
            .map(|&i| mesh.positions[i] + mesh.normals[i] * 0.1)
            .collect();
        match k.project_to_face(body, fid, &pushed) {
            Ok(res) => {
                for (&i, r) in idx.iter().zip(&res) {
                    match r {
                        Some((uv, p)) => {
                            let d = p.distance(mesh.positions[i]);
                            let on = k
                                .face_eval(body, fid, &[*uv])
                                .map(|e| e[0].p.distance(*p))
                                .unwrap_or(f64::MAX);
                            if !within(d, o.tol.max(1e-2 * o.mesh_tol)) || !within(on, o.tol) {
                                bad!(
                                    "face {fid}: projection of vertex {i} landed {d:.2e} away (uv off by {on:.2e})"
                                );
                            }
                        }
                        None => bad!("face {fid}: projection of vertex {i} failed"),
                    }
                }
            }
            Err(e) => bad!("face {fid}: project_to_face failed: {e}"),
        }
    }

    for eid in t.edge_ids() {
        let e = t.edge(eid);
        if e.degenerate {
            continue;
        }
        let (a, b) = e.t_range;
        let ts: Vec<f64> = (1..4).map(|i| a + (b - a) * i as f64 / 4.0).collect();
        let pts = match k.edge_eval(body, eid, &ts) {
            Ok(p) => p,
            Err(err) => {
                bad!("edge {eid}: edge_eval failed: {err}");
                continue;
            }
        };
        match k.project_to_edge(body, eid, &pts) {
            Ok(res) => {
                for (p, r) in pts.iter().zip(&res) {
                    let back = r
                        .and_then(|t| k.edge_eval(body, eid, &[t]).ok())
                        .map(|q| q[0].distance(*p));
                    if !back.is_some_and(|d| d <= o.tol) {
                        bad!("edge {eid}: projection did not return to the curve ({back:?})");
                    }
                }
            }
            Err(err) => bad!("edge {eid}: project_to_edge failed: {err}"),
        }
    }
    errs
}
