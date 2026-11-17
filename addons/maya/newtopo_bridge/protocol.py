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
# Limits. The push layout lives in the JSON header (SPEC 9.3), so the header
# cap is a few MiB (enough for ~10k objects), not KiB. Requests are
# authenticated as soon as the header is complete, before any payload byte
# is read, so an unauthenticated client can make a host buffer at most
# MAX_CONNECTIONS * (4 + MAX_HEADER) bytes.
MAX_HEADER = 4 << 20
MAX_PAYLOAD = 512 << 20
MAX_CONNECTIONS = 4
IDLE_SECONDS = 10.0  # no bytes received for this long: drop
REQUEST_SECONDS = 120.0  # whole request must arrive within this
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


REQUEST_TYPES = ("hello", "push")


def validate_header(header, token):
    """Checks a complete request header before any payload is read. Returns
    (error_frame or None, payload_len)."""
    if not isinstance(header, dict):
        return error_frame("bad_request", "header is not an object"), 0
    err = check(header, token)
    if err is not None:
        return err, 0
    if header.get("type") not in REQUEST_TYPES:
        return error_frame("bad_request", "unexpected message type %r" % header.get("type")), 0
    n = header.get("payload_len", 0)
    if not isinstance(n, int) or isinstance(n, bool) or n < 0:
        return error_frame("bad_request", "bad payload_len"), 0
    if n > MAX_PAYLOAD:
        return error_frame("too_large", "payload of %d bytes exceeds %d" % (n, MAX_PAYLOAD)), 0
    if header.get("type") == "hello" and n != 0:
        return error_frame("bad_request", "hello carries no payload"), 0
    return None, n


class Request(object):
    """Incremental reader for one request. Asks only for the bytes the
    current stage needs (length prefix, then header, then exactly the
    declared payload), and validates the header (token, version, type,
    size) before asking for any payload byte.

    After each feed(): `reject` holds an error frame if the request is
    refused, else `complete` tells whether header and payload are in.
    """

    def __init__(self, token):
        self.token = token
        self.buf = bytearray()
        self.header = None
        self.end = None  # total frame length once the header is known
        self.reject = None

    @property
    def complete(self):
        return self.reject is None and self.end is not None and len(self.buf) >= self.end

    @property
    def done(self):
        return self.reject is not None or self.complete

    def need(self):
        """Bytes wanted next (0 when done)."""
        if self.done:
            return 0
        if len(self.buf) < 4:
            return 4 - len(self.buf)
        if self.end is None:
            (hl,) = struct.unpack_from("<I", self.buf, 0)
            return 4 + hl - len(self.buf)
        return self.end - len(self.buf)

    def feed(self, chunk):
        if len(chunk) > self.need():
            raise ProtocolError("read past the requested bytes")
        self.buf += chunk
        if len(self.buf) >= 4 and self.end is None:
            (hl,) = struct.unpack_from("<I", self.buf, 0)
            if hl > MAX_HEADER:
                self.reject = error_frame("too_large", "header of %d bytes exceeds %d" % (hl, MAX_HEADER))
                return
            if len(self.buf) == 4 + hl:
                try:
                    header = json.loads(bytes(self.buf[4:]).decode("utf-8"))
                except ValueError:
                    self.reject = error_frame("bad_request", "header is not JSON")
                    return
                err, n = validate_header(header, self.token)
                if err is not None:
                    self.reject = err
                    return
                self.header = header
                self.end = 4 + hl + n

    def payload(self):
        return bytes(self.buf[self.end - int(self.header.get("payload_len", 0)):self.end])


def read_request(sock, token, deadline_s=REQUEST_SECONDS, idle_s=IDLE_SECONDS):
    """Blocking variant for a worker thread (Maya). Returns (header,
    payload) for an accepted request, or None after sending the refusal (or
    on timeout / disconnect). Never reads payload bytes of a refused
    request."""
    req = Request(token)
    end = time.time() + deadline_s
    while not req.done:
        left = end - time.time()
        if left <= 0:
            _send_quietly(sock, error_frame("timeout", "request took too long"))
            return None
        sock.settimeout(min(idle_s, left))
        try:
            chunk = sock.recv(min(req.need(), 1 << 20))
        except socket.timeout:
            _send_quietly(sock, error_frame("timeout", "request idle too long"))
            return None
        if not chunk:
            return None
        req.feed(chunk)
    if req.reject is not None:
        _send_quietly(sock, req.reject)
        return None
    return req.header, req.payload()


