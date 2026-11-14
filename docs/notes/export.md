# Fill / export worker notes

For merging into `docs/PROVENANCE.md` and `docs/QUESTIONS.md`.

## Provenance

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

## Decisions (questions the spec doesn't answer)

1. **Export units.** Coordinates are scene units × `ExportOptions::scale`
   and every format declares metres (FBX `UnitScaleFactor = 100`; glTF by
   definition; OBJ by convention). The app chooses `scale` (e.g. 0.001 for
   a millimetre scene). All three formats then import into Blender at the
   same size (verified).
2. **glTF up axis.** glTF is +Y up by definition, so `.glb` is always Y-up;
   `ExportOptions::up` applies to OBJ and FBX only.
3. **Transforms.** OBJ bakes the body transform. glTF and FBX keep it on the
   object node when it is a clean TRS with positive scale (so pivots
   survive), otherwise bake it (mirrors reverse winding to keep faces
   outward).
4. **Normals at non-hard edges.** SPEC §8.3 says normals are "split at hard
   edges"; where two CAD faces meet at a non-hard edge that is not tangent,
   the corners are averaged (one normal per smooth fan, each CAD face
   counted once). A user-hardened edge between coplanar faces stays visually
   smooth with exact normals (both sides have the same normal) but is marked
   hard in FBX edge smoothing; face-weighted mode splits it.
5. **OBJ grouping.** Each object writes `o <name>`, then `g <layer>` when
   layers become hierarchy (spaces → `_`), else `g <name>`. Objects without
   a material are written first, because `usemtl` stays in effect across
   objects and OBJ has no "no material" statement. Blender ignores `g` by
   default, so the OBJ layer grouping is only checked by the golden file.
6. **OBJ `mtllib`.** In-memory output references `newtopo.mtl`;
   `ExportFiles::save(path)` writes `<stem>.mtl` and rewrites that line.
   No `mtllib` is written when there are no materials.
7. **Smoothing groups (`s`).** Connected components across non-hard edges;
   polygons with no smooth neighbour get `s off`.
8. **Material names and colours.** One material per distinct CAD colour,
   named `ntopo_rrggbb`. CAD colours are treated as display (sRGB) values:
   glTF `baseColorFactor` and the Blender bridge convert to linear; FBX
   `DiffuseColor` and MTL `Kd` carry the value as is (that is what DCC
   importers expect there). Blender therefore shows FBX/OBJ colours
   slightly lighter than glTF ones; revisit if Silas wants them identical.
9. **FBX footer.** Not written (the allowed source documents none). Works
   in Blender and ufbx; Autodesk-SDK readers untested.
10. **Fill tie-breaking.** The polygon is rotated to start at its
    lowest-rank vertex before any stage runs, and quality ties within 1e-9
    keep the first candidate in that order, so output depends only on the
    polygon and its vertex keys. The fan fallback fans from that vertex
    (SPEC says "the first vertex").
11. **Fill output winding.** If a polygon arrives clockwise in UV, pieces
    are returned clockwise too (same direction as the input boundary).
12. **Bridge protocol details.** One request per connection. Push layout:
    `{units: {scale, up: "z"}, keep_both, objects: [{name, layer,
    transform (column-major 4x4), material {name, color} | null, arrays}]}`;
    arrays `positions` (f32×3), `poly_sizes`, `poly_verts` (u32),
    `normals` (f32×3 per corner), `hard_edges` (u32 pairs), optional `uvs`
    (f32×2 per corner). Positions are body-space and Z-up; hosts apply
    `scale` and convert the up axis. Errors carry `info.code` (`bad_token`,
    `version`, `bad_request`, `internal`). Ack `info`: `{objects: [{name,
    actual}], elapsed_ms, host}`.
13. **Bridge UVs.** `OutputMesh` has no UVs yet, so `BridgeObject` carries
    optional per-corner UVs in `OutputMesh::polygons` corner order.
14. **probe() takes the token** (`probe(port, token)`), so the panel can
    show "bad token" separately from "offline".
15. **Blender add-on licence.** The manifest says GPL-3.0-or-later, which
    Blender's extension platform requires for add-ons that use `bpy`.
    Silas should confirm (SPEC §13.1 is still open).
16. **Blender custom normals.** Blender 4.5+ stores free custom normals as a
    `custom_normal` FLOAT_VECTOR corner attribute; setting it with
    `foreach_set` is ~1000× faster than `normals_split_custom_set` on 50k
    faces (0.3 ms vs 345 ms). Older versions fall back to the old call.

## Environment note

Blender 5.2.3 on Silas's desktop doesn't start: `libosdGPU.so` links
`libcudart.so.13` and CUDA isn't installed. The Blender tests here ran with
a no-op stub `libcudart.so.13` on `LD_LIBRARY_PATH` (built in the session
scratchpad, nothing installed). Set `NEWTOPO_BLENDER_CMD` to such a wrapper
or install CUDA.

## Untested

- Maya and 3ds Max add-ons were never run in their hosts (no Maya or Max
  here). Only their host-free parts are tested (threaded server, smoothing
  groups, shared protocol).
- FBX in Autodesk-SDK-based apps (Maya, 3ds Max, Unity, Unreal).
- `ExportOptions` has no forward-axis option (SPEC §8.3 mentions "up and
  forward axis"); the forward axis is fixed per up axis (see
  `docs/fbx-subset.md`).
