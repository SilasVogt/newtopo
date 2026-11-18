# Open questions and decisions

Questions the spec doesn't answer, with the decision taken (SPEC §0 rule 2).
Project-level decisions are listed here; the detailed per-area decisions
live next to the code they shaped:

- Engine (document, evaluator, tools): [`notes/engine.md`](notes/engine.md#questions--decisions) (39 decisions)
- CAD kernel and import: [`notes/occt.md`](notes/occt.md#decisions-questions-the-spec-does-not-answer)
- Auto-fill, export, bridge: [`notes/export.md`](notes/export.md#decisions-questions-the-spec-doesnt-answer)
- App and renderer: [`notes/app.md`](notes/app.md#app-decisions-spec-gaps)

| # | Question | Decision |
| --- | --- | --- |
| 1 | §1 lists installers as out of scope until Silas decides. | Silas asked for installable builds on Windows, macOS and Linux (2026-11-14). Ship per-OS installers (`docs/INSTALL.md`), unsigned for now. |
| 2 | §13.1 Open source or commercial? | Silas (2026-11-14): MIT licence, public repo. OCCT stays dynamically linked (LGPL-2.1 + exception); its notices ship in every package. The Blender add-on is GPL-3.0-or-later, as Blender add-ons must be. |
| 3 | §13.2 Name. | `newtopo` stays the working name. |
| 4 | §13.3 Connect across unselected neighbours. | Spec default: lines stay inside the selection. Lines continue across tangent *selected* patches (chains). |
| 5 | §13.4 Symmetry planes. | Spec default: world planes through the origin. |
| 6 | §13.5 FBX writer. | Own binary FBX 7.4 writer shipped in v0.1 (`docs/fbx-subset.md`). |
| 7 | §13.6 Auto-fill default. | On for new scenes. |
| 8 | Which OCCT version? | 8.0.1, built from source with the same recipe on every OS (`scripts/build-occt.*`). The shim avoids 8.0-only APIs; 7.8 is untested. |
| 9 | macOS target. | Apple silicon only (`macos-15` runners). Intel would need a second OCCT build. |
| 10 | Picking. | CPU ray casting over the display tessellation with a BVH; the GPU id buffer of §5.2 is a follow-up (the API allows swapping it in). |
| 11 | Selection as an undo step (§4.7). | Not done: selection is UI state and stays out of document snapshots. |

## Mesh mode (SPEC §9.5)

Decisions taken while building the engine side of the mesh tools
(`core/src/meshedit/`). The UI integration is still to do.

| # | Question | Decision |
| --- | --- | --- |
| 12 | Section number. | Started as §9.4; main then added §9.4 Update notice, so mesh mode is §9.5 (still contiguous, no existing section renumbered). |
| 13 | Where do decimated vertices go: the optimal quadric position or an existing vertex? | Existing vertex (subset placement). Every result vertex stays exactly on the CAD surface and keeps its exact CAD normal and UV; the cost is a slightly higher error than optimal placement. |
| 14 | What does the decimate target count? | Triangles (an n-gon counts n − 2), the usual game budget. Quads count as two, so joining triangle pairs never changes the count. Not reaching it is reported, not refused. |
| 15 | Normals after edits. | Unchanged corners keep the baked CAD normal; corners of changed polygons look the normal up by (vertex, CAD face); moved vertices interpolate it along the edge they moved on; geometric normal as the last fallback. |
| 16 | UV seams: optional? | Always preserved when UVs exist (moving a vertex across a seam would corrupt the UVs). Only CAD and hard edges have Preserve toggles. Today's bakes carry no UVs (§9.2 is not implemented), so this only matters later. |
| 17 | Which vertices are pinned at bake? | CAD vertices and poles (keys `Cad` / `Pole`, anchor `CadVertex`). They are never moved or removed. A CAD circle exported as two arcs therefore keeps its two arc-end vertices. |
| 18 | Does Cleanup weld vertices that aren't connected (open cracks)? | No. Only vertices that share a polygon merge, so the topology never changes; baked meshes are watertight. Welding open seams is Merge by Distance (§9.5.7). |
| 19 | Planar mode output. | N-gons it creates are refilled with the §7 decomposition in their best-fit plane, then triangle pairs are joined: the result is tris and quads only. |
| 20 | Resample with a count on several runs. | The count applies to each run separately. An open run counts both ends; a closed run counts its vertices. |
| 21 | Resample of curved runs. | New positions lie on the old polyline (the true CAD curve is gone after bake), so repeated resampling of a curve slowly cuts its corners, like resampling any polyline. Straight runs are exactly idempotent. |
| 22 | What does "all" select? | All feature runs (open boundaries, CAD/hard edges that are preserved, UV seams); not every edge loop of the mesh. On a mesh without features it is refused with a reason. |
| 23 | Rules after Finish. | Locked, kept read-only in the scene so "Back to rules" can re-evaluate them (discarding mesh edits after a confirmation). Suggested storage for the baked mesh: `mesh/<body>.json` in the `.ntopo` with a format bump. Left for the integration. |
| 24 | Export of a baked body. | `EditMesh` has everything export needs (positions, polygons, per-corner normals, hard flags, groups); the export path still takes `OutputMesh`. The integration needs a small adapter (or a `VertexKey` variant for baked vertices). |
| 25 | Section numbers for the two added tools. | Vertex Slide §9.5.6 and Merge by Distance §9.5.7; Testing moved from §9.5.6 to §9.5.8. Nothing outside §9.5 was renumbered (keeps the `fix/user-testing` merge easy). |
| 26 | Vertex Slide: what does factor 1 do? It would make a zero-length edge. | The vertex merges into the neighbour it reached (it "lands"), with the usual merge checks; refused if that merge isn't allowed. Factors just short of 1 are plain moves. |
| 27 | Vertex Slide: refuse or clamp an invalid factor? | Clamp to the largest valid factor (bisection, reported, `reached_target` false), so a drag stops at the limit instead of failing; refuse only if no movement at all is possible. |
| 28 | Vertex Slide: "Even" reference length; Blender-style "Flipped". | Even moves every vertex by the factor times the shortest slide edge, so factor 1 never overshoots an edge. Flipped (measure from the far end) is left out: at factor 0 it would jump the vertices. |
| 29 | Vertex Slide: override for feature vertices. | The existing Preserve CAD / hard toggles are the override (off = those edges are not features). Boundaries, UV seams and pinned vertices have no override. |
| 30 | Vertex Slide: a polygon around the vertex is already past the normal limit or concave (happens in bakes, e.g. slivers on hole walls). | It may change but not get worse (normal no further from the CAD normal, non-zero area, same orientation); otherwise one bad polygon would block every slide near it. |
| 31 | Vertex Slide: several vertices sliding into each other. | A vertex whose chosen neighbour also slides is skipped (explicit edges) or that edge is not a candidate (direction hint), so "lands on the neighbour" stays well defined. |
| 32 | Merge by Distance: the "unconnected-only" option. | Three modes: All, Connected (only along edges, like a weld modifier's connected mode) and Unconnected (only pairs that share no polygon: welds duplicates and cracks, never collapses a polygon). Pairs sharing a polygon but no edge (quad diagonals) merge only in All. |
| 33 | Merge by Distance: transitive clusters (union-find) or bounded ones? | Bounded: a survivor takes only vertices within the distance of itself, strongest role first, so a finely sampled row can't collapse into one point. Rounds repeat until nothing merges, which also makes the tool idempotent with centroid placement. |
| 34 | Merge by Distance: may it change the topology? | Only by welding open boundaries (that is its purpose); merges inside the surface keep it, and every result stays manifold. This is the one exception to §9.5.2's topology rule. |
| 35 | Merge by Distance: feature rules. | Pinned vertices never disappear. Merges inside the surface follow §9.5.2 (feature-curve vertices only along their curve, corners never). Welds of vertices that share no polygon may drop any non-pinned vertex, because duplicates usually sit on boundaries and feature ends; the survivor inherits the edge flags. Centroid placement moves free survivors only. |
| 36 | Merge by Distance: default distance. | 0.01 % of the bounding-box diagonal, the same as Cleanup's merge distance. |
| 37 | Normal limit: per corner or mean (review finding)? | Mean of the corner CAD normals, as the code always did; per corner would reject most large polygons on curved faces. SPEC wording made explicit. |
| 38 | Resample: a count below the fixed corners, or a huge count, with several runs. | Skipped for that run (reported), refused only if no run can be resampled, like any other failing run. One run gets at most 10 000 edges (keeps the matching tables bounded). |
