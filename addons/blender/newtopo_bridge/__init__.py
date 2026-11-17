# newtopo bridge for Blender (SPEC 9.3).
#
# Install: Edit > Preferences > Get Extensions > Install from Disk (zip of
# this folder, Blender 4.2+), or as a legacy add-on (Add-ons > Install).
# Paste the token shown in newtopo into the add-on preferences. The add-on
# listens on 127.0.0.1 only. A bpy.app.timers callback polls a non-blocking
# socket, so waiting and receiving never block the UI; building the meshes
# of a push runs on the UI thread (about 70 ms for 50k faces).
#
# Headless use (tests): call start_server(port, token) and then poll() in a
# loop, since timers may not fire in background mode.

bl_info = {
    "name": "newtopo bridge",
    "author": "newtopo",
    "version": (0, 1, 0),
    "blender": (4, 2, 0),
    "location": "Edit > Preferences > Add-ons > newtopo bridge",
    "description": "Receive meshes sent from the newtopo app",
    "category": "Import-Export",
}

import bpy
from bpy.props import BoolProperty, IntProperty, StringProperty

from . import protocol

HOST_NAME = "Blender %s" % bpy.app.version_string
POLL_INTERVAL = 0.1

_server = None
_token = ""
_last_message = ""


def _handle(header, payload):
    global _last_message
    err = protocol.check(header, _token)
    if err is not None:
        _last_message = "refused a request (%s)" % header.get("type")
        return err
    kind = header.get("type")
    if kind == "hello":
        return protocol.hello_frame(HOST_NAME)
    if kind == "push":
        from . import build

        objects, elapsed = build.apply_push(header.get("layout") or {"objects": []}, payload, HOST_NAME)
        _last_message = "received %d object(s) in %.0f ms" % (len(objects), elapsed)
        return protocol.ack_frame(objects, elapsed, HOST_NAME)
    return protocol.error_frame("bad_request", "unexpected message type %r" % kind)


def start_server(port=protocol.DEFAULT_PORTS["blender"], token=""):
    """Start listening (idempotent). Raises OSError if the port is taken."""
    global _server, _token
    _token = token
    if _server is not None and _server.port != port:
        stop_server()
    if _server is None:
        _server = protocol.PollingServer(port, _handle, lambda: _token)
    _server.start()
    return _server


def stop_server():
    global _server
    if _server is not None:
        _server.stop()
        _server = None


def poll():
    """Service pending connections once. Returns the number handled."""
    return _server.poll() if _server is not None else 0


def is_running():
    return _server is not None and _server.running


def _timer():
    try:
        poll()
    except Exception as exc:  # keep the timer alive
        print("newtopo bridge:", exc)
    return POLL_INTERVAL


def _prefs():
    addon = bpy.context.preferences.addons.get(__package__)
    return addon.preferences if addon else None


def _apply_prefs(_self=None, _ctx=None):
    p = _prefs()
    if p is None:
        return
    global _token
    _token = p.token.strip()
    if p.enabled:
        try:
            start_server(p.port, _token)
        except OSError as exc:
            print("newtopo bridge: cannot listen on port %d: %s" % (p.port, exc))
    else:
        stop_server()


class NEWTOPO_AP_preferences(bpy.types.AddonPreferences):
    bl_idname = __package__

    enabled: BoolProperty(name="Listen", default=True, update=_apply_prefs)
    port: IntProperty(name="Port", default=protocol.DEFAULT_PORTS["blender"], min=1024, max=65535, update=_apply_prefs)
    token: StringProperty(name="Token", description="Paste the token shown in newtopo", subtype="PASSWORD", update=_apply_prefs)

    def draw(self, context):
        col = self.layout.column()
        col.prop(self, "enabled")
        col.prop(self, "port")
        col.prop(self, "token")
        state = "listening on 127.0.0.1:%d" % self.port if is_running() else "not listening"
        col.label(text="Status: " + state)
        if _last_message:
            col.label(text="Last: " + _last_message)


def register():
    bpy.utils.register_class(NEWTOPO_AP_preferences)
    if not bpy.app.timers.is_registered(_timer):
        bpy.app.timers.register(_timer, first_interval=POLL_INTERVAL, persistent=True)
    # Preferences exist once the add-on is enabled; start from them.
    bpy.app.timers.register(lambda: (_apply_prefs(), None)[1], first_interval=0.0)


def unregister():
    if bpy.app.timers.is_registered(_timer):
        bpy.app.timers.unregister(_timer)
    stop_server()
    bpy.utils.unregister_class(NEWTOPO_AP_preferences)
