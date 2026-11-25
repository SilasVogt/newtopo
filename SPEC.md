# newtopo: product and engineering spec

A desktop app that turns CAD solids (STEP/IGES) into clean, game-ready polygon
meshes. The artist puts edge lines on the exact CAD surfaces, the app fills
the rest, and the result goes to Blender, Maya, 3ds Max or a file.

Status: draft v0.2 (2026-11-14). Audience: the implementing agent.

---

## 0. Clean-room rules (read first)

The spec author studied an existing commercial product through its public
product page and its public tutorial videos. This document describes
**behaviour and requirements only**, in the author's own words and with the
author's own design decisions filling the gaps. The implementing agent must:

1. Build only from this spec, public academic papers, and the documentation of
   the open-source libraries named here.
2. Not search for, download, install, run, decompile or screenshot any existing
   commercial CAD-retopology product, and not read or watch its docs,
   tutorials, videos or forums. If this spec doesn't answer a question, write
   it down in `docs/QUESTIONS.md`, decide it yourself, and note the decision.
3. Invent all UI art, icons, colours, themes, wording, file formats, network
   ports and names (the ones this spec already fixes, such as `.ntopo` and
   ports 47110–47112, are original and may be used). Generic industry terms (loop, ring, weld, collapse, slice,
   cut, iso, grid, preview, layer) are fine. Single-letter tool shortcuts may
   be mnemonic (§5.8); don't try to match anyone's keymap.
4. Keep `docs/PROVENANCE.md`: where each non-trivial algorithm came from
   (paper, library docs, own design).
5. Never commit research material (transcripts, screenshots, notes on the
   other product) to this repo.

---

## 1. Product summary

**Problem.** Hard-surface artists model in CAD tools (Plasticity, Fusion,
Onshape, SolidWorks). A CAD export holds exact trimmed surfaces. Tessellating
them gives dense triangle soup with no edge flow and bad shading. Retopologising
that by hand in a DCC tool takes days.

**Solution.** Load the B-rep itself. The starting mesh is "one polygon per CAD
face": every CAD face is an n-gon bounded by its CAD edges. The artist adds
**lines** that follow the exact surfaces (by count with Connect, Surface Grid
and Slice, or one at a time with Iso and Cut). Lines split the n-gons.
**Auto-fill** then splits whatever is still bigger than a quad into quads and
triangles using only existing vertices. The user cleans up (remove, collapse,
weld), checks the **preview mesh**, and exports or sends it to a DCC app.

Everything the artist does is **re-editable**: a face's segment count can be
changed at any time and the mesh regenerates, keeping manual work that still
applies.

**Primary user.** A hard-surface/game artist used to Blender, Maya, 3ds Max
or Plasticity. Not a CAD engineer.

**Core loop.** Import → organise layers → add lines (Connect, Surface Grid,
Slice, Iso, Cut) → auto-fill → clean up → preview → rename, quick UV →
export or send to a DCC app.

### Goals
- G1. Import STEP (AP203/AP214/AP242) and IGES with names, layers, colours and assemblies.
- G2. A procedural, re-editable line model on top of the read-only B-rep (§4).
- G3. The tool set in §6, all live, all undoable, all symmetry- and link-aware.
- G4. Auto-fill that never adds vertices and never fails (§7).
- G5. Game-ready output: hard edges from CAD sharpness, names, layers, materials, units, up axis.
- G6. Save and reload scenes.
- G7. Batch rename, quick UV unwrap, live bridge to Blender, then Maya and 3ds Max.
- G8. A headless CLI that drives the whole core, so agents and CI can test every tool without a screen.

### Non-goals (v1)
- Editing the CAD model. The B-rep is read-only, always. (Split lines and edge
  removal in §6 change only the retopo layer.)
- Retopology of plain meshes (OBJ/FBX input).
- Auto-fill that adds new interior vertices. The user adds lines; auto-fill
  only connects them. (Possible later as a separate "smart fill" tool.)
- A UV editor, sculpting, baking, LODs.
- Installers, licensing, activation, trials. Out of scope until Silas decides (§13).
- Localisation. English only, but keep UI strings in one table.

---

## 2. Glossary

| Term | Meaning |
| --- | --- |
| **B-rep** | The CAD model: bodies → faces → wires → edges → vertices; each face has a parametric surface (u, v) and trimming loops. Read-only. |
| **CAD edge / CAD vertex** | B-rep topology. CAD edges are always boundaries in the output mesh unless the user removes them (§6.10). |
| **Seam edge** | A CAD edge that only exists because a periodic surface (cylinder, torus) is cut open. Never shown, never an output edge. |
| **Sharp edge** | A CAD edge whose adjacent face normals differ by more than the sharp threshold (default 30°). Becomes a hard edge on export. |
| **Split line** | A user-added line that divides a CAD face into **patches**. Behaves exactly like a CAD edge for every tool. Drawn in the CAD-edge colour. |
| **Patch** | A CAD face, or one piece of a CAD face divided by split lines. The unit that segment tools work on. |
| **Line** | A user-requested curve on the surface that becomes edges in the output mesh. Generated by a **rule** (§4.3). Drawn in the line colour. |
| **Rule** | A stored, editable instruction that generates lines: Connect counts on a patch, a grid, a slice set, one iso line, one cut path. |
| **Edit** | A manual change to the generated mesh: remove an edge, collapse, weld. Stored against stable keys (§4.5). |
| **Output mesh** | What export writes: patches cut by lines, edits applied, auto-fill applied. Shown in **preview mode**. |
| **Fill edge** | A keyless edge the evaluator adds or keeps: bridge chords (always, §4.6 step 6), seam cuts kept by the topology guard (§4.1), and auto-fill diagonals (when on, §7). Drawn in its own colour; not editable directly. |
| **Link set** | Faces in a body that have the same shape (congruent), so a rule on one applies to all (§6.11). |

---

## 3. Platforms and stack

### 3.1 Targets
- Linux x86-64 (dev target; Wayland and X11). Windows 10/11 x64 (main user
  target). macOS arm64 best effort, late phase.
- GPU: whatever wgpu supports natively (Vulkan, DX12, Metal).
- Baseline machine: 4-core CPU, 16 GB RAM, integrated GPU must stay usable on
  the reference models in §11.

### 3.2 Stack

