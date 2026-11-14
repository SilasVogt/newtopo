# newtopo bridge wire protocol (SPEC 9.3), host-independent.
#
# This file is shared verbatim by the Blender, Maya and 3ds Max add-ons
# (a test checks the copies are identical). Pure Python 3.7+, no numpy.
#
# Frame: header_len (u32 little-endian) + UTF-8 JSON header + payload_len
# bytes of binary payload. Header: {"version", "type", "token",
# "payload_len", "layout"?, "info"?}. Types: hello, push, ack, error.
# Payload arrays are little-endian f32/u32, described per object in
# header["layout"]["objects"][i]["arrays"][name] = {offset, count, dtype, width}.

import json
import socket
import struct
import sys
import time

PROTOCOL_VERSION = 1
ADDON_VERSION = "0.1.0"
MAX_HEADER = 64 << 20
MAX_PAYLOAD = 4 << 30
DEFAULT_PORTS = {"blender": 47110, "maya": 47111, "max": 47112}


class ProtocolError(Exception):
    pass


def encode(header, payload=b""):
    """Encode one frame; sets header["payload_len"]."""
    h = dict(header)
    h.setdefault("version", PROTOCOL_VERSION)
    h.setdefault("token", "")
    h["payload_len"] = len(payload)
    js = json.dumps(h, separators=(",", ":")).encode("utf-8")
    return struct.pack("<I", len(js)) + js + bytes(payload)


def try_decode(buf):
    """Decode one frame from the start of `buf` (bytes/bytearray).

    Returns (header, payload, used) or None if incomplete. `payload` is a
    memoryview into `buf`, so copy it before mutating `buf`.
    """
    if len(buf) < 4:
        return None
    (hl,) = struct.unpack_from("<I", buf, 0)
    if hl > MAX_HEADER:
        raise ProtocolError("header too large")
    if len(buf) < 4 + hl:
        return None
    try:
        header = json.loads(bytes(buf[4:4 + hl]).decode("utf-8"))
    except ValueError as exc:
        raise ProtocolError("bad header: %s" % exc)
    n = int(header.get("payload_len", 0))
    if n < 0 or n > MAX_PAYLOAD:
        raise ProtocolError("payload too large")
    end = 4 + hl + n
    if len(buf) < end:
        return None
    return header, memoryview(buf)[4 + hl:end], end


def error_frame(code, message):
    return encode({"type": "error", "info": {"code": code, "message": message}})


def hello_frame(host):
    return encode({"type": "hello", "info": {"host": host, "addon_version": ADDON_VERSION}})


def ack_frame(objects, elapsed_ms, host):
    return encode({"type": "ack", "info": {"objects": objects, "elapsed_ms": elapsed_ms, "host": host}})


def check(header, token):
    """Return an error frame if the request must be refused, else None."""
    if header.get("version") != PROTOCOL_VERSION:
        return encode({"type": "error", "version": PROTOCOL_VERSION,
                       "info": {"code": "version", "message": "protocol %s not supported" % header.get("version")}})
    if not token or header.get("token") != token:
        return error_frame("bad_token", "token mismatch")
    return None


def array(payload, spec):
    """Array as a flat memoryview of floats ('f') or unsigned ints ('I')."""
    code = {"f32": "f", "u32": "I"}[spec["dtype"]]
    n = int(spec["count"]) * int(spec["width"])
    off = int(spec["offset"])
    if off % 4 or off + 4 * n > len(payload):
        raise ProtocolError("array out of range")
    if sys.byteorder != "little":
        raise ProtocolError("big-endian hosts are not supported")
    return memoryview(payload)[off:off + 4 * n].cast("B").cast(code)


def column_major_rows(t):
    """4x4 column-major list -> list of 4 rows."""
    return [[t[c * 4 + r] for c in range(4)] for r in range(4)]


def srgb_to_linear(c):
    c = min(max(float(c), 0.0), 1.0)
    return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4


class Connection(object):
    def __init__(self, sock):
        self.sock = sock
        self.buf = bytearray()
        self.started = time.time()


class PollingServer(object):
    """Non-blocking server on 127.0.0.1 for hosts that poll from a UI timer
    (Blender's bpy.app.timers, 3ds Max's QTimer). One request per
    connection. `handler(header, payload) -> reply frame bytes` runs on the
    polling (UI) thread once a frame is complete.
    """

    READ_BUDGET = 256 << 20  # bytes per poll per connection
    STALE_SECONDS = 60.0

    def __init__(self, port, handler):
        self.port = port
        self.handler = handler
        self.sock = None
        self.conns = []

    @property
    def running(self):
        return self.sock is not None

    def start(self):
        if self.sock is not None:
            return
        s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        s.bind(("127.0.0.1", self.port))  # loopback only, never 0.0.0.0
        s.listen(8)
        s.setblocking(False)
        self.sock = s

    def stop(self):
        for c in self.conns:
            try:
                c.sock.close()
            except OSError:
                pass
        self.conns = []
        if self.sock is not None:
            self.sock.close()
            self.sock = None

    def poll(self):
        """Accept, read and handle what is ready. Returns requests handled."""
        if self.sock is None:
            return 0
        while True:
            try:
                s, _ = self.sock.accept()
            except (BlockingIOError, InterruptedError):
                break
            s.setblocking(False)
            self.conns.append(Connection(s))
        handled = 0
        keep = []
        for c in self.conns:
            done = False
            try:
                done = self._service(c)
            except Exception as exc:  # never let a bad client kill the timer
                try:
                    c.sock.setblocking(True)
                    c.sock.sendall(error_frame("internal", repr(exc)))
                except OSError:
                    pass
                done = True
            if done:
                handled += 1
                try:
                    c.sock.close()
                except OSError:
                    pass
            elif time.time() - c.started < self.STALE_SECONDS:
                keep.append(c)
            else:
                c.sock.close()
        self.conns = keep
        return handled

    def _service(self, c):
        budget = self.READ_BUDGET
        closed = False
        while budget > 0:
            try:
                chunk = c.sock.recv(min(budget, 1 << 20))
            except (BlockingIOError, InterruptedError):
                break
            if not chunk:
                closed = True
                break
            c.buf += chunk
            budget -= len(chunk)
        frame = try_decode(c.buf)
        if frame is None:
            return closed
        header, payload, _ = frame
        reply = self.handler(header, bytes(payload))
        c.sock.setblocking(True)
        c.sock.settimeout(10.0)
        c.sock.sendall(reply)
        return True
