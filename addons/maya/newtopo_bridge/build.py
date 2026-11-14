# Build Maya meshes from a newtopo push (SPEC 9.3). Main thread only.
#
# UNTESTED: written without access to Maya, against the public OpenMaya 2.0
# and maya.cmds documentation.

import re

import maya.api.OpenMaya as om
import maya.cmds as cmds

from . import protocol


def _valid_name(name):
    """Maya node names: letters, digits and underscores, not starting with a
    digit."""
    n = re.sub(r"[^A-Za-z0-9_]", "_", name) or "newtopo"
    return "_" + n if n[0].isdigit() else n


def _y_up():
    return cmds.upAxis(q=True, axis=True).lower() == "y"


def _conv(x, y, z, y_up):
    # Payload is Z-up; Maya's default is Y-up: (x, y, z) -> (x, z, -y).
    return (x, z, -y) if y_up else (x, y, z)


def _matrix(t, scale, y_up):
    """Column-major 4x4 body->scene (Z-up) to Maya's xform matrix (row-vector
    convention: rows are the transformed axes and the translation)."""
    cols = [[t[c * 4 + r] for r in range(4)] for c in range(4)]
    axes = [_conv(*cols[c][:3], y_up=y_up) for c in range(3)]
    tr = _conv(*(v * scale for v in cols[3][:3]), y_up=y_up)
    if y_up:
        # Conjugate the linear part: C L C^-1. Columns of L map body axes;
        # in Maya space the body's x, y, z axes are C e_x, C e_y, C e_z.
        # Reordering columns accordingly: new col for Maya-y is old col z,
        # new col for Maya-z is -(old col y).
        axes = [axes[0], axes[2], tuple(-v for v in axes[1])]
    rows = [list(a) + [0.0] for a in axes] + [list(tr) + [1.0]]
    return [float(v) for row in rows for v in row]  # 16 floats, row-major


def _layer(name):
    name = _valid_name(name)
    if cmds.objExists(name) and cmds.nodeType(name) == "displayLayer":
        return name
    return cmds.createDisplayLayer(name=name, empty=True, noRecurse=True)


def _shading_group(spec):
    name = _valid_name(spec["name"])
    sg = name + "SG"
    if not cmds.objExists(name):
        name = cmds.shadingNode("lambert", asShader=True, name=name)
    if not cmds.objExists(sg):
        sg = cmds.sets(renderable=True, noSurfaceShader=True, empty=True, name=sg)
        cmds.connectAttr(name + ".outColor", sg + ".surfaceShader", force=True)
    r, g, b = spec["color"]
    cmds.setAttr(name + ".color", r, g, b, type="double3")
    return sg


def _dag(name):
    sel = om.MSelectionList()
    sel.add(name)
    return sel.getDependNode(0)


def build_object(o, payload, scale, keep_both, y_up):
    a = o["arrays"]
    pos = protocol.array(payload, a["positions"])
    sizes = protocol.array(payload, a["poly_sizes"])
    verts = protocol.array(payload, a["poly_verts"])
    nrm = protocol.array(payload, a["normals"])
    hard = protocol.array(payload, a["hard_edges"])
    nv, nf, nc = len(pos) // 3, len(sizes), len(verts)

    points = om.MFloatPointArray()
    points.setLength(nv)
    for i in range(nv):
        points[i] = om.MFloatPoint(*_conv(pos[3 * i] * scale, pos[3 * i + 1] * scale, pos[3 * i + 2] * scale, y_up))
    counts = om.MIntArray(list(sizes))
    connects = om.MIntArray(list(verts))

    name = o["name"]
    wanted = _valid_name(name)
    if not keep_both and cmds.objExists(wanted) and cmds.nodeType(wanted) == "transform":
        if cmds.listRelatives(wanted, shapes=True, type="mesh"):
            cmds.delete(wanted)
    xform = cmds.createNode("transform", name=wanted)  # Maya suffixes clashes
    fn = om.MFnMesh()
    if "uvs" in a:
        uv = protocol.array(payload, a["uvs"])
        us = om.MFloatArray([uv[2 * k] for k in range(nc)])
        vs = om.MFloatArray([uv[2 * k + 1] for k in range(nc)])
        fn.create(points, counts, connects, us, vs, parent=_dag(xform))
        fn.assignUVs(counts, om.MIntArray(range(nc)))
    else:
        fn.create(points, counts, connects, parent=_dag(xform))
    shape = fn.name()
    cmds.rename(shape, xform.split("|")[-1] + "Shape")

    # Hard edges: smooth everything, then harden the listed vertex pairs.
    edge_of = {}
    for e in range(fn.numEdges):
        v0, v1 = fn.getEdgeVertices(e)
        edge_of[(min(v0, v1), max(v0, v1))] = e
    ids = om.MIntArray()
    smooth = []
    hard_set = set()
    for k in range(len(hard) // 2):
        a0, b0 = hard[2 * k], hard[2 * k + 1]
        e = edge_of.get((min(a0, b0), max(a0, b0)))
        if e is not None:
            hard_set.add(e)
    for e in range(fn.numEdges):
        ids.append(e)
        smooth.append(e not in hard_set)
    fn.setEdgeSmoothings(ids, smooth)

    # Per-corner normals (locked, so shading follows the CAD normals).
    normals = om.MVectorArray()
    normals.setLength(nc)
    face_ids = om.MIntArray()
    vert_ids = om.MIntArray()
    k = 0
    for f in range(nf):
        for _ in range(sizes[f]):
            normals[k] = om.MVector(*_conv(nrm[3 * k], nrm[3 * k + 1], nrm[3 * k + 2], y_up))
            face_ids.append(f)
            vert_ids.append(verts[k])
            k += 1
    fn.setFaceVertexNormals(normals, face_ids, vert_ids)
    fn.updateSurface()

    cmds.xform(xform, matrix=_matrix(o["transform"], scale, y_up), worldSpace=True)
    if o.get("material"):
        cmds.sets(fn.fullPathName(), edit=True, forceElement=_shading_group(o["material"]))
    else:
        cmds.sets(fn.fullPathName(), edit=True, forceElement="initialShadingGroup")
    layer = o.get("layer")
    if layer:
        cmds.editDisplayLayerMembers(_layer(layer), xform, noRecurse=True)
    return {"name": name, "actual": xform.split("|")[-1]}


def apply_push(layout, payload):
    scale = float(layout.get("units", {}).get("scale", 1.0))
    keep_both = bool(layout.get("keep_both", False))
    y_up = _y_up()
    results = []
    cmds.undoInfo(openChunk=True, chunkName="newtopo push")
    try:
        for o in layout["objects"]:
            results.append(build_object(o, payload, scale, keep_both, y_up))
    finally:
        cmds.undoInfo(closeChunk=True)
    return results
