//! Vertex Slide (SPEC §9.5.6): move vertices along one of their edges.
//! Own design.
//!
//! [`SlidePlan::new`] picks, once, the edge every vertex slides along (an
//! explicit neighbour, or the allowed edge best aligned with a direction)
//! and applies the feature rules. [`SlidePlan::apply`] then places the
//! vertices for a factor and is cheap enough to call on every mouse move:
//! it checks only the polygons around the moving vertices, and if the
//! factor would fold, flip or degenerate one of them it stops at the
//! largest factor that is still valid (bisection) and says so. A vertex
//! that lands exactly on its neighbour merges into it.

use super::mesh::{area_vec, ekey};
use super::work::{Prov, VClass, Work, polygon_shape_ok};
use super::{EditOutcome, EditReport, Preserve, check_input};
use crate::meshedit::mesh::EditMesh;
use crate::ops::Refusal;
use glam::DVec3;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};

/// Which edge each vertex slides along.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SlideEdge {
    /// `[vertex, neighbour]` pairs: slide each vertex towards that
    /// neighbour (they must share an edge).
    Toward(Vec<[u32; 2]>),
    /// Slide each vertex along its allowed edge whose direction is closest
    /// to `dir` (for example the drag direction). Edges to other sliding
    /// vertices are not candidates.
    Direction { vertices: Vec<u32>, dir: DVec3 },
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SlideParams {
    /// 0 = where the vertex is, 1 = on its neighbour (see `even`).
    pub factor: f64,
    /// Every vertex moves the same distance: the factor times the length
    /// of the shortest slide edge.
    pub even: bool,
    /// Keep the factor in `[0, 1]`. Off, vertices may move past either end
    /// of their edge along its line.
    pub clamp: bool,
    pub preserve: Preserve,
    pub max_normal_change_deg: f64,
}

impl Default for SlideParams {
    fn default() -> Self {
        SlideParams {
            factor: 0.5,
            even: false,
            clamp: true,
            preserve: Preserve::default(),
            max_normal_change_deg: 60.0,
        }
    }
}

/// One sliding vertex.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SlideMove {
    pub vertex: u32,
    pub toward: u32,
    pub from: DVec3,
    pub to: DVec3,
}

impl SlideMove {
    fn length(&self) -> f64 {
        self.from.distance(self.to)
    }
}

/// A prepared slide: the chosen edges and the polygons to check.
#[derive(Clone, Debug)]
pub struct SlidePlan<'a> {
    mesh: &'a EditMesh,
    work: Work<'a>,
    moves: Vec<SlideMove>,
    /// Polygons around the moving vertices.
    around: Vec<Around>,
    shortest: f64,
    /// Moving vertex → index in `moves`.
    index: HashMap<u32, usize>,
    skipped: Vec<String>,
}

/// A polygon around the moving vertices, as it was before the slide.
#[derive(Clone, Copy, Debug)]
struct Around {
    poly: u32,
    /// Geometric unit normal.
    normal: DVec3,
    /// CAD reference normal.
    reference: DVec3,
    /// Smallest allowed cosine to the reference: the normal limit, or the
    /// polygon's own cosine if it is already beyond the limit (it may
    /// change but not get worse).
    min_cos: f64,
    /// Whether it passed the shape check (convex, non-degenerate). One
    /// that didn't only has to keep a non-zero area and its orientation.
    shape_ok: bool,
}

/// Iterations of the bisection that finds the largest valid factor.
const BISECT_STEPS: usize = 48;

