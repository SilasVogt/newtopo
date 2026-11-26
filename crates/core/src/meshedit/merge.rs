//! Merge by Distance (SPEC §9.5.7): merge vertices that are closer than a
//! distance. Own design:
//!
//! 1. Clustering: vertices are visited strongest role first (pinned, corner,
//!    feature curve, free; then lower index). Each vertex not yet in a
//!    cluster starts one and takes every free-to-join vertex closer than the
//!    distance to it (spatial hash with the distance as the cell size), so
//!    every member is within the distance of its cluster's survivor and
//!    nothing chains across a dense row.
//! 2. All clusters are applied together (closing a crack needs both ends of
//!    each glued edge at once): members are replaced by their survivor,
//!    polygons that lose corners shrink, polygons left with fewer than three
//!    corners disappear, a polygon pinched at a repeated corner splits, and
//!    two polygons left on the same corners with opposite winding (a closed
//!    pocket) both go.
//! 3. The result is checked around every changed polygon: manifold edges
//!    and vertex fans, no duplicate faces, no degenerate, concave or flipped
//!    polygon, normal limit. Clusters around a failure are dropped (and
//!    reported) and the rest is tried again.
//! 4. With centroid placement, free survivors then move to the mean of their
//!    cluster if that is valid. The rounds repeat until nothing merges, so
//!    applying the tool to its own result changes nothing.

use super::mesh::{area_vec, ekey, single_fan};
use super::work::{Prov, VClass, WPoly, Work, polygon_shape_ok, same_cycle};
use super::{EditOutcome, EditReport, Preserve, check_input};
use crate::meshedit::mesh::EditMesh;
use crate::ops::Refusal;
use glam::DVec3;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// Which vertex pairs may merge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MergeMode {
    /// Any pair closer than the distance.
    All,
    /// Only pairs joined by an edge (collapses short edges).
    Connected,
    /// Only pairs that share no polygon (welds duplicates and cracks, never
    /// collapses an existing polygon).
    Unconnected,
}

/// Where a cluster's surviving vertex ends up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MergePosition {
    /// The mean of the cluster, if the survivor is free (no feature edge)
    /// and the move is valid; otherwise the survivor stays.
    Centroid,
    /// The survivor (the cluster's strongest vertex) stays where it is.
    Survivor,
}

/// The vertices Merge by Distance works on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VertexScope {
    /// Every vertex of the mesh.
    All,
    /// These vertices (input indices, any order, duplicates ignored).
    Vertices(Vec<u32>),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MergeParams {
    /// Vertices closer than this merge (body units, above zero).
    pub distance: f64,
    pub mode: MergeMode,
    pub position: MergePosition,
    /// With a vertex scope: also merge scope vertices with vertices outside
    /// it (otherwise both vertices of a pair must be in the scope).
    pub with_unselected: bool,
    pub preserve: Preserve,
    pub max_normal_change_deg: f64,
}

impl MergeParams {
    /// Defaults scaled to the mesh: distance 0.01 % of the bounding-box
    /// diagonal.
    pub fn for_mesh(mesh: &EditMesh) -> MergeParams {
        MergeParams {
            distance: 1e-4 * mesh.diagonal().max(1e-12),
            ..MergeParams::default()
        }
    }
}

impl Default for MergeParams {
    /// The [`MergeParams::for_mesh`] values for a mesh with a unit diagonal.
    fn default() -> Self {
        MergeParams {
            distance: 1e-4,
            mode: MergeMode::All,
            position: MergePosition::Centroid,
            with_unselected: false,
            preserve: Preserve::default(),
            max_normal_change_deg: 60.0,
        }
    }
}

const MAX_ROUNDS: usize = 32;

pub fn merge_by_distance(
    mesh: &EditMesh,
    scope: &VertexScope,
    p: &MergeParams,
) -> Result<EditOutcome, Refusal> {
    merge_preferring(mesh, scope, p, None)
}

