# Build Blender objects from a newtopo push (SPEC 9.3). Blender 4.2+.

import time

import bpy
import numpy as np
from mathutils import Matrix

from . import protocol


def _np(payload, spec, dtype):
    return np.frombuffer(protocol.array(payload, spec), dtype=dtype)


def layer_collection(scene, name):
    """Collection for a layer: reuse one with that name, else create it
    under the scene collection. Make sure it's in the scene."""
    coll = bpy.data.collections.get(name)
    if coll is None:
        coll = bpy.data.collections.new(name)
    if scene.collection.children.find(coll.name) < 0 and not any(
        coll.name in c.children for c in bpy.data.collections if c.users
    ):
        scene.collection.children.link(coll)
    return coll


def material(spec):
    mat = bpy.data.materials.get(spec["name"])
    if mat is None:
        mat = bpy.data.materials.new(spec["name"])
    # CAD colours are display (sRGB) values; Blender stores linear.
    lin = [protocol.srgb_to_linear(c) for c in spec["color"]]
    mat.diffuse_color = (lin[0], lin[1], lin[2], 1.0)
    try:
        if hasattr(mat, "use_nodes") and not mat.use_nodes:
            mat.use_nodes = True
    except AttributeError:
        pass
    if mat.node_tree is not None:
        for n in mat.node_tree.nodes:
            if n.type == "BSDF_PRINCIPLED":
                n.inputs["Base Color"].default_value = (lin[0], lin[1], lin[2], 1.0)
    return mat


def build_mesh(name, payload, arrays, scale):
    pos = _np(payload, arrays["positions"], np.float32) * np.float32(scale)
    sizes = _np(payload, arrays["poly_sizes"], np.uint32).astype(np.int32)
    verts = _np(payload, arrays["poly_verts"], np.uint32).astype(np.int32)
    nrm = _np(payload, arrays["normals"], np.float32).reshape(-1, 3)
    hard = _np(payload, arrays["hard_edges"], np.uint32).astype(np.int64).reshape(-1, 2)
    nv, nf, nc = len(pos) // 3, len(sizes), len(verts)

    me = bpy.data.meshes.new(name)
    me.vertices.add(nv)
    me.vertices.foreach_set("co", pos)
    me.loops.add(nc)
    me.loops.foreach_set("vertex_index", verts)
    me.polygons.add(nf)
    starts = np.zeros(nf, dtype=np.int32)
    if nf > 1:
        np.cumsum(sizes[:-1], out=starts[1:])
    me.polygons.foreach_set("loop_start", starts)
    me.update(calc_edges=True)

    # Sharp edges from the hard-edge vertex pairs.
    ne = len(me.edges)
    if ne and len(hard):
        ev = np.empty(ne * 2, dtype=np.int32)
        me.edges.foreach_get("vertices", ev)
        ev = ev.reshape(-1, 2).astype(np.int64)
        keys = np.minimum(ev[:, 0], ev[:, 1]) * nv + np.maximum(ev[:, 0], ev[:, 1])
        order = np.argsort(keys)
        hk = np.minimum(hard[:, 0], hard[:, 1]) * nv + np.maximum(hard[:, 0], hard[:, 1])
        pos_in = np.searchsorted(keys[order], hk)
        pos_in = np.clip(pos_in, 0, ne - 1)
        found = keys[order][pos_in] == hk
        flags = np.zeros(ne, dtype=bool)
        flags[order[pos_in[found]]] = True
        attr = me.attributes.get("sharp_edge") or me.attributes.new("sharp_edge", "BOOLEAN", "EDGE")
        attr.data.foreach_set("value", flags)

    if "uvs" in arrays:
        uv = _np(payload, arrays["uvs"], np.float32)
        if len(uv) == nc * 2:
            layer = me.uv_layers.new(name="UVMap")
            layer.data.foreach_set("uv", uv)

    if hasattr(me, "shade_smooth"):
        me.shade_smooth()
    else:
        me.polygons.foreach_set("use_smooth", np.ones(nf, dtype=bool))
    me.validate(clean_customdata=False)
    if len(nrm) == nc:
        set_corner_normals(me, nrm)
    return me


def set_corner_normals(me, nrm):
    """Per-corner custom normals. Blender 4.5+ stores free custom normals as
    a FLOAT_VECTOR corner attribute named "custom_normal", which takes a
    bulk foreach_set (about 1000x faster than normals_split_custom_set on
    50k faces); older versions use the encoded API."""
    if bpy.app.version >= (4, 5, 0):
        try:
            old = me.attributes.get("custom_normal")
            if old is not None:
                me.attributes.remove(old)
            attr = me.attributes.new("custom_normal", "FLOAT_VECTOR", "CORNER")
            attr.data.foreach_set("vector", np.ascontiguousarray(nrm, dtype=np.float32).ravel())
            if me.has_custom_normals:
                return
            me.attributes.remove(me.attributes["custom_normal"])
        except (RuntimeError, TypeError, KeyError):
            pass
    me.normals_split_custom_set(nrm)


def apply_push(layout, payload, host_name):
    """Create or replace objects. Returns ack info (list of names)."""
    t0 = time.perf_counter()
    scene = bpy.context.scene
    scale = float(layout.get("units", {}).get("scale", 1.0))
    keep_both = bool(layout.get("keep_both", False))
    results = []
    for o in layout["objects"]:
        name = o["name"]
        me = build_mesh(name, payload, o["arrays"], scale)
        if o.get("material"):
            me.materials.append(material(o["material"]))
        rows = protocol.column_major_rows(o["transform"])
        for r in range(3):
            rows[r][3] *= scale
        mw = Matrix(rows)
        coll = layer_collection(scene, o.get("layer") or "newtopo")
        ob = bpy.data.objects.get(name)
        if ob is not None and not keep_both and ob.type == "MESH":
            old = ob.data
            ob.data = me
            if old.users == 0:
                bpy.data.meshes.remove(old)
            me.name = name
            for c in list(ob.users_collection):
                if c != coll:
                    c.objects.unlink(ob)
        else:
            ob = bpy.data.objects.new(name, me)
        if coll.objects.get(ob.name) is None:
            coll.objects.link(ob)
        ob.matrix_world = mw
        results.append({"name": name, "actual": ob.name})
    elapsed = (time.perf_counter() - t0) * 1000.0
    return results, elapsed