impl<'a> SlidePlan<'a> {
    pub fn new(
        mesh: &'a EditMesh,
        edge: &SlideEdge,
        preserve: Preserve,
        max_normal_change_deg: f64,
    ) -> Result<SlidePlan<'a>, Refusal> {
        check_input(mesh)?;
        if !(max_normal_change_deg > 0.0 && max_normal_change_deg < 90.0) {
            return Err(Refusal::new(
                "The normal-change limit must be between 0° and 90°",
            ));
        }
        let mut w = Work::new(mesh, preserve.mask());
        w.min_cos = max_normal_change_deg.to_radians().cos();
        let nv = mesh.positions.len();
        let valid_vertex = |v: u32| (v as usize) < nv && w.valive[v as usize];
        // (vertex, explicit neighbour)
        let requests: Vec<(u32, Option<u32>)> = match edge {
            SlideEdge::Toward(pairs) => {
                if pairs.is_empty() {
                    return Err(Refusal::new("Select one or more vertices first"));
                }
                let mut seen = BTreeSet::new();
                let mut out = Vec::new();
                for &[x, y] in pairs {
                    if !valid_vertex(x)
                        || !valid_vertex(y)
                        || x == y
                        || w.edge_polys(x, y).is_empty()
                    {
                        return Err(Refusal::new(format!("{x}-{y} is not an edge of this mesh")));
                    }
                    if !seen.insert(x) {
                        return Err(Refusal::new(format!(
                            "Vertex {x} is given more than one edge to slide along"
                        )));
                    }
                    out.push((x, Some(y)));
                }
                out
            }
            SlideEdge::Direction { vertices, dir } => {
                if vertices.is_empty() {
                    return Err(Refusal::new("Select one or more vertices first"));
                }
                if !dir.is_finite() || dir.length() <= 1e-300 {
                    return Err(Refusal::new(
                        "The slide direction must be a non-zero vector",
                    ));
                }
                let mut vs = vertices.clone();
                vs.sort_unstable();
                vs.dedup();
                for &v in &vs {
                    if !valid_vertex(v) {
                        return Err(Refusal::new(format!("Vertex {v} is not in this mesh")));
                    }
                }
                vs.into_iter().map(|v| (v, None)).collect()
            }
        };
        let sliding: BTreeSet<u32> = requests.iter().map(|r| r.0).collect();
        let dir = match edge {
            SlideEdge::Direction { dir, .. } => dir.normalize(),
            SlideEdge::Toward(_) => DVec3::ZERO,
        };
        let mut moves = Vec::new();
        let mut skipped = Vec::new();
        for (x, want) in requests {
            let allowed = allowed_edges(&w, x);
            if allowed.is_empty() {
                skipped.push(format!(
                    "vertex {x} is a corner (pinned, a feature end or a junction) and stays"
                ));
                continue;
            }
            let y = match want {
                Some(y) => {
                    if !allowed.contains(&y) {
                        skipped.push(format!(
                            "vertex {x} may only slide along its feature edges, not towards {y}"
                        ));
                        continue;
                    }
                    if sliding.contains(&y) {
                        skipped.push(format!("vertex {x} slides towards {y}, which slides too"));
                        continue;
                    }
                    y
                }
                None => {
                    let mut best: Option<(f64, u32)> = None;
                    for &y in &allowed {
                        if sliding.contains(&y) {
                            continue;
                        }
                        let d = (w.pos[y as usize] - w.pos[x as usize]).normalize_or_zero();
                        let s = d.dot(dir);
                        if best.is_none_or(|b| s > b.0) {
                            best = Some((s, y));
                        }
                    }
                    match best {
                        Some((_, y)) => y,
                        None => {
                            skipped.push(format!(
                                "every edge vertex {x} may slide along leads to another sliding vertex"
                            ));
                            continue;
                        }
                    }
                }
            };
            moves.push(SlideMove {
                vertex: x,
                toward: y,
                from: w.pos[x as usize],
                to: w.pos[y as usize],
            });
        }
        if moves.is_empty() {
            return Err(Refusal::new(format!(
                "Nothing can slide: {}",
                skipped.first().map(String::as_str).unwrap_or("no vertices")
            )));
        }
        let mut polys: BTreeSet<u32> = BTreeSet::new();
        for m in &moves {
            polys.extend(w.vpolys[m.vertex as usize].iter().copied());
        }
        let around = polys
            .into_iter()
            .map(|q| {
                let p = &w.polys[q as usize];
                let pos = w.poly_pos(&p.verts);
                let normal = area_vec(&pos).normalize_or_zero();
                let reference = w.reference_normal(&p.verts, p);
                Around {
                    poly: q,
                    normal,
                    reference,
                    min_cos: w.min_cos.min(normal.dot(reference) - 1e-12),
                    shape_ok: polygon_shape_ok(&pos, w.diag),
                }
            })
            .collect();
        let shortest = moves
            .iter()
            .map(SlideMove::length)
            .fold(f64::INFINITY, f64::min);
        let index = moves
            .iter()
            .enumerate()
            .map(|(i, m)| (m.vertex, i))
            .collect();
        Ok(SlidePlan {
            mesh,
            work: w,
            moves,
            around,
            shortest,
            index,
            skipped,
        })
    }

    /// The vertices that slide and the edge each one slides along.
    pub fn moves(&self) -> &[SlideMove] {
        &self.moves
    }

    /// Vertices the plan leaves out, with the reason.
    pub fn skipped(&self) -> &[String] {
        &self.skipped
    }

    /// Fraction of its own edge each vertex moves for `factor`.
    fn fractions(&self, factor: f64, even: bool) -> impl Iterator<Item = f64> + '_ {
        self.moves.iter().map(move |m| {
            if even {
                factor * self.shortest / m.length()
            } else {
                factor
            }
        })
    }

    /// Positions for a factor, without any check (for drawing a preview).
    pub fn positions(&self, factor: f64, even: bool, clamp: bool) -> Vec<(u32, DVec3)> {
        let f = if clamp {
            factor.clamp(0.0, 1.0)
        } else {
            factor
        };
        self.moves
            .iter()
            .zip(self.fractions(f, even))
            .map(|(m, t)| (m.vertex, m.from + (m.to - m.from) * t))
            .collect()
    }

    /// Are the polygons around the moving vertices valid at `factor`
    /// (vertices that would land on their neighbour excluded)?
    fn valid(&self, factor: f64, even: bool) -> bool {
        if !factor.is_finite() {
            return false;
        }
        let pos = self.positions(factor, even, false);
        let w = &self.work;
        let at = |v: u32| -> DVec3 {
            self.index
                .get(&v)
                .map(|&i| pos[i].1)
                .unwrap_or(w.pos[v as usize])
        };
        self.around.iter().all(|a| {
            let p: Vec<DVec3> = w.polys[a.poly as usize]
                .verts
                .iter()
                .map(|&v| at(v))
                .collect();
            a.accepts(&p, w.diag)
        })
    }

    /// The largest factor between 0 and `factor` (same sign) at which every
    /// polygon around the moving vertices is still valid.
    pub fn limit(&self, factor: f64, even: bool, clamp: bool) -> f64 {
        let f = if clamp {
            factor.clamp(0.0, 1.0)
        } else {
            factor
        };
        if f == 0.0 || self.valid(f, even) {
            return f;
        }
        let (mut ok, mut bad) = (0.0, f);
        for _ in 0..BISECT_STEPS {
            let mid = 0.5 * (ok + bad);
            if self.valid(mid, even) {
                ok = mid;
            } else {
                bad = mid;
            }
        }
        ok
    }

    /// Slide to `factor`. Factor 0 returns the input unchanged; a factor
    /// that would make a polygon invalid stops at the largest valid one
    /// (reported, `reached_target` false); refused only if no movement at
    /// all is possible or a vertex can't merge into the neighbour it
    /// lands on.
    pub fn apply(&self, factor: f64, even: bool, clamp: bool) -> Result<EditOutcome, Refusal> {
        if !factor.is_finite() {
            return Err(Refusal::new("The slide factor must be a finite number"));
        }
        let before = self.mesh.stats();
        let f = if clamp {
            factor.clamp(0.0, 1.0)
        } else {
            factor
        };
        let fr: Vec<f64> = self.fractions(f, even).collect();
        let lands: Vec<bool> = fr.iter().map(|&t| t == 1.0).collect();
        let any_land = lands.iter().any(|&l| l);
        // Landing vertices merge; the others must stay valid while moving.
        let (f_used, reached) = if any_land {
            (f, true)
        } else {
            let l = self.limit(f, even, false);
            (l, l == f)
        };
        if f_used == 0.0 {
            if f != 0.0 {
                return Err(Refusal::new(
                    "Can't slide: any movement would fold, flip or flatten a polygon around a sliding vertex",
                ));
            }
            let identity: Vec<Option<u32>> =
                (0..self.mesh.positions.len() as u32).map(Some).collect();
            let mut selected_vertices: Vec<u32> = self.moves.iter().map(|m| m.vertex).collect();
            selected_vertices.sort_unstable();
            return Ok(EditOutcome {
                mesh: self.mesh.clone(),
                selection: self.edges_in(&identity),
                vertex_map: identity,
                selected_vertices,
                report: EditReport {
                    before,
                    after: before,
                    reached_target: true,
                    message: "Factor 0: nothing moved".into(),
                    skipped: self.skipped.clone(),
                },
            });
        }
        let mut w = self.work.clone();
        let fr: Vec<f64> = self.fractions(f_used, even).collect();
        for (m, &t) in self.moves.iter().zip(&fr) {
            if t == 1.0 {
                continue;
            }
            let p = m.from + (m.to - m.from) * t;
            let prov = Prov::lerp(
                &w.prov[m.vertex as usize],
                &w.prov[m.toward as usize],
                t.clamp(0.0, 1.0),
            );
            w.set_position(m.vertex, p, prov);
        }
        let mut merged = 0;
        for (m, &t) in self.moves.iter().zip(&fr) {
            if t != 1.0 {
                continue;
            }
            if !w.try_merge(m.vertex, m.toward) {
                return Err(Refusal::new(format!(
                    "Vertex {} can't land on {}: merging them would break a preserved edge, the topology or a polygon",
                    m.vertex, m.toward
                )));
            }
            merged += 1;
        }
        // A landing merge must not leave the other moved vertices' polygons
        // invalid (they were checked without the merge).
        if any_land && !self.result_valid(&w) {
            return Err(Refusal::new(
                "Can't slide this far: a polygon around a sliding vertex would fold or flip",
            ));
        }
        let (out, map) = w.finish();
        let after = out.stats();
        let vertex_map: Vec<Option<u32>> = map[..self.mesh.positions.len()].to_vec();
        let mut selected_vertices: Vec<u32> = self
            .moves
            .iter()
            .filter_map(|m| vertex_map[m.vertex as usize].or(vertex_map[m.toward as usize]))
            .collect();
        selected_vertices.sort_unstable();
        selected_vertices.dedup();
        let slid = self.moves.len() - merged;
        let mut message = format!("Slid {slid} vertex(es)");
        if merged > 0 {
            message += &format!(", merged {merged} into the neighbour it reached");
        }
        if !reached {
            message += &format!(
                "; stopped at factor {f_used:.4} (asked {f:.4}): further would fold, flip or flatten a polygon"
            );
        }
        Ok(EditOutcome {
            selection: self.edges_in(&vertex_map),
            vertex_map,
            mesh: out,
            selected_vertices,
            report: EditReport {
                before,
                after,
                reached_target: reached,
                message,
                skipped: self.skipped.clone(),
            },
        })
    }

    /// Polygons around the moved vertices after the slide and any landing
    /// merges.
    fn result_valid(&self, w: &Work) -> bool {
        let mut polys: BTreeSet<u32> = BTreeSet::new();
        for m in &self.moves {
            for v in [m.vertex, m.toward] {
                if w.valive[v as usize] {
                    polys.extend(w.vpolys[v as usize].iter().copied());
                }
            }
        }
        polys.into_iter().all(|q| {
            let p = &w.polys[q as usize];
            let pos = w.poly_pos(&p.verts);
            match self.around.iter().find(|a| a.poly == q) {
                Some(a) => a.accepts(&pos, w.diag),
                // A polygon the landing merge replaced: the usual checks
                // (try_merge ran them already).
                None => {
                    polygon_shape_ok(&pos, w.diag)
                        && area_vec(&pos)
                            .normalize_or_zero()
                            .dot(w.reference_normal(&p.verts, p))
                            >= w.min_cos
                }
            }
        })
    }

    /// The slide edges that still exist, in output indices.
    fn edges_in(&self, map: &[Option<u32>]) -> Vec<[u32; 2]> {
        let mut out: Vec<[u32; 2]> = self
            .moves
            .iter()
            .filter_map(|m| {
                let (a, b) = (map[m.vertex as usize]?, map[m.toward as usize]?);
                let k = ekey(a, b);
                Some([k.0, k.1])
            })
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }
}