/// [`merge_by_distance`] where `prefer`, if given, is visited first and so
/// survives its cluster whatever its role (Target Merge's weld: the target
/// stays).
pub(crate) fn merge_preferring(
    mesh: &EditMesh,
    scope: &VertexScope,
    p: &MergeParams,
    prefer: Option<u32>,
) -> Result<EditOutcome, Refusal> {
    check_input(mesh)?;
    if !(p.distance > 0.0 && p.distance.is_finite()) {
        return Err(Refusal::new("The merge distance must be above zero"));
    }
    if !(p.max_normal_change_deg > 0.0 && p.max_normal_change_deg < 90.0) {
        return Err(Refusal::new(
            "The normal-change limit must be between 0° and 90°",
        ));
    }
    let nv = mesh.positions.len();
    let mut in_scope = vec![false; nv];
    match scope {
        VertexScope::All => in_scope.fill(true),
        VertexScope::Vertices(vs) => {
            if vs.is_empty() {
                return Err(Refusal::new("Select one or more vertices first"));
            }
            for &v in vs {
                if v as usize >= nv {
                    return Err(Refusal::new(format!("Vertex {v} is not in this mesh")));
                }
                in_scope[v as usize] = true;
            }
        }
    }
    let with_unselected = p.with_unselected && matches!(scope, VertexScope::Vertices(_));
    let before = mesh.stats();
    let mut w = Work::new(mesh, p.preserve.mask());
    w.min_cos = p.max_normal_change_deg.to_radians().cos();
    let (mut merged, mut moved) = (0usize, 0usize);
    let mut skipped: BTreeMap<String, usize> = BTreeMap::new();
    let mut skipped_total = 0;
    for _ in 0..MAX_ROUNDS {
        let clusters = clusters(&w, &in_scope, with_unselected, p, prefer);
        if clusters.is_empty() {
            break;
        }
        let (applied, refused) = apply_clusters(&mut w, &clusters);
        // Only the last round's refusals are reported: earlier ones were
        // tried again against the merged mesh.
        skipped.clear();
        skipped_total = refused.len();
        for r in refused {
            *skipped.entry(r).or_default() += 1;
        }
        if applied.is_empty() {
            break;
        }
        for c in &applied {
            merged += c.members.len();
        }
        if p.position == MergePosition::Centroid {
            moved += centre(&mut w, &applied);
        }
    }
    let (mut out, map) = w.finish();
    // Welding two charts together makes the glued edges UV seams.
    out.flag_seams();
    let after = out.stats();
    let vertex_map = map[..nv].to_vec();
    let mut selected_vertices: Vec<u32> = (0..nv)
        .filter(|&v| in_scope[v])
        .filter_map(|v| final_index(&w, &map, v as u32))
        .collect();
    selected_vertices.sort_unstable();
    selected_vertices.dedup();
    let skipped: Vec<String> = skipped
        .into_iter()
        .map(|(r, n)| format!("{n} cluster(s) not merged: {r}"))
        .collect();
    let message = if merged == 0 && skipped.is_empty() {
        "No vertices closer than the merge distance".to_string()
    } else {
        format!(
            "Removed {merged} vertex(es){}{}",
            if moved > 0 {
                format!(", {moved} moved to their cluster centre")
            } else {
                String::new()
            },
            if skipped_total == 0 {
                String::new()
            } else {
                format!("; {skipped_total} cluster(s) skipped")
            }
        )
    };
    Ok(EditOutcome {
        vertex_map,
        mesh: out,
        selection: Vec::new(),
        selected_vertices,
        report: EditReport {
            before,
            after,
            reached_target: true,
            message,
            skipped,
        },
    })
}

/// Output index of input vertex `v`, following merges to the survivor.
fn final_index(w: &Work, map: &[Option<u32>], v: u32) -> Option<u32> {
    if let Some(m) = map[v as usize] {
        return Some(m);
    }
    w.aliases
        .iter()
        .find(|(_, a)| a.contains(&v))
        .and_then(|(&s, _)| map[s as usize])
}

