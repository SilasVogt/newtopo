# FBX subset written by newtopo

`crates/core/src/export/fbx.rs` writes binary FBX version 7400. Sources, as
SPEC §3.2 allows: the public "FBX binary file format specification" post on
the Blender developer blog (container: header, node records, property
types, array encoding) and Autodesk's public FBX documentation (object,
property and layer-element names). No exporter source code was read.

## Container

- Header: `Kaydara FBX Binary  \0`, bytes `1A 00`, `u32` version `7400`.
- Node record: `u32 EndOffset` (absolute), `u32 NumProperties`,
  `u32 PropertyListLen`, `u8 NameLen`, name, properties, children.
- A 13-byte NULL record ends every child list, and also follows nodes with
  no properties and no children (an explicit empty list). The top-level
  list ends with a NULL record. The file ends there.
- Property types written: `C` (bool), `I` (i32), `L` (i64), `D` (f64), `S`
  (string), `R` (raw bytes), arrays `i` (i32) and `d` (f64). Arrays above
  128 bytes use encoding 1 (zlib, via `flate2`); smaller ones encoding 0.
- **No footer.** The format post says the footer has "unknown contents", so
  none is written. Blender (both importers) and ufbx don't need one. Whether
  Autodesk-SDK readers (Maya, 3ds Max, Unity, Unreal) accept it is untested.
- Output is deterministic: fixed creation time stamp (2026-01-01), fixed
  `FileId` (`newtopo-fbx-7400`), ids assigned in order from 1,000,001.

## Top-level nodes

| Node | Contents |
| --- | --- |
| `FBXHeaderExtension` | `FBXHeaderVersion` 1003, `FBXVersion` 7400, `EncryptionType` 0, `CreationTimeStamp`, `Creator` |
| `FileId`, `CreationTime`, `Creator` | fixed values |
| `GlobalSettings` | `Version` 1000; `Properties70`: axes (below), `UnitScaleFactor` = `OriginalUnitScaleFactor` = `unit_metres` x 100, `AmbientColor`, `DefaultCamera`, `TimeMode` 11, `TimeSpanStart`/`Stop`, `CustomFrameRate` -1 |
| `Documents` | one `Document` "Scene" with `RootNode` 0 |
| `References` | empty |
| `Definitions` | `Version` 100, total `Count`, one `ObjectType` per used class with its `Count` (no property templates) |
| `Objects` | `Material`, `NodeAttribute` (layer nulls), `Model`, `Geometry` |
| `Connections` | `C: "OO", child, parent` |

### Axes and units

The scene is Z-up. Data is converted to the requested up axis and the axis
system declared to match:

| `ExportUp` | UpAxis | UpAxisSign | FrontAxis | FrontAxisSign | CoordAxis | CoordAxisSign |
| --- | --- | --- | --- | --- | --- | --- |
| Y (default) | 1 | 1 | 2 | 1 | 0 | 1 |
| Z | 2 | 1 | 1 | -1 | 0 | 1 |

Y-up conversion is `(x, y, z) -> (x, z, -y)`. Coordinates are written in
the export unit (scene units times `ExportOptions::scale`), and
`UnitScaleFactor` declares that unit in centimetres:
`ExportOptions::unit_metres` x 100 (0.1 for mm, 1 for cm, 100 for m, 2.54
for in). A 100 mm part exported in mm is written as 100 and imports as
0.1 m.

## Objects

- **Model** (`id`, `"<name>\0\1Model"`, `"Mesh"` or `"Null"`): `Version`
  232; `Properties70` with `Lcl Translation`, `Lcl Rotation` (degrees,
  default rotation order XYZ: X first, then Y, then Z), `Lcl Scaling`,
  `DefaultAttributeIndex` 0, `InheritType` 1; `Shading` true; `Culling`
  `"CullingOff"`. The body transform is kept on the model when it is a
  clean TRS with positive scale; otherwise (mirror, shear) it is baked into
  the vertices and the model is identity.
- **Layer nulls** (when `layers_as_hierarchy`): a `Model` of type `"Null"`
  per layer plus a `NodeAttribute` (`TypeFlags` `"Null"`) connected to it;
  object models connect to their layer null, nulls to the root (0).
- **Geometry** (`id`, `"<name>\0\1Geometry"`, `"Mesh"`): empty
  `Properties70`; `GeometryVersion` 124; `Vertices` (`d`, xyz per vertex,
  never split); `PolygonVertexIndex` (`i`, last index of each polygon stored
  as `-(index + 1)`; quads and n-gons kept unless triangulating);
  `Edges` (`i`, one per unique edge: the polygon-vertex index where the edge
  starts, in first-use order);
  - `LayerElementNormal` 0: `Version` 101, `MappingInformationType`
    `ByPolygonVertex`, `ReferenceInformationType` `Direct`, `Normals` (`d`);
  - `LayerElementSmoothing` 0: `Version` 102, `ByEdge`, `Direct`,
    `Smoothing` (`i`, 1 smooth, 0 hard, parallel to `Edges`);
  - `LayerElementMaterial` 0 (if the object has a colour): `Version` 101,
    `AllSame`, `IndexToDirect`, `Materials` `[0]`;
  - `Layer` 0: `Version` 100 and one `LayerElement` (`Type`, `TypedIndex`
    0) per element above.
- **Material** (`id`, `"<name>\0\1Material"`, `""`): `Version` 102,
  `ShadingModel` `"lambert"`, `MultiLayer` 0, `Properties70`:
  `DiffuseColor` (the CAD colour as written, no colour-space conversion),
  `DiffuseFactor` 1, `AmbientColor` 0. One material per distinct colour,
  named `ntopo_rrggbb`, shared between objects.

Connections: model -> layer null (or 0), geometry -> model, material ->
model, node attribute -> layer null.

## Not written

UVs (OutputMesh has none yet), tangents, vertex colours, animation, takes,
property templates, textures, cameras, lights, the 7.5 64-bit record
layout, and the footer.

## Verification

- `export::tests::golden_fbx`: decoded node-tree dump (insta snapshot).
- `export::tests::fbx_reads_back_with_ufbx`: ufbx (MIT, test-only
  dependency) loads the file with no warnings; world positions, face
  counts, hard edges, hierarchy, material names, axes and units match.
- `tests/blender_import.rs` (opt-in): Blender 5.2's C++ FBX importer and
  the legacy Python importer, Y-up and Z-up/triangulated.