| Concern | Choice | Why |
| --- | --- | --- |
| Language | **Rust (stable, 2024 edition)** for everything except the kernel shim | Memory safety, enums for rules/anchors, cargo workspace, good test tooling. |
| CAD kernel | **Open CASCADE Technology 7.8+** (LGPL-2.1 + exception) behind a thin C++ shim crate (`occt-shim`) using **`cxx`** | The only mature open-source STEP/IGES reader with exact B-rep, pcurves, projection, intersection, healing and layers (XCAF). See §3.4. Link dynamically; ship its licence notices. |
| UI | **egui + eframe** (wgpu backend), **egui_dock** for docking/floating panels | Immediate mode suits tool overlays and the command palette. |
| Rendering | **wgpu**, own small renderer drawn into the viewport via `egui_wgpu` paint callbacks | Custom overlays (edge kinds, direction arrows, tints); same renderer runs headless for `newtopo-cli render`. |
| Math | `glam` (f64 for geometry, f32 for GPU buffers); `faer` for sparse solves | |
| 2D geometry | `robust` crate (Shewchuk's adaptive exact predicates) for orientation/in-circle tests; `spade` (constrained Delaunay, MIT/Apache) | Arrangements and fills in UV space. |
| Fill | Own dynamic program over polygon diagonals (§7) | Exact optimum for typical polygon sizes. |
| UV unwrap / pack | xatlas through a small `cc`-built binding (MIT) | Phase 6. |
| Export | OBJ: own writer. glTF: `gltf-json` for the JSON plus a small own GLB packer (header, JSON chunk, BIN chunk, accessors). FBX: own binary FBX 7.4 writer (no usable Rust crate). Sources for the FBX format: the public "FBX binary file format specification" post on the Blender developer blog and Autodesk's public FBX docs; validate output by reading it back with `ufbx` (MIT, via a small binding in tests only) and by the Blender import test. Document the written subset in `docs/fbx-subset.md`. Never ship the Autodesk FBX SDK and never copy GPL exporter code. | |
| Serialisation | `serde`, `serde_json`, `zstd`, `zip` | |
| Undo/state | Persistent data structures (`imbl`, MPL-2.0, the maintained fork of `im`; or `Arc` + copy-on-write) | Cheap snapshots of the document. |
| Build | cargo workspace; OCCT from the system package on Linux (`opencascade` on Arch) and vcpkg on Windows, found by `build.rs` via `OCCT_DIR` / pkg-config | `cargo build` is the one command. CI caches OCCT. |
| Tests | `cargo test`, `proptest`, `insta` (golden files), `criterion` (perf budgets) | |
| DCC addons | Python for Blender, Maya and 3ds Max (pymxs) | |

**Alternatives considered.**
- *C++ end to end* (ImGui + OCCT): least FFI, but all app code in C++. Rejected by Silas.
- *Pure-Rust kernel* (`truck` + `truck-stepio`): no IGES, partial STEP
  coverage, no healing, no layers; real Plasticity/Fusion exports would fail.
  The app talks to the kernel only through `trait CadKernel`, so a pure-Rust
  kernel can replace OCCT later.
- *Web/WASM* (opencascade.js): too slow and memory-heavy for big assemblies.

### 3.3 Repository layout

```
newtopo/
  Cargo.toml (workspace)  SPEC.md
  crates/
    occt-shim/   # the only C++ we write: cxx bridge + shim.cpp/.h, build.rs
                 # (third-party C++ such as xatlas is vendored as a dependency)
    kernel/      # CadKernel trait, OCCT impl, MockKernel, B-rep index, face
                 # classification, congruence + symmetry matching
    core/        # no UI, no GPU; everything unit-testable
      doc/       # document: patches, rules, edits, settings, undo, save/load
      anchor/    # anchors, evaluation, projection, surface paths
      eval/      # rule evaluation -> arrangement -> output mesh (incremental)
      fill/      # auto-fill
      meshedit/  # mesh mode after Finish: baked mesh, decimate, cleanup, resample
      select/    # loop, ring, every-other, strips, link/symmetry expansion
      export/    # obj, gltf, fbx, normals, hard edges, uv, rename
      bridge/    # wire protocol, client side
    render/      # wgpu renderer, windowed and headless
    app/         # eframe shell, panels, viewport, input, palette
    cli/         # newtopo-cli
    import/      # newtopo-import helper process (§3.4)
    testmodels/  # generates reference STEP files through the kernel (§11)
  addons/{blender,maya,max}/
  docs/          # PROVENANCE.md, QUESTIONS.md, fbx-subset.md, user guide
```

### 3.4 Kernel boundary (`occt-shim` + `kernel`)

Keep the C++ small, boring and fenced off:

- A flat, **id-based** API through `cxx`. No OCCT types cross the boundary.
  Faces, edges and vertices are `u32` ids into tables the shim owns per model.
- Every shim function catches `Standard_Failure` and all C++ exceptions and
  returns an error status. The app process never installs OCCT's signal
  handlers (`OSD::SetSignal`), because they're process-wide and would
  intercept faults from wgpu, drivers and Rust. Only the import helper
  enables them.
- **Import runs in a separate process** (`newtopo-import`, a small binary
  built from the same crates): it reads STEP/IGES, heals, and writes OCCT
  `.bbrep` files (geometry only, OCCT's binary format via `BinTools`; `.brep`
  text via `BRepTools` is accepted on input for tests) plus a JSON sidecar
  (per body: names, layers,
  colours, assembly transforms; plus the heal report). The app loads both:
  geometry from the `.bbrep` files, attributes and the report from the
  sidecar, and keeps the sidecar data in the scene (§8.2). A crash or
  hang in OCCT's file readers on a bad file kills the helper, not the app,
  and the app reports "import failed" with the helper's stderr. Timeout
  configurable (default 5 minutes).
- Batch calls: evaluation and projection take slices and return slices, so a
  live preview makes a handful of FFI calls, not thousands.
- OCCT has process-global state: **one kernel thread for all models** in the
  app process, calls serialised through a channel; the UI never blocks on it.
  Heavy queries (tessellation, congruence matching) are batched on that
  thread; parallelism, if ever needed, comes from extra helper processes.
- Shim surface (target < 2,000 lines of C++): `load_step/load_iges/load_brep/
  save_brep`, `heal(options) -> report`, `topology()` (faces, edges, vertices,
  wires, adjacency, orientation, seam flags, assembly tree, names, layers,
  colours, transforms), `surface_info(face)` (type: plane, cylinder, cone,
  sphere, torus, revolution, extrusion, bspline/other; axis and radii where
  defined; periodicity; uv bounds), `tessellate(face, tol)`,
  `face_eval(face, uv[]) -> (p, n, du, dv)[]`, `face_curvature(face, uv[]) ->
  principal dirs + curvatures`, `face_classify_uv(face, uv[])`,
  `edge_eval(edge, t[])`, `edge_pcurve_eval(edge, face, t[])`,
  `project_to_face/edge/nearest(p[])`, `section_with_plane(faces, plane) ->
  polylines with (face, uv)`, `iso_curve(face, dir, value) -> list of uv
  polyline segments clipped to trims` (a trimmed iso line can be several
  pieces).
- **Test-shape builder** behind a cargo feature `testshapes` (off in release
  builds, outside the line budget): primitives, fillets, chamfers, booleans,
  revolve, patterns, and an XCAF STEP writer with names, layers and colours.
  It exists only to generate the reference models in §11.
- Everything else is Rust and talks to `trait CadKernel`. A `MockKernel` with
  analytic surfaces (plane, cylinder, cone, sphere, torus) lets most core tests
  run without OCCT.

---

## 4. Data model

The document is **procedural**: B-rep + split lines + rules + edits →
evaluated output mesh. The output mesh is a cache, never the source of truth.

### 4.1 B-rep layer (read-only)
- The shim keeps the OCCT shape per body; Rust keeps an index with stable ids
  for faces, edges and vertices, assigned in deterministic traversal order so
  ids survive reload of the same file.
- Per face: surface info (§3.4), trimming loops, orientation, display
  tessellation, area, **bend class** (§6.2).
- Per edge: curve, pcurves, flags `seam`, `sharp`, `boundary` (open shell),
  dihedral angle.
- Healing on import (`ShapeFix_Shape`, sewing with a tolerance setting).
  Report what was fixed and which faces failed.
- Objects keep their CAD name, layer, colour and assembly transform.

**Closed edges, seams and poles.** These need explicit handling because OCCT
represents them in ways that don't map to polygons directly:
- **Shared vertices close exactly.** Pcurve end points that belong to the same
  CAD vertex are snapped to their mean in UV (within 0.1 % of the face's
  parameter span) before the arrangement is built, so loops close whatever
  the STEP file's vertex tolerance is (exporters often leave gaps far above
  the arrangement's own tolerance). Copies of one vertex on both sides of a
  seam are a period apart and stay separate.
- **Seam edges** are never output edges. The arrangement is computed in the
  face's UV domain cut at the CAD seam(s) (one for a cylinder, two for a
  torus), with vertices on both copies of each seam identified. Afterwards
  every seam edge is dissolved (unless the topology guard below keeps it),
  merging the polygons on its two sides, and
  every vertex that exists **only because of the seam** is dropped: the
  closed CAD edge's own CAD vertex where it meets the seam, crossings of
  lines with the seam (`OnBoundary(line, seam, …)`), and the torus seam
  corner. A vertex stays if a line, a split line or another non-seam CAD edge ends there. The
  drop is per vertex, so every face sharing that vertex drops it. A polygon
  that wraps around the period becomes a polygon with two loops or a
  self-touching boundary, which the always-on bridging step (§4.6 step 6)
  makes simple.
- **Topology guard.** A seam is dissolved inside an arrangement region (one
  cell of the arrangement, not a whole patch) only if that region
  stays a disk with holes (check the Euler characteristic of the region after
  dissolving: χ = 2 − number of boundary loops). Seams are tried in a fixed
  order (u-seam, then v-seam), so which seam stays is deterministic; a kept
  seam loop with fewer than 3 vertices gets placeholders like any loop. Otherwise (for example a torus
  face trimmed only by a hole, which would become a handle with one boundary
  loop) the seam edges bounding that region stay as output edges marked as
  fill edges (keyless, drawn in the fill colour), keeping their vertices,
  until lines make the region a disk; then they dissolve normally. With the
  implicit grid and the ≥ 3 clamp this is a safety net, rarely reached.
- **Short closed loops.** After seams are dissolved (§4.6 step 3), any
  closed loop (a closed CAD edge, a CAD circle exported as two arcs, a closed
  split line, or a closed generated line such as an around-direction iso, a
  low-count grid line or a slice ring) that has **fewer than 3** output
  vertices, counting vertices from both sides of the loop, **also** gets
  placeholder vertices: `min_closed_segments` (setting, default 8) evenly
  spaced positions, phase-aligned to the loop's first remaining vertex (or to
  the phase reference, below, if it has none), skipping any position within
  half a spacing of an existing vertex. Existing vertices are always kept.
  Placeholders have keys `Placeholder(loop id, i)` and are shared by every
  face on that loop. `loop id` is canonical: for a CAD loop, the sorted ids
  of the CAD edges forming it (so a circle exported as two arcs has one id);
  for a split line, its `SplitLine` id; for a generated line, its `LineKey`. So an untouched cylinder has two 8-vertex rims aligned
  to the phase reference, and a cylinder with one generator line has that
  line's endpoints on both rims plus placeholders.
- **Phase reference.** Every periodic face, every planar ring face, and every closed CAD edge (so
  planar caps bounded by a circle have one too), has a phase reference
  direction used for the canonical angle (§6.2) and for placeholders.
  Default: world X projected onto the plane perpendicular to the face's or
  circle's axis (for a non-circular closed edge: the normal of its best-fit
  plane; world Y if X is within 10° of the axis, then Z). A face
  mapping (Link §6.11, symmetry §6.12, paste §6.13) sets the target's phase
  reference to the source's reference mapped through the same rigid
  transform, stored per face/edge in the document. Paste sets it on every
  matched face and closed edge, whether or not a rule lands there. If two
  mappings reach the same target, the most recent one wins (and the History
  panel lists phase-reference overrides alongside rules and edits). So rotated copies get rigidly transformed lines and
  placeholders, not world-aligned ones.
- **Faces with no outer boundary** in the seam-cut domain (full torus, full
  sphere, a torus trimmed only by holes, other closed periodic surfaces):
  until a spread rule is applied, they get an implicit
  Grid with U = V = `min_closed_segments` (sphere: V = half of that, with
  pole fans). The implicit grid is shown like any rule and is replaced by
  the first spread rule the user applies. It applies per **patch**: every
  patch of such a face with no spread rule and no outer boundary gets it.
- **Poles and apexes** (sphere poles, cone tips: degenerate edges or
  singular points where `du` or `dv` vanish) collapse to a single
  `CadVertex`-like pole vertex. Polygons touching a pole may be triangles
  fanning into it. The pole normal is the surface axis direction (or the
  limit of normals sampled just off the pole). Validation exempts the
  zero-length degenerate edge itself, which is never output.

### 4.2 Patches and split lines
- `SplitLine { id, group, face, kind: Iso { dir: U|V, value, seed: Option<f64> } }`
  (v1 only needs iso splits; keep the enum open for arbitrary UV curves).
  `seed` is the click position as the parameter along the iso line: a trimmed
  iso line can have several disconnected pieces (§3.4), and with `seed` set
  only the piece containing it (or nearest to it) is used; `None` means all
  pieces (through mode). A through
  split that crosses several faces makes one split line per face sharing a
  `group` id, so it can be deleted or undone as one. Split lines are their own
  document objects, not rules: they are the **only** place splits are stored.
- Patches = connected regions of a CAD face after cutting it with its split
  lines (holes stay holes).
- **PatchId** is stable under unrelated splits: `PatchId { face, u_lo, u_hi,
  v_lo, v_hi, region }` where each bound is `FaceBoundary` or `Split(id)` (the
  nearest split line on that side, in that direction), and `region` numbers
  disconnected pieces within the same cell, ordered by the lexicographic
  (u, v) of each piece's interior sample point.
- Adding a split line through a patch gives both new patches the same kind
  of spread rule. Lines **parallel to the split** each fall in one piece, so
  their count is **distributed** by each piece's width across the split
  direction (largest-remainder rounding, at least 1 where the original was
  ≥ 1); line density stays about the same and lines re-space evenly within
  each piece. Lines **crossing the split** run through both pieces, so their
  count is **copied** unchanged to both.
  A split that doesn't separate the patch (a generator line on a full
  cylinder, one iso on a torus) leaves a single patch: its rule is unchanged,
  and the split line is just an output edge that lines end on.
  Slice rules aren't distributed: their planes are geometry, so the pieces
  just clip them. If the two pieces carry different rule kinds when merged,
  the merged patch takes the rule of the larger piece (by area).
  Removing a split line merges patches **direction-aware**: the count of lines
  parallel to the split is the sum of the pieces' counts; the count of lines
  crossing it is the larger piece's count (by area). Split-then-unsplit is not guaranteed to
  restore counts exactly; undo is (toast says what changed).