#[derive(Clone, Debug)]
struct Cluster {
    survivor: u32,
    /// Merged into the survivor, ascending.
    members: Vec<u32>,
}

/// Strength of a vertex role: which vertex of a cluster survives.
fn rank(w: &Work, x: u32) -> u8 {
    if w.pinned[x as usize] {
        return 3;
    }
    match w.vclass(x) {
        VClass::Corner => 2,
        VClass::Curve(_) => 1,
        VClass::Free => 0,
    }
}

fn shares_polygon(w: &Work, a: u32, b: u32) -> bool {
    w.vpolys[a as usize]
        .iter()
        .any(|&q| w.polys[q as usize].verts.contains(&b))
}

/// May `u` merge into the survivor `s` (feature rules and mode)?
fn may_join(w: &Work, u: u32, s: u32, mode: MergeMode) -> bool {
    if w.pinned[u as usize] {
        return false;
    }
    let edge = !w.edge_polys(u, s).is_empty();
    let shared = edge || shares_polygon(w, u, s);
    match mode {
        MergeMode::Connected if !edge => return false,
        MergeMode::Unconnected if shared => return false,
        _ => {}
    }
    if shared {
        // A merge inside the surface: the feature rules of every tool
        // (feature-curve vertices only along their curve, corners stay).
        w.merge_allowed(u, s)
    } else {
        // A weld across a crack or of a duplicate: any vertex but a pinned
        // one; it moves less than the merge distance and its feature edges
        // carry over to the survivor.
        true
    }
}

fn clusters(
    w: &Work,
    in_scope: &[bool],
    with_unselected: bool,
    p: &MergeParams,
    prefer: Option<u32>,
) -> Vec<Cluster> {
    let d = p.distance;
    let nv = w.pos.len();
    let in_s = |x: u32| (x as usize) < in_scope.len() && in_scope[x as usize];
    // Vertices that take part: alive, and in the scope (or anywhere, if
    // scope vertices may merge with unselected ones).
    let part: Vec<u32> = (0..nv as u32)
        .filter(|&x| w.valive[x as usize] && (with_unselected || in_s(x)))
        .collect();
    let cell = |q: DVec3| -> (i64, i64, i64) {
        let c = q / d;
        (c.x.floor() as i64, c.y.floor() as i64, c.z.floor() as i64)
    };
    let mut grid: HashMap<(i64, i64, i64), Vec<u32>> = HashMap::new();
    for &x in &part {
        grid.entry(cell(w.pos[x as usize])).or_default().push(x);
    }
    let mut order = part.clone();
    order.sort_by_key(|&x| (Some(x) != prefer, std::cmp::Reverse(rank(w, x)), x));
    let mut taken = vec![false; nv];
    let mut out = Vec::new();
    for s in order {
        if taken[s as usize] {
            continue;
        }
        taken[s as usize] = true;
        let ps = w.pos[s as usize];
        let (cx, cy, cz) = cell(ps);
        let mut near: Vec<u32> = Vec::new();
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    // Saturating: a tiny distance can put coordinates at the
                    // ends of the i64 range (the exact distance test below
                    // still decides).
                    let k = (
                        cx.saturating_add(dx),
                        cy.saturating_add(dy),
                        cz.saturating_add(dz),
                    );
                    if let Some(list) = grid.get(&k) {
                        near.extend(list.iter().copied());
                    }
                }
            }
        }
        near.sort_unstable();
        let mut members = Vec::new();
        for u in near {
            if taken[u as usize]
                || w.pos[u as usize].distance(ps) >= d
                || !(in_s(u) || in_s(s))
                || !may_join(w, u, s, p.mode)
                // Unconnected never collapses a polygon, also not one that
                // two members share.
                || p.mode == MergeMode::Unconnected
                    && members.iter().any(|&o| shares_polygon(w, u, o))
            {
                continue;
            }
            taken[u as usize] = true;
            members.push(u);
        }
        if !members.is_empty() {
            out.push(Cluster {
                survivor: s,
                members,
            });
        }
    }
    out
}

