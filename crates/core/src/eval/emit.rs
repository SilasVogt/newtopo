//! Half-edge mesh → [`OutputMesh`] (pole collapse, deterministic ordering,
//! per-corner CAD normals, hard edges, edge paths).

use super::Owner;
use super::assemble::HMesh;
use super::bridge::he_path3;
use super::ctx::BodyCtx;
use crate::keys::EdgeKey;
use crate::mesh::{OutEdge, OutEdgeKind, OutPolygon, OutVertex, OutputMesh};
use glam::{DVec2, DVec3};
use newtopo_kernel::{BodyId, EdgeId};
use std::collections::{BTreeMap, HashMap, HashSet};

pub fn emit(
    ctx: &BodyCtx,
    body: BodyId,
    mesh: &HMesh,
    pole_target: &HashMap<EdgeId, usize>,
    hard: &HashMap<EdgeKey, bool>,
    sharp_deg: f64,
    fan: &HashSet<usize>,
) -> OutputMesh {
    let fv = |v: usize| mesh.final_v(v, pole_target);
    let walks = mesh.all_region_walks();
    let mut regions: Vec<usize> = walks
        .keys()
        .copied()
        .filter(|r| mesh.regions[*r].alive)
        .collect();
    regions.sort();
    // Polygons as (final vertex, corner half-edge).
    let mut polys: Vec<(Vec<(usize, usize)>, usize)> = Vec::new();
    for r in regions {
        for w in &walks[&r] {
            if std::env::var("NT_DEBUG_EMIT").is_ok() {
                for k in 0..w.len() {
                    let (h, n) = (w[k], w[(k + 1) % w.len()]);
                    if mesh.he[h].to != mesh.he[n].from {
                        eprintln!(
                            "emit: broken walk in region {r}: he {h} {:?}->{:?} owner {:?} then he {n} from {:?} owner {:?}",
                            mesh.he[h].from,
                            mesh.he[h].to,
                            mesh.he[h].owner,
                            mesh.he[n].from,
                            mesh.he[n].owner
                        );
                    }
                }
            }
            let mut corners: Vec<(usize, usize)> =
                w.iter().map(|&h| (fv(mesh.he[h].from), h)).collect();
            // Remove consecutive duplicates (cyclic).
            let mut out: Vec<(usize, usize)> = Vec::new();
            for c in corners.drain(..) {
                if out.last().is_some_and(|l| l.0 == c.0) {
                    continue;
                }
                out.push(c);
            }
            while out.len() > 1 && out[0].0 == out[out.len() - 1].0 {
                out.pop();
            }
            let distinct: HashSet<usize> = out.iter().map(|c| c.0).collect();
            if distinct.len() < 3 {
                continue;
            }
            polys.push((out, r));
        }
    }
    // Vertex order by key.
    let mut used: Vec<usize> = polys.iter().flat_map(|p| p.0.iter().map(|c| c.0)).collect();
    used.sort();
    used.dedup();
    used.sort_by(|&a, &b| mesh.verts[a].key.cmp(&mesh.verts[b].key).then(a.cmp(&b)));
    let mut idx: HashMap<usize, u32> = HashMap::new();
    let mut out = OutputMesh {
        body,
        ..Default::default()
    };
    for (i, &v) in used.iter().enumerate() {
        idx.insert(v, i as u32);
        let gv = &mesh.verts[v];
        out.vertices.push(OutVertex {
            key: gv.key.clone(),
            anchor: gv.anchor,
            position: gv.pos,
        });
    }
    // Kernel evaluations batched per face (one call per face, SPEC §3.4).
    let mut batch: BTreeMap<newtopo_kernel::FaceId, Vec<DVec2>> = BTreeMap::new();
    let mut ask = |f: newtopo_kernel::FaceId, uv: DVec2| -> (newtopo_kernel::FaceId, usize) {
        let l = batch.entry(f).or_default();
        l.push(uv);
        (f, l.len() - 1)
    };
    // Corner normal requests.
    let corner_req = |h: usize,
                      ask: &mut dyn FnMut(
        newtopo_kernel::FaceId,
        DVec2,
    ) -> (newtopo_kernel::FaceId, usize)| {
        let he = &mesh.he[h];
        if let Some(uv) = he.uv.first() {
            return Some(ask(he.face, *uv));
        }
        let p = mesh.prev(h);
        let hp = &mesh.he[p];
        hp.uv.last().map(|uv| ask(hp.face, *uv))
    };
    let mut poly_reqs: Vec<Vec<Option<(newtopo_kernel::FaceId, usize)>>> =
        Vec::with_capacity(polys.len());
    for (corners, _) in &polys {
        poly_reqs.push(
            corners
                .iter()
                .map(|&(_, h)| corner_req(h, &mut ask))
                .collect(),
        );
    }
    // Edge path requests.
    let mut hs: Vec<usize> = mesh.alive_he().collect();
    hs.sort();
    let mut path_req: HashMap<usize, (newtopo_kernel::FaceId, usize, usize)> = HashMap::new();
    for &h in &hs {
        let he = &mesh.he[h];
        if he.p3.is_some() || he.uv.is_empty() {
            continue;
        }
        let mut uv = he.uv.clone();
        if uv.len() == 2 {
            let (a, b) = (uv[0], uv[1]);
            uv = (0..=12).map(|i| a + (b - a) * (i as f64 / 12.0)).collect();
        }
        let (f, first) = ask(he.face, uv[0]);
        for q in &uv[1..] {
            ask(he.face, *q);
        }
        path_req.insert(h, (f, first, uv.len()));
    }
    let evals: BTreeMap<newtopo_kernel::FaceId, Vec<newtopo_kernel::SurfEval>> = batch
        .iter()
        .map(|(f, uvs)| (*f, ctx.face_eval(*f, uvs)))
        .collect();
    let got =
        |r: (newtopo_kernel::FaceId, usize)| evals.get(&r.0).and_then(|v| v.get(r.1)).copied();
    // Polygons, rotated to start at the smallest index, sorted.
    let mut opolys: Vec<OutPolygon> = Vec::new();
    for (pi, (corners, r)) in polys.iter().enumerate() {
        let n = corners.len();
        let start = (0..n).min_by_key(|&k| idx[&corners[k].0]).unwrap();
        let mut verts = Vec::with_capacity(n);
        let mut normals = Vec::with_capacity(n);
        for k in 0..n {
            let (v, h) = corners[(start + k) % n];
            verts.push(idx[&v]);
            let nrm = poly_reqs[pi][(start + k) % n].and_then(got).map(|e| e.n);
            normals.push(nrm.unwrap_or_else(|| corner_normal(ctx, mesh, h)));
        }
        let face = mesh.he[corners[0].1].face;
        opolys.push(OutPolygon {
            verts,
            face,
            normals,
            fan_fallback: fan.contains(r),
        });
    }
    opolys.sort_by(|a, b| a.verts.cmp(&b.verts).then(a.face.cmp(&b.face)));
    out.polygons = opolys;
    // Edges.
    let sharp = sharp_deg.to_radians();
    let mut seen: HashMap<(u32, u32, OutEdgeKind), usize> = HashMap::new();
    let mut edges: Vec<OutEdge> = Vec::new();
    for h in hs {
        let he = &mesh.he[h];
        let (a, b) = (fv(he.from), fv(he.to));
        if a == b {
            continue;
        }
        let (Some(&ia), Some(&ib)) = (idx.get(&a), idx.get(&b)) else {
            continue;
        };
        let kind = match he.owner {
            Owner::Cad(e) => OutEdgeKind::Cad(e),
            Owner::Split(s) => OutEdgeKind::Split(s),
            Owner::Line(k) => OutEdgeKind::Line(k),
            Owner::Seam(_) | Owner::Pole(_) | Owner::Fill => OutEdgeKind::Fill,
        };
        let k = (ia.min(ib), ia.max(ib), kind);
        if seen.contains_key(&k) {
            continue;
        }
        let key = mesh.edge_key(h, pole_target);
        let mut is_hard = match he.owner {
            Owner::Cad(e) => ctx.topo.edge(e).dihedral.is_some_and(|d| d > sharp),
            _ => false,
        };
        if let Some(k) = &key
            && let Some(&o) = hard.get(k)
        {
            is_hard = o;
        }
        let mut path = match path_req.get(&h) {
            Some(&(f, first, n)) => match evals.get(&f) {
                Some(ev) if ev.len() >= first + n => {
                    ev[first..first + n].iter().map(|e| e.p).collect()
                }
                _ => he_path3(ctx, mesh, h),
            },
            None => he_path3(ctx, mesh, h),
        };
        if ia > ib {
            path.reverse();
        }
        if let Some(f) = path.first_mut() {
            *f = out.vertices[k.0 as usize].position;
        }
        if let Some(l) = path.last_mut() {
            *l = out.vertices[k.1 as usize].position;
        }
        seen.insert(k, edges.len());
        edges.push(OutEdge {
            a: k.0,
            b: k.1,
            kind,
            key,
            hard: is_hard,
            path,
        });
    }
    edges.sort_by(|x, y| {
        (x.a, x.b)
            .cmp(&(y.a, y.b))
            .then_with(|| format!("{:?}", x.kind).cmp(&format!("{:?}", y.kind)))
    });
    out.edges = edges;
    out
}

fn corner_normal(ctx: &BodyCtx, mesh: &HMesh, h: usize) -> DVec3 {
    let he = &mesh.he[h];
    if let Some(uv) = he.uv.first()
        && let Some(e) = ctx.face_eval(he.face, &[*uv]).first()
    {
        return e.n;
    }
    // Fall back to the incoming half-edge's end.
    let p = mesh.prev(h);
    let hp = &mesh.he[p];
    if let Some(uv) = hp.uv.last()
        && let Some(e) = ctx.face_eval(hp.face, &[*uv]).first()
    {
        return e.n;
    }
    // Project the vertex onto its region's face.
    let pos = mesh.verts[he.from].pos;
    if let Ok(v) = ctx.kernel.project_to_face(ctx.kbody, he.face, &[pos])
        && let Some(Some((uv, _))) = v.first()
        && let Some(e) = ctx.face_eval(he.face, &[*uv]).first()
    {
        return e.n;
    }
    DVec3::Z
}