impl Around {
    /// May the polygon take these corner positions?
    fn accepts(&self, p: &[DVec3], diag: f64) -> bool {
        let av = area_vec(p);
        if self.shape_ok {
            if !polygon_shape_ok(p, diag) {
                return false;
            }
        } else if av.length() <= 1e-12 * diag * diag {
            return false;
        }
        let n = av.normalize_or_zero();
        n.dot(self.reference) >= self.min_cos && n.dot(self.normal) > 0.0
    }
}

/// Edges vertex `x` may slide along: none for corners, the two curve edges
/// for a vertex on a feature curve (open boundary, preserved CAD or hard
/// edge, UV seam), any edge for a free vertex.
fn allowed_edges(w: &Work, x: u32) -> Vec<u32> {
    match w.vclass(x) {
        VClass::Corner => Vec::new(),
        VClass::Curve([a, b]) => vec![a, b],
        VClass::Free => w.neighbors(x),
    }
}

/// Slide vertices in one call (see [`SlidePlan`] for repeated calls while
/// dragging).
pub fn vertex_slide(
    mesh: &EditMesh,
    edge: &SlideEdge,
    p: &SlideParams,
) -> Result<EditOutcome, Refusal> {
    SlidePlan::new(mesh, edge, p.preserve, p.max_normal_change_deg)?
        .apply(p.factor, p.even, p.clamp)
}