/// Pieces of a polygon after replacing vertices by their survivors: drop
/// repeated neighbours, split where a corner repeats further on, keep
/// pieces with at least three corners.
fn rewrite(verts: &[u32], to: &HashMap<u32, u32>) -> Vec<Vec<u32>> {
    let mut c: Vec<u32> = verts.iter().map(|x| *to.get(x).unwrap_or(x)).collect();
    c.dedup();
    while c.len() > 1 && c.first() == c.last() {
        c.pop();
    }
    split_cycle(c)
}

fn split_cycle(c: Vec<u32>) -> Vec<Vec<u32>> {
    let n = c.len();
    for i in 0..n {
        if let Some(j) = (i + 1..n).find(|&j| c[j] == c[i]) {
            let a: Vec<u32> = c[i..j].to_vec();
            let mut b: Vec<u32> = c[j..].to_vec();
            b.extend_from_slice(&c[..i]);
            let mut out = split_cycle(a);
            out.extend(split_cycle(b));
            return out;
        }
    }
    if n >= 3 { vec![c] } else { Vec::new() }
}

/// Apply all clusters together, dropping clusters around failures until
/// the rest is valid. Returns the applied clusters and one reason per
/// dropped cluster.
fn apply_clusters(w: &mut Work, clusters: &[Cluster]) -> (Vec<Cluster>, Vec<String>) {
    let mut active: Vec<bool> = vec![true; clusters.len()];
    let mut refused = Vec::new();
    loop {
        let live: Vec<usize> = (0..clusters.len()).filter(|&i| active[i]).collect();
        if live.is_empty() {
            return (Vec::new(), refused);
        }
        let mut to: HashMap<u32, u32> = HashMap::new();
        let mut owner: HashMap<u32, usize> = HashMap::new();
        for &i in &live {
            owner.insert(clusters[i].survivor, i);
            for &m in &clusters[i].members {
                to.insert(m, clusters[i].survivor);
                owner.insert(m, i);
            }
        }
        let mut trial = w.clone();
        let changed = commit(&mut trial, &to);
        let survivors: BTreeSet<u32> = live.iter().map(|&i| clusters[i].survivor).collect();
        match check(&trial, w, &changed, &survivors) {
            Ok(()) => {
                *w = trial;
                // Input vertices merged into each survivor, including the
                // ones merged into its members in earlier rounds.
                for &i in &live {
                    let mut add = Vec::new();
                    for &m in &clusters[i].members {
                        add.push(m);
                        add.extend(w.aliases.remove(&m).unwrap_or_default());
                    }
                    w.aliases
                        .entry(clusters[i].survivor)
                        .or_default()
                        .extend(add);
                }
                return (live.iter().map(|&i| clusters[i].clone()).collect(), refused);
            }
            Err((bad, reason)) => {
                // Drop every cluster whose survivor or member is on a
                // failing polygon or vertex.
                let mut drop: BTreeSet<usize> = BTreeSet::new();
                for x in bad {
                    if let Some(&i) = owner.get(&x) {
                        drop.insert(i);
                    }
                }
                if drop.is_empty() {
                    // Should not happen: every change involves a cluster.
                    drop.extend(live.iter().copied());
                }
                for i in drop {
                    active[i] = false;
                    refused.push(reason.clone());
                }
            }
        }
    }
}

