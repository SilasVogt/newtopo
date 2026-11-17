# App and renderer notes

Owner: app worker (`crates/app`, `crates/render`, `assets/`). Decisions taken
where SPEC.md is silent, the API the app still needs from the core, and how to
run and check the UI.

## How to run and check

| What | Command |
| --- | --- |
| App (OCCT) | `OCCT_DIR=$HOME/.cache/newtopo-occt/install cargo run -p newtopo [-- file.step\|scene.ntopo]` |
| App (no OCCT, no import) | `cargo run -p newtopo --no-default-features` |
| Real-model end-to-end run + screenshots | `newtopo-testmodels <models>` then `newtopo --shot <dir> --models <models>` |
| App with the synthetic sample loaded | `... -- --sample` |
| Headless UI screenshots (no window) | `cargo run -p newtopo --no-default-features -- --shot <dir>` |
| Real-window self-check: run, capture, quit | `... -- --sample --window-shot <png>` |
| Headless 3D render of the sample | `cargo run -p newtopo-render --example render_demo -- out.png [--preview --wire --flat --ortho --view top]` |
| Tests | `cargo test -p newtopo -p newtopo-render --no-default-features` |

Anything that touches the GPU runs through `~/.agents/bin/gpu-lock`. Set
`NEWTOPO_CONFIG_DIR=<dir>` to keep prefs/keymap/layout/autosaves out of the
user's config folder (tests and agents should always do this).

`--shot` drives the real `App::frame` against an offscreen `egui_wgpu`
renderer with the viewport paint callback installed, so screenshots show
exactly what the window shows (no window, no user settings). `uitest.rs` drives
the UI with synthetic keyboard/mouse events (no GPU) and checks state: mode
keys, palette, click/Shift/Ctrl selection, box and lasso, radial tap vs. hold,
Esc precedence, loops and double-press, middle-drag orbit, and that every
command degrades gracefully with no scene.

## Renderer (`newtopo-render`)

- One `gpu::Renderer` (pipelines + uniforms + offscreen MSAA×4 target) used
  by both the egui paint callback (`egui_viewport`, feature `egui`) and
  `Headless`. The callback renders into the offscreen target in `prepare` and
  blits it into egui's pass in `paint` (gamma-correct for both sRGB and
  non-sRGB swapchains). Colour target is `Rgba8UnormSrgb`; colours in a
  `RenderScene` are sRGB with straight alpha.
- Lines are instanced screen-space quads (width in logical px, AA feather,
  optional dashes); dots are instanced discs with a dark rim. Overlays get a
  depth bias by moving view-space points towards the eye (perspective: scale
  towards the eye so the screen position doesn't change; ortho: shift in z).
  Lines are clipped against the near plane in the vertex shader.
- Layers are uploaded only when their revision changes; revisions come from
  `egui_viewport::next_revision()` (process-unique, so a shared GPU renderer
  never shows stale content).
- Picking is CPU-only for now (`pick.rs`): BVH ray cast over display
  triangles → (body, face, barycentrics, point); screen-space nearest
  edge/vertex with an occlusion test against the BVH. SPEC §5.2 asks for a GPU
  id buffer refined on the CPU; the CPU path answers the same questions and a
  GPU id pass can replace the first-hit ray cast later without API changes.
- `core_scene::render_core_scene_to_png(scene, view, display, up, size, path)`
  is the one-call helper for `newtopo-cli render` (normal or preview mode,
  named views, auto-framing).
- `synth.rs` is an analytic sample (rounded plate with a filleted, chamfered
  boss, plus a torus) with display tessellation, CAD edges, generated lines and
  a coarse quad "output mesh". It powers the sample model, tests and
  screenshots while the engine is not available.
- Matrices: own `look_at_rh` / `perspective_rh` / `orthographic_rh` (depth
  0..1) because glam 0.34 moved them; covered by projection/ray round-trip
  tests.

## App decisions (SPEC gaps)

1. **Engine guard.** Every `Scene` call goes through `docstate::guard`
   (`catch_unwind`), so `todo!()` stubs show a toast instead of aborting.
   A panic inside `apply` marks the document changed and blocks further
   `apply` calls until a new scene is loaded (the scene may be half-modified).
   Remove once the facade is complete.
2. **Kernel**: `docstate::make_kernel()` = `newtopo_kernel::default_kernel()`.
   Without the `occt` feature, import shows "This build has no CAD import".
   **Kernel calls take the kernel's body handle** (`scene.ctx(body).kbody`),
   not the document's `BodyId`: they differ (the kernel numbers bodies per
   process). `AppState::kbody()` maps them.