- **Minimum counts on closed patches**: only on patches with **no outer
  boundary** (a full torus, a full sphere, a torus trimmed only by holes),
  any spread rule's count in a
  periodic direction is clamped to at least 3, so the stitched output can't
  degenerate. The panel shows the clamp. Patches with a boundary (a cylinder
  with rims) aren't clamped: low counts there make self-touching polygons,
  which bridging (§4.6 step 6) handles.

### 4.3 Rules

```rust
pub enum Rule {
    Connect { patches: Vec<PatchId>, count_a: u32, count_b: u32,
              start_on_seam: bool, flipped: Vec<PatchId> },
    Grid    { patch: PatchId, count_u: u32, count_v: u32,
              start_on_seam: bool, spacing: Spacing },  // enum Spacing { ArcLength, Parametric }
    Slice   { scope: SliceScope, count: u32, orient: SliceOrient, through: bool,
              frozen_planes: Option<Vec<Plane>> },
              // frozen_planes: set on Body→Patches conversion; when set it
              // overrides count/orient; editing count or orient recomputes the
              // planes from the patches' extent and clears it
    Iso     { pieces: Vec<IsoPiece>, through: bool },
    Cut     { points: Vec<CutPoint> },          // polyline over the surface
    Join    { a: VertexKey, b: VertexKey },     // "connect vertices"
}
pub enum SliceScope  { Body(BodyId), Patches(Vec<PatchId>) }
pub enum Spacing     { ArcLength, Parametric }
pub struct Plane     { origin: DVec3, normal: DVec3 }   // body coordinates
pub enum SliceOrient { Auto, GuideEdge(EdgeId) }
```

- **Spread rules** (Connect, Grid, Slice) own patches: a patch has at most one
  spread rule. Applying another spread tool to a patch takes it over from the
  previous rule (the old rule shrinks; if empty it is deleted). Re-running the
  same tool on a selection replaces the rule on those patches.
- A `Slice` with `SliceScope::Body` that loses any patch (takeover by another
  spread rule, or count 0 on a subset) is first converted to
  `SliceScope::Patches` listing every patch of the body, then shrinks like
  any other rule. Its planes stay the same (they're computed from the
  original scope's extent, stored in the rule as `orient` plus the plane
  positions at conversion time). **AC**: body Slice with 10 planes, then
  Connect on one patch: that patch has only Connect lines; every other patch
  keeps exactly its previous Slice lines.
- **Single-line rules** (Iso, Cut, Join) are additive and can coexist with
  spread rules.
- Each rule has a stable `RuleId` and an ordinal for deterministic evaluation.

### 4.4 Anchors and surface paths
Every output vertex position is computed from an anchor, never stored.

```rust
pub enum Anchor {
    CadVertex { vertex: VertexId },
    CadEdge   { edge: EdgeId, t: f64 },
    Face      { face: FaceId, uv: DVec2 },
}
```

- **Collapse** merges to the end with the higher anchor priority:
  CadVertex > CadEdge > Face (ties: the surface-path midpoint). **Weld**
  always keeps the **target's** anchor (the user chose where it goes); if the
  source has a higher-priority anchor than the target, the weld needs a
  confirmation because it pulls a vertex off a CAD edge or corner (§6.9).
- Each output edge keeps a `SurfacePath` (along a CAD edge, a UV polyline on a
  face, or a chain across faces) so it can be drawn on the surface and so
  lines can be intersected exactly. Exported edges are straight segments.

### 4.5 Stable keys and edits
- Every generated vertex and edge gets a **key** from its provenance, e.g.
  `LineKey { rule, index }`, `VertexKey::Crossing(LineKey, LineKey, occurrence)`
  (`occurrence` counts crossings of the same two lines in order along the
  first line, since cut paths and through-iso loops can cross twice),
  `VertexKey::OnBoundary(LineKey, EdgeOrSplit, end)`, `VertexKey::Cad(VertexId)`,
  `VertexKey::CutPoint(rule, i)`. Keys don't depend on evaluation order.
- Edits are stored against keys:

```rust
pub enum Edit {
    RemoveEdge { edge: EdgeKey },              // suppress one generated edge
    RemoveCadEdge { edge: EdgeId },            // output-only merge, §6.10
    Collapse { edge: EdgeKey },
    Weld { from: VertexKey, to: VertexKey },
    SetHard { edge: EdgeKey, hard: bool },     // override CAD sharpness for export
}
```

- **Fill edges have no keys and can't be edited**; they're recomputed every
  evaluation. Edge selection skips them by default (setting "select fill
  edges", off). Deleting one is refused with a hint: add a line (Iso/Cut) or
  weld; turning auto-fill off removes auto-fill diagonals but not bridge
  chords, which are always needed. Collapse and weld work on their end vertices,
  which are real vertices with keys.

- When regeneration removes a key an edit refers to, the edit becomes
  **dormant**: kept, listed greyed in the History panel, re-applied if the key
  comes back (for example the count goes back up). "Purge dormant edits" is a
  command. Cut points anchored to a face or CAD edge never go dormant; cut
  points anchored to a generated edge go dormant with it.
- Index-based keys mean that changing a count from 8 to 6 can move an edit to
  a different line. That is accepted behaviour; document it in the user guide.

### 4.6 Evaluation pipeline
1. Patches from faces + split lines.
2. Each rule generates lines as UV polylines per face (exact iso lines, plane
   sections, sampled geodesic-in-UV for cut segments).
3. Per patch, build a planar **arrangement** in UV of patch boundary + lines,
   with exact/robust predicates, in the seam-cut domain. Then, as one global
   pass over all patches (these steps cross patch boundaries): dissolve seam
   edges and seam-only vertices, then add placeholders to short closed loops
   (§4.1).
4. Create output vertices (anchors + keys) and polygons; share vertices on CAD
   edges and split lines between neighbouring patches so the mesh is watertight.
5. Apply edits in order (removals, collapses, welds, CAD-edge removals can
   merge polygons and create new holes or annuli).
6. **Polygon normalisation / bridging (always runs, after edits, independent
   of auto-fill)**. First, re-run the **topology guard** (§4.1) on every
   region after edits: if an edit merged regions into something that isn't
   a disk with holes (e.g. RemoveEdge on a torus-with-hole grid recreating
   the handle), re-insert the face's seam curve(s) through that region as
   fill-edge cuts, with their seam vertices, until it is a disk with holes.
   Then a polygon needs bridging when its boundary is **not one simple
   cycle**: several loops (holes, an annulus around a period, one created by
   an edit), or one boundary walk that visits a vertex twice (a cylinder band
   cut by a single generator, a line from a hole to the outer edge, a cut
   that ends inside a face). Repeatedly add a chord between existing
   vertices until every piece is simple:
   - a candidate chord joins **two distinct vertices not already joined by
     an edge**, and either connects two different boundary loops or, on one
     walk, touches a repeated vertex or a dangling end (so every chord fixes
     something); it crosses nothing (including chords already added) and
     lies inside the polygon, tested in the periodic cover of the domain;
   - pick the shortest candidate, measured as the 3D length of the chord's
     surface path (its domain segment mapped onto the surface), not the
     straight distance between its ends; ties broken by vertex key order,
     where key kinds order as `Cad` < `OnBoundary` < `Crossing` < `CutPoint`
     < `Placeholder`, then by their fields;
   - a chord between two loops joins them; a second chord between the same
     two loops (with different endpoints on both) then splits the joined
     region. Loops may be joined to any other loop, not only the "outer" one
     (on a periodic band neither rim is outer).
   The domain is the patch UV domain, the seam-cut domain lifted
   continuously along the boundary for periodic patches, or the flattened
   domain of §7 step 1 for polygons spanning patches. Chords are fill edges;
   nothing with a key depends on them, so running after edits is safe.
7. If auto-fill is on, decompose every polygon with more than 4 sides (§7).
8. Validate (debug builds and `newtopo-cli validate`): manifold, watertight
   where the B-rep is closed, no zero-length edges, no degenerate or flipped
   faces (against the CAD normal), no polygon with a hole or repeated vertex,
   every anchor within tolerance.

**AC** (engine, P2):
- Plate with a hole and a Grid around it; RemoveEdge on every generated edge
  touching the hole rim: the merged holed polygon is bridged into simple
  polygons and the output validates, with auto-fill on and off.
- A cylinder with exactly one generator line, and a planar face with one
  iso line from a hole to the outer edge: both validate (no repeated vertex).
- Rectangular planar Grid with 10 constant-u lines and 4 constant-v lines (count_u = 10
  counts the constant-u lines), split at u = 0.3 of the u-extent (a
  constant-u split, parallel to the constant-u lines): the pieces get
  count_u = 3 and 7 (largest-remainder rounding of 10 × 0.3 / 0.7), both keep
  count_v = 4, and the constant-v lines meet across the split. Removing the
  split gives count_u = 10, count_v = 4.
- Torus trimmed by one hole: untouched, it gets the implicit grid and
  validates; with Grid 6 × 6 it validates and all seams dissolve; then
  RemoveEdge on every grid edge: the seams come back as fill-edge cuts and
  the output validates, with auto-fill on and off.

**Incremental**: track dirty patches. Changing a rule re-evaluates only its
patches and their neighbours' shared boundaries. Budgets: < 16 ms for a
one-patch change, < 50 ms for a 200-face link set change, < 300 ms full rebuild
of a 2,000-face body (auto-fill on).

### 4.7 Scene and undo
- Scene: bodies, layers, split lines, rules, edits, per-scene settings
  (auto-fill on/off, symmetry axes), selection, camera.
- **Undo** stores document snapshots (structural sharing; cheap). Selection
  changes are undo steps by default (setting). A wheel gesture that changes a
  count is coalesced into one undo step once the wheel stops for 400 ms.
- Depth: 500 steps default.
- **Preview transactions** (§5.10): while a preview is open, ops change the
  scene provisionally. Commit turns everything applied since the preview
  opened into one undo step; cancel restores the document and the undo and
  redo history exactly. Undo while previewing cancels; redo is off.

---

## 5. UI and interaction

### 5.1 Layout
- **Top bar**: File/Edit/View/Tools/Help menus, component mode buttons
  (vertex, edge, face, object), toggles for preview, auto-fill, link and
  symmetry, scene title with an unsaved-changes marker.
- **Left: outliner** with layers and objects (§5.5).
- **Centre: viewport** with a clickable view cube.
- **Right: panels**: the active tool's panel, statistics, history (rules,
  phase-reference overrides and
  edits, including dormant ones).
- Panels can dock or float (egui_dock). "Hide panels" hides docked panels;
  floating ones stay where they are. Layout saved per user.

### 5.2 Viewport display
- **Normal mode**: CAD shaded, CAD edges and split lines in the CAD-edge
  colour, generated lines in the line colour, fill edges in the fill colour,
  vertices as dots when in vertex mode, linked faces tinted, symmetry plane(s)
  drawn faintly (toggle), Connect direction arrows on selected faces.