/// Rewrite every polygon of a merged vertex. Returns the ids of the
/// polygons that changed (alive or not).
fn commit(w: &mut Work, to: &HashMap<u32, u32>) -> Vec<u32> {
    let mut polys: BTreeSet<u32> = BTreeSet::new();
    for &m in to.keys() {
        polys.extend(w.vpolys[m as usize].iter().copied());
    }
    let mut changed: Vec<u32> = Vec::new();
    for &q in &polys {
        let pieces = rewrite(&w.polys[q as usize].verts, to);
        changed.push(q);
        if pieces.is_empty() {
            w.polys[q as usize].alive = false;
            continue;
        }
        let parent = w.polys[q as usize].clone();
        for (i, piece) in pieces.into_iter().enumerate() {
            if i == 0 {
                let p = &mut w.polys[q as usize];
                p.verts = piece;
                p.touched = true;
            } else {
                w.polys.push(WPoly {
                    verts: piece,
                    touched: true,
                    ..parent.clone()
                });
                changed.push((w.polys.len() - 1) as u32);
            }
        }
    }
    // Pockets: a changed polygon and another polygon on the same corners
    // with opposite winding both go.
    let mut by_set: BTreeMap<Vec<u32>, Vec<u32>> = BTreeMap::new();
    let mut near: BTreeSet<u32> = BTreeSet::new();
    for &q in &changed {
        if w.polys[q as usize].alive {
            for &x in &w.polys[q as usize].verts {
                near.extend(w.vpolys[x as usize].iter().copied());
            }
            near.insert(q);
        }
    }
    for &q in &near {
        let p = &w.polys[q as usize];
        if p.alive {
            let mut k = p.verts.clone();
            k.sort_unstable();
            by_set.entry(k).or_default().push(q);
        }
    }
    for (_, qs) in by_set {
        if qs.len() == 2 {
            let (a, b) = (
                &w.polys[qs[0] as usize].verts,
                &w.polys[qs[1] as usize].verts,
            );
            let rb: Vec<u32> = b.iter().rev().copied().collect();
            if same_cycle(a, &rb) {
                for q in qs {
                    w.polys[q as usize].alive = false;
                    if !changed.contains(&q) {
                        changed.push(q);
                    }
                }
            }
        }
    }
    // Rebuild the vertex → polygon index and liveness.
    for vp in &mut w.vpolys {
        vp.clear();
    }
    w.vpolys.resize(w.pos.len(), Vec::new());
    for q in 0..w.polys.len() as u32 {
        if w.polys[q as usize].alive {
            for &x in &w.polys[q as usize].verts.clone() {
                w.vpolys[x as usize].push(q);
            }
        }
    }
    for x in 0..w.pos.len() {
        w.valive[x] = !w.vpolys[x].is_empty();
    }
    // Edge flags follow their vertices; collapsed edges lose theirs.
    let old: Vec<((u32, u32), u8)> = {
        let mut v: Vec<_> = w.flags.iter().map(|(k, f)| (*k, *f)).collect();
        v.sort_unstable();
        v
    };
    w.flags.clear();
    for ((a, b), f) in old {
        let (a, b) = (*to.get(&a).unwrap_or(&a), *to.get(&b).unwrap_or(&b));
        if a != b {
            *w.flags.entry(ekey(a, b)).or_default() |= f;
        }
    }
    changed.sort_unstable();
    changed
}

