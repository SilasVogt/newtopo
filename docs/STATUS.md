# Status (v0.2)

What [`SPEC.md`](../SPEC.md) asks for and where the implementation stands.
✅ done and tested · 🟡 partial · ❌ not started.

## Phases (SPEC §12)

| Phase | State | Notes |
| --- | --- | --- |
| P0 Skeleton | ✅ | Workspace, `occt-shim` on OCCT 8.0.1, `CadKernel` + `MockKernel`, reference models, `newtopo-cli info`. CI on Linux, Windows, macOS. |
| P1 Import and view | ✅ | Import helper process with progress/cancel/timeout, import list, heal report, layers, outliner, navigation presets, view cube, selection modes, box/lasso, hide/isolate, command palette, statistics. |
| P2 Document engine | ✅ | Patches, split lines, rules, anchors, keys, arrangement, seams/poles/placeholders, bridging, undo, `.ntopo` save/load, preview, `run`/`validate`/`render`. |
| P3 Core loop | ✅ | Connect, Surface Grid, Slice, shared segment panel, auto-fill, loop/ring/every-other. |
| P4 Precision and cleanup | ✅ | Iso (through, split), Cut, Join, Remove edge, Collapse, Target Weld, Remove CAD edge, dormant edits. The radial menu shipped here was later removed. |
| P5 Repeat and ship | 🟡 | Link, Symmetry, copy/paste rules, OBJ/glTF/FBX export, Blender import test. Link/Symmetry mirror rules and split lines but not edits (remove/collapse/weld). |
| P6 Studio | 🟡 | Batch rename, Blender bridge + add-on, themes, keymap editor, autosave. Quick UV unwrap (§9.2) is not implemented (panel says "coming soon"). Update notice (§9.4): opt-in daily check of this repo's GitHub releases, link only, no self-update. |
| P7 Polish | 🟡 | Crash logs and problem reports (§5.11): rotated log file, panic hook, previous-crash offer, "Report a problem" dialog with a zip, a pre-filled GitHub issue link and an AI agent prompt; nothing is uploaded. From user testing: every mesh change previews until Apply, with a before/after compare (§5.10); auto-fill off by default; Undo/Redo buttons; STEP files with loose vertex tolerances now work. Maya and 3ds Max add-ons written but untested in those hosts. macOS arm64 build ships. User guide with screenshots of every tool not written yet. |

## Mesh mode (SPEC §9.5)

| Part | State | Notes |
| --- | --- | --- |
| Editing mode (§9.5.14) | ✅ | `CAD \| Mesh` switch (`M`), own tool panel and vertex/edge/face selection, the CAD selection kept aside, once-per-session entry warning with "Don't show this again" (Preferences row to turn it back on), amber accent in both themes, viewport badge. No previews or prompts; every edit one undo step; tool values redo the last edit live. |
| Bake and the CAD lock (§9.5.1) | ✅ | Entering bakes on demand (nothing written); the first edit stores the mesh in the document and locks the body's CAD stage; "Discard mesh edits" (undoable) unlocks it. Undo/redo across modes. |
| Storage and output (§9.5.1) | ✅ | `.ntopo` format 2 (`mesh/<body>.json`) with a format 1 migration; statistics, export, the Blender bridge, validation and headless render use the edited mesh. |
| Selection (§9.5.9) | ✅ | Alt-click loops (Shift+Alt adds; also double-click and `L`), Ctrl-click shortest path, Shift-click toggles. With the Maya navigation preset Alt+click orbits, so loops use double-click or `L` there. |
| Vertex Slide, Edge Slide (§9.5.6, §9.5.10) | ✅ | Viewport drag; Edge Slide also from the factor alone. Vertex Slide's factor can be fine-tuned only after a drag (the drag picks the edges). |
| Relax, Space (§9.5.11, §9.5.12) | ✅ | Engine and UI; Stay on CAD surface for every move tool. |
| Target Merge (§9.5.13) | ✅ | Two clicks; merges inside the surface and welds across slits. |
| Merge by Distance, Loop Cleanup, Resample, Decimate | ✅ | Engine (§9.5.3–§9.5.7) and UI. |

## Known gaps

- **Problem reports (§5.11)**: model files go into the report as they are,
  so a STEP header's author or original path is included when the user
  switches the model on. Two app instances share one session marker, so a
  crash in one of them may go unnoticed at the next start. Backtraces of
  Windows builds without `.pdb` files carry no function names.