def _send_quietly(sock, data):
    """Best-effort send of a small frame; never blocks for long."""
    try:
        sock.settimeout(1.0)
        sock.sendall(data)
    except OSError:
        pass


class Connection(object):
    def __init__(self, sock, token):
        self.sock = sock
        self.req = Request(token)
        self.started = time.time()
        self.last_data = self.started


class PollingServer(object):
    """Non-blocking server on 127.0.0.1 for hosts that poll from a UI timer
    (Blender's bpy.app.timers, 3ds Max's QTimer). One request per
    connection. `get_token()` returns the current token; `handler(header,
    payload) -> reply frame bytes` runs on the polling (UI) thread once an
    authenticated request is complete.

    Limits: at most MAX_CONNECTIONS open connections (extra ones get a
    "busy" error and are closed); headers are validated before any payload
    is read; at most `total_buffer` bytes buffered across connections
    (reading pauses beyond it); idle or slow connections are dropped.
    """

    READ_BUDGET = 64 << 20  # bytes per poll per connection (UI stays responsive)

    def __init__(self, port, handler, get_token):
        self.port = port
        self.handler = handler
        self.get_token = get_token
        self.sock = None
        self.conns = []
        self.max_connections = MAX_CONNECTIONS
        self.idle_seconds = IDLE_SECONDS
        self.request_seconds = REQUEST_SECONDS
        self.total_buffer = MAX_PAYLOAD + MAX_CONNECTIONS * (4 + MAX_HEADER)
        self.bytes_read = 0  # for tests and diagnostics

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
            self._close(c.sock)
        self.conns = []
        if self.sock is not None:
            self.sock.close()
            self.sock = None

    @staticmethod
    def _close(sock, frame=None):
        if frame is not None:
            try:
                sock.send(frame)  # non-blocking; small frames fit the buffer
            except OSError:
                pass
        try:
            sock.close()
        except OSError:
            pass

    def buffered(self):
        return sum(len(c.req.buf) for c in self.conns)

    def poll(self):
        """Accept, read and handle what is ready. Returns the number of
        requests finished (answered or refused)."""
        if self.sock is None:
            return 0
        while True:
            try:
                s, _ = self.sock.accept()
            except (BlockingIOError, InterruptedError):
                break
            s.setblocking(False)
            if len(self.conns) >= self.max_connections:
                self._close(s, error_frame("busy", "too many connections"))
                continue
            self.conns.append(Connection(s, self.get_token()))
        handled = 0
        keep = []
        now = time.time()
        for c in self.conns:
            try:
                done = self._service(c)
            except Exception as exc:  # never let a bad client kill the timer
                self._close(c.sock, error_frame("internal", repr(exc)))
                handled += 1
                continue
            if done:
                handled += 1
            elif now - c.last_data > self.idle_seconds or now - c.started > self.request_seconds:
                self._close(c.sock, error_frame("timeout", "request idle or too slow"))
                handled += 1
            else:
                keep.append(c)
        self.conns = keep
        return handled

    def _service(self, c):
        """Read what this connection may read; finish it if done. Returns
        True when the connection was answered and closed."""
        budget = self.READ_BUDGET
        closed = False
        while budget > 0 and not c.req.done:
            room = self.total_buffer - self.buffered()
            want = min(c.req.need(), budget, room, 1 << 20)
            if want <= 0:
                break  # buffer cap reached: resume on a later poll
            try:
                chunk = c.sock.recv(want)
            except (BlockingIOError, InterruptedError):
                break
            if not chunk:
                closed = True
                break
            self.bytes_read += len(chunk)
            c.last_data = time.time()
            budget -= len(chunk)
            c.req.feed(chunk)
        if c.req.reject is not None:
            self._close(c.sock, c.req.reject)
            return True
        if c.req.complete:
            reply = self.handler(c.req.header, c.req.payload())
            c.req.buf = bytearray()  # release the frame
            c.sock.setblocking(True)
            c.sock.settimeout(10.0)
            try:
                c.sock.sendall(reply)
            finally:
                self._close(c.sock)
            return True
        if closed:
            self._close(c.sock)
            return True
        return False
