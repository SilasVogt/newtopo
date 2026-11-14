# App and renderer notes

Owner: app worker (`crates/app`, `crates/render`, `assets/`). Decisions taken
where SPEC.md is silent, the API the app still needs from the core, and how to
run and check the UI.

## How to run and check

| What | Command |
| --- | --- |
| App (no OCCT) | `cargo run -p newtopo --no-default-features` |
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
   Remove once the facade is complete.
2. **No kernel constructor yet.** `docstate::make_kernel()` returns `None`;
   import/open then report "no CAD kernel in this build". Wire it to the
   kernel's constructor (see API requests).
3. **Face mode selects whole CAD faces** as `PatchId::whole(body, face)` until
   the facade exposes patches.
4. **Selection is app-side** and not an undo step (SPEC §4.7 says it should
   be by default; the preference exists but has no effect yet).
5. **Box = left-drag, lasso = right-drag** in every navigation preset
   (Maya's Alt+right zoom still wins because navigation is checked first).
6. **Navigation presets**: Blender (MMB orbit, Shift+MMB pan, Ctrl+MMB zoom),
   Maya (Alt+LMB orbit, Alt/plain MMB pan, Alt+RMB zoom), 3ds Max (Alt+MMB
   orbit, MMB pan, Ctrl+Alt+MMB zoom). Wheel zooms towards the cursor. Plain
   wheel always zooms; Shift/Ctrl+wheel go to an active segment tool.
7. **`IsoDir::U` = the line of constant u** (it runs along v); `value` is the
   u value and `seed` the v at the click. Please confirm in the engine.
8. **Iso preview** samples the iso line via `kernel.face_eval` and clips with
   `face_classify_uv` (96 samples). Through mode can't be previewed across
   faces without an engine query.
9. **Wheel coalescing** (SPEC §4.7, 400 ms): implemented as undo-then-apply
   for every tick after the first within 400 ms, so a gesture leaves one undo
   step. Costs an extra evaluation per tick; see API requests.
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
    recovery. Clean exit removes the current autosave.
16. **Import** runs `run_import` on a worker thread with a progress bar and
    cancel (cancel flag returned from the progress callback). Files with more
    than one body open the import list (Keep/Exclude/Replace, search,
    multi-select, all/none, apply choice to selected rows). Replace imports as
    new with a toast until the engine supports it.
17. **Export** clones the output meshes and runs `core::export::export` on a
    worker thread; unit conversion = scene units → chosen units folded into
    `scale`. Lists fan-fallback polygons and the n-gon note when auto-fill is
    off.
18. **Batch rename** preview/collision logic lives in `studio.rs` (pure
    functions with tests) because `core::rename` is still empty. Regex
    replace is not offered (no regex crate in the workspace). Apply issues one
    `Op::RenameBody` per changed object.
19. **Bridge panel** goes through `studio::bridge_adapter` (always
    "unavailable" for now). The per-install token is 128 bits from the std
    random-seeded hasher, stored in prefs, with copy/regenerate.
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
26. **Copy/paste rules** (Ctrl+C/V) and **Connect Tab flip** toast "not
    available yet" (no ops).

## API requests (engine facade, `crates/core/src/scene.rs` and `ops.rs`)

Exact signatures the app would use; current workaround in brackets.

1. Kernel construction:
   `pub fn newtopo_kernel::default_kernel() -> std::sync::Arc<dyn CadKernel>`
   (OCCT when the feature is on, else the mock). [`make_kernel()` returns `None`.]
2. Gesture coalescing:
   `pub fn Scene::apply_coalesced(&mut self, op: Op, gesture: u64) -> Result<OpOutcome, Refusal>`
   — replaces the previous undo step if it carried the same `gesture` id.
   [undo + apply per wheel tick.]
3. One undo step for several ops:
   `pub fn Scene::apply_all(&mut self, ops: Vec<Op>) -> Result<Vec<OpOutcome>, Refusal>`
   (all or nothing). Needed for multi-edge Remove/Collapse, batch rename,
   per-body Slice. [one step per op.]
4. Patches for face mode:
   `pub fn Scene::patches(&self, body: BodyId) -> Vec<PatchView>` with
   `pub struct PatchView { pub id: PatchId, pub face: FaceId, pub triangles: Vec<[u32; 3]> /* into face_meshes[face] */ }`.
   [whole faces.]
5. Connect bend directions for the arrows (SPEC §5.2/6.2):
   `pub fn Scene::bend_info(&self, patch: PatchId) -> Option<BendInfo>` with
   `pub struct BendInfo { pub origin: DVec3, pub normal: DVec3, pub a: Option<DVec3>, pub b: Option<DVec3>, pub warn: bool }`
   (body coordinates; `warn` = direction disagrees with selected neighbours).
   [arrows only on the sample model.]
6. Link sets: `pub fn Scene::link_set(&self, body: BodyId, face: FaceId) -> Vec<FaceId>` for the
   linked tint and selection expansion. [none.]
7. Layer and lock ops (`Op` variants):
   `SetBodyLocked { body: BodyId, locked: bool }`,
   `SetLayerVisible { layer: LayerId, visible: bool }`,
   `SetLayerLocked { layer: LayerId, locked: bool }`,
   `CreateLayer { name: String }`, `DeleteLayer { layer: LayerId }`,
   `RenameLayer { layer: LayerId, name: String }`,
   `MoveBodyToLayer { body: BodyId, layer: LayerId }`,
   `SetActiveLayer { layer: LayerId }`. [app-side overrides / toasts.]
8. Selection in the document (SPEC §4.7):
   `Op::SetSelection { selection: doc::Selection }` (objects, patches, edge
   keys, vertex keys). [app-side selection.]
9. Copy/paste rules (SPEC §6.13):
   `pub fn Scene::copy_rules(&self, body: BodyId) -> RuleClipboard` and
   `Op::PasteRules { clip: RuleClipboard, targets: Vec<BodyId> }` returning the
   unmatched faces in `OpOutcome::message`. [toast.]
10. Flip A/B on a patch: `Op::FlipConnect { patch: PatchId }`. [toast.]
11. Delete a single edit from History: `Op::DeleteEdit { edit: u32 }`.
12. Replace on import: `pub fn Scene::replace_body(&mut self, old: BodyId, brep: &[u8], attrs: BodyAttributes, heal: HealReport) -> anyhow::Result<BodyId>`.
    [imports as new.]
13. Autosave without touching the saved marker:
    `pub fn Scene::save_copy(&mut self, path: &Path) -> anyhow::Result<()>`.
    [`save` to the autosave path; app tracks dirty itself.]
14. Camera into the document: `Op::SetCamera { camera: doc::Camera }` or
    `pub fn Scene::set_camera(&mut self, c: Camera)` (not an undo step).
    [camera is read on open, never written.]
15. Loops/rings from `core::select`:
    `pub fn Scene::edge_loop(&self, body: BodyId, edge: &EdgeKey, ring: bool, every_other: bool) -> Vec<EdgeKey>`
    and `pub fn Scene::face_strip(&self, body: BodyId, patch: PatchId) -> Vec<PatchId>`.
    [app walks the `OutputMesh` topology itself (`selection::TopoMesh`).]
16. Iso preview including through mode:
    `pub fn Scene::iso_preview(&self, body: BodyId, face: FaceId, dir: IsoDir, value: f64, seed: Option<f64>, through: bool) -> Vec<Vec<DVec3>>`.
    [kernel sampling, no through.]
17. Confirm the assumption that `doc::BodyEntry::id` is also the kernel's
    `BodyId` (the app calls `scene.kernel.project_to_face/edge` with it).
18. `core::bridge` (when it lands): `hosts() -> Vec<HostStatus>`,
    `send(host, objects, keep_both) -> Result<SendReport>`; plug into
    `studio::bridge_adapter`.
19. `core::rename`: if it exposes `preview(op, names) -> Vec<String>`, the
    rename panel will call it instead of `studio::rename_preview`.

## Known gaps

- GPU id-buffer picking (CPU BVH only); no "check" overlay for thin
  triangles (SPEC §7) yet.
- Linked-face tint, symmetry-mirrored selection, Connect warning tint and
  arrows on real scenes wait for API requests 5–6.
- Record ops (SPEC §10) is not wired in the UI.
- The import list shows names/layers only (no per-object face counts until
  the helper reports them).
- `newtopo-cli render` wiring is left to the lead
  (`newtopo_render::core_scene::render_core_scene_to_png`).
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
