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