3. **Face mode selects CAD faces**; ops get every patch of each selected face
   from `Scene::patches` (so split faces work). Picking a single patch of a
   split face needs a UV-containment query (see API requests).
4. **Selection is app-side** and not an undo step (SPEC §4.7 says it should
   be by default; the preference exists but has no effect yet).
5. **Box = left-drag, lasso = right-drag** in every navigation preset
   (Maya's Alt+right zoom still wins because navigation is checked first).
6. **Navigation presets**: Blender (MMB orbit, Shift+MMB pan, Ctrl+MMB zoom),
   Maya (Alt+LMB orbit, Alt/plain MMB pan, Alt+RMB zoom), 3ds Max (Alt+MMB
   orbit, MMB pan, Ctrl+Alt+MMB zoom). Wheel zooms towards the cursor. Plain
   wheel always zooms; Shift/Ctrl+wheel go to an active segment tool.
7. **`IsoDir::U` = the line of constant u** (confirmed in the engine);
   `Op::Iso` takes `value` and `seed` normalised to the face's parameter range
   (0..1). The app normalises the projected uv with `SurfaceInfo::u_range` /
   `v_range`.
8. **Iso preview** samples the iso line via `kernel.face_eval` and clips with
   `face_classify_uv` (96 samples). Through mode can't be previewed across
   faces without an engine query.
9. **Wheel coalescing** (SPEC §4.7, 400 ms): `Scene::apply_coalesced` per
   tick; the frame loop calls `end_coalesce` after 400 ms without ticks.
   Multi-op actions (collapse of several edges, batch rename, Slice on several
   objects, removing edges on several bodies) are grouped the same way
   (`begin_coalesce` … `end_coalesce`) so each is one undo step.
10. **Double-press bindings** run the single command on the first press; on
    the second press the selection from before the first press is restored
    and the double command runs (so `L L` gives every other edge of the loop,
    not of the already-selected loop).
11. **Delete on CAD edges**: first press arms (toast, warning tint), second
    within 1 s opens the confirmation; confirmed → `RemoveCadEdge {confirmed:
    true}`. With several CAD edges selected only the first is removed per
    confirmation.
12. **Hide (H)** is view-only (not saved, not undoable). The outliner eye uses
    `Op::SetBodyVisible`. Layer eye/lock and object lock are app-side until
    ops exist.
13. **Layer create/rename/delete and drag-between-layers** work on the sample
    model; on a real scene they toast "not available yet" (no ops).
14. **Statistics always describe the output mesh** (preview or not); the
    quad/tri/n-gon bar is extra.
15. **Autosave** writes `<scene>.ntopo.autosave` (or
    `<data>/untitled.ntopo.autosave`) every N minutes when dirty, via
    `Scene::save` to the autosave path. The app tracks "dirty" itself
    (revision counter) because `Scene::save` may reset `is_modified`. On start,
    autosaves newer than their scene (from the recent list) are offered for
    recovery. Exit removes the current autosave only when nothing is unsaved;
    closing the window with unsaved work asks Save / Discard / Cancel first.
    Unreadable settings files are kept as `*.json.bak` instead of being
    overwritten with defaults.
16. **Import** runs `run_import` on a worker thread with a progress bar and
    cancel (cancel flag returned from the progress callback). Files with more
    than one body open the import list (Keep/Exclude/Replace, search,
    multi-select, all/none, apply choice to selected rows). Replace imports as
    new with a toast until the engine supports it.
17. **Export** clones the output meshes and runs `core::export::export` on a
    worker thread; unit conversion = scene units → chosen units folded into
    `scale`. Lists fan-fallback polygons and the n-gon note when auto-fill is
    off.
18. **Batch rename** runs on `core::rename` (`preview` for the live list,
    regex and match-case options). Apply is one undo step (grouped
    `Op::RenameBody`s).
19. **Bridge panel** runs on `core::bridge`: `probe` every 3 s on a
    background thread while the panel is open (never blocks the UI), `send_with`
    on a worker thread (scale = scene units → metres, keep-both option), host
    renames reported in a toast. Token from `bridge::generate_token`.
20. **UV panel** shows the options from SPEC §9.2 and a "coming soon" note.
21. **Themes**: `assets/themes/graphite-dark.json` and `paper-light.json`,
    designed for this app (mint accent on graphite; deep green accent on paper).
    Users can drop more theme JSONs into `<config>/themes/`. Overlay colours
    default to the theme's and can be overridden per colour in Preferences.
22. **Fonts**: Noto Sans Regular/Medium vendored in `assets/fonts` (SIL OFL
    1.1, licence alongside). Monospace stays egui's Hack.
23. **Icons** are drawn with the egui painter in `icons.rs` (no icon font, no
    third-party art). The app icon is the existing `assets/icon.svg`
    rasterised once to `assets/icon.png` (`rsvg-convert -w 256`); the in-app
    mark is a separate painter-drawn logo. Unify them when the product name
    and brand are decided.
24. **Default keymap** is SPEC §5.8 exactly; extra commands use free combos
    (Ctrl+N/O/S/Shift+S/Shift+I/E/Q, Ctrl+A select all, Alt+A select none,
    Ctrl+F outliner search, Ctrl+\\ hide panels). A test asserts there are no
    conflicts.
25. **Layout persistence**: `<config>/layout.json` (egui_dock `DockState`
    with serde), `prefs.json`, `keymap.json` (overrides only).
27. **Connect arrows** on real scenes come from `PatchInfo::bend` (A along
    ∂u for OneU/Two/Radial, ∂v for OneV; B along ∂v for Two), evaluated at the
    display-mesh vertex nearest each face's centroid (projected to exact uv).
28. **Loops and rings** on real scenes use `core::select::Adjacency`
    (+ `editable_only`); the sample model keeps the app's own walker.
29. **Delete** sends one `Op::RemoveEdges` per body (one `RemoveEdge` when a
    single edge is selected).
30. **Launch**: the first free argument is a `.ntopo` or CAD file (file
    associations, `Exec=newtopo %f`, Windows `"%1"`); `file://` URLs are
    decoded, relative paths made absolute, macOS `-psn_…` ignored. The import
    helper is looked up next to the *canonical* executable (through
    `/usr/bin` symlinks, AppImage, `.app/Contents/MacOS`, Windows install
    dir), then next to the invoked path, then the kernel's own search.
    macOS "open document" Apple Events (double-click in Finder while running)
    are not handled yet.
31. **Windows icon**: `crates/app/build.rs` embeds `packaging/icons/newtopo.ico`
    through `embed-resource` (a `cfg(windows)` build-dependency, resource
    script `crates/app/newtopo.rc`); a missing resource compiler only warns.
    Not built on Windows from here — please check it in Windows CI.
32. **Check output mesh** (Edit menu / palette) runs `Scene::validate_all`
    and toasts the problem kinds; History tooltips show `dormant_reasons`.
26. **Copy/paste rules** (Ctrl+C/V) and **Connect Tab flip** toast "not
    available yet" (no ops).

## API requests (engine facade, `crates/core/src/scene.rs` and `ops.rs`)

Round 2 wired: `default_kernel`, `HAS_CAD_IMPORT`, coalescing, `patches` /
`patch_records`, `select`, `Op::RemoveEdges`, `validate_all`,
`dormant_reasons`, `issue_reason`, `core::rename`, `core::bridge`. Still
open (workaround in brackets):

1. Several ops as one step: `Scene::apply_all(ops)`. [grouped with
   `begin_coalesce`/`end_coalesce`; fine unless a later op is refused, which
   leaves the earlier ones applied within the step.]
2. Layer and lock ops: `SetBodyLocked`, `SetLayerVisible`, `SetLayerLocked`,
   `CreateLayer`, `DeleteLayer`, `RenameLayer`, `MoveBodyToLayer`,
   `SetActiveLayer`. [app-side overrides / toasts.]
3. Selection in the document (`Op::SetSelection`). [app-side.]
4. Copy/paste rules, `Op::FlipConnect`, `Op::DeleteEdit`. [toasts.]
5. `Scene::replace_body(...)`. [Replace imports as new.]
6. `Scene::save_copy(path)`. [autosave uses `save`; app tracks dirty.]
7. `Scene::set_camera(Camera)`. [camera read on open, never written.]
8. `Scene::iso_preview(...)` with through mode. [kernel sampling, no through.]
9. `Scene::bend_info(patch)` with origin/directions/warn flag. [derived from
   `PatchInfo::bend` + kernel; no warning tint.]
10. `Scene::link_set(body, face)` for the linked tint. [none.]
11. `Scene::patch_at(body, face, uv) -> Option<PatchId>` to pick one piece of
    a split face. [whole face.]
12. A public `Scene::kernel_body(body) -> Option<BodyId>` would be clearer
    than reaching through `ctx(body).kbody`.

## Engine notes from the round-2 run

No engine bug blocked the app. Exercised headless on the reference models
(`newtopo --shot <dir> --models <dir>`): import (single and the 5-body
assembly through the import list), Slice ×10, Connect A=8 on 16 hole walls,
Grid 4×3, Iso, Iso split, Cut, Collapse, Remove (3 edges, one step, undo),
Weld (confirmation, then a correct refusal "a face would flip against the
CAD surface"), preview, OBJ/FBX/GLB export, save → open with identical stats,
`validate_all` clean. Two things the app had wrong and now handles:
document vs kernel body ids, and normalised iso values.

## Known gaps

- GPU id-buffer picking (CPU BVH only); no "check" overlay for thin
  triangles (SPEC §7) yet.
- Linked-face tint, symmetry-mirrored selection, Connect warning tint and
  arrows on real scenes wait for API requests 5–6.
- Record ops (SPEC §10) is not wired in the UI.
- The import list shows names/layers only (no per-object face counts until
  the helper reports them).
- `cargo clippy -D warnings` on the whole workspace currently fails in
  `crates/core` (scaffold: `doc_lazy_continuation` in `doc.rs`,
  `manual_non_exhaustive` in `scene.rs`); the app and render crates are clean
  with `--no-deps`.

## Provenance (rows to merge into docs/PROVENANCE.md)

Kept here to avoid merge conflicts with the other workers; the lead can move
them.

| Area | Source |
| --- | --- |
| Ray–triangle test (`render/pick.rs`) | Möller & Trumbore, "Fast, Minimum Storage Ray/Triangle Intersection", JGT 1997 |
| Picking BVH (median split on the longest centroid axis), slab ray–box test | Textbook technique (Kay & Kajiya 1986 slabs); own implementation |
| Screen-space-width lines, near-plane clipping in the vertex shader, depth bias by scaling view-space points towards the eye | Own design |
| Polygon triangulation for preview (Newell normal + ear clipping) | Newell's method (Sutherland et al. 1974); ear clipping (Meisters 1975); own implementation |
| Orbit camera, framing, projection matrices (depth 0..1) | Standard formulas; own implementation |
| Edge loop / ring / face strip walks over the output mesh | Own design from SPEC §6.14 |
| Fuzzy command matching (subsequence with word-start and run bonuses) | Own design |
| Themes, icons, logo, colours, wording | Own design (no third-party art); fonts: Noto Sans (SIL OFL 1.1) |
