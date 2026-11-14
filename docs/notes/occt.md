# OCCT worker notes

Provenance entries and open questions from the `wip/occt` worker, for the lead
to fold into `docs/PROVENANCE.md` and `docs/QUESTIONS.md`. Sources: SPEC.md,
the Open CASCADE 8.0.1 reference documentation and headers, own design.

## Provenance

| Area | Source |
| --- | --- |
| `occt-shim` cxx bridge, id tables, error conversion (`rust::behavior::trycatch` catching `Standard_Failure` and every C++ exception) | cxx docs (custom `trycatch`), own design |
| Deterministic face/edge/vertex ids | `TopExp::MapShapes` traversal order (OCCT docs) |
| Wires, coedge orientation, outer wire | `BRepTools_WireExplorer` on the FORWARD face, `ShapeAnalysis::OuterWire` (OCCT docs) |
| Seam / degenerate / closed flags | `BRep_Tool::IsClosed(edge, face)`, `BRep_Tool::Degenerated`, `TopExp::Vertices` |
| u/v closedness of a face | own design: the two pcurves of a seam differ by a shift in u or in v |
| Dihedral angle | own design: mean angle between oriented normals at 5 interior edge samples, normals from each face's pcurve |
| Normals at poles/apexes | own design (SPEC §4.1 "limit of normals sampled just off the pole"): step from the singular uv towards the face's uv-box centre by 1e-7…1e-2 of the distance until du×dv is non-zero |
| Projection onto trimmed faces | `GeomAPI_ProjectPointOnSurf` restricted to the face's uv box, solutions filtered by `BRepTopAdaptor_FClass2d`; plus the closest point on every boundary edge (`GeomAPI_ProjectPointOnCurve` + endpoints) mapped through its pcurve. Own combination. |
| Classification | `BRepTopAdaptor_FClass2d`, cached per face |
| Tessellation | `BRepMesh_IncrementalMesh` per body (re-meshed when the tolerance changes, after `BRepTools::Clean`), `Poly_Triangulation` nodes/UVs; normals from the surface; winding flipped for REVERSED faces |
| Healing | `ShapeFix_Shape` (+ `ShapeExtend_MsgRegistrator` message keys), `BRepBuilderAPI_Sewing` only when the body has free edges, closed shells → solids via `ShapeFix_Solid::SolidFromShell`, `BRepCheck_Analyzer` per face for the failed-face list |
| XCAF import | `STEPCAFControl_Reader` / `IGESCAFControl_Reader`, `XCAFDoc_ShapeTool` (free shapes, components, referred shapes, locations), `XCAFDoc_ColorTool`, `XCAFDoc_LayerTool`, `TDataStd_Name`; unit conversion through `XCAFDoc_DocumentTool::SetLengthUnit` |
| Import helper protocol (`progress <f> <msg>` lines, `warning: ` stderr lines, `sidecar.json` v1) | own design |
| Test-shape builder | `BRepPrimAPI_*`, `BRepFilletAPI_*`, `BRepAlgoAPI_*`, `GeomAPI_Interpolate`, `BRepPrimAPI_MakeRevol/Prism`, `STEPCAFControl_Writer` |
| Reference model shapes and dimensions | own design (SPEC §11 list) |

## Decisions (questions the spec does not answer)

1. **Wire orientation convention.** Wires are reported for the face in FORWARD
   orientation: outer wire counter-clockwise in (u, v), holes clockwise.
   `FaceTopo::reversed` says the outward normal is −(du × dv); `face_eval` and
   `tessellate` already apply it. `CoEdge::reversed` is the edge orientation in
   that FORWARD wire, which is exactly what `edge_pcurve_eval` needs to pick a
   seam's pcurve.
2. **Body splitting.** One body per XCAF leaf shape. A leaf holding several
   solids/shells is split into `name.1`, `name.2`, … (loose faces of the same
   leaf become one extra body). Leaves with no faces are skipped with a warning.
3. **IGES loose surfaces.** IGES files usually have one root per trimmed
   surface. All leaves that are only loose faces are merged (in world space)
   into one body `Surfaces`, so sewing can join them; a warning says so.
4. **Colour.** sRGB 0..1 as written in the file. Lookup order: instance label,
   prototype label (surface colour, then generic), the shape itself, its first
   coloured sub-shape label, then the parent assembly. Face-level colours are
   not reported per face (first one wins for the body).
5. **Layer.** First layer of the instance label, else the prototype, else the
   parent assembly. The contract has one layer per body.
6. **Assembly path.** Names of the enclosing assemblies, root first, *without*
   the body's own name. A single-part STEP file whose part is wrapped in a
   one-component product (common in exports) yields `path = [part name]`.
