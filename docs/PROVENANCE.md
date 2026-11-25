# Provenance

Where each non-trivial algorithm came from (SPEC §0 rule 4): public papers, open-source library documentation, or our own design. No commercial retopology product, its documentation or its videos were consulted.


## Engine (document, evaluator, tools, mock kernel)

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
| Snapping pcurve ends that share a CAD vertex (per-vertex proximity clusters, mean position) | Own design |
| Sliver test for the 3D orientation check (Newell normal vs perimeter²) | Newell's method; threshold own design |
| Preview transactions (snapshot document + undo/redo, commit as one step) | Own design on the existing snapshot undo (SPEC §4.7) |


## CAD kernel, import helper, reference models

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


## Auto-fill, export, bridge, rename

| Area | Source |
| --- | --- |
| Auto-fill DP (`core/src/fill/dp.rs`) | Own design after the textbook dynamic program for optimal polygon triangulation over sub-chains (as in standard computational-geometry texts), extended with quad splits and a lexicographic (quads, quality) objective. |
| Diagonal validity, in-cone test, segment touch (`fill/geom.rs`) | Standard computational-geometry predicates; exact orientation from Shewchuk's adaptive predicates via the `robust` crate (docs). |
| Large polygons (`fill/cdt.rs`) | `spade` constrained Delaunay triangulation (docs), inside found by flooding from boundary edges; greedy pairing of adjacent triangles is own design. |
| Ear clipping (`fill/ear.rs`) | Textbook ear clipping (two-ears theorem), exact predicates; best-shaped ear for small polygons is own design. |
| Fill quality (`fill/quality.rs`) | Own design: weighted corner squareness, diagonal-gap planarity, aspect, iso alignment. |
| Export normals (`export/prep.rs`) | Own design: corners around a vertex joined across non-hard edges form a smooth fan; exact mode averages distinct CAD-face normals, face-weighted mode sums area-weighted polygon normals. |
| OBJ/MTL writer | Wavefront OBJ/MTL as commonly documented (public format descriptions); own writer. |
| GLB writer | Khronos glTF 2.0 specification (public): GLB header and chunks, accessors, buffer views, nodes, PBR material. JSON hand-built with `serde_json` (not `gltf-json`). |
| FBX writer | Blender developer blog "FBX binary file format specification" (container) and Autodesk's public FBX docs (names). See `docs/fbx-subset.md`. No exporter code read. |
| FBX read-back test | `ufbx` crate (MIT), test-only. |
| Bridge protocol, add-ons | SPEC §9.3 plus own design (layout JSON, ack contents). Blender: bpy docs; Maya: OpenMaya 2.0 / maya.cmds docs; 3ds Max: pymxs / MAXScript docs. |
| Batch rename | SPEC §9.1, own design; `regex` crate for regex replace. |


## Renderer and app

| Area | Source |
| --- | --- |
| Ray–triangle test (`render/pick.rs`) | Möller & Trumbore, "Fast, Minimum Storage Ray/Triangle Intersection", JGT 1997 |
| Picking BVH (median split on the longest centroid axis), slab ray–box test | Textbook technique (Kay & Kajiya 1986 slabs); own implementation |
| Screen-space-width lines, near-plane clipping in the vertex shader, depth bias by scaling view-space points towards the eye | Own design |
| Polygon triangulation for preview (Newell normal + ear clipping) | Newell's method (Sutherland et al. 1974); ear clipping (Meisters 1975); own implementation |
| Orbit camera, framing, projection matrices (depth 0..1) | Standard formulas; own implementation |
| Edge loop / ring / face strip walks over the output mesh | Own design from SPEC §6.14 |
| Before/after compare (one camera, one render target per viewport slot) | Own design |
| Fuzzy command matching (subsequence with word-start and run bonuses) | Own design |
| Themes, icons, logo, colours, wording | Own design (no third-party art); fonts: Noto Sans (SIL OFL 1.1) |


## Mesh mode (`core/src/meshedit/`, SPEC §9.5)

Behaviour was designed from the SPEC and the papers below. No add-on, plug-in or other implementation of similar tools was read, downloaded or run.

