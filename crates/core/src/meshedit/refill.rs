//! Post-steps shared by the mesh tools: refill n-gons a tool created with
//! the auto-fill decomposition (SPEC §7) in the polygon's best-fit plane,
//! and join triangle pairs back into quads.

use super::mesh::{area_vec, ekey};
use super::work::{Work, fan_tris, point_triangle_distance, polygon_shape_ok};
use crate::fill::{FillInput, fill_polygon, geom};
use glam::{DVec2, DVec3};
use std::collections::BTreeSet;

/// Refill every touched polygon with more than four sides (and touched
/// concave quads) with quads and triangles. Polygons whose fill would add a
/// diagonal that already exists elsewhere, or would flip, stay as they are.
/// Returns how many polygons were refilled.
pub(crate) fn fill_touched_ngons(w: &mut Work) -> usize {
    let mut count = 0;
    let ids: Vec<u32> = w
        .alive_polys()
        .filter(|&q| {
            let p = &w.polys[q as usize];
            p.touched
                && (p.verts.len() > 4
                    || p.verts.len() == 4 && !polygon_shape_ok(&w.poly_pos(&p.verts), w.diag))
        })
        .collect();
    for q in ids {
        let verts = w.polys[q as usize].verts.clone();
        let pos = w.poly_pos(&verts);
        let Some(uv) = geom::project_best_fit(&pos) else {
            continue;
        };
        let iso = principal_dirs(&uv);
        let rank: Vec<u32> = verts.clone();
        let res = fill_polygon(&FillInput {
            uv: &uv,
            pos: &pos,
            rank: &rank,
            iso_dirs: iso,
        });
        if res.polys.len() < 2 {
            continue;
        }
        let pieces: Vec<Vec<u32>> = res
            .polys
            .iter()
            .map(|p| p.iter().map(|&i| verts[i as usize]).collect())
            .collect();
        // New diagonals must not already be edges.
        let own: BTreeSet<(u32, u32)> = (0..verts.len())
            .map(|i| ekey(verts[i], verts[(i + 1) % verts.len()]))
            .collect();
        let mut ok = true;
        for p in &pieces {
            for i in 0..p.len() {
                let (a, b) = (p[i], p[(i + 1) % p.len()]);
                if !own.contains(&ekey(a, b)) && w.neighbors(a).contains(&b) {
                    ok = false;
                }
            }
            let pp = w.poly_pos(p);
            let n = area_vec(&pp).normalize_or_zero();
            let r = w.reference_normal(p, &w.polys[q as usize]);
            if !polygon_shape_ok(&pp, w.diag) || n.dot(r) < w.min_cos {
                ok = false;
            }
        }
        if !ok {
            continue;
        }
        // Witnesses go to their nearest piece.
        let wit = w
            .witnesses
            .as_ref()
            .map(|ws| ws[q as usize].clone())
            .unwrap_or_default();
        let first_new = w.polys.len() as u32;
        let npieces = pieces.len();
        let piece_pos: Vec<Vec<DVec3>> = pieces.iter().map(|p| w.poly_pos(p)).collect();
        w.replace_polygon(q, pieces);
        if let Some(ws) = &mut w.witnesses {
            ws[q as usize].clear();
            for x in wit {
                let p = w.src.positions[x as usize];
                let mut best = (f64::INFINITY, 0usize);
                for (i, pp) in piece_pos.iter().enumerate() {
                    for t in fan_tris(pp) {
                        let d = point_triangle_distance(p, t[0], t[1], t[2]);
                        if d < best.0 {
                            best = (d, i);
                        }
                    }
                }
                let id = if best.1 == 0 {
                    q
                } else {
                    first_new + best.1 as u32 - 1
                };
                debug_assert!((best.1) < npieces);
                ws[id as usize].push(x);
            }
        }
        count += 1;
    }
    count
}

/// Two orthogonal unit directions for fill quality: the longest boundary
/// edge's direction and its perpendicular.
fn principal_dirs(uv: &[DVec2]) -> [DVec2; 2] {
    let n = uv.len();
    let mut best = (0.0, DVec2::X);
    for i in 0..n {
        let d = uv[(i + 1) % n] - uv[i];
        let l = d.length();
        if l > best.0 {
            best = (l, d / l);
        }
    }
    [best.1, best.1.perp()]
}

