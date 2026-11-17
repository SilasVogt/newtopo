"""Headless Blender import check for newtopo exports (SPEC §8.3 AC).

Usage:
    blender -b --factory-startup -P tests/blender/check_import.py -- \
        LABEL=IMPORTER:PATH [LABEL=IMPORTER:PATH ...]

IMPORTER is one of: obj (Y-up file), obj-zup (Z-up file), gltf, fbx (Blender's built-in C++ FBX importer,
when present), fbx-legacy (the Python FBX add-on). Each file is imported into
an empty scene. Prints one line `NEWTOPO_JSON <json>` with, per label:

    objects: {name: {type, parent, collections, polygons, vertices,
              triangles, unique_positions, world_positions (rounded, sorted),
              materials: [{name, color}], sharp_edges, split_edges,
              custom_normals}}

`split_edges` counts geometric edges (matched by world position, so it works
for glTF's split vertices too) shared by two or more faces whose corner
normals differ on the two sides: what a viewer shows as a hard edge.
"""

import json
import sys

import bpy

ROUND = 4  # decimal places for position matching


def args():
    argv = sys.argv
    return argv[argv.index("--") + 1:] if "--" in argv else []


def reset():
    bpy.ops.wm.read_factory_settings(use_empty=True)


def do_import(kind, path):
    if kind == "obj":
        bpy.ops.wm.obj_import(filepath=path)
    elif kind == "obj-zup":
        bpy.ops.wm.obj_import(filepath=path, forward_axis="Y", up_axis="Z")
    elif kind == "gltf":
        bpy.ops.import_scene.gltf(filepath=path)
    elif kind == "fbx":
        bpy.ops.wm.fbx_import(filepath=path)
    elif kind == "fbx-legacy":
        bpy.ops.import_scene.fbx(filepath=path)
    else:
        raise ValueError("unknown importer " + kind)


def base_name(name):
    # Blender appends .001 etc. on clashes; report the clean name too.
    return name


def material_info(mat):
    color = list(mat.diffuse_color[:3])
    if mat.use_nodes and mat.node_tree:
        for n in mat.node_tree.nodes:
            if n.type == "BSDF_PRINCIPLED":
                color = list(n.inputs["Base Color"].default_value[:3])
                break
    return {"name": mat.name, "color": [round(c, 4) for c in color]}


def key(v):
    return (round(v.x, ROUND) + 0.0, round(v.y, ROUND) + 0.0, round(v.z, ROUND) + 0.0)


def mesh_info(ob):
    me = ob.data
    mw = ob.matrix_world
    world = [mw @ v.co for v in me.vertices]
    rot = mw.to_3x3().inverted_safe().transposed()
    normals = [(rot @ n.vector).normalized() for n in me.corner_normals]
    edges = {}
    for poly in me.polygons:
        loops = list(poly.loop_indices)
        for i, li in enumerate(loops):
            lj = loops[(i + 1) % len(loops)]
            va, vb = me.loops[li].vertex_index, me.loops[lj].vertex_index
            ka, kb = key(world[va]), key(world[vb])
            na, nb = normals[li], normals[lj]
            if kb < ka:
                ka, kb, na, nb = kb, ka, nb, na
            edges.setdefault((ka, kb), []).append((na, nb))
    split = 0
    for uses in edges.values():
        if len(uses) < 2:
            continue
        a0, b0 = uses[0]
        if any((a - a0).length > 1e-3 or (b - b0).length > 1e-3 for a, b in uses[1:]):
            split += 1
    sharp = 0
    attr = me.attributes.get("sharp_edge")
    if attr is not None:
        sharp = sum(1 for d in attr.data if d.value)
    positions = sorted({key(w) for w in world})
    return {
        "polygons": len(me.polygons),
        "vertices": len(me.vertices),
        "triangles": sum(p.loop_total - 2 for p in me.polygons),
        "polygon_sizes": sorted({p.loop_total for p in me.polygons}),
        "unique_positions": len(positions),
        "world_positions": positions,
        "materials": [material_info(m) for m in me.materials if m is not None],
        "sharp_edges": sharp,
        "split_edges": split,
        "custom_normals": bool(me.has_custom_normals),
    }


def scene_info():
    out = {}
    for ob in bpy.data.objects:
        info = {
            "type": ob.type,
            "parent": ob.parent.name if ob.parent else None,
            "collections": sorted(c.name for c in ob.users_collection),
        }
        if ob.type == "MESH":
            info.update(mesh_info(ob))
        out[ob.name] = info
    return out


def main():
    result = {}
    for spec in args():
        label, rest = spec.split("=", 1)
        kind, path = rest.split(":", 1)
        reset()
        try:
            do_import(kind, path)
            result[label] = {"objects": scene_info()}
        except Exception as exc:  # report, don't abort the other files
            result[label] = {"error": repr(exc)}
    print("NEWTOPO_JSON " + json.dumps(result, sort_keys=True))


main()
