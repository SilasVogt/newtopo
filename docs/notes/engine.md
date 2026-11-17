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

## Round 2

### Confirmations for the app
- `IsoDir::U` is the line of **constant u**: `value` is the u position
  (normalised 0..1 over the face's `SurfaceInfo::u_range`), `seed` is the v
  position of the click (normalised over `v_range`). `IsoDir::V` likewise.
- The document `BodyId` **equals** the kernel `BodyId`: `add_body` /
  `add_kernel_body` use the kernel's id, `Scene::load` remaps the saved ids
  to the freshly loaded kernel ids (`Document::remap_bodies`), and
  `replace_body` gives the new body its kernel id. So
  `scene.kernel.project_to_face(body, …)` with a document id is correct.
- `Op::SetSelection` was skipped: selection lives in the app, and making
  it an undo step would put UI state in the document snapshots. The app can
  keep its own selection history.

### Decisions
| # | Question | Decision |
| --- | --- | --- |
| 28 | Link / symmetry for edits (Remove, Collapse, Weld). | Not mirrored: edit keys name generated topology of one face; mapping them needs key-level correspondence. Rules (Connect, Grid, Slice on patches, Iso, Cut) and split lines are mirrored/linked. |
| 29 | Link / symmetry on split patches. | Only whole-face patches are expanded to partners. |
| 30 | Grid mapped to a face whose UV axes are swapped by the mapping. | Counts are swapped; a planar face whose UV axes are rotated (not swapped) gets its own grid in its own UV axes. |
| 31 | Congruence. | Same surface kind and parameters, wire/edge counts, area (0.2 %), sorted edge lengths (0.1 %), and a rigid transform verified by projecting sampled surface points (1e-4 × size) and boundary samples onto the other face both ways. Candidates: preferred transforms (body transform, transforms already found for other faces), a pure translation, PCA-aligned rotations with all sign flips (reflections included). |
| 32 | Copy/paste across bodies. | One consistent transform is preferred for the whole body (`link::match_bodies`); pasted spread rules replace spread rules on the same target patches; phase references follow the mapping (§4.1). |
| 33 | `replace_body`. | Faces matched like paste (identity preferred); rules whose faces all fail to match are dropped; edits are dropped (keyed by the old topology); the old body stays in the kernel so undo works. |
| 34 | Mock `face_classify_uv` "On" band. | 2e-4 × the UV span (the loops are 256-point polylines); needed so boundary points of other parameterisations project exactly. |
| 35 | Connect chains (§6.2) on selections spanning faces. | Each patch's A lines are walked across tangent (G1) shared edges into the other selected faces (`lines::connect_chain`, the through-iso walk restricted to the selection); faces reached take their A lines from the walk, so lines join end to end. Directions are matched in 3D (15°), so no A/B reconciliation is needed. |
| 36 | Polygons whose straight-edge vertex polygon has no area although the curved boundary does (half annulus, cap segment cut by a tangent line). | The most curved edges are split at their midpoint before bridging (`HMesh::split_degenerate_walks`, placeholder keys `i ≥ 120000`). |
| 37 | Slice planes containing a CAD edge or touching a pole line. | Section pieces lying along the face boundary are dropped (they would duplicate boundary curves). |
| 38 | Implicit grid on a closed face cut by split lines (meridian split on a torus, equator split on a sphere). | The implicit grid still applies (`Patch::closed_patch` ignores split bounds). |
| 39 | Bridging join chords. | After Kruskal joins every loop, a second chord per joined pair (different endpoints) splits the region in the same pass (SPEC §4.6 step 6 wording); ranked by metric-UV distance with exact 3D lengths per window of 64. |

### Incremental evaluation (SPEC §4.6 budgets)

Content-keyed caches per body (`eval::cache::EvalCache`, owned by the
Scene): stage 1 (patches) per face keyed by its split lines; stage 3
(arrangement + seam gluing) per face keyed by its patches and generated
lines; bridging + fill per region keyed by the region's canonical walks
(vertex keys, owners, UV paths) and the edges joining its vertices. A hit is
always exact (keys hash the full inputs). Kernel calls in emission and
anchor validation are batched per face. The property test checks that the
cached output equals a full evaluation.

Release timings (one-patch change = Connect/Grid count change through
`Scene::apply`, validation gate included):

| Body | Faces | Full | One-patch change, before → after |
| --- | --- | --- | --- |
| mock plate 2×2 holes | 10 | 8 ms | 9 → 4 ms |
| mock plate 5×5 holes | 31 | 45 ms | 70 → 21 ms |
| mock plate 10×10 holes | 106 | 199 ms | 467 → 92 ms |
| OCCT filleted_box | 26 | 5.5 ms | 3.4–4.1 ms |
| OCCT plate_holes (4×4) | 22 | 29 ms | 92 → 18 ms |
| OCCT flange | 13 | 21 ms | 11 ms |

The remaining cost on the mock 10×10 plate is the top and bottom faces,
each one region with 100 holes that is re-bridged when any hole rim
changes (~25 ms each), plus assembly (15 ms) and emission (8 ms), which
are still per body. No 2,000-face reference body exists yet to measure
the 300 ms full-rebuild budget; extrapolating the mock plates it holds for
bodies with few holes per face and not for faces with hundreds of holes.

## Round 3 (review fixes)

40. `Document::phase_overrides` is written to `scene.json` as a list of
    `[target, point]` records (serde adapter `doc::phase_records`); an
    empty JSON object still reads. FORMAT_VERSION stays 1.
41. Saving writes a temporary sibling (`.<name>.<random>.tmp`), syncs it,
    and renames it over the destination (`tempfile::persist`: `rename(2)`
    on Unix, `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` on Windows). `save`
    and `save_copy` share the path.
42. Loading is bounded: `LoadLimits` (default 4096 entries, 64 MiB
    `scene.json`, 1 GiB per `.bbrep`, 4 GiB total); reads go through
    `take()`, and declared sizes are only used to refuse early.
43. Loaded documents are checked against the CAD topology
    (`Scene::check_document`): every body, face, edge and vertex id in
    rules, split lines, edits, generated keys and phase overrides; duplicate
    bodies or rules; missing layers; non-finite numbers; counts above
    `MAX_RULE_COUNT` (100 000). Rule and split ids inside keys are not
    checked (a dangling one makes an edit dormant, not invalid).
    `try_set_document` applies the same check; `set_document` stays for
    in-process documents and debug-asserts it.
44. Link / symmetry for Cut: one mapped cut per partner transform, through
    `link::map_rule`. A face of the cut without a partner under that
    transform maps to itself when the transformed points stay on it.
45. Link / symmetry for RemoveEdge, Collapse and Weld: keys are mapped by
    geometry on the current output (the transformed ends and middle must
    land on a keyed output edge / vertices), not by rewriting key fields:
    line keys encode faces in `sub` differently per rule kind, so a
    symbolic mapping would be fragile. The transforms come from the link
    sets / mirror maps of the faces of the polygons around the edge (or
    both vertices). An edge on a CAD edge belongs to both faces, so e.g. a
    rim edge also maps through the plate face's own link partner.
    Partners that do not map, or whose copy goes dormant, are skipped and
    counted in the outcome message. Everything is one undo step.
46. Paste takes patches over with the tools' takeover (`Scene::takeover`),
    including Body → Patches conversion with frozen planes; a pasted body
    Slice takes every target patch.
47. Paste maps patches bounded by split lines: split lines are pasted
    first, then each source patch's interior point (a cell sample; the
    patch `sample` lies on its boundary) is transformed and located among
    the target face's new patches.
48. The clipboard keeps the source edits (`Clipboard::edit_list`, the
    `edits` count stays). Join rules and edits are mapped by position
    between the source output (without its edits) and the target output
    (after the pasted rules). Unmapped or dormant ones are reported.
