# newtopo bridge for 3ds Max (SPEC 9.3). 3ds Max 2022+ (Python 3, PySide2
# or PySide6, pymxs).
#
# UNTESTED: written without access to 3ds Max. See docs/notes/export.md.
#
# Install: copy addons/max/newtopo_bridge_startup.py into the user startup
# scripts folder (<user scripts>/startup) and set NEWTOPO_BRIDGE_DIR, or
# edit the path at its top. Set the token once from the MAXScript listener
# (Python mode):
#
#     import newtopo_bridge; newtopo_bridge.set_token("<token from newtopo>")
#
# A QTimer on the UI thread polls a non-blocking socket on 127.0.0.1 and
# applies pushes through pymxs, so the UI never blocks and scene changes
# stay on the main thread.

from . import protocol

INI_SECTION = "bridge"
POLL_MS = 100

_server = None
_timer = None
_token = ""


def _rt():
    from pymxs import runtime as rt

    return rt


def _ini():
    rt = _rt()
    return rt.pathConfig.appendPath(rt.getDir(rt.Name("plugcfg")), "newtopo.ini")


def get_token():
    return _rt().getINISetting(_ini(), INI_SECTION, "token") or ""


def set_token(token):
    global _token
    _token = token.strip()
    _rt().setINISetting(_ini(), INI_SECTION, "token", _token)


def get_port():
    v = _rt().getINISetting(_ini(), INI_SECTION, "port")
    return int(v) if v else protocol.DEFAULT_PORTS["max"]


def host_name():
    try:
        return "3ds Max %s" % (_rt().maxVersion()[0] // 1000 + 1998)
    except Exception:
        return "3ds Max"


def _handle(header, payload):
    err = protocol.check(header, _token)
    if err is not None:
        return err
    kind = header.get("type")
    if kind == "hello":
        return protocol.hello_frame(host_name())
    if kind == "push":
        import time

        from . import build

        t0 = time.perf_counter()
        objects = build.apply_push(header.get("layout") or {"objects": []}, payload)
        return protocol.ack_frame(objects, (time.perf_counter() - t0) * 1000.0, host_name())
    return protocol.error_frame("bad_request", "unexpected message type %r" % kind)


def _qtcore():
    try:
        from PySide6 import QtCore
    except ImportError:
        from PySide2 import QtCore
    return QtCore


def poll():
    """Service pending connections once (also usable without the timer)."""
    return _server.poll() if _server is not None else 0


def start(port=None, token=None):
    """Start listening and polling (idempotent)."""
    global _server, _timer, _token
    _token = token if token is not None else get_token()
    if _server is None:
        srv = protocol.PollingServer(port or get_port(), _handle)
        srv.start()  # raises OSError if the port is taken; nothing is kept then
        _server = srv
    if _timer is None:
        QtCore = _qtcore()
        _timer = QtCore.QTimer()
        _timer.timeout.connect(poll)
        _timer.start(POLL_MS)
    print("newtopo bridge: listening on 127.0.0.1:%d" % _server.port)
    return _server


def stop():
    global _server, _timer
    if _timer is not None:
        _timer.stop()
        _timer = None
    if _server is not None:
        _server.stop()
        _server = None