- **Preview mode** (toggle): hides the CAD model and shows only the output
  mesh, flat- or smooth-shaded with hard edges. Separate wireframe toggle.
  Statistics show the output mesh. What you see is what export writes.
- Orthographic/perspective toggle. Frame selected / frame all (frames the
  selection in any mode, or everything when nothing is selected).
- Navigation presets **Blender**, **Maya**, **3ds Max** (orbit/pan/zoom
  bindings), chosen in Preferences; default Blender.
- GPU id-buffer picking, refined on the CPU so anchors are exact.
- 60 fps orbit with 2 M display triangles on a mid-range GPU.

### 5.3 Selection
- Modes: vertex, edge, face, object (keys 1–4). Faces in face mode are
  **patches**; edges are output edges (CAD, split, generated) and vertices are
  output vertices.
- Click, Shift add, Ctrl remove (in every mode), box and lasso.
- **Visible-only** toggle: box/lasso select only what the camera sees.
- Double-click a face: select the face strip/loop through it. Double-click an
  edge: edge loop.
- Loop and ring commands; **every-other** variants (double-press the loop or
  ring key).
- Invert selection: in a component mode, inverts only within the objects
  being edited, not the whole scene.
- Hide / unhide all; **isolate** selected objects, with a "step" command that
  isolates the next object of the original selection each press, and an exit.
- Link and symmetry expand selections live (§6.11, §6.12).

### 5.4 Command palette
- **Command palette** (default `Ctrl+K`): every command with its shortcut,
  fuzzy search, and a per-command menu: change shortcut, toggle favourite.
  Favourites appear at the top and can be removed by right-click.
- There is no radial menu: an earlier version had one, and it was removed.
- Shortcuts: any key combo, plus **double-press** bindings (press the same key
  twice within 300 ms). Keymap stored as JSON; reset to defaults per command.

### 5.5 Outliner and layers
- Layers from the CAD file (STEP presentation layers via XCAF; Plasticity
  writes these). Objects with no layer go to the **active layer** on import.
  A layer with the same name as an existing one merges into it.
- Per layer and per object: visibility (eye) and lock (locked = can't be
  selected). Click-drag down the column to set many rows at once.
- Expand all / collapse all, search (Ctrl+F), rename, drag objects between
  layers, create/delete layers.
