# Engine notes (worker: engine)

For the lead to fold into `docs/PROVENANCE.md` and `docs/QUESTIONS.md`.

## Provenance

| Area | Source |
| --- | --- |
| Mock kernel surfaces/curves (plane, cylinder, cone, sphere, torus parameterisations, closest point by Gauss–Newton) | Textbook differential geometry; own code |
| Mock display tessellation | Constrained Delaunay via `spade` (crate docs); adaptive chord subdivision; own code |
| Planar arrangement, face extraction by half-edge "next = clockwise neighbour of twin" | Standard doubly-connected edge list technique (de Berg, Cheong, van Kreveld, Overmars, *Computational Geometry*, ch. 2); own code |
| Orientation tests | Shewchuk's adaptive predicates via the `robust` crate |
| Seam gluing and topology guard (χ = 2 − b per region) | SPEC §4.1; Euler characteristic of surfaces (standard topology); own code |
| Plane sections for Slice | Marching squares on a sampled signed distance + Newton projection onto the plane; own code |
| Canonical angle | Bisection on the signed angle of the radial direction / normal around the axis; own code |
| Bridging (§4.6 step 6) | SPEC rules; Kruskal-style batch (union–find) for joining loops; uniform-grid segment index; own code |
| Edge collapse link condition | Dey, Edelsbrunner, Guha, Nekhayev, "Topology preserving edge contraction" (1999); own code |
| Largest-remainder count distribution | Hamilton's method (standard apportionment); own code |
| TEMP auto-fill (to be replaced by the fill worker) | Ear clipping (Meisters' two-ears theorem; Eberly, "Triangulation by Ear Clipping", public note) + greedy convex pairing; own code |
| Loop / ring / strip selection | Standard quad-mesh traversal; own code |

## Questions → decisions

| # | Question | Decision |
| --- | --- | --- |
| 1 | Iso / split `value` and `seed` units (§4.2, §10 example uses 0.5 / 0.25)? | Normalised 0..1 of the face's `SurfaceInfo` u / v range (`eval::lines::iso_abs`). |
| 2 | Which patches get the implicit grid (§4.1)? | Patches of faces whose outer wire has only seam/degenerate edges, and which are not bounded by a split line. Rule id `0xFF00_0000 | face`. |
| 3 | §4.2 minimum count 3: also for count 0? | Clamp only counts 1–2 in a direction the patch wraps around; 0 still means "no lines". |
| 4 | Bridging chord rule "touches a repeated vertex" can't fix anything on its own (the touched vertex stays repeated). | A same-walk chord must *separate* the occurrences of some repeated vertex (one strictly inside, one strictly outside the chord's walk interval). Loops are joined first, then separations. |
| 5 | Bridging cost with many holes per face. | Candidates are the 12 nearest occurrences (metric-scaled UV distance) per occurrence; exact 3D surface-path lengths rank them; chords are chosen in non-crossing batches (Kruskal for joins). Equivalent to strict shortest-first except in rare ties. |
| 6 | Torus with a hole after removing every line: the guard dissolves the u-seam (cylinder with a hole is a disk with two holes) but the region then wraps the tube with vertices only on the v-seam, so every polygon circles the tube. | Guard extension: a region that borders both sides of a kept seam keeps the other seam too. The §4.6 AC then validates. |
| 7 | Removing a generated line that lies on a seam (start-on-seam or canonical angle on the seam). | The seam comes back under it and goes through the guard like any seam. Generated-edge removals are applied on the per-face glued graph so the guard sees them (two-pass evaluation); other edits apply on the half-edge mesh in order. |
| 8 | Vertices on a kept seam with no line ending there. | Seam-only, dropped like any seam-only vertex; the kept seam loop then gets placeholders. |
| 9 | Two different curves joining the same two vertices (a through-iso closed over two faces, a line chord against a CAD arc). | Each curved piece of such a pair gets a midpoint placeholder (`Placeholder{lp, 0}` for lines, `i = 10000+…` for CAD arcs). |
| 10 | Placeholder phase on a loop with no remaining vertex. | Phase reference via the loop's best-fit plane (Newell normal) and centroid; overrides from `phase_overrides`. |
| 11 | Canonical angle of a torus' minor (v) direction. | Where the surface normal points along the face's phase reference (the outer equator by default). |
| 12 | Slice spacing / radial count. | Stack: planes at (k+1)/(N+1) of the scope extent. Radial: N planes through the axis at (k+½)·π/N from the phase reference (2N radial lines, never on a default seam). No straight edge and no shared axis: longest bounding-box axis. |
| 13 | Connect chains across patches: automatic A/B reconciliation. | Not automatic: per-patch bend class, `flipped` list per patch; lines share their index across patches and join where their positions meet (e.g. both halves of a split cylinder). |
| 14 | Connect on a selection mixing flat and curved patches. | Flat ones are skipped with a message; refused only if every patch is flat. |
| 15 | §11 property "validates or refused": what about spread ops whose result can't validate? | `Scene::apply` evaluates every op that changes a body and refuses it if validation gains issues (message from `scene::issue_reason`). `apply_unchecked` exists for debugging. |
| 16 | Deleting a selection of edges. | New `Op::RemoveEdges` (one undo step; only the final result must validate). |
| 17 | Flip test on large curved n-gons. | The 3D Newell-vs-normal test only runs on polygons whose corner normals are within ~37° of their mean; the evaluator also checks every final polygon's orientation in its UV chart. |
| 18 | Collapse tie (§4.4 "surface-path midpoint"). | Face–Face: midpoint of the edge's UV path; same CAD edge: midpoint t; otherwise the lower key's anchor. |
| 19 | Weld between vertices that share neither an edge nor a polygon. | Refused ("don't share a polygon"). Edits touching pole vertices are refused. |
| 20 | Remove CAD edge: flattened domain for the merged polygon (§7 step 1 LSCM). | Best-fit plane projection (no LSCM yet); allowed only for dihedral < 5°. |
| 21 | Cut across faces. | Crossing point on the shared edge minimising the 3D length (sampled + ternary search), not a tessellation shortest path. Join vertices must share a face or an edge. |
| 22 | PatchId bounds when several splits border one side. | Nearest split on that side among those bordering the patch (largest value below / smallest above); region numbers by the lexicographically smallest boundary point. |
| 23 | Through-iso tolerances. | G1 = dihedral < 5°, direction match within 15° (both from the spec). |
| 24 | Canonical vertex key among aliases. | Smallest key excluding transient seam-crossing keys. |
| 25 | Mock `.bbrep` bytes. | `NTMOCKBREP1\n` + JSON of `MockBody` (`serde_json` with `float_roundtrip`). |
| 26 | `cad/source.sha256` layout. | One line per body: `<body id> <sha256 hex>`. |
| 27 | Edges that join the same two vertices after dropping seam vertices / collapsing poles (sphere lunes between bare meridians, a chord against an arc). | Curved ones are split at their midpoint (`HMesh::split_parallel_edges`, placeholder keys `i ≥ 20000`) before bridging. |

## Known gaps

- Performance: per-body dirty tracking only. Release numbers on mock plates:
  10 faces 8 ms, 31 faces 61 ms, 106 faces (100 holes) ≈ 410 ms full; a
  one-patch change re-evaluates the whole body. Budgets (16 ms / 300 ms for
  2,000 faces) need per-face caching of stages 1–3 and per-region caching
  of bridging/fill.
- The validation gate refuses some ops whose result the evaluator can't
  make valid. Seeded property run, 200 cases (release): 469 accepted, 89
  refused, 22 of them by the gate; the rest are tool rules (pole edits,
  missing edges, confirmations). Remaining gate refusals: a slice with one
  or two planes through a planar ring, plate or sphere (polygons whose
  vertices end up collinear), split-through iso on a torus, removing a
  meridian edge on a bare torus.
- Link, symmetry and copy/paste rules (§6.11–§6.13) are not implemented.
