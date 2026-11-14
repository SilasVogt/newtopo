# Build 3ds Max Editable Poly objects from a newtopo push (SPEC 9.3).
#
# UNTESTED: written without access to 3ds Max, against the public pymxs and
# MAXScript (polyop, LayerManager) documentation.
#
# Max is Z-up like the payload, so no axis change. Hard edges become
# smoothing groups (faces joined by smooth edges share a group; neighbouring
# groups across hard edges get different bits). Explicit per-corner
# normals are not written: smoothing groups carry the hard edges.

import pymxs
from pymxs import runtime as rt

from . import protocol


def smoothing_groups(sizes, verts, hard_pairs):
    """Per-face smoothing bitmask (0 = faceted)."""
    nf = len(sizes)
    hard = set()
    for k in range(len(hard_pairs) // 2):
        a, b = hard_pairs[2 * k], hard_pairs[2 * k + 1]
        hard.add((min(a, b), max(a, b)))
    parent = list(range(nf))

    def find(x):
        while parent[x] != x:
            parent[x] = parent[parent[x]]
            x = parent[x]
        return x

    first = {}
    across_hard = []
    smooth_neighbour = [False] * nf
    at = 0
    for f in range(nf):
        n = sizes[f]
        for k in range(n):
            a, b = verts[at + k], verts[at + (k + 1) % n]
            e = (min(a, b), max(a, b))
            g = first.get(e)
            if g is None:
                first[e] = f
            elif e in hard:
                across_hard.append((g, f))
            else:
                parent[find(g)] = find(f)
                smooth_neighbour[f] = smooth_neighbour[g] = True
        at += n
    comp = [find(f) for f in range(nf)]
    adj = {}
    for g, f in across_hard:
        cg, cf = comp[g], comp[f]
        if cg != cf:
            adj.setdefault(cg, set()).add(cf)
            adj.setdefault(cf, set()).add(cg)
    bit_of = {}
    out = []
    for f in range(nf):
        if not smooth_neighbour[f]:
            out.append(0)
            continue
        c = comp[f]
        if c not in bit_of:
            used = {bit_of[o] for o in adj.get(c, ()) if o in bit_of}
            # More than 32 differently-smoothed neighbours can't be
            # represented; bit 0 is reused then (that hard edge shades smooth).
            bit = next((i for i in range(32) if i not in used), 0)
            bit_of[c] = bit
        out.append(1 << bit_of[c])
    return out


def _layer(name):
    layer = rt.LayerManager.getLayerFromName(name)
    if layer is None:
        layer = rt.LayerManager.newLayerFromName(name)
    return layer


def _material(spec):
    for m in rt.sceneMaterials:
        if m.name == spec["name"]:
            return m
    r, g, b = (int(round(c * 255)) for c in spec["color"])
    return rt.StandardMaterial(name=spec["name"], diffuse=rt.Color(r, g, b))


def build_object(o, payload, scale, keep_both):
    a = o["arrays"]
    pos = protocol.array(payload, a["positions"])
    sizes = protocol.array(payload, a["poly_sizes"])
    verts = protocol.array(payload, a["poly_verts"])
    hard = protocol.array(payload, a["hard_edges"])
    nv, nf, nc = len(pos) // 3, len(sizes), len(verts)

    name = o["name"]
    if not keep_both:
        old = rt.getNodeByName(name)
        # Only replace geometry; never delete a same-named camera, helper...
        if old is not None and rt.superClassOf(old) == rt.GeometryClass:
            rt.delete(old)

    obj = rt.convertToPoly(rt.Editable_Mesh())
    obj.name = name
    for i in range(nv):
        rt.polyop.createVert(obj, rt.Point3(pos[3 * i] * scale, pos[3 * i + 1] * scale, pos[3 * i + 2] * scale))
    at = 0
    for f in range(nf):
        n = sizes[f]
        rt.polyop.createPolygon(obj, [verts[at + k] + 1 for k in range(n)])  # 1-based
        at += n

    for f, bits in enumerate(smoothing_groups(sizes, verts, hard)):
        rt.polyop.setFaceSmoothGroup(obj, f + 1, bits)

    if "uvs" in a:
        uv = protocol.array(payload, a["uvs"])
        rt.polyop.setMapSupport(obj, 1, True)
        rt.polyop.setNumMapVerts(obj, 1, nc)
        for k in range(nc):
            rt.polyop.setMapVert(obj, 1, k + 1, rt.Point3(uv[2 * k], uv[2 * k + 1], 0.0))
        at = 0
        for f in range(nf):
            n = sizes[f]
            rt.polyop.setMapFace(obj, 1, f + 1, [at + k + 1 for k in range(n)])
            at += n

    # Column-major 4x4 -> Matrix3 (rows: x axis, y axis, z axis, translation).
    t = o["transform"]
    rows = [rt.Point3(t[c * 4], t[c * 4 + 1], t[c * 4 + 2]) for c in range(3)]
    rows.append(rt.Point3(t[12] * scale, t[13] * scale, t[14] * scale))
    obj.transform = rt.Matrix3(*rows)

    if o.get("material"):
        obj.material = _material(o["material"])
    if o.get("layer"):
        _layer(o["layer"]).addNode(obj)
    rt.update(obj)
    return {"name": name, "actual": str(obj.name)}


def apply_push(layout, payload):
    scale = float(layout.get("units", {}).get("scale", 1.0))
    keep_both = bool(layout.get("keep_both", False))
    results = []
    with pymxs.undo(True, "newtopo push"):
        for o in layout["objects"]:
            results.append(build_object(o, payload, scale, keep_both))
    rt.redrawViews()
    return results

