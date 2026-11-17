"""Bridge server hardening tests. Plain Python 3, no Blender needed:

    python3 tests/blender/test_protocol.py

Covers the shared protocol module used by all three add-ons: requests are
authenticated from the header before any payload byte is read, oversized
headers and payloads are refused, connections are capped, idle ones
dropped, and total buffering is bounded. Also checks the Blender add-on's
wiring with a stub `bpy` and the Maya-style blocking reader.
"""

import json
import os
import socket
import struct
import sys
import threading
import time
import types
import unittest

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
sys.dont_write_bytecode = True
sys.path.insert(0, os.path.join(ROOT, "addons", "blender", "newtopo_bridge"))
import protocol as p  # noqa: E402


def raw_frame(header, payload=b""):
    js = json.dumps(header).encode()
    return struct.pack("<I", len(js)) + js + payload, len(js)


def read_reply(sock, timeout=5.0):
    sock.settimeout(timeout)
    buf = bytearray()
    while True:
        r = p.try_decode(buf)
        if r:
            return r[0]
        try:
            chunk = sock.recv(65536)
        except (ConnectionResetError, socket.timeout):
            return None
        if not chunk:
            return None
        buf += chunk


class Harness(object):
    def __init__(self, token="right", handler=None):
        self.calls = []

        def default(header, payload):
            self.calls.append((header, bytes(payload)))
            return p.ack_frame([], 0.0, "test")

        self.server = p.PollingServer(0, handler or default, lambda: token)
        self.server.start()
        self.port = self.server.sock.getsockname()[1]

    def connect(self):
        return socket.create_connection(("127.0.0.1", self.port), timeout=5)

    def send_bg(self, sock, data):
        def run():
            try:
                sock.sendall(data)
            except OSError:
                pass

        t = threading.Thread(target=run, daemon=True)
        t.start()
        return t

    def pump(self, until, seconds=5.0):
        end = time.time() + seconds
        while time.time() < end:
            self.server.poll()
            if until():
                return True
            time.sleep(0.005)
        return False

    def close(self):
        self.server.stop()


class PollingServerTests(unittest.TestCase):
    def setUp(self):
        self.h = Harness()

    def tearDown(self):
        self.h.close()

    def test_wrong_token_with_huge_payload_is_refused_after_header(self):
        frame, hl = raw_frame({"version": 1, "type": "push", "token": "wrong", "payload_len": 400 << 20})
        s = self.h.connect()
        # The client also streams payload bytes; none may be read.
        self.h.send_bg(s, frame + b"\0" * (256 << 10))
        self.assertTrue(self.h.pump(lambda: self.h.server.bytes_read >= 4 + hl and not self.h.server.conns))
        reply = read_reply(s)
        if reply is not None:  # the reset may race the reply on some stacks
            self.assertEqual(reply["info"]["code"], "bad_token")
        self.assertEqual(self.h.server.bytes_read, 4 + hl, "read past the header")
        self.assertEqual(self.h.calls, [])
        s.close()

    def test_payload_over_cap_is_refused_after_header(self):
        frame, hl = raw_frame({"version": 1, "type": "push", "token": "right", "payload_len": p.MAX_PAYLOAD + 1})
        s = self.h.connect()
        self.h.send_bg(s, frame + b"\0" * 4096)
        self.assertTrue(self.h.pump(lambda: not self.h.server.conns and self.h.server.bytes_read > 0))
        reply = read_reply(s)
        if reply is not None:
            self.assertEqual(reply["info"]["code"], "too_large")
        self.assertEqual(self.h.server.bytes_read, 4 + hl)
        s.close()

    def test_header_over_cap_is_refused_after_length_prefix(self):
        s = self.h.connect()
        self.h.send_bg(s, struct.pack("<I", p.MAX_HEADER + 1) + b"{" * 4096)
        self.assertTrue(self.h.pump(lambda: not self.h.server.conns and self.h.server.bytes_read > 0))
        self.assertEqual(self.h.server.bytes_read, 4)
        s.close()

    def test_bad_version_and_type_are_refused(self):
        for header, code in [
            ({"version": 99, "type": "push", "token": "right", "payload_len": 10}, "version"),
            ({"version": 1, "type": "ack", "token": "right", "payload_len": 0}, "bad_request"),
            ({"version": 1, "type": "hello", "token": "right", "payload_len": 5}, "bad_request"),
        ]:
            frame, _ = raw_frame(header)
            s = self.h.connect()
            s.sendall(frame)
            self.assertTrue(self.h.pump(lambda: not self.h.server.conns and self.h.server.bytes_read > 0))
            self.assertEqual(read_reply(s)["info"]["code"], code)
            s.close()

    def test_connection_cap(self):
        socks = [self.h.connect() for _ in range(p.MAX_CONNECTIONS + 2)]
        self.assertTrue(self.h.pump(lambda: len(self.h.server.conns) == p.MAX_CONNECTIONS, 2.0))
        time.sleep(0.05)
        self.h.server.poll()
        self.assertEqual(len(self.h.server.conns), p.MAX_CONNECTIONS)
        busy = 0
        for s in socks:
            s.settimeout(0.2)
            try:
                data = s.recv(4096)
            except socket.timeout:
                continue  # an accepted, waiting connection
            r = p.try_decode(bytearray(data)) if data else None
            if r is not None and r[0]["info"]["code"] == "busy":
                busy += 1
        self.assertEqual(busy, 2)
        for s in socks:
            s.close()

    def test_idle_and_partial_connections_time_out(self):
        self.h.server.idle_seconds = 0.2
        s = self.h.connect()
        s.sendall(b"\x10\x00")  # half a length prefix, then nothing
        self.assertTrue(self.h.pump(lambda: len(self.h.server.conns) == 1, 1.0))
        self.assertTrue(self.h.pump(lambda: not self.h.server.conns, 2.0))
        self.assertEqual(read_reply(s)["info"]["code"], "timeout")
        s.close()

    def test_total_buffer_is_bounded_and_requests_still_finish(self):
        payload = os.urandom(300 << 10)
        frame, hl = raw_frame({"version": 1, "type": "push", "token": "right", "payload_len": len(payload)}, payload)
        self.h.server.total_buffer = len(frame) + 1000  # room for one push
        a, b = self.h.connect(), self.h.connect()
        self.h.send_bg(a, frame)
        self.h.send_bg(b, frame)
        peak = 0
        end = time.time() + 5
        while len(self.h.calls) < 2 and time.time() < end:
            self.h.server.poll()
            peak = max(peak, self.h.server.buffered())
            time.sleep(0.002)
        self.assertEqual(len(self.h.calls), 2)
        self.assertLessEqual(peak, self.h.server.total_buffer)
        self.assertTrue(all(c[1] == payload for c in self.h.calls))
        self.assertEqual(read_reply(a)["type"], "ack")
        self.assertEqual(read_reply(b)["type"], "ack")
        a.close()
        b.close()

    def test_valid_hello_and_push(self):
        s = self.h.connect()
        s.sendall(p.encode({"type": "push", "token": "right"}, b"abc"))
        self.assertTrue(self.h.pump(lambda: len(self.h.calls) == 1))
        self.assertEqual(self.h.calls[0][1], b"abc")
        self.assertEqual(read_reply(s)["type"], "ack")
        s.close()


