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
| Fuzzy command matching (subsequence with word-start and run bonuses) | Own design |
| Themes, icons, logo, colours, wording | Own design (no third-party art); fonts: Noto Sans (SIL OFL 1.1) |
