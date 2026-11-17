# Build 3ds Max Editable Poly objects from a newtopo push (SPEC 9.3).
#
# UNTESTED: written without access to 3ds Max, against the public pymxs and
# MAXScript (polyop, LayerManager) documentation.
#
# Max is Z-up like the payload, so no axis change. Hard edges become
# smoothing-group bitmasks: faces across each smooth edge share a bit, faces
# across each hard edge share none (see smoothing_groups). Explicit per-corner
# normals are not written: smoothing groups carry the hard edges.

import pymxs
from pymxs import runtime as rt

from . import protocol


def smoothing_groups(sizes, verts, hard_pairs, max_bits=32):
    """Per-face smoothing-group bitmasks for 3ds Max.

    Max smooths across an edge when the two faces share any bit. So every
    smooth edge needs a bit common to both faces, and faces across a hard
    edge must share none, even when they are also joined around through
    smooth edges (a cylinder with one hard seam). Greedy, in face order:
    for each smooth edge whose faces share no bit yet, add a bit to one or
    both faces, preferring a bit one face already has, never one a hard
    neighbour of the receiving face holds. Adding bits never breaks an
    edge already satisfied.

    Returns (masks, unresolved): `unresolved` lists face pairs whose smooth
    edge could not get a shared bit within `max_bits` groups (they render
    hard), plus face pairs that share both a smooth and a hard edge.
    """
    nf = len(sizes)
    hard = set()
    for k in range(len(hard_pairs) // 2):
        a, b = hard_pairs[2 * k], hard_pairs[2 * k + 1]
        hard.add((min(a, b), max(a, b)))
    faces_of = {}
    at = 0
    for f in range(nf):
        n = sizes[f]
        for k in range(n):
            a, b = verts[at + k], verts[at + (k + 1) % n]
            faces_of.setdefault((min(a, b), max(a, b)), []).append(f)
        at += n
    hard_nb = [set() for _ in range(nf)]
    smooth_pairs = []
    for e, fs in faces_of.items():
        if e in hard:
            for x in range(len(fs)):
                for y in range(x + 1, len(fs)):
                    if fs[x] != fs[y]:
                        hard_nb[fs[x]].add(fs[y])
                        hard_nb[fs[y]].add(fs[x])
        else:
            for x in range(1, len(fs)):
                if fs[0] != fs[x]:
                    smooth_pairs.append((min(fs[0], fs[x]), max(fs[0], fs[x])))
    smooth_pairs = sorted(set(smooth_pairs))
    # Faces that are both smooth and hard neighbours (two shared edges with
    # different flags) can't be honoured; the hard edge wins.
    masks = [0] * nf

    def free(face, bit):
        return all(not (masks[h] >> bit) & 1 for h in hard_nb[face])

    unresolved = []
    for f, g in smooth_pairs:
        if g in hard_nb[f]:
            unresolved.append((f, g))
            continue
        if masks[f] & masks[g]:
            continue
        chosen = None
        for bit in range(max_bits):  # a bit one side already has
            if (masks[f] >> bit) & 1 and free(g, bit):
                chosen = bit
                break
            if (masks[g] >> bit) & 1 and free(f, bit):
                chosen = bit
                break
        if chosen is None:
            for bit in range(max_bits):  # a fresh bit for both
                if free(f, bit) and free(g, bit):
                    chosen = bit
                    break
        if chosen is None:
            unresolved.append((f, g))
            continue
        masks[f] |= 1 << chosen
        masks[g] |= 1 << chosen
    return masks, unresolved


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

    masks, unresolved = smoothing_groups(sizes, verts, hard)
    for f, bits in enumerate(masks):
        rt.polyop.setFaceSmoothGroup(obj, f + 1, bits)
    warning = None
    if unresolved:
        warning = "%d smooth edge(s) could not share a smoothing group (32-group limit) and render hard" % len(unresolved)
        print("newtopo bridge: %s: %s" % (name, warning))

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
    result = {"name": name, "actual": str(obj.name)}
    if warning:
        result["warning"] = warning
    return result


def apply_push(layout, payload):
    scale = float(layout.get("units", {}).get("scale", 1.0))
    keep_both = bool(layout.get("keep_both", False))
    results = []
    with pymxs.undo(True, "newtopo push"):
        for o in layout["objects"]:
            results.append(build_object(o, payload, scale, keep_both))
    rt.redrawViews()
    return results

