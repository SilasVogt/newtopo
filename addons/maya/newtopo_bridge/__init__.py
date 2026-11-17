# newtopo bridge for Maya (SPEC 9.3). Maya 2022+ (Python 3).
#
# UNTESTED: written without access to Maya. See docs/notes/export.md.
#
# Install: put the folder containing this package on PYTHONPATH (e.g. in
# Maya.env: PYTHONPATH=/path/to/addons/maya) and add to userSetup.py:
#
#     import maya.utils
#     maya.utils.executeDeferred("import newtopo_bridge; newtopo_bridge.start()")
#
# Set the token once (it is stored in Maya's optionVars):
#
#     import newtopo_bridge; newtopo_bridge.set_token("<token from newtopo>")
#
# A background thread accepts connections on 127.0.0.1 one at a time and
# authenticates each request from its header before reading the payload
# (protocol.read_request). Each request is handed to
# maya.utils.executeDeferred so all scene changes run on the main thread. The thread waits for the main thread's
# reply and sends it back. Maya needs unique node names: duplicates get a
# suffix and the ack reports the actual name of every object.

import queue
import socket
import threading
import time

from . import protocol

OPTVAR_TOKEN = "newtopoBridgeToken"
OPTVAR_PORT = "newtopoBridgePort"
REPLY_TIMEOUT = 300.0

_server = None


def _cmds():
    import maya.cmds as cmds

    return cmds


def get_token():
    cmds = _cmds()
    return cmds.optionVar(q=OPTVAR_TOKEN) if cmds.optionVar(exists=OPTVAR_TOKEN) else ""


def set_token(token):
    _cmds().optionVar(sv=(OPTVAR_TOKEN, token.strip()))
    if _server is not None:
        _server.token = token.strip()


def get_port():
    cmds = _cmds()
    return cmds.optionVar(q=OPTVAR_PORT) if cmds.optionVar(exists=OPTVAR_PORT) else protocol.DEFAULT_PORTS["maya"]


def host_name():
    try:
        return "Maya %s" % _cmds().about(version=True)
    except Exception:
        return "Maya"


def _handle_main(header, payload):
    """Runs on Maya's main thread (via executeDeferred)."""
    kind = header.get("type")
    if kind == "hello":
        return protocol.hello_frame(host_name())
    if kind == "push":
        from . import build

        t0 = time.perf_counter()
        objects = build.apply_push(header.get("layout") or {"objects": []}, payload)
        return protocol.ack_frame(objects, (time.perf_counter() - t0) * 1000.0, host_name())
    return protocol.error_frame("bad_request", "unexpected message type %r" % kind)


def _run_on_main(header, payload):
    import maya.utils

    box = queue.Queue(1)

    def job():
        try:
            box.put(_handle_main(header, payload))
        except Exception as exc:
            box.put(protocol.error_frame("internal", repr(exc)))

    maya.utils.executeDeferred(job)
    return box.get(timeout=REPLY_TIMEOUT)


class ThreadedServer(object):
    def __init__(self, port, token, dispatch=_run_on_main):
        self.port = port
        self.token = token
        self.dispatch = dispatch
        self._sock = None
        self._stop = threading.Event()
        self._thread = None

    def start(self):
        s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        s.bind(("127.0.0.1", self.port))  # loopback only
        s.listen(8)
        s.settimeout(0.5)
        self._sock = s
        self._stop.clear()
        self._thread = threading.Thread(target=self._loop, name="newtopo-bridge", daemon=True)
        self._thread.start()

    def stop(self):
        self._stop.set()
        if self._thread is not None:
            self._thread.join(2.0)
        if self._sock is not None:
            self._sock.close()
        self._sock = None
        self._thread = None

    def _loop(self):
        while not self._stop.is_set():
            try:
                conn, _ = self._sock.accept()
            except socket.timeout:
                continue
            except OSError:
                break
            try:
                self._serve(conn)
            except Exception as exc:
                try:
                    conn.sendall(protocol.error_frame("internal", repr(exc)))
                except OSError:
                    pass
            finally:
                conn.close()

    def _serve(self, conn):
        # Header first: token, version and size are checked before any
        # payload byte is read (protocol.read_request sends the refusal).
        req = protocol.read_request(conn, self.token)
        if req is None:
            return
        header, payload = req
        conn.settimeout(10.0)
        conn.sendall(self.dispatch(header, payload))

def start(port=None, token=None):
    """Start listening (idempotent)."""
    global _server
    if _server is not None:
        return _server
    srv = ThreadedServer(port or get_port(), token if token is not None else get_token())
    srv.start()  # raises OSError if the port is taken; nothing is kept then
    _server = srv
    print("newtopo bridge: listening on 127.0.0.1:%d" % _server.port)
    return _server


def stop():
    global _server
    if _server is not None:
        _server.stop()
        _server = None