- **Performance budgets (§4.6)** are met on the reference models (a one-patch change takes 3–21 ms) but not on very large multi-hole faces: a 106-face plate with 100 holes in one face needs ~90 ms per change (budget 16 ms). The 2,000-face rebuild budget is unmeasured (no such reference body yet).
- **Evaluator edge cases** refused by the validation gate instead of producing output: welds/collapses into sphere/cone poles, a single radial slice of a bare torus with auto-fill on, a few collapses that would flip a face. They're refused with a reason, never silently broken.
- **Remove CAD edge** fills the merged polygon in a best-fit plane, not an LSCM flattening (§7 step 1).
- **Connect** doesn't yet flag patches whose direction disagrees with their selected neighbours (the warning tint).
- **Picking** is CPU ray casting, not the GPU id buffer of §5.2.
- **Quick UV unwrap (§9.2)** and xatlas binding: not started. Exports carry no UVs.
- **Mesh mode**: there is no "Finish" per body; the bake happens when entering mesh mode and the first edit locks the body. Mesh-mode selection is not restored by undo/redo (a body whose mesh changes loses its mesh selection). Tool values are percentages of the object's size, not scene units. The CAD lock is per body: a rule that spans an edited and an unedited body can't be changed at all. Mesh tools run on the UI thread with no progress display or cancel: Resample refuses to add more than 100,000 vertices at once (§9.5.5), other tools have no such cap.
- **Density limits and untrusted scenes:** a rule may ask for at most 1000 lines per direction and 300 x 300 crossings, a face at most 600 generated lines across all rules, and a scene at most 200,000 lines. These bound the work per face (worst single face: a 300 x 300 grid, about 5 s and 1.6 GB peak), not per document: a crafted `.ntopo` with many bodies at the limit can still freeze the app for minutes or exhaust memory, and the B-rep inside a `.ntopo` is loaded by OCCT in-process. **Don't open `.ntopo` files from untrusted sources.** The real fix is evaluating off the UI thread with cancel (follow-up).
- **In-process CAD loading:** STEP/IGES files are read in the crash-isolated import helper, but the healed B-rep inside a `.ntopo` is loaded by OCCT inside the app process (SPEC §14 allows this). Scene JSON is size-limited and every reference is validated before use.
- **Fuzzing** of the STEP/IGES path (§11) is covered by malformed-file tests, not a fuzzer.
- **Perf regressions in CI** (criterion, §11): benches exist, the 20% regression gate doesn't.
- **Visual tests** (§11 render diffs): `newtopo-cli render` works; no perceptual-diff suite yet.
- **Installers** are unsigned (SmartScreen / Gatekeeper warnings; see `INSTALL.md`). No Intel macOS build.
- **Blender** add-on and imports are tested headless on Blender 5.2; Maya and 3ds Max add-ons have never run in those hosts.
- **FBX** imports cleanly in Blender and `ufbx`; Autodesk-SDK readers (Maya, Max, Unity, Unreal) are untested.

## Test inventory

`cargo test --workspace` (with `OCCT_DIR` set): 451 passing, 6 ignored
opt-in tests (Blender headless import and bridge, which need `NEWTOPO_BLENDER=1`, and perf timings).

- Engine ACs (§4.6), tool ACs (§6.2–§6.14), API tests (Link 4×4 holes,
  symmetry, copy/paste), OCCT-backed ACs on the reference models (filleted
  box slice, fillet chain, fillet ring through-iso, bottle, plate link) and
  property tests with random op sequences, undo-all and save/load on every
  mock and OCCT model.
- Auto-fill: exhaustive-search equivalence on 10,000 random polygons (≤ 9
  vertices), quads-only on convex even polygons, timing.
- Export goldens for OBJ/GLB/FBX, FBX read-back with `ufbx`, Blender
  headless import of all formats (opt-in).
- OCCT kernel invariants on all 12 reference models; import helper crash,
  hang, timeout and malformed-file handling.
- App: UI tests driven with synthetic input; renderer incl. one headless GPU
  render. Problem reports: scrubbing of home and user name, folder
  stripping, report files matching the toggles (zip read back), issue URL
  cap, prompt without switched-off data, dialog once per caught panic and
  never after a refusal, previous-crash detection, log rotation.
- Mesh mode: tool ACs on synthetic meshes, every mock model baked and run
  through every tool, property tests with random meshes and tool sequences
  (`MESHEDIT_CASES=1000` for a longer soak); the mesh-mode scene (storage,
  lock, migration, export) and UI tests (mode switch, entry warning, lock
  and discard, Alt-click loops, Ctrl-click paths, undo across modes,
  save/open).
- CLI: `info` on every reference model; `run` → `validate` → `export` end to
  end.