- Rename (double-click) focuses the field once. Enter or clicking anywhere
  else keeps the new name; Esc cancels. Esc in any text field only leaves
  that field (it doesn't close dialogs or leave the tool).

### 5.6 Statistics panel
- Shows totals, selection, or both (setting), for verts, edges, faces, tris,
  quads, n-gons, and the count of dormant edits. Each row can be turned off.

### 5.7 Preferences
- General: navigation preset, scene units, up axis (Y or Z), autosave interval.
- Appearance: every overlay colour (CAD edge, line, fill edge, selection,
  Connect arrow A and B, linked faces, symmetry plane), UI theme.
  Themes are JSON files; ship a dark and a light one, designed from scratch.
- Statistics: which rows and which totals.
- Keymap (also reachable from the palette).

### 5.8 Default keymap (own design; all rebindable)

| Action | Key | Action | Key |
| --- | --- | --- | --- |
| Vertex/edge/face/object mode | 1/2/3/4 | Command palette | Ctrl+K |
| Preferences | Ctrl+, | | |
| Preview mode | V | Wireframe in preview | Shift+V |
| Auto-fill on/off | Shift+A | Link on/off | Shift+L |
| Symmetry on/off | Shift+M | Mirror X / Y / Z | Alt+X / Alt+Y / Alt+Z |
| Connect | C | Surface Grid | G |
| Slice | S | Iso | I |
| Cut | K | Target weld | W |
| Collapse | Alt+C | Remove edge | Delete |
| Remove CAD edge (confirm) | Delete twice (one press on a CAD edge only arms it, with a toast, for 1 s) | Loop / ring | L / R (twice = every other) |
| Frame | F | Ortho/persp | O |
| Hide / unhide all | H / Alt+H | Isolate / step / exit | / then / again, Esc |
| Visible-only selection | Alt+V | Invert selection | Ctrl+I |
| Copy / paste rules | Ctrl+C / Ctrl+V | Undo / redo | Ctrl+Z / Ctrl+Shift+Z, Ctrl+Y |
| Rename panel | F1 | Bridge panel | F2 |
| UV panel | F3 | Snap on/off (Iso, Cut) | N |

Inside tools: `A` toggles **through** mode (continue past holes/boundaries),
`Tab` toggles a tool's direction, `X` toggles Iso's split mode, `Enter`
accepts, `Esc` cancels, `Backspace` removes the last cut point. `Esc`
precedence: cancel the active tool's pending action first, then leave the
tool, then exit isolate.

### 5.9 Feedback
- Every tool shows a one-line hint in the status bar.
- Refusals are toasts with a reason. Nothing fails silently.
- Long work (import, export, bridge sends) shows progress with cancel.
- The top bar has Undo and Redo buttons (disabled when there's nothing to
  undo or redo), next to the menus.

### 5.10 Preview, then Apply
The app never changes the mesh on its own, and nothing is written until the
user applies it.
- Every op that changes the output mesh (tools, edge edits, rule changes,
  auto-fill / link / symmetry toggles, paste) opens a **preview**: the
  viewport shows the result, and an Apply bar over the viewport says
  "Preview · Not applied yet" with **Compare**, **Discard** and **Apply**.
  Further changes join the same preview. Names, layers, visibility, locks
  and units apply at once.
- **Apply** (button or Enter, after the active tool had its turn, so Cut
  keeps Enter) keeps the changes as one undo step. **Discard** (button, Esc
  after the tool's own pending action, or Undo) restores everything.
- Save, export, Send to Blender and autosave refuse while a preview is open
  ("Apply or discard the preview first").
- **Compare** splits the viewport into Before | After with the same camera
  on both halves (navigation moves both; picking pauses until the compare
  closes). Switching auto-fill on or off opens the compare in preview
  display with a short explanation of what auto-fill does, and restores the
  display mode when the preview ends.

---

## 6. Tools

Each tool is a function in `core` that turns a selection + parameters into a
rule or edit change. The UI and the CLI share it. Every tool respects link and
symmetry. Acceptance criteria (AC) must become automated tests.

### 6.1 Segment tool panel (shared by Connect, Surface Grid, Slice)
- One panel for the three spread tools. Switching tool while it's open
  re-applies the current selection with the other tool (the user can flip
  between Connect, Grid and Slice and compare).
- Counts change with the mouse wheel and modifiers, a slider, or typed numbers.
  The panel shows **total lines added** across the selection.
- Count 0 removes the rule from the selected patches.
- Selecting patches that already have a rule loads its values into the panel.

### 6.2 Connect
Adds lines that follow the way a face bends.

- **Bend class** of a patch, from its surface:
  - plane: none, **except** a planar ring (annulus between two concentric
    circles), which bends "around" (lines run radially).
  - cylinder, cone, extrusion of a curve, any surface curved in one direction:
    one direction, **A** = around the curvature. Lines run along the straight
    direction (cylinder generators).
  - torus, sphere, surface of revolution, fillet corners, double-curved
    freeform: two directions **A** and **B**, each with its own count.
  - freeform B-spline: from principal curvature directions sampled on a grid;
    a direction counts if its curvature exceeds a threshold over most of the
    patch, and lines follow the closest iso direction.
- **Counts**: wheel+Shift changes **A**, wheel+Ctrl changes **B**. Each
  selected patch shows an arrow per available direction, coloured A or B.
- **Spacing**: even by arc length across the patch. On a periodic face, N lines
  give N segments around; on an open face, N lines give N+1 segments.
- **Start on seam** (periodic faces, default off): on puts the first line on
  the CAD seam, which depends on how the part was built and rotated in CAD.
  Off puts it at the **canonical angle**: where the face's phase reference
  direction (§4.1; world-X based unless the face was reached through a
  Link/symmetry/paste mapping) meets the surface. A cylinder spun about its
  own axis in CAD looks identical geometrically, so with start-on-seam off it
  gets identical lines.
- **Modifier+wheel**: while a segment tool is active and the cursor is over
  the viewport, Shift+wheel and Ctrl+wheel go to the tool and override any
  navigation binding that uses those modifiers. Plain wheel still zooms.
- **Multiple patches**: adjacent selected patches whose A directions join up
  across their shared edges form chains; their lines join end to end into
  continuous lines across the chain. Patches whose direction disagrees with
  their selected neighbours are tinted with a warning colour; `Tab` flips the
  A/B assignment on the hovered patch.
- **Neighbours**: lines end on the patch boundary. Their end vertices are shared
  with the neighbouring patch, so the neighbour's polygon picks them up and
  auto-fill connects them. Lines never extend into unselected patches.
- Flat patches (non-ring planes) can't be connected: toast suggests Surface
  Grid, Slice or auto-fill.
- **Connect vertices**: in vertex mode, Connect with 2+ selected vertices makes
  `Join` rules between consecutive vertices (a line over the surface).
- **AC**:
  - A full cylinder face with Connect A = 16 gives 16 lines along the
    generators, every line exactly on the surface (< 1e-6 × diagonal).
  - Changing 16 → 8 → 16 gives an identical output mesh.
  - Start-on-seam off: a cylinder whose seam is rotated 40° about its axis
    (same geometry, different seam) gets the same world-space lines as the
    unrotated one. Start-on-seam on: the lines move with the seam.
  - A torus face shows two arrows; A = 12, B = 8 gives a 12 × 8 quad grid.
  - Three fillet patches in a chain with A = 4 produce 4 continuous lines.

### 6.3 Surface Grid
- Per patch: a U × V grid of iso lines, evenly spaced by arc length (or in
  parameter space, toggle), clipped to the patch and its holes.
- Counting matches Connect: along a periodic direction, N lines give N
  segments, starting at the canonical angle (or the seam if start-on-seam is
  on), never doubling a line at the seam; along an open direction, N lines
  give N + 1 segments.
- Works on flat and curved patches. **Nothing carries over**: every selected
  patch gets its own grid; neighbours only share boundary vertices.
- Wheel+Shift = U count (number of constant-u lines), wheel+Ctrl = V count (constant-v lines), `Tab` swaps U and V.
- **AC**: a trimmed planar face with a circular hole gets a grid that stops at
  the hole, with no polygon crossing the hole.

### 6.4 Slice
Evenly spaced cuts across part of a body, like a stack of planes.

- **Scope**: object mode → the whole body; face mode → only the selected
  patches.
- **Orientation (auto)**: if the scope's curved faces share one axis of
  revolution (coaxial cylinders, cones, tori, revolutions), planes go
  **through that axis**, evenly spaced in angle, so lines radiate around it.
  Otherwise, planes are perpendicular to the longest straight CAD edge in the
  scope, evenly spaced along it.
- **Guide edge**: a panel button lets the user pick a CAD edge. A straight
  edge sets the plane normal; a circular edge sets the axis for radial planes.
- One count (wheel). Lines stop at holes and split lines unless **through**
  (`A`) is on.
- Running Slice again on the same scope replaces the rule.
- **AC**: slicing a filleted box body with 10 planes gives 10 closed loops that
  pass through the fillets. Slicing a revolved part gives radial lines through
  its axis.

### 6.5 Iso
Places one exact iso line where you click.

- Hover over a face: preview the iso line (u or v constant) through the cursor.
  `Tab` switches direction. Snapping (toggle `N`) to existing vertices and
  edge midpoints along the line's path.
- Click commits one line; the tool stays active.
- By default the line stops at holes and at the face boundary. **Through**
  (`A`) continues it past holes in the same face and across face boundaries:
  at the crossing point on a shared edge that is G1-continuous (normal angle
  < 5°), take the line's incoming 3D tangent; in the neighbour, pick the iso
  direction (u or v) whose 3D tangent at that point is within 15° of it, at
  the parameter value through that point, and continue. Neighbouring faces'
  UV axes don't need to line up; the match is in 3D. Stop where no direction
  matches, at a sharp edge, or when the walk returns to its start (a loop).
  The rule stores the pieces it produced as
  `IsoPiece { face, dir, value, seed: Option<f64> }`. `seed` (the click
  position, as a parameter along
  the iso line) picks which disconnected component of a trimmed iso line is
  meant when through is off; with through on it is `None` and every
  component on that face is used.
- **Split mode** (`X`, preview drawn in a different colour): the click adds
  **split lines** (§4.2) instead of an Iso rule, one per face the line
  crosses, sharing a group. Split lines divide the face into patches, so
  later Connect/Grid/Slice counts apply only to the side they're applied on.
  Use: a long curved face that needs dense lines in one area and few in another.
- **AC**: a split at mid-height of a cylinder lets the two halves carry
  different Connect counts; a through-iso on a fillet ring makes one closed loop.

### 6.6 Cut
Draws lines exactly where you click.

- Each click places a point, snapping (toggle `N`) to vertices, edge midpoints,
  edges, then face interior. Consecutive points are joined by a surface path
  (straight in UV on one face; shortest path over the display tessellation
  across faces, projected back exactly). Crossed edges get new vertices.
- Enter finishes, Esc cancels, Backspace removes the last point, closing on the
  first point makes a loop.
- Persistence: points snapped to CAD or face geometry keep their place when
  counts change. Points on generated edges follow their key (§4.5).
- **AC**: a cut across three faces of a box puts every vertex on the exact
  surface and splits each crossed polygon in two. After changing a Connect
  count, face-anchored cut points haven't moved.

### 6.7 Remove edges
- `Delete` in edge mode removes selected generated edges (a `RemoveEdge` edit).
  The rule stays; only those edges are suppressed. Works with ring/loop and
  every-other selections for quick thinning; loops and rings walk through
  fill edges for traversal but only generated edges end up selected. With
  auto-fill on, the polygons merged by a removal are simply re-filled.
- Removing a split line edge removes the split line (merges patches).

### 6.8 Collapse
- Collapses selected edges to one vertex. The merged vertex keeps the
  higher-priority anchor (§4.4). An edge between a generated vertex and a CAD
  vertex/edge always collapses onto the CAD side, so CAD boundaries stay
  intact. Refuses (with a toast) if the result would flip a face against the
  CAD normal or be non-manifold. Target Weld (§6.9) refuses the same cases.
- Collapsing an edge whose both ends have CAD anchors (CadVertex or CadEdge,
  whether or not on the same CAD edge) is allowed only after a
  confirmation, warns that the output will no longer match the CAD outline,
  and is output-only.
- **AC**: collapsing an edge with one end on a CAD edge leaves the vertex on
  that CAD edge.

### 6.9 Target Weld
- Click a source vertex (highlighted as "from"), then a target vertex
  ("to"), or drag from one to the other. The source merges into the target
  and the result keeps the target's anchor and position (§4.4);
  degenerate edges/faces are removed. Refuses (with a toast) if the result
  would be non-manifold. Tool stays active.
- A weld whose source has a higher-priority anchor than the target (e.g. a
  CAD vertex into a face point), or whose both ends have CAD anchors (as in
  §6.8, anchor-based), needs a
  confirmation, as in §6.8.
- **AC**: welding two vertices of a quad strip turns one quad into a triangle
  and the mesh still validates.

### 6.10 Remove CAD edge (advanced)
- `Delete` twice on a CAD edge: output-only merge of the two polygons it
  separates. Only allowed when the adjacent faces are G1-continuous or
  coplanar; otherwise refused with a reason. Patches and spread rules are
  unchanged (each side keeps its own patch and UV domain); only the output
  polygons merge, and auto-fill fills the merged polygon in a flattened
  domain (§7 step 1). Vertices on the removed edge stay unless they end up
  with only two edges on a straight-enough run, in which case they're
  dropped from the merged polygon.
- Warns once per session that this can make the output depart from the CAD
  shape. Never changes the B-rep.

### 6.11 Link (repeated shapes)
- A toggle, **off** each time the app starts.
- On import (in the background), the kernel groups faces of each body into
  **link sets** by congruence: same surface type and intrinsic parameters
  (radii, cone angle, extents), same boundary topology, matching edge lengths
  and an exact rigid transform (rotation + translation, optional reflection)
  confirmed by sampled distances. Use geometric hashing to keep it fast.
- With Link on, selecting a patch also selects its link set (tinted in the
  linked colour). Rules and edits made on any member apply to all members
  through the face-to-face mapping (parameters remapped, not 3D-projected).
  Connect, Grid, Slice (patch scope), Iso, Cut, Remove, Collapse and Weld all
  respect it, live.
- With Link off, edits affect only the picked face. Existing linked rules stay
  linked; "unlink" on a rule makes per-face copies.
- Performance: with large link sets and auto-fill on, updates must still meet
  §4.6 budgets.
- **AC**: on a plate with a 4 × 4 grid of identical holes, Connect A = 8 on one
  hole wall gives 8 lines in all 16, live.

### 6.12 Symmetry
- A toggle plus per-axis mirror planes **X, Y, Z**. In v1 the planes are the
  world planes through the origin. Body-local planes (through the body's
  bounding-box centre) are a v1.1 option.
- Partner faces are found by mirroring each face and matching congruent
  faces. Faces with no partner are unaffected.
- Mirrors rules and edits from Connect, Grid, Slice, Iso, Cut, Remove,
  Collapse and Weld. Works together with Link. Plane drawing can be turned off.
- **AC**: on a part symmetric about X, Connect on the left fillet appears on
  the right fillet; turning symmetry off and editing one side affects only it.

### 6.13 Copy and paste rules
- `Ctrl+C` with an object selected copies its split lines, rules and edits.
  `Ctrl+V` on one or more other objects maps them through face congruence (as
  Link does, across bodies). Applies where faces match; reports unmatched faces.
- **AC**: two identical bodies at different positions/rotations: copy/paste
  gives identical output meshes (up to the rigid transform).

### 6.14 Loop, ring, strips
- Edge loop through valence-4 vertices; face loop/strip through quads across
  opposite edges; edge ring across quad strips. Stop at poles, n-gons and
  boundaries; close if they return. Every-other variants select alternate
  elements along the result.

---

## 7. Auto-fill

A scene setting (on/off, saved with the scene, default **off** for new scenes,
so the app never adds edges the user didn't ask for; imported objects follow
it). Switching it is previewed like any other change (§5.10). It changes only the output mesh and is
re-evaluated live with everything else.

For every polygon with more than four sides (all polygons are simple by now, §4.6 step 6):

1. **Domain**: use the patch's UV domain, rescaled so a unit of u and of v map
   to similar 3D lengths (sample the metric). If the polygon spans patches
   (after §6.10), flatten the union of their display tessellation with LSCM.
2. **Input is always simple**: holes and periodic annuli were already split
   into simple polygons by the evaluator's bridging step (§4.6 step 6), so
   auto-fill only ever decomposes simple polygons in a planar domain.
3. **Decompose** with a dynamic program over diagonals, like optimal polygon
   triangulation but allowing quads: for each sub-chain (i, j) whose chord is
   a valid interior diagonal, choose either a triangle (i, k, j) or a quad
   (i, k, l, j) whose chords are all valid diagonals, maximising first the
   number of quads, then the summed quality. Quality = convex in UV,
   near-planar in 3D, low aspect ratio, edges aligned with the patch's iso
   directions. A quad is **admissible** if it's strictly convex in UV and
   its area is above a tiny epsilon; quality only ranks admissible quads, it
   never rejects them. Diagonal validity uses the exact predicates.
   Cost is O(n⁴) in polygon size; use it for n ≤ 48.
4. **Large polygons** (n > 48): constrained Delaunay of the boundary vertices
   only (no Steiner points), then greedy maximum-quality pairing of adjacent
   triangles into admissible quads.
5. Emit fill edges. No new vertices, ever.

Properties:
- Deterministic (same input → same output; ties broken by vertex key order).
- **Never blocks export.** Fallback chain: DP/CDT in UV → ear clipping in
  UV → ear clipping in the polygon's best-fit 3D plane → triangle fan from the
  first vertex. A polygon that only the fan could fill is flagged by
  validation and listed in the export dialog, but export still proceeds.
- Long thin triangles are a known weakness; the fix is user-driven (an Iso or
  Cut line, a weld). A "check" overlay highlights triangles with aspect > 10.

**AC**:
- Vertex count is unchanged by auto-fill.
- A strictly convex polygon with an even number of vertices, n ≤ 48 (the DP
  path), fills with quads only. Above 48 the greedy path is quad-dominant best
  effort with no all-quad guarantee.
- For polygons with ≤ 9 vertices (seeded generator: random star-shaped
  polygons, 10,000 cases), the quad count equals the maximum found by
  exhaustive search over all decompositions into triangles and **admissible**
  quads (the same strict-convexity rule as step 3), in every case.
- No flipped or overlapping faces in UV on the whole reference set.
- 2,000-face body fills in < 100 ms.

---

## 8. Import, save, export

### 8.1 Import
- File > Import CAD or drag and drop: `.step .stp .iges .igs`, and `.brep` for tests.
- Files with more than one object open an **import list** first: each object
  marked **Keep**, **Exclude**, or **Replace** (swap it for the same-named
  object already in the scene, carrying over that object's rules where faces
  still match). Search box, multi-select, select all / none.
- Import options (remembered): healing on/off, sewing tolerance, units.
- Runs in the import helper process (§3.4) with a progress bar; UI stays
  responsive.
- Post-import report: bodies, faces, fixes applied, faces that failed. If
  faces are missing after healing, the report says the file itself is probably
  damaged at export time and suggests re-exporting from the CAD tool, trying
  another format (IGES or a different STEP schema) if the problem repeats.
- The source file is never written.

### 8.2 Scenes
- Scene file `.ntopo`: zip with `scene.json` (format version, settings,
  layers, objects, split lines, rules, edits, camera), `cad/<body>.bbrep`
  (healed B-rep in OCCT's `BinTools` binary format, so reload doesn't need the
  source file),
  `cad/source.sha256` (phase-reference overrides are part of `scene.json`), `uv/<body>.bin` (UVs from §9.2 keyed by polygon
  vertex keys, with a stale flag). The output mesh is not stored; it's
  re-evaluated. Hard-edge overrides are edits (§4.5) and live in `scene.json`.
- Newer format → refuse with a clear message. Older → migrate; one migration
  test per version bump.
- Title shows "unsaved" for new scenes and a modified marker after changes.
  Recent files list (8). Autosave to `.ntopo.autosave` every 5 minutes;
  recovery offered after a crash.

### 8.3 Export
- File > Export: selected objects, or the whole scene when nothing is selected.
- Formats: **FBX** (binary 7.4), **OBJ** (+MTL), **glTF 2.0** (.glb).
- Options: triangulate on/off for FBX and OBJ (quads kept when off; glTF is
  always triangulated because it has no quad primitive), scale/units, up and
  forward axis, layers as hierarchy (FBX/glTF nodes, OBJ groups), materials
  from CAD colours, object names (after rename, §9.1).
- **Normals**: hard edges = CAD sharp edges (plus user overrides). Smooth
  normals come from the exact CAD surface normal at each vertex, split at hard
  edges; option for face-weighted normals instead. OBJ writes per-corner
  `vn` normals (split at hard edges) and also smoothing groups (`s`) for
  tools that read them; Blender's OBJ importer treats `s` only as smooth
  on/off and derives sharp edges from the split normals. FBX writes
  per-corner normals and edge smoothing; glTF splits vertices.
- If auto-fill is off, n-gons are exported as n-gons (FBX/OBJ) or triangulated
  (glTF), with a note in the export dialog.
- **Polygons with holes are never output**, auto-fill on or off: FBX, OBJ
  and Blender can't represent a face with a hole, and a single bridged face
  would repeat vertices. The evaluator's bridging step (§4.6 step 6) always
  splits them into simple polygons, so preview, statistics and export agree.
  The untouched cylinder (two rims) therefore shows as two n-gons in preview
  with auto-fill off.
- **AC**: exported files import into Blender 4.x, checked by a headless
  Blender script in CI (`blender -b -P`): FBX and OBJ match the output mesh's
  polygon count and unique vertex positions (after merging by distance for
  OBJ); glTF matches the triangle count (vertices are split at hard edges, so
  vertex counts differ). All formats: names, layer hierarchy, materials, and
  sharp edges (FBX/OBJ) or split normals (glTF) as expected.

---

## 9. Studio features

### 9.1 Batch rename (F1)
- Works on selected objects. Operations, each with live preview (old → new):
  - **Set name** (with a button that copies the active object's name in).
  - **Add** text as prefix or suffix.
  - **Remove** N characters from the start, end, or both.
  - **Replace** text (plain; regex optional).
  - **Number**: separator (e.g. `_`), start, step, zero-padding; and "remove
    numbering" to strip a trailing number added before.
- Name collisions are highlighted. Each apply is one undo step.

### 9.2 Quick UV unwrap (F3)
- Selected objects (or all). Seams at hard edges (from CAD sharpness), then
  xatlas charting where distortion is too high, then packing.
- Options: padding (px at a chosen texture size), rotation allowed, uniform
  texel density across objects, checker preview on/off with tile count.
- It's a starting point, not a UV editor. UVs are invalidated (marked stale)
  on polygons that change afterwards.

### 9.3 Bridge to DCC apps (F2)
- One row per host (Blender, Maya, 3ds Max) with connection status and a Send
  button.
- **Protocol**: TCP on `127.0.0.1` only. Default ports 47110 (Blender), 47111
  (Maya), 47112 (3ds Max), configurable. Frames: `header_len` (u32,
  little-endian) + UTF-8 JSON header (`version`, `type`, `token`,
  `payload_len`) + `payload_len` bytes of binary payload (little-endian
  arrays, layout described in the header). Types `hello`,
  `push`, `ack`, `error`. A per-install random token, shown in the app and
  pasted once into each addon, authenticates the connection.
- **Send**: selected objects, or the whole scene including hidden layers when
  nothing is selected. Payload: objects with names, layers, transforms,
  polygons, normals, hard edges, UVs, material names/colours. Re-sending an
  object with the same name replaces it in the host (option: keep both).
- **Hosts**: layers become Blender collections, 3ds Max layers, Maya display
  layers. Maya needs unique names: the addon suffixes duplicates and reports
  them back so the app can show which names changed.
- **Addons** ship in `addons/`, installed once. They listen on their port
  without blocking the host's UI. Blender: a `bpy.app.timers` callback polls
  a non-blocking socket. Maya: a background thread reads the socket and hands
  each complete message to `maya.utils.executeDeferred` so scene changes run
  on the main thread. 3ds Max: a `QTimer` (via the bundled PySide) polls a
  non-blocking socket and applies changes through `pymxs`.
- **AC**: sending 50k faces arrives in Blender in < 1 s with layers, names,
  sharp edges and UVs intact. In CI (`blender -b`), timers may not fire, so
  the test calls the addon's poll function directly in a loop.

### 9.4 Update notice
The app can tell the user that a newer release exists. It never downloads,
installs or runs anything; it only links to the release page.

- **Source**: exactly one URL,
  `https://api.github.com/repos/SilasVogt/newtopo/releases/latest`, over
  HTTPS (rustls with bundled web PKI roots). Redirects are not followed; the
  request has a global timeout and the reply a size cap. Only `tag_name`,
  `draft` and `prerelease` are read. The tag must be a plain
  `vMAJOR.MINOR.PATCH`; drafts, pre-releases and anything else are ignored.
- **Link**: built locally from the parsed version
  (`https://github.com/SilasVogt/newtopo/releases/tag/v<version>`), never
  taken from the reply, and checked again before the browser opens it.
  No release text from the reply is shown.
- **Privacy**: the request carries no user data: a `User-Agent` with the
  app version and the fixed `Accept` / `X-GitHub-Api-Version` headers.
  No account, token or telemetry.
- **When**: opt-in. On first launch the app asks once ("Check once a day" /
  "Don't check"; Esc counts as don't). If on, it checks at most once a day
  at startup, in the background, and stays silent on errors. *Help > Check
  for updates* checks on demand and reports every outcome. Preferences >
  General has the switch and a "Check now" button. Setting
  `NEWTOPO_NO_UPDATE_CHECK` disables the automatic check and the question
  (for packagers and sandboxes); headless and screenshot runs never check.
- **UI**: a newer version shows an accent "Update x.y.z" chip in the top bar
  (and a toast after an automatic check). The chip opens a dialog with
  *Open release page*, *Later* and *Skip this version*; a skipped version
  is not announced again by automatic checks.

### 9.5 Mesh mode (after Finish)

The rule stage (§4–§7) is procedural and stays editable. When the artist is
happy with it, **Finish** bakes a body into a plain polygon mesh and the body
enters **mesh mode**, where a small set of mesh tools edit that mesh
directly. Design decision: **bake, then edit**. Mesh edits apply to the baked
mesh, never back to rules; the rules are locked once the body is baked.

#### 9.5.1 Finish (bake)
- Per body, from the evaluated output mesh (§4.6, after auto-fill). Finish is
  one undo step; undoing it returns to the rule stage and drops every mesh
  edit made after it. There is no way back into the rule stage that keeps
  mesh edits: "Back to rules" (with a confirmation that names how many mesh
  edits will be lost) re-evaluates the locked rules and discards the mesh.
- **Kept**: vertex positions; polygons with their winding; the CAD face of
  each polygon (its *group*: normals, materials, export grouping); the exact
  per-corner CAD normals; per-corner UVs if §9.2 ran; edge flags **CAD**
  (lies on a CAD edge) and **hard** (§8.3, including `SetHard` overrides);
  **pinned** vertices (CAD vertices and poles).
- **Dropped**: anchors, keys, surface paths, the line/fill/split distinction
  and dormant edits. Rules, split lines and edits stay in the scene file,
  read-only, so "Back to rules" works after a reload.
- Refused (with the validation messages) if the evaluated mesh is not a
  valid manifold polygon mesh. The evaluator's validation gate (§4.6 step 8)
  should make this unreachable.
- The baked mesh is saved in the `.ntopo` (`mesh/<body>.json`, format
  version bump with a migration test) and exported with the §8.3 rules;
  untouched polygons keep their exact CAD normals.

#### 9.5.2 Rules shared by all mesh tools
- Every tool is a pure function: mesh + selection + parameters → new mesh,
  or a refusal with a reason. Each apply is one undo step. The tool returns
  the new mesh, a map from old to new vertex indices, the selection after
  the edit, and a one-line report (counts before/after, what it did, runs it
  skipped and why).
- **Feature edges**: open boundary edges and UV seams (always), CAD edges and
  hard edges (each with a "Preserve" toggle, both on by default). A UV seam
  is an edge whose two sides carry different UVs.
- **Vertex roles**: *free* (no feature edge), *on a feature curve* (exactly
  two feature edges of the same kind) or *corner* (pinned, the end of a
  feature curve, a junction of three or more, or where two kinds meet).
  Corners never move or disappear. Feature-curve vertices move or merge only
  along their curve. A closed feature loop keeps at least three vertices.
- **Never**: change the topology (Euler characteristic, number of open
  boundary loops, number of connected parts; the one exception is Merge by
  Distance welding open boundaries together, §9.5.7), create a non-manifold edge or
  vertex, a duplicate polygon, a zero-length edge or zero-area polygon, a
  concave triangle or quad, or a polygon whose normal is more than the
  normal limit (default 60°) away from the mean of the CAD normals at its
  corners (a flipped face). "Further from the result" (deviation limits)
  means from the polygons as exported: a quad counts the worse of its two
  triangulations.
- **Attributes**: a vertex that stays where it was keeps its exact normals
  and UVs. A vertex that moves along existing edges gets normals and UVs
  interpolated along them, per side of each seam. Polygons keep their group.
- **Selection**: edge runs (chains of selected edges, split where three or
  more selected edges meet; a run that leaves a junction and returns to it
  is closed), or **all feature runs** of the mesh, for Cleanup and
  Resample; vertices for Vertex Slide and Merge by Distance (the result
  carries the vertex selection after the edit too).
- Refused with a reason: empty mesh, invalid mesh (first problem named),
  parameters out of range, empty selection, a selected pair that is not an
  edge, a vertex that is not in the mesh, "all feature runs" on a mesh
  without feature edges.

#### 9.5.3 Decimate
Reduces the polygon count while keeping the shape, boundaries and feature
edges.

| Parameter | Default | Meaning |
| --- | --- | --- |
| Mode | Collapse | Collapse (to a target) or Planar (merge flat areas) |
| Target | 50 % | Ratio of the current triangle count, or a triangle count (an n-gon counts n − 2) |
| Max deviation | off | No baked vertex may end up further than this from the result |
| Preserve CAD / hard edges | on / on | See §9.5.2 |
| Keep quads | on | Prefer quad-preserving merges; join triangle pairs afterwards |
| Planar angle | 5° | Planar mode: merge polygons whose normals differ by at most this |
| Normal limit | 60° | §9.5.2 |

- **Collapse**: repeatedly merge one vertex into a neighbour, cheapest first,
  until the triangle count reaches the target or no valid merge is left.
  The kept vertex stays where it is (*subset placement*), so every result
  vertex is a baked vertex that lies exactly on the CAD surface and keeps its
  exact normal. Cost: the quadric error of the merged vertex (sum of squared
  distances to the planes of the polygons around both vertices, plus
  constraint planes along feature edges so feature curves keep their shape).
  Merge targets are edge neighbours and, for free vertices with "Keep quads",
  the opposite corner of a quad (removes one quad, keeps its neighbours as
  quads). With "Keep quads" a merge that turns quads into triangles or
  worsens polygon shape costs extra. Afterwards adjacent triangles of the
  same group are joined into quads when the result is convex, folds by at
  most 40° and stays within the deviation limit, best-shaped pairs first.
- **Planar**: join neighbouring polygons across non-feature edges while the
  joined polygon's normal stays within the planar angle of every polygon
  merged into it and the result is one simple polygon (shared edges must form
  one chain); dissolve vertices left with two edges that run straight
  through (same feature kind on both sides); refill the resulting n-gons with
  the §7 decomposition in their best-fit plane (no new vertices); join
  triangle pairs as above. No target.
- Not reaching the target is not a refusal: the result is returned with the
  report "stopped at N triangles (target M)".
- **AC**:
  - A cube with 8 × 8 quads per side, Collapse to 10 %: target reached, the
    result is still exactly the cube (sampled Hausdorff distance < 1e-9), all
    8 corners present, all 12 cube edges still flagged with their full
    length, corner normals exact.
  - The same cube, Planar: 8 vertices, 6 quads.
  - A 32 × 16 UV sphere to 30 %: at least half the polygons are quads,
    Hausdorff distance < 8 % of the radius; with a deviation limit of 1 %
    of the radius no baked vertex is further than that from the result.
  - A torus keeps χ = 0; a cylinder keeps both rims (every rim vertex on the
    rim circle) and its UV seam, and every side corner's UV still matches its
    position.
  - Same input, same output (bit-identical); ratio 1 leaves the mesh
    unchanged.

#### 9.5.4 Cleanup
Tidies dense or jagged edge runs, typically where two parts were joined and
the seam between them has too many, too close or zig-zagging vertices.

| Parameter | Default | Meaning |
| --- | --- | --- |
| Merge distance | 0.01 % of the bounding-box diagonal | Merge vertices that share a polygon and are closer than this (one of them on the selection) |
| Short edge | 0.5 % of the diagonal | Collapse selected edges shorter than this |
| Straight angle | 5° | Dissolve run vertices where the run turns by at most this (0 = off) |
| Slide | on | Slide run vertices along their cross edges to straighten the run |
| Tolerance | 0.1 % of the diagonal | No baked vertex may end up further than this from the result |
| Preserve CAD / hard edges, normal limit | on / on, 60° | §9.5.2 |

- Steps, repeated until nothing changes (at most 32 rounds): merge close
  vertices; collapse short selected edges (run ends and junctions stay);
  slide; dissolve straight-through run vertices into their nearer run
  neighbour (the one that leaves better-shaped polygons). When two vertices
  merge, the one with the stronger role is kept (corner over feature curve
  over free, run vertex over others, then the lower index).
- **Shape guard** (slide and dissolve): a step may not leave its worst
  neighbouring polygon worse than it was before or than a minimum shape
  (smallest corner about 14° for a quad), so dissolving a long straight run
  can't pile all cross edges into one fan of slivers. Merging close
  vertices and short edges is exempt: it removes degenerate shapes and may
  pass through worse ones.
- **Slide**: a free run vertex moves along one of its non-run edges (at most
  75 % of the way) to the point closest to the line through its two run
  neighbours, if that at least halves its distance to the line. Because it
  moves along an existing edge it stays on the old surface. Vertices on a
  feature curve never slide off it.
- **AC**: a plane whose middle row zig-zags by ±1 % comes out with that run
  straight (every run vertex within 1 % of the zig-zag of the line) and fewer run
  vertices; on a curved height field every baked vertex stays within the
  tolerance; a sliver row 1e-5 above the run merges into it (one vertex per
  column fewer, no edge shorter than the merge distance left); a 1e-3 run
  edge collapses; applying Cleanup again to its own result and selection
  changes nothing; on a cube with "all feature runs" the cube stays exactly
  the cube.

#### 9.5.5 Resample
Respaces the vertices of edge runs evenly by arc length, optionally to a new
vertex count or edge length.

| Parameter | Default | Meaning |
| --- | --- | --- |
| Spacing | Even | Even (same count), Count (vertices per run; an open run counts both ends) or Edge length |
| Corner angle | 30° | Run vertices where the run turns by more than this stay fixed |
| Fill n-gons | on | Refill polygons that grew beyond four sides (§7 decomposition) |
| Preserve CAD / hard edges, normal limit | on / on, 60° | §9.5.2 |

- Fixed: open run ends, corners (§9.5.2), vertices with feature edges off the
  run, and corners sharper than the corner angle. A closed run without any
  fixed vertex keeps its lowest-index vertex. The pieces between fixed
  vertices are resampled independently; a vertex count is shared out over
  the pieces by length (largest-remainder rounding, at least one edge each).
  Edge length gives each piece the nearest whole number of edges, at least
  one.
- New positions lie on the old run (the polyline), at equal arc length.
  Old vertices are matched to new positions in order, moving as little as
  possible; spare old vertices are merged into a run neighbour, missing ones
  are inserted into the run's edges (the polygons beside the run grow and
  are refilled when "Fill n-gons" is on).
- With "Fill n-gons" on, each run's grown polygons are refilled before
  its result is checked (a triangle that gains one vertex is a quad with a
  straight corner until it is split).
- A run whose new spacing would fold or flatten a polygon beside it is left
  unchanged and listed in the report; if every run fails the tool refuses.
  The same holds for a count smaller than a run's fixed corners allow, and
  for more than 10 000 edges on one run. A closed run keeps at least three
  vertices. Vertices where the selection does not run straight through
  (junctions shared by several runs) stay fixed.
- **AC**: an unevenly spaced straight run comes out with equal edge lengths
  (1e-9) and resampling again changes nothing; a 10-edge run resampled to
  21 / 6 / 11 vertices has 20 / 5 / 10 equal edges, no n-gons and the same
  surface; an L-shaped run keeps its corner; a jittered closed ring on a
  cylinder comes out evenly spaced (chords within 1 %), and with 36
  vertices keeps the UV seam; Count 12 on the rims of a 24-segment cylinder
  gives two closed rims of 12 edges with every vertex on the old rim; on a
  cube with Edge length 0.5, every cube edge has 4 edges and the cube is
  unchanged.

#### 9.5.6 Vertex Slide
Moves vertices along one of their edges, for example to line a vertex up
with a neighbouring loop. Built for dragging: the edges are chosen once
(a *plan*), then the factor changes on every mouse move.

| Parameter | Default | Meaning |
| --- | --- | --- |
| Edge | (from the drag) | Per vertex, an explicit neighbour; or the allowed edge whose direction is closest to a direction hint (the drag direction) |
| Factor | 0.5 | 0 = where the vertex is, 1 = on the neighbour |
| Even | off | Off: every vertex moves the same fraction of its own edge. On: every vertex moves the same distance, the factor times the shortest slide edge |
| Clamp | on | Keep the factor in [0, 1]. Off: vertices may move past either end of their edge, along its line |
| Preserve CAD / hard edges, normal limit | on / on, 60° | §9.5.2 |

- **Which edges**: a free vertex may slide along any of its edges; a vertex
  on a feature curve only along its two curve edges (so a boundary vertex
  stays on the boundary, a CAD/hard-edge vertex on its edge; switching the
  Preserve toggle off for CAD or hard edges is the explicit override);
  corners never slide. With a direction hint, edges that lead to another
  sliding vertex are not candidates, so a selected loop slides along its
  cross edges. Ties go to the lower vertex index.
- **Skipped** (listed in the report): corners, an explicit neighbour that
  the feature rules don't allow, a vertex whose neighbour also slides. If
  every vertex is skipped the tool refuses with the first reason.
- **Validity**: the polygons around the moving vertices may not become
  degenerate, concave (triangles and quads), tilted past the normal limit,
  or flipped. A polygon that was already concave or past the limit before
  the slide may change, but its normal may not turn further from its CAD
  normal and it must keep a non-zero area and its orientation. If the factor
  is not valid, the slide stops just short (0.01 %) of the largest valid
  factor between 0 and the asked one (bisection), so nothing is left on
  the edge of degenerate, and reports "stopped at factor F (asked G)".
  The normal limit is checked against the CAD normals the result will
  carry (interpolated along the edge for the moving corners);
  it refuses only if no movement at all is possible.
- **Landing**: a vertex that reaches exactly 1 merges into its neighbour
  (same feature and topology checks as every merge); if that merge is not
  allowed the slide refuses. Positions just short of 1 are plain moves.
- **Attributes**: within the edge, normals and UVs interpolate along it
  (per side of each seam); past either end, the vertex keeps the
  attributes of the nearer end.
- A slide along an edge in [0, 1] keeps every vertex on the old surface;
  the topology never changes.
- **AC**: factor 0 returns the input unchanged; factor t on a plane puts
  the vertex at exactly `lerp(from, to, t)` and moves nothing else; factor
  1 removes the vertex and leaves its neighbour where it was; Even moves
  two vertices with 1.5- and 0.5-long edges by 0.25 each at factor 0.5;
  unclamped, −0.5 extends backwards and −3 stops just before the polygons
  behind the vertex fold; a plane border vertex slides only along the
  border, a cube-edge vertex only along the cube edge (the cube stays
  exactly the cube) unless CAD and hard edges are not preserved; a pinned
  corner is refused; repeated applies of one plan equal one-shot calls.

#### 9.5.7 Merge by Distance
Merges vertices that are closer than a distance: duplicates, cracks between
parts that should be one surface, and very short edges.

| Parameter | Default | Meaning |
| --- | --- | --- |
| Scope | whole mesh | The whole mesh or the selected vertices |
| Distance | 0.01 % of the bounding-box diagonal | Vertices closer than this merge |
| Mode | All | All pairs; Connected (only pairs joined by an edge); Unconnected (only pairs that share no polygon: welds, never collapses an existing polygon) |
| Position | Centroid | Centroid (free survivors move to the mean of their cluster) or Survivor (the survivor stays) |
| With unselected | off | With a vertex scope: also merge selected vertices with unselected ones |
| Preserve CAD / hard edges, normal limit | on / on, 60° | §9.5.2 |

- **Clusters**: vertices are visited strongest first (pinned, corner,
  feature curve, free; then lower index). Each vertex not yet in a cluster
  becomes a *survivor* and takes every vertex closer than the distance to
  it that may join: a pinned vertex never does; a vertex sharing a polygon
  with the survivor follows the merge rules of §9.5.2 (feature-curve
  vertices only along their curve, corners never); a vertex sharing no
  polygon (a weld) may join unless it is pinned. Members are within the
  distance of their survivor, so a dense row can't chain into one point.
- **All at once**: every cluster is applied together, because closing a
  crack needs both ends of every glued edge at the same time. Polygons that
  lose corners shrink, polygons left with fewer than three corners
  disappear, a polygon pinched at a repeated corner splits, and two polygons
  left on the same corners with opposite winding (a closed pocket) both go.
  Edge flags follow their vertices; collapsed edges lose theirs; edges
  whose two sides now carry different UVs become seams.
- **Refusals per cluster**: the result around every changed polygon must
  stay manifold (edges used at most twice and once per direction, one fan
  per vertex), without duplicate polygons, degenerate, concave or flipped
  polygons, and within the normal limit; no survivor or pinned vertex may
  be left without polygons (a whole piece of the surface collapsing).
  Clusters around a failure are
  dropped and the rest is tried again; the report counts the dropped
  clusters by reason ("2 cluster(s) not merged: it would make a
  non-manifold vertex"). Nothing to merge is not a refusal.
- **Centroid**: after merging, a survivor that is free moves to the mean of
  its cluster if that move is valid; a survivor on a feature or a corner
  stays, so features never move. The rounds repeat until nothing merges, so
  the tool applied to its own result changes nothing.
- **Attributes**: a corner that was a merged vertex keeps the normal and UV
  it had when the survivor has none in that polygon's CAD face or UV chart
  (welds across charts); otherwise it takes the survivor's.
- **Topology**: merges inside the surface keep it (§9.5.2); welds change
  it only by joining open boundaries (fewer boundary loops or parts).
- **AC**: two sheets with duplicated vertices 1e-6 apart along one edge
  weld into one sheet (5 vertices removed, one boundary loop, one part, the
  topology of the uncut sheet), each side keeps its own UVs and the glued
  edges are seams; interior seam vertices end halfway (Centroid) or on the
  first sheet (Survivor); Connected merges nothing there; a 1e-5 edge in a
  plane collapses (Connected, the topology unchanged, no seam appears);
  a pinned cube corner with an edge neighbour 1e-6 away keeps its exact
  position and the cube keeps its edges; two triangles touching at one
  corner are not welded (non-manifold, reported); a mesh with nothing that
  close comes back unchanged with "No vertices closer than the merge
  distance"; applying the tool to its own result changes nothing.

#### 9.5.8 Testing
In addition to the ACs above (unit tests on synthetic meshes with exact
normals: height fields, cubes, cylinders with UVs, UV spheres, tori):
- **Baked models**: every mock reference model with a Grid on every face,
  baked and run through every tool and mode.
- **Property tests**: random synthetic meshes, optionally jittered, and
  random sequences of tool calls with random parameters and selections.
  After each call either the tool refused with a reason, or the result is
  valid (§9.5.2), unflipped, has the same topology and pinned vertices,
  is identical when the call is repeated, and keeps its tool-specific
  guarantee (Decimate: vertices are a subset of the input, triangle count
  never grows; Cleanup: deviation within the tolerance; Vertex Slide,
  clamped: every vertex on the input surface; Merge by Distance: vertex
  count never grows, applying it again at the same distance changes
  nothing).

---

## 10. Headless CLI and scripting

```
newtopo-cli info model.step                         # bodies, faces, surface types, layers, link sets, symmetry
newtopo-cli run script.json -o out.ntopo            # apply a JSON op script
newtopo-cli validate in.ntopo                       # invariants + stats; non-zero exit on failure
newtopo-cli export in.ntopo --format fbx -o out.fbx [--triangulate] [--up y]
newtopo-cli render in.ntopo --view iso [--preview] -o shot.png   # headless wgpu
```

The script format is what the UI records with "record ops" (for bug reports
and tests). Selections refer to CAD ids, patch ids or keys, so scripts are
deterministic:

```json
{ "version": 1, "input": "filleted_box.step",
  "settings": { "autofill": true, "link": false, "symmetry": [] },
  "ops": [
    { "op": "connect", "patches": { "faces": [3, 4, 5] }, "a": 8 },
    { "op": "grid",    "patches": { "faces": [1] }, "u": 4, "v": 2 },
    { "op": "iso",     "face": 7, "dir": "u", "value": 0.5, "seed": 0.25, "split": true },
    { "op": "collapse","edge": { "line": [2, 0], "crossing": [3, 1, 0] } } ] }
```

---

## 11. Testing and quality

- **Reference models are generated in code** via `make_test_shape`
  (`crates/testmodels` → `.step`): box, filleted box, cylinder with chamfers,
  plate with a 4 × 4 hole grid, revolved bottle, torus, sphere (poles),
  cone with a sharp tip (apex), torus trimmed by one hole, X-symmetric bracket,
  6-fold flange, small assembly with layers, names and colours. Optionally the
  public-domain NIST CAD test models for a large-file benchmark (downloaded at
  test time, not committed).
- **Unit tests** for every AC in §6–§9.
- **Property tests**: seeded random sequences of tool calls on every reference
  model. After each step either the output validates or the tool refused
  the step with a reason (Collapse and Weld must refuse results that flip a
  face against the CAD normal or are non-manifold); undoing everything returns the
  initial document exactly; save → load → evaluate gives the same output.
- **Mesh mode** (§9.5.8): baked reference models through every mesh tool,
  and property tests with random meshes and random tool sequences.
- **Golden tests**: CLI scripts → exported OBJ compared by topology hash and
  positions within tolerance (`insta`).
- **Visual tests**: `newtopo-cli render` per tool on reference models, diffed
  with a perceptual threshold, attached to PRs for a human to look at.
- **Robustness**: fuzz the STEP/IGES path with malformed files. The import
  helper may crash or time out; the app must not, and must report the failure.
- **Performance budgets** (`criterion`, CI fails on > 20% regression): §4.6
  budgets, §7 fill budget, import of a 20 MB STEP < 8 s, save/load < 1 s.
- GPU-using tests run through `~/.agents/bin/gpu-lock` on Silas's machines.

---

## 12. Delivery phases

Each phase: pushed branch, PR, green CI, `newtopo-cli render` screenshots in
the PR, and Silas's on-screen check.

| Phase | Scope | Exit criteria |
| --- | --- | --- |
| **P0 Skeleton** | Cargo workspace; `occt-shim` building and linking OCCT on Linux + Windows CI; `CadKernel` + `MockKernel`; `testshapes` feature and `crates/testmodels` generating the reference models; eframe window with a wgpu viewport; `newtopo-cli info` (bodies, faces, surface types, layers; link sets and symmetry are added in P5) | CI green on both OSes; `info` prints stats for every reference model. **Do the FFI build first.** |
| **P1 Import and view** | Import helper + list dialog, healing report, layers, outliner, navigation presets, view cube, selection (modes, box/lasso, visible-only, hide/isolate), command palette (search, run, favourites; shortcut editing arrives with the keymap editor in P6), statistics | Silas opens the reference models and a real Plasticity export; 60 fps orbit. |
| **P2 Document engine** | Patches, split lines, rules, anchors, keys, arrangement, incremental evaluation, undo, save/load, preview mode, `run`/`validate`/`render` CLI. Engine-level `Grid` and plain `Iso` rules (no tool UI yet) to exercise it | Property tests green using split lines, Grid and Iso rules, undo and save/load; closed edges, seams and poles validate on every reference model. |
| **P3 Core loop** | Connect, Surface Grid, Slice, shared panel, auto-fill, loop/ring/every-other | §6.1–6.4, §6.14 and §7 AC pass. First internal milestone: import → lines → fill → preview. |
| **P4 Precision and cleanup** | Iso (through, split), Cut, Join, Remove edge, Collapse, Target Weld, Remove CAD edge, dormant edits (the radial menu once planned here was removed) | §6.5–6.10 AC pass. |
| **P5 Repeat and ship** | Link, Symmetry, copy/paste rules, OBJ/glTF/FBX export with normals and hard edges, Blender headless import test | §6.11–6.13 and §8.3 AC pass. **First usable release.** |
| **P6 Studio** | Batch rename, quick UV, Blender bridge + addon, themes, keymap editor, autosave | §9 AC pass for Blender. |
| **P7 Polish** | Maya and 3ds Max addons, macOS build, user guide in `docs/` | — |

---

## 13. Open decisions for Silas

Defaults apply if no answer arrives.

1. **Open source or commercial?** Default: private repo; decide before P6.
   OCCT's LGPL allows closed source with dynamic linking.
2. **Name.** Default: `newtopo` as a working name; pick a product name before
   any public release and check it isn't confusable with existing products.
3. **Connect across unselected neighbours.** Default (§6.2): lines stay inside
   the selection and only share end vertices with neighbours. Alternative:
   carry lines automatically through tangent-continuous neighbours.
4. **Symmetry planes.** Default: world planes in v1, body-local in v1.1.
5. **FBX writer.** Default: own Rust writer; if it slips, P5 ships OBJ + glTF
   and FBX moves to P6.
6. **Auto-fill default.** Decided (Silas, 2026-11-18): off for new scenes;
   turning it on is previewed with a before/after compare (§5.10, §7).

---

## 14. Definition of done (v1)

- All §6–§9 acceptance criteria pass in CI on Linux and Windows.
- A user can take a Plasticity STEP export of a filleted hard-surface part to
  a preview with only tris and quads, export it, and open it in Blender with
  matching counts, names, layers and sharp edges, without touching a DCC tool
  for the retopology.
- The app never crashes on any reference model or fuzzed import file (the import helper may fail and is reported); every refusal explains itself. In-process kernel calls after import are covered by the property tests, not by a no-crash guarantee.
- `docs/` has a short user guide with screenshots of every tool, written fresh.