7. **Units.** `--units` sets the XCAF document length unit before transfer,
   so OCCT scales STEP/IGES geometry. BREP has no unit and is never scaled
   (the import adds a warning saying so). B-rep files are read in Rust and
   passed as bytes, so non-ASCII paths work on Windows too.
15. **Instances get their own shapes.** XCAF instances of one prototype share
   a `TShape`; every body after the first gets a `BRepBuilderAPI_Copy`, so
   meshing or healing one body never changes another.
8. **Healing report.** ShapeFix status flags are only reported when faces were
   actually rebuilt or ShapeFix logged messages (its "shells fixed" flag is set
   even for clean solids). Sewing runs only when free edges exist, so valid
   solids are never turned into shells. `failed_faces` = faces failing
   `BRepCheck_Analyzer` after healing; an "invalid faces before/after" line is
   added when any were invalid.
9. **One OCCT thread per process.** The thread lives in `occt-shim`
   (`newtopo_occt_shim::run`), shared by `OcctKernel` and the test-shape
   builder, so body ids are unique per process and nothing touches OCCT
   concurrently. `OcctKernel` is a zero-sized handle.
10. **Signals.** Only the helper calls `OSD::SetSignal(false)` (no FPE
    trapping). The shim compiles with `OCC_CONVERT_SIGNALS` on Linux/macOS and
    wraps `import_cad` and `heal` in `OCC_CATCH_SIGNALS`, so a SIGSEGV inside
    the readers becomes an error message in the helper. In the app these
    macros are inert because no handler is installed.
11. **OCCT console output.** The default messenger's printers are removed when
    the store is created (they print to stdout, which carries the helper's
    progress protocol). A few OCCT readers still write to `std::cout`
    directly; the runner ignores non-`progress` lines.
12. **rpath.** Binaries and test targets of crates that depend on the shim get
    `-Wl,-rpath,<OCCT lib dir>` (Linux uses `--disable-new-dtags` so the
    rpath also covers OCCT's transitive dependencies, which carry no RUNPATH).
    Each such crate has a tiny `build.rs` reading `DEP_OCCT_SHIM_LIB_DIR`;
    **the app crate needs the same `build.rs` and a direct optional dependency
    on `newtopo-occt-shim`** (see `crates/cli/build.rs`).
13. **Helper lookup.** Explicit path, `$NEWTOPO_IMPORT_HELPER`, next to the
    executable, then up to three parent directories and their `debug/` and
    `release/` children.
14. **`BUILD_RELEASE_DISABLE_EXCEPTIONS`.** OCCT's Release config defines
    `No_Exception`, which disables many range checks inside OCCT. Bad input to
    in-process kernel calls can therefore crash rather than throw; the shim
    validates ids itself, and SPEC §14 already scopes the no-crash guarantee
    to the import helper.

## Not implemented yet (shim surface in SPEC §3.4 beyond the current trait)

`face_curvature`, `project_to_nearest`, `section_with_plane`, `iso_curve`.
They need trait methods first; the shim structure (flat functions over the
`Body` tables) makes each a small addition.

## Portability concerns (MSVC / macOS)

* **Windows DLLs.** No rpath on Windows: `PATH` must contain the OCCT `bin`
  directory (`win64\vc14\bin` for the CMake install layout) for tests and dev
  runs. `build.rs` exports it as `DEP_OCCT_SHIM_BIN_DIR`; CI should prepend it
  to `PATH`. Shipping needs the DLLs next to the executables.
* **Windows signals.** `OSD::SetSignal` installs an SEH translator for the
  calling thread (the OCCT thread, which is the right one). The shim is
  compiled with `/EHa` so translated access violations are catchable; cc-rs
  also passes `/EHsc`, so MSVC prints warning D9025 (the later `/EHa` wins).
  Untested on Windows.
* **MSVC flags.** `build.rs` adds `/bigobj`, `/EHa`, `_USE_MATH_DEFINES`.
  The C++ is plain C++17 (no VLAs, no GNU extensions, no `M_PI`).
* **macOS.** OCCT dylibs from the CMake build use `@rpath` install names;
  the `-rpath` link arg covers them. Homebrew's `opencascade` (7.x/8.x) is
  found under `/opt/homebrew/opt/opencascade`. Not built here.
* **OCCT 7.8 vs 8.0.** The shim avoids 8.0-only spellings (`occ::handle`,
  `Standard_Failure::ExceptionType`) and deprecated 8.0 typedef headers; it
  uses `Handle()`, `NCollection_*` templates and `Standard_Failure::Print`,
  which exist in both. Library names are detected (`TKDESTEP` vs
  `TKSTEP`/`TKXDESTEP`). Building against 7.8 is untested.
* **Linker as-needed.** All present toolkits are linked; `--as-needed`
  (default on many distros) drops unused ones.