/// Validate the changed region of `trial` (`base` is the state before).
/// On failure returns the vertices involved and a reason.
fn check(
    trial: &Work,
    base: &Work,
    changed: &[u32],
    survivors: &BTreeSet<u32>,
) -> Result<(), (Vec<u32>, String)> {
    let mut verts: BTreeSet<u32> = BTreeSet::new();
    for &q in changed {
        // Corners before and after: vertices that lost polygons too.
        verts.extend(
            base.polys
                .get(q as usize)
                .map(|p| p.verts.clone())
                .unwrap_or_default(),
        );
        verts.extend(trial.polys[q as usize].verts.iter().copied());
    }
    // Collapses: a survivor or a pinned vertex left without polygons means
    // a whole piece of the surface collapsed (and a pinned vertex would be
    // lost); vertices of cancelled pockets may go.
    for &x in &verts {
        let gone = trial.vpolys[x as usize].is_empty() && !base.vpolys[x as usize].is_empty();
        if gone && (survivors.contains(&x) || base.pinned[x as usize]) {
            return Err((vec![x], "it would collapse part of the mesh".into()));
        }
    }
    // Geometry of the changed polygons that survive.
    for &q in changed {
        let p = &trial.polys[q as usize];
        if !p.alive {
            continue;
        }
        let pos = trial.poly_pos(&p.verts);
        // The polygon before; a split-off piece compares with the input
        // polygon it descends from.
        let before = match base.polys.get(q as usize) {
            Some(b) => base.poly_pos(&b.verts),
            None => base.poly_pos(&base.src.polygons[p.src as usize].verts),
        };
        let parent_old = area_vec(&before).normalize_or_zero();
        if !polygon_shape_ok(&pos, trial.diag) {
            return Err((
                p.verts.clone(),
                "it would leave a degenerate or concave polygon".into(),
            ));
        }
        let n = area_vec(&pos).normalize_or_zero();
        let r = trial.reference_normal(&p.verts, p);
        if n.dot(r) < trial.min_cos || (parent_old != DVec3::ZERO && n.dot(parent_old) <= 0.0) {
            return Err((p.verts.clone(), "it would flip or tilt a polygon".into()));
        }
    }
    // Manifold edges.
    let mut seen_edges: HashSet<(u32, u32)> = HashSet::new();
    for &q in changed {
        let p = &trial.polys[q as usize];
        if !p.alive {
            continue;
        }
        let n = p.verts.len();
        for i in 0..n {
            let (a, b) = (p.verts[i], p.verts[(i + 1) % n]);
            if !seen_edges.insert((a, b)) {
                continue;
            }
            let (mut same, mut any) = (0, 0);
            for &r in &trial.vpolys[a as usize] {
                let rv = &trial.polys[r as usize].verts;
                if super::work::has_directed(rv, a, b) {
                    same += 1;
                    any += 1;
                } else if super::work::has_directed(rv, b, a) {
                    any += 1;
                }
            }
            if same > 1 || any > 2 {
                return Err((
                    around(trial, &[a, b]),
                    "it would make a non-manifold edge".into(),
                ));
            }
        }
    }
    // Manifold fans and no duplicate faces.
    for &x in &verts {
        let ps = &trial.vpolys[x as usize];
        if ps.is_empty() {
            continue;
        }
        let mut links = Vec::new();
        let mut sets: HashSet<Vec<u32>> = HashSet::new();
        for &q in ps {
            let v = &trial.polys[q as usize].verts;
            let n = v.len();
            if let Some(i) = v.iter().position(|&y| y == x) {
                links.push((v[(i + n - 1) % n], v[(i + 1) % n]));
            }
            let mut k = v.clone();
            k.sort_unstable();
            if !sets.insert(k) {
                return Err((
                    around(trial, &[x]),
                    "it would make a duplicate polygon".into(),
                ));
            }
        }
        if !single_fan(&links) {
            return Err((
                around(trial, &[x]),
                "it would make a non-manifold vertex".into(),
            ));
        }
    }
    Ok(())
}

/// `xs` and every corner of their polygons.
fn around(w: &Work, xs: &[u32]) -> Vec<u32> {
    let mut out: BTreeSet<u32> = xs.iter().copied().collect();
    for &x in xs {
        for &q in &w.vpolys[x as usize] {
            out.extend(w.polys[q as usize].verts.iter().copied());
        }
    }
    out.into_iter().collect()
}

/// Move free survivors to the mean of their cluster where that is valid.
fn centre(w: &mut Work, applied: &[Cluster]) -> usize {
    let mut n = 0;
    for c in applied {
        let s = c.survivor;
        if !w.valive[s as usize] || w.vclass(s) != VClass::Free {
            continue;
        }
        let all: Vec<u32> = std::iter::once(s)
            .chain(c.members.iter().copied())
            .collect();
        let target = all.iter().map(|&x| w.pos[x as usize]).sum::<DVec3>() / all.len() as f64;
        if target.distance(w.pos[s as usize]) <= 1e-15 * w.diag {
            continue;
        }
        let provs: Vec<Prov> = all.iter().map(|&x| w.prov[x as usize].clone()).collect();
        if w.try_move(s, target, Prov::mean(&provs)) {
            n += 1;
        }
    }
    n
}