| Area | Source |
| --- | --- |
| Decimate cost (`quadric.rs`, `decimate.rs`) | Garland & Heckbert, "Surface Simplification Using Quadric Error Metrics", SIGGRAPH 1997: area-weighted plane quadrics, constraint planes along feature edges (their §6.1), subset placement (their §4). Own code. |
| Quad-preserving merges (opposite-corner merge in a quad) | Diagonal collapse as described by Daniels, Silva, Shepherd, Cohen, "Quadrilateral mesh simplification", SIGGRAPH Asia 2008, and Tarini, Pietroni, Cignoni, Panozzo, Puppo, "Practical quad mesh simplification", Eurographics 2010. The quad/shape penalty in the cost is own design. |
| Merge validity (`work.rs`) | Own design: Euler characteristic unchanged plus local manifold checks (edge use counts, single fan per vertex, no duplicate faces), in the spirit of the link condition (Dey, Edelsbrunner, Guha, Nekhayev 1999). |
| Deviation bound (witness vertices per polygon) | Own design after the error-tracking idea of Klein, Liebich, Straßer, "Mesh Reduction with Error Control", IEEE Visualization 1996 (removed vertices stay attached to the nearest new face). |
| Point–triangle distance | Ericson, *Real-Time Collision Detection*, §5.1.5. |
| Planar mode (normal-cone region growing, straight-vertex dissolve) | Own design; refill uses the auto-fill decomposition (SPEC §7, `fill/`). |
| Triangle pairing into quads | Own design (greedy best-quality pairing, as in the §7 large-polygon path). |
| Cleanup (merge close vertices, short edges, slide along cross edges, dissolve straight vertices) | Own design from SPEC §9.5.4. Closest point between two lines: standard vector algebra. |
| Resample (arc-length respacing, corner detection, count distribution, old→new matching) | Own design; count distribution by largest remainder (Hamilton's method); old→new matching by a monotone-assignment dynamic program (standard sequence alignment). |
| Attribute carry-over (exact for kept corners, interpolated along edges, seam side chosen by nearest UV) | Own design. |
| Vertex Slide (`slide.rs`): edge choice by direction, Even, Clamp, landing merge, largest valid factor by bisection | Behaviour: the generic "slide vertices along an adjacent edge" tool of DCC apps, as described in the Blender user manual's *Slide Vertices* page (factor, Even, Clamp; read for behaviour only, no source code). Feature constraints, validity rules, the "not worse" rule for already-bad polygons and the plan/apply split for dragging are own design. Bisection: standard numerics. |
| Merge by Distance (`merge.rs`): role-ordered greedy clustering on a uniform spatial hash, batch weld with pocket cancellation, per-cluster rollback, centroid placement, rounds to a fixed point | Behaviour: the generic "merge by distance" / weld cleanup of DCC apps, as described in the Blender user manual's *Merge by Distance* entries (merge distance, centroid on/off, include unselected; behaviour only, no source code). Spatial hash with the query radius as cell size: standard technique (e.g. Teschner et al., "Optimized Spatial Hashing for Collision Detection of Deformable Objects", VMV 2003). Clustering order, the weld/manifold checks, the rollback and the modes are own design. |
| Edge Slide (`edgeslide.rs`): per run vertex, the cross edge is the edge beside the run edge in the polygon on the chosen side (side picked by the run edge's winding in that polygon); the slide itself is Vertex Slide's plan | Own design from SPEC §9.5.10. Behaviour reference only: the generic "edge slide" tool of DCC apps (slide a loop along its neighbouring edges, signed factor picks the side); no add-on or application source was read. |
| Edge Slide drag mapping (`app/src/meshmode.rs`): factor = the drag vector projected onto each side's average screen-space cross-edge vector, the larger positive projection picking the side, clamped and rounded to 0.001 | Own design (vector projection, standard linear algebra). |
| Target Merge (`edgeslide.rs`): merge into a polygon neighbour with the shared merge planning and checks of `work.rs`; for vertices that share no polygon, a weld that moves the source onto the target, checks the source's polygons as a move, and runs Merge by Distance (unconnected pairs) with the target forced to survive | Own design from SPEC §9.5.13, reusing the merge validity rows above. Behaviour reference only: the generic "target weld / merge at last" tool of DCC apps (click a vertex, then the one it merges into); no add-on or application source was read. |
| Relax (`smooth.rs`): per iteration, each movable run vertex moves half way to a predicted point (midpoint of its run neighbours, or the cubic through two run vertices on each side, weights −1/6, 2/3, 2/3, −1/6), the step halved twice more if it breaks a polygon, then skipped; optional even respacing after each iteration | Own design from SPEC §9.5.11. The cubic weights are Lagrange interpolation of the cubic through four uniformly spaced samples, evaluated at the middle (standard numerical analysis); damping by a fixed fraction and step halving are standard numerics. Behaviour reference only: the generic "relax loop" tool of DCC apps; no add-on or application source was read. |
| Space (`smooth.rs`): each run piece between fixed vertices respaced to equal arc length along its own polyline (linear interpolation on the cumulative chord length), attributes interpolated along the segment | Own design from SPEC §9.5.12; arc-length parameterisation of a polyline is textbook. Behaviour reference only: the generic "space loop evenly" tool of DCC apps; no add-on or application source was read. |
| Edge-loop walk (`pick.rs`): through every interior vertex with four edges take the edge sharing no polygon with the incoming one; along open boundaries follow the boundary; stop elsewhere or when closed | Own design from SPEC §9.5.9 (standard quad-mesh loop traversal, as in the "Loop / ring / strip selection" row above). |
| Shortest edge path (`pick.rs`) | Dijkstra's algorithm (Dijkstra 1959, textbook) over edge lengths with a binary heap; tie-break on the lower vertex index is own design. |
| Stay on CAD surface (`project.rs`): each vertex a move tool moved is put on the closest point of the CAD faces of its polygons, unless that would make a polygon concave, degenerate, flipped or tilted past the normal limit | Own design from SPEC §9.5.2. The closest-point query is the CAD kernel's face projection (see "Projection onto trimmed faces" above; the mock kernel's Gauss–Newton closest point in tests). |
| Synthetic test meshes and Hausdorff sampling (`synth.rs`) | Textbook parameterisations; own code. |
