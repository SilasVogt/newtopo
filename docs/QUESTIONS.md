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
| 18 | Does Cleanup weld vertices that aren't connected (open cracks)? | No. Only vertices that share a polygon merge, so the topology never changes; baked meshes are watertight. Welding open seams would be a separate tool. |
| 19 | Planar mode output. | N-gons it creates are refilled with the §7 decomposition in their best-fit plane, then triangle pairs are joined: the result is tris and quads only. |
| 20 | Resample with a count on several runs. | The count applies to each run separately. An open run counts both ends; a closed run counts its vertices. |
| 21 | Resample of curved runs. | New positions lie on the old polyline (the true CAD curve is gone after bake), so repeated resampling of a curve slowly cuts its corners, like resampling any polyline. Straight runs are exactly idempotent. |
| 22 | What does "all" select? | All feature runs (open boundaries, CAD/hard edges that are preserved, UV seams); not every edge loop of the mesh. On a mesh without features it is refused with a reason. |
| 23 | Rules after Finish. | Locked, kept read-only in the scene so "Back to rules" can re-evaluate them (discarding mesh edits after a confirmation). Suggested storage for the baked mesh: `mesh/<body>.json` in the `.ntopo` with a format bump. Left for the integration. |
| 24 | Export of a baked body. | `EditMesh` has everything export needs (positions, polygons, per-corner normals, hard flags, groups); the export path still takes `OutputMesh`. The integration needs a small adapter (or a `VertexKey` variant for baked vertices). |