/// Join pairs of adjacent triangles into quads where the result is convex,
/// nearly planar and well shaped, best pairs first. Never joins across a
/// feature edge. Returns the number of quads made.
pub(crate) fn pair_triangles(w: &mut Work, max_fold_deg: f64) -> usize {
    let cos_fold = max_fold_deg.to_radians().cos();
    let mut cands: Vec<(f64, u32, u32, u32, u32, Vec<u32>)> = Vec::new();
    for q in w.alive_polys().collect::<Vec<_>>() {
        let v = w.polys[q as usize].verts.clone();
        if v.len() != 3 {
            continue;
        }
        for i in 0..3 {
            let (a, b, c) = (v[i], v[(i + 1) % 3], v[(i + 2) % 3]);
            if a > b || w.is_feature(a, b) {
                continue;
            }
            let others = w.edge_polys(a, b);
            let Some(&r) = others.iter().find(|&&r| r != q) else {
                continue;
            };
            let rv = &w.polys[r as usize].verts;
            if rv.len() != 3 || w.polys[r as usize].group != w.polys[q as usize].group {
                continue;
            }
            let Some(&d) = rv.iter().find(|&&x| x != a && x != b) else {
                continue;
            };
            if d == c {
                continue;
            }
            let quad = vec![b, c, a, d];
            let pos = w.poly_pos(&quad);
            if !polygon_shape_ok(&pos, w.diag) {
                continue;
            }
            let n1 = area_vec(&w.poly_pos(&v)).normalize_or_zero();
            let n2 = area_vec(&w.poly_pos(rv)).normalize_or_zero();
            let fold = n1.dot(n2);
            if fold < cos_fold {
                continue;
            }
            if !quad_deviation_ok(w, q, r, &pos) {
                continue;
            }
            let quality = quad_quality(&pos) * fold;
            cands.push((quality, a, b, q, r, quad));
        }
    }
    cands.sort_by(|x, y| y.0.total_cmp(&x.0).then((x.1, x.2).cmp(&(y.1, y.2))));
    let mut used = BTreeSet::new();
    let mut made = 0;
    for (_, _, _, q, r, quad) in cands {
        if used.contains(&q) || used.contains(&r) {
            continue;
        }
        used.insert(q);
        used.insert(r);
        let wit_r = w
            .witnesses
            .as_mut()
            .map(|ws| std::mem::take(&mut ws[r as usize]))
            .unwrap_or_default();
        // Kill r, turn q into the quad.
        for &x in &w.polys[r as usize].verts.clone() {
            w.vpolys[x as usize].retain(|&p| p != r);
        }
        w.polys[r as usize].alive = false;
        w.replace_polygon(q, vec![quad]);
        if let Some(ws) = &mut w.witnesses {
            ws[q as usize].extend(wit_r);
        }
        made += 1;
    }
    made
}

/// Witnesses of both triangles stay within the bound for either
/// triangulation of the quad.
fn quad_deviation_ok(w: &Work, q: u32, r: u32, quad: &[DVec3]) -> bool {
    let Some(ws) = &w.witnesses else {
        return true;
    };
    let tri_a = [[quad[0], quad[1], quad[2]], [quad[0], quad[2], quad[3]]];
    let tri_b = [[quad[0], quad[1], quad[3]], [quad[1], quad[2], quad[3]]];
    ws[q as usize]
        .iter()
        .chain(ws[r as usize].iter())
        .all(|&x| {
            let p = w.src.positions[x as usize];
            let da = tri_a
                .iter()
                .map(|t| point_triangle_distance(p, t[0], t[1], t[2]))
                .fold(f64::INFINITY, f64::min);
            let db = tri_b
                .iter()
                .map(|t| point_triangle_distance(p, t[0], t[1], t[2]))
                .fold(f64::INFINITY, f64::min);
            da.max(db) <= w.max_dev
        })
}

/// Shape quality in [0, 1]: smallest corner sine relative to the ideal
/// corner (90° for quads, 60° for triangles).
pub(crate) fn quad_quality(pos: &[DVec3]) -> f64 {
    let n = pos.len();
    let ideal = if n == 3 { 3f64.sqrt() / 2.0 } else { 1.0 };
    let nrm = area_vec(pos).normalize_or_zero();
    let mut q = f64::INFINITY;
    for i in 0..n {
        let a = (pos[(i + n - 1) % n] - pos[i]).normalize_or_zero();
        let b = (pos[(i + 1) % n] - pos[i]).normalize_or_zero();
        let s = b.cross(a).dot(nrm);
        q = q.min(s / ideal);
    }
    if q.is_finite() {
        q.clamp(0.0, 1.0)
    } else {
        0.0
    }
}