class BlockingReaderTests(unittest.TestCase):
    """protocol.read_request, used by the Maya add-on's worker thread."""

    def test_refused_request_leaves_payload_unread(self):
        srv, cli = socket.socketpair()
        frame, hl = raw_frame({"version": 1, "type": "push", "token": "wrong", "payload_len": 1 << 20})
        cli.sendall(frame + b"PAYLOAD")
        self.assertIsNone(p.read_request(srv, "right"))
        self.assertEqual(read_reply(cli)["info"]["code"], "bad_token")
        srv.settimeout(0.5)
        self.assertEqual(srv.recv(7), b"PAYLOAD", "the payload must still be unread")
        srv.close()
        cli.close()

    def test_accepted_request_and_idle_timeout(self):
        srv, cli = socket.socketpair()
        cli.sendall(p.encode({"type": "push", "token": "right"}, b"xyz"))
        header, payload = p.read_request(srv, "right")
        self.assertEqual((header["type"], payload), ("push", b"xyz"))
        cli.sendall(b"\x05")
        self.assertIsNone(p.read_request(srv, "right", idle_s=0.2))
        self.assertEqual(read_reply(cli)["info"]["code"], "timeout")
        srv.close()
        cli.close()


class BlenderWiringTests(unittest.TestCase):
    """The Blender add-on passes its token to the polling server (stub bpy)."""

    def test_blender_addon_refuses_wrong_token_before_payload(self):
        bpy = types.ModuleType("bpy")
        bpy.app = types.SimpleNamespace(version_string="stub", version=(5, 2, 0), timers=None)
        bpy.props = types.ModuleType("bpy.props")
        for n in ("BoolProperty", "IntProperty", "StringProperty"):
            setattr(bpy.props, n, lambda **kw: None)
        bpy.types = types.SimpleNamespace(AddonPreferences=object)
        sys.modules["bpy"] = bpy
        sys.modules["bpy.props"] = bpy.props
        sys.path.insert(0, os.path.join(ROOT, "addons", "blender"))
        try:
            import newtopo_bridge as nb

            server = nb.start_server(0, "right")
            port = server.sock.getsockname()[1]
            frame, hl = raw_frame({"version": 1, "type": "push", "token": "wrong", "payload_len": 100 << 20})
            s = socket.create_connection(("127.0.0.1", port), timeout=5)
            s.sendall(frame + b"\0" * 1024)
            end = time.time() + 5
            while (server.bytes_read == 0 or server.conns) and time.time() < end:
                nb.poll()
                time.sleep(0.005)
            self.assertEqual(server.bytes_read, 4 + hl)
            s.close()
            nb.stop_server()
        finally:
            sys.path.remove(os.path.join(ROOT, "addons", "blender"))
            for m in [m for m in sys.modules if m == "bpy" or m.startswith(("bpy.", "newtopo_bridge"))]:
                del sys.modules[m]


if __name__ == "__main__":
    unittest.main(verbosity=2)
