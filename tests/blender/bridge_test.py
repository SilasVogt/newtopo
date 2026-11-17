"""Headless Blender side of the bridge test (SPEC 9.3 AC).

Usage:
    blender -b --factory-startup -P tests/blender/bridge_test.py -- PORT TOKEN PUSHES [TIMEOUT_S]

Registers the add-on from addons/blender, starts the server on PORT, prints
`NEWTOPO_READY`, then calls the add-on's poll() in a loop (timers don't fire
in background mode) until PUSHES pushes were handled, and prints
`NEWTOPO_JSON <json>` describing the scene: per mesh object its collection,
polygon/vertex counts, sharp edge count, UV layers, materials and
custom-normal state, plus timings.
"""

import json
import os
import sys
import time

import bpy

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "..", "addons", "blender"))

import newtopo_bridge  # noqa: E402


def main():
    argv = sys.argv[sys.argv.index("--") + 1:]
    port, token, pushes = int(argv[0]), argv[1], int(argv[2])
    timeout = float(argv[3]) if len(argv) > 3 else 120.0
    bpy.ops.wm.read_factory_settings(use_empty=True)
    newtopo_bridge.register()
    newtopo_bridge.start_server(port, token)
    print("NEWTOPO_READY", flush=True)

    handled, polls = 0, 0
    push_times = []
    deadline = time.time() + timeout
    while time.time() < deadline:
        before = newtopo_bridge._last_message
        t = time.perf_counter()
        n = newtopo_bridge.poll()
        polls += 1
        if n:
            if newtopo_bridge._last_message != before and newtopo_bridge._last_message.startswith("received"):
                push_times.append((time.perf_counter() - t) * 1000.0)
                handled += 1
                if handled >= pushes:
                    break
        else:
            time.sleep(0.002)

    objects = {}
    for ob in bpy.data.objects:
        if ob.type != "MESH":
            continue
        me = ob.data
        sharp = me.attributes.get("sharp_edge")
        objects[ob.name] = {
            "collections": sorted(c.name for c in ob.users_collection),
            "polygons": len(me.polygons),
            "vertices": len(me.vertices),
            "loops": len(me.loops),
            "sharp_edges": sum(1 for d in sharp.data if d.value) if sharp else 0,
            "uv_layers": [u.name for u in me.uv_layers],
            "materials": [m.name for m in me.materials if m],
            "custom_normals": bool(me.has_custom_normals),
            "location": list(ob.matrix_world.translation),
            "first_vertex_world": list(ob.matrix_world @ me.vertices[0].co) if len(me.vertices) else None,
        }
    result = {
        "handled": handled,
        "polls": polls,
        "push_ms": push_times,
        "objects": objects,
        "collections": sorted(c.name for c in bpy.context.scene.collection.children),
        "meshes": len(bpy.data.meshes),
    }
    newtopo_bridge.unregister()
    print("NEWTOPO_JSON " + json.dumps(result, sort_keys=True), flush=True)


main()
