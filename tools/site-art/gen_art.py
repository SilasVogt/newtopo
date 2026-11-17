#!/usr/bin/env python3
"""Generate the illustration SVGs for the marketing site (site/assets/art/).

Original art, drawn from first principles: a tiny orthographic renderer projects
parametric surfaces (planes, cylinders, fillets) of an invented demo part and
writes theme-aware SVG. No external assets, no dependencies beyond Python 3.

    python3 tools/site-art/gen_art.py
"""
from __future__ import annotations

import math
import os
from dataclasses import dataclass, field

OUT = os.path.join(os.path.dirname(__file__), "..", "..", "site", "assets", "art")
LEVELS = 24

# ---------------------------------------------------------------- vector math

def add(a, b): return (a[0] + b[0], a[1] + b[1], a[2] + b[2])
def sub(a, b): return (a[0] - b[0], a[1] - b[1], a[2] - b[2])
def mul(a, s): return (a[0] * s, a[1] * s, a[2] * s)
def dot(a, b): return a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
def cross(a, b): return (a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0])
def norm(a):
    l = math.sqrt(dot(a, a)) or 1.0
    return (a[0] / l, a[1] / l, a[2] / l)


class Camera:
    def __init__(self, yaw_deg=-38.0, elev_deg=30.0, scale=100.0, cx=0.0, cy=0.0):
        a, e = math.radians(yaw_deg), math.radians(elev_deg)
        ca, sa, ce, se = math.cos(a), math.sin(a), math.cos(e), math.sin(e)
        # world -> rotated frame (about z), then viewer at -y' elevated by e
        self.R = (ca, -sa, 0.0)
        self.U = (sa * se, ca * se, ce)
        self.V = (-sa * ce, -ca * ce, se)  # towards viewer
        self.scale, self.cx, self.cy = scale, cx, cy
        self.L = norm(add(add(mul(self.R, -0.45), mul(self.U, 0.75)), mul(self.V, 0.55)))

    def proj(self, p):
        return (self.cx + dot(p, self.R) * self.scale, self.cy - dot(p, self.U) * self.scale)

    def depth(self, p):
        return dot(p, self.V)

    def facing(self, n):
        return dot(n, self.V)

    def shade(self, n):
        d = max(0.0, dot(n, self.L))
        rim = max(0.0, 1.0 - self.facing(n)) ** 3 * 0.12
        v = 0.10 + 0.86 * d + rim
        return max(0, min(LEVELS, int(round(v * LEVELS))))


def fmt(v):
    s = f"{v:.1f}"
    return s[:-2] if s.endswith(".0") else s


def simplify(pts, tol=0.25):
    """Ramer-Douglas-Peucker on screen points."""
    if len(pts) < 3:
        return list(pts)
    (x0, y0), (x1, y1) = pts[0], pts[-1]
    dx, dy = x1 - x0, y1 - y0
    L = math.hypot(dx, dy)
    best, idx = -1.0, 0
    for k in range(1, len(pts) - 1):
        px, py = pts[k]
        d = abs(dy * (px - x0) - dx * (py - y0)) / L if L > 1e-9 else math.hypot(px - x0, py - y0)
        if d > best:
            best, idx = d, k
    if best <= tol:
        return [pts[0], pts[-1]]
    return simplify(pts[: idx + 1], tol)[:-1] + simplify(pts[idx:], tol)


def path_d(pts, close=True):
    pts = simplify(pts) if not close else pts
    out = "M" + " ".join(f"{fmt(x)} {fmt(y)}" for x, y in pts)
    return out + ("Z" if close else "")


# ---------------------------------------------------------------- scene items

@dataclass
class Surface:
    """Smooth parametric surface: f(u, v) -> point, n(u, v) -> outward normal."""
    f: object
    n: object
    u: tuple
    v: tuple
    nu: int
    nv: int
    lines: list = field(default_factory=list)  # (cls, [(u, v), ...])
    lines3d: list = field(default_factory=list)  # (cls, [p3, ...]) lying on this surface


@dataclass
class Planar:
    loops: list  # list of 3D point loops (first outer, rest holes)
    normal: tuple
    lines3d: list = field(default_factory=list)  # (cls, [p3...])


@dataclass
class Faces:
    """Flat-shaded polygons (a mesh), each (pts3d, normal)."""
    polys: list
    cls: str = "m"


def render_items(cam, items, smooth=True):
    """Return SVG markup for the ordered items (painter's order = list order)."""
    out = []
    for it in items:
        if isinstance(it, Surface):
            out.append(render_surface(cam, it))
        elif isinstance(it, Planar):
            out.append(render_planar(cam, it))
        elif isinstance(it, Faces):
            out.append(render_faces(cam, it))
    return "".join(out)


def lines_markup(cam, segs_by_cls):
    out = []
    for cls, subpaths in segs_by_cls.items():
        if not subpaths:
            continue
        d = "".join(path_d(sp, close=False) for sp in subpaths if len(sp) > 1)
        if d:
            out.append(f'<path class="{cls}" pathLength="1" d="{d}"/>')
    return "".join(out)


def render_surface(cam, s: Surface):
    u0, u1 = s.u
    v0, v1 = s.v
    by_level = {}
    us = [u0 + (u1 - u0) * i / s.nu for i in range(s.nu + 1)]
    for j in range(s.nv):
        va, vb = v0 + (v1 - v0) * j / s.nv, v0 + (v1 - v0) * (j + 1) / s.nv
        vm = (va + vb) / 2
        # run-length merge consecutive cells of one row that share a shade level
        run_lvl, run_start = None, 0
        cells = []
        for i in range(s.nu):
            n = s.n((us[i] + us[i + 1]) / 2, vm)
            cells.append(cam.shade(n) if cam.facing(n) > 0.0 else None)
        cells.append("end")
        for i, lvl in enumerate(cells):
            if lvl != run_lvl:
                if run_lvl is not None and run_lvl != "end":
                    lo = [cam.proj(s.f(us[k], va)) for k in range(run_start, i + 1)]
                    hi = [cam.proj(s.f(us[k], vb)) for k in range(i, run_start - 1, -1)]
                    by_level.setdefault(run_lvl, []).append(simplify(lo) + simplify(hi))
                run_lvl, run_start = lvl, i
    out = []
    for lvl in sorted(by_level):
        d = "".join(path_d(q) for q in by_level[lvl])
        out.append(f'<path class="l{lvl}" d="{d}"/>')
    # lines on the surface, culled where the surface faces away
    segs = {}
    for cls, pts in s.lines:
        cur = []
        for k in range(len(pts)):
            u, v = pts[k]
            vis = cam.facing(s.n(u, v)) > 0.02
            if vis:
                cur.append(cam.proj(s.f(u, v)))
            else:
                if len(cur) > 1:
                    segs.setdefault(cls, []).append(cur)
                cur = []
        if len(cur) > 1:
            segs.setdefault(cls, []).append(cur)
    for cls, pts in s.lines3d:
        segs.setdefault(cls, []).append([cam.proj(p) for p in pts])
    out.append(lines_markup(cam, segs))
    return "".join(out)


def render_planar(cam, p: Planar):
    if cam.facing(p.normal) <= 0:
        return ""
    lvl = cam.shade(p.normal)
    d = "".join(path_d([cam.proj(q) for q in loop]) for loop in p.loops)
    out = [f'<path class="l{lvl}" fill-rule="evenodd" d="{d}"/>']
    segs = {}
    for cls, pts in p.lines3d:
        segs.setdefault(cls, []).append([cam.proj(q) for q in pts])
    out.append(lines_markup(cam, segs))
    return "".join(out)


def render_faces(cam, f: Faces):
    vis = [(sum(cam.depth(q) for q in pts) / len(pts), pts, n) for pts, n in f.polys if cam.facing(n) > 0.0]
    vis.sort(key=lambda t: t[0])
    out = [f'<g class="{f.cls}">']
    for _, pts, n in vis:
        out.append(f'<path class="l{cam.shade(n)}" d="{path_d([cam.proj(q) for q in pts])}"/>')
    out.append("</g>")
    return "".join(out)


# ---------------------------------------------------------------- the demo part
# A rounded plate with a filleted top edge, and a filleted boss with a blind hole.

A, B, RC, H, RF = 3.0, 2.0, 0.85, 1.0, 0.22  # plate half sizes, corner radius, height, edge fillet
C = (0.75, 0.2)  # boss centre
R, HB, RBF, RTF, RH, HD = 1.0, 1.15, 0.26, 0.14, 0.42, 0.75  # boss radius, height, fillets, hole radius, hole depth
ZT = H + HB


class RoundRect:
    """Arc-length parameterised rounded rectangle, s in [0, 1), counter-clockwise from +x mid side."""

    def __init__(self, a, b, r):
        self.a, self.b, self.r = a, b, r
        sx, sy, arc = 2 * (a - r), 2 * (b - r), math.pi / 2 * r
        # segments: right(half), TR arc, top, TL arc, left, BL arc, bottom, BR arc, right(half)
        self.segs = [("line", (a, 0), (a, b - r), (1, 0)), ("arc", (a - r, b - r), 0.0),
                     ("line", (a - r, b), (-a + r, b), (0, 1)), ("arc", (-a + r, b - r), math.pi / 2),
                     ("line", (-a, b - r), (-a, -b + r), (-1, 0)), ("arc", (-a + r, -b + r), math.pi),
                     ("line", (-a + r, -b), (a - r, -b), (0, -1)), ("arc", (a - r, -b + r), 1.5 * math.pi),
                     ("line", (a, -b + r), (a, 0), (1, 0))]
        self.lens = [sy / 2, arc, sx, arc, sy, arc, sx, arc, sy / 2]
        self.total = sum(self.lens)
        acc, self.starts = 0.0, []
        for l in self.lens:
            self.starts.append(acc / self.total)
            acc += l

    def at(self, s):
        s = s % 1.0
        L = s * self.total
        for seg, l in zip(self.segs, self.lens):
            if L <= l + 1e-12:
                t = L / l if l else 0
                if seg[0] == "line":
                    _, p0, p1, n = seg
                    return (p0[0] + (p1[0] - p0[0]) * t, p0[1] + (p1[1] - p0[1]) * t), n
                _, c, a0 = seg
                ang = a0 + t * math.pi / 2
                n = (math.cos(ang), math.sin(ang))
                return (c[0] + n[0] * self.r, c[1] + n[1] * self.r), n
            L -= l
        return self.at(0.0)

    def junctions(self):
        return self.starts[1:]


RR = RoundRect(A, B, RC)


def stations_plate():
    """CAD-aware stations along the plate outline (junctions + evenly spread interior)."""
    counts = [3, 4, 10, 4, 5, 4, 10, 4, 2]  # segments per CAD face (half sides at the ends)
    st = []
    for k, (s0, cnt) in enumerate(zip(RR.starts, counts)):
        s1 = RR.starts[k + 1] if k + 1 < len(RR.starts) else 1.0
        for i in range(cnt):
            st.append(s0 + (s1 - s0) * i / cnt)
    return st


def plate_side():
    def f(s, t):
        (x, y), _ = RR.at(s)
        return (x, y, t * (H - RF))
    def n(s, t):
        _, (nx, ny) = RR.at(s)
        return (nx, ny, 0.0)
    return f, n


def plate_fillet():
    def f(s, ph):
        (x, y), (nx, ny) = RR.at(s)
        k = RF * (1 - math.sin(ph))
        return (x - nx * k, y - ny * k, H - RF * (1 - math.cos(ph)))
    def n(s, ph):
        _, (nx, ny) = RR.at(s)
        return (nx * math.sin(ph), ny * math.sin(ph), math.cos(ph))
    return f, n


def boss_ring(radius_fn, z_fn, normal_fn):
    def f(th, ph):
        r = radius_fn(ph)
        return (C[0] + r * math.cos(th), C[1] + r * math.sin(th), z_fn(ph))
    def n(th, ph):
        a, b = normal_fn(ph)
        return (a * math.cos(th), a * math.sin(th), b)
    return f, n


BASE_FILLET = boss_ring(lambda p: R + RBF - RBF * math.sin(p), lambda p: H + RBF - RBF * math.cos(p),
                        lambda p: (math.sin(p), math.cos(p)))
BOSS_CYL = boss_ring(lambda z: R, lambda z: z, lambda z: (1.0, 0.0))
TOP_FILLET = boss_ring(lambda p: R - RTF + RTF * math.sin(p), lambda p: ZT - RTF + RTF * math.cos(p),
                       lambda p: (math.sin(p), math.cos(p)))
HOLE_WALL = boss_ring(lambda z: RH, lambda z: z, lambda z: (-1.0, 0.0))


def circle(r, z, n=96, c=C):
    return [(c[0] + r * math.cos(2 * math.pi * i / n), c[1] + r * math.sin(2 * math.pi * i / n), z) for i in range(n)]


def top_inner(s):
    (x, y), (nx, ny) = RR.at(s)
    return (x - nx * RF, y - ny * RF, H)


NB = 32  # stations around the boss
TAU = 2 * math.pi


def polyline_u(u0, u1, v, n=64):
    return [(u0 + (u1 - u0) * i / n, v) for i in range(n + 1)]


def polyline_v(u, v0, v1, n=12):
    return [(u, v0 + (v1 - v0) * i / n) for i in range(n + 1)]


def fill_chords():
    """Auto-fill style chords on the top face: connect plate-outline stations to boss stations."""
    outer = [top_inner(s) for s in stations_plate()]
    inner = [(C[0] + (R + RBF) * math.cos(TAU * k / NB), C[1] + (R + RBF) * math.sin(TAU * k / NB), H) for k in range(NB)]
    def ang(p):
        return math.atan2(p[1] - C[1], p[0] - C[0]) % TAU
    oa = [ang(p) for p in outer]
    ia = [TAU * k / NB for k in range(NB)]
    chords = set()
    def nearest(a, arr):
        return min(range(len(arr)), key=lambda k: min(abs(arr[k] - a), TAU - abs(arr[k] - a)))
    for i, a in enumerate(oa):
        chords.add((i, nearest(a, ia)))
    for j, a in enumerate(ia):
        chords.add((nearest(a, oa), j))
    return outer, inner, sorted(chords)


def part_items(mode):
    """mode: 'import' (CAD edges), 'lines' (+ user lines), 'fill' (+ auto-fill chords)."""
    want_lines = mode in ("lines", "fill")
    st = stations_plate()
    junc = set(round(j, 9) for j in RR.junctions()) | {0.0}
    gen = [s for s in st if round(s, 9) not in junc]

    sf, sn = plate_side()
    side = Surface(sf, sn, (0, 1), (0, 1), 240, 1)
    ff, fn = plate_fillet()
    fil = Surface(ff, fn, (0, 1), (0, math.pi / 2), 240, 6)
    for s in RR.junctions():
        side.lines.append(("cad", polyline_v(s, 0, 1, 2)))
        fil.lines.append(("cad", polyline_v(s, 0, math.pi / 2)))
    side.lines.append(("cad", polyline_u(0, 1, 0.0, 400)))
    side.lines.append(("cad", polyline_u(0, 1, 1.0, 400)))
    fil.lines.append(("cad", polyline_u(0, 1, 0.0, 400)))
    if want_lines:
        for s in gen:
            side.lines.append(("ln", polyline_v(s, 0, 1, 2)))
            fil.lines.append(("ln", polyline_v(s, 0, math.pi / 2)))
        fil.lines.append(("ln", polyline_u(0, 1, math.pi / 4, 400)))

    outer = [top_inner(i / 400) for i in range(400)]
    top = Planar([outer, circle(R + RBF, H)], (0, 0, 1))
    if mode == "fill":
        o, inn, ch = fill_chords()
        for i, j in ch:
            top.lines3d.append(("fe", [o[i], inn[j]]))

    bf = Surface(*BASE_FILLET, (0, TAU), (0, math.pi / 2), 128, 6)
    cyl = Surface(*BOSS_CYL, (0, TAU), (H + RBF, ZT - RTF), 128, 1)
    tf = Surface(*TOP_FILLET, (0, TAU), (0, math.pi / 2), 128, 4)
    bf.lines3d.append(("cad", circle(R + RBF, H, 128) + [circle(R + RBF, H, 128)[0]]))
    for srf, lo, hi in ((bf, 0, math.pi / 2), (cyl, H + RBF, ZT - RTF), (tf, 0, math.pi / 2)):
        srf.lines.append(("cad", polyline_u(0, TAU, hi if srf is not tf else hi, 160)))
        if want_lines:
            for k in range(NB):
                srf.lines.append(("ln", polyline_v(TAU * k / NB, lo, hi, 8)))
    tf.lines.append(("cad", polyline_u(0, TAU, 0.0, 160)))
    if want_lines:
        bf.lines.append(("ln", polyline_u(0, TAU, math.pi / 4, 160)))
        cyl.lines.append(("ln", polyline_u(0, TAU, (H + RBF + ZT - RTF) / 2, 160)))

    hw = Surface(*HOLE_WALL, (0, TAU), (ZT - HD, ZT), 96, 1)
    hb = Planar([circle(RH, ZT - HD)], (0, 0, 1))
    hw.lines.append(("cad", polyline_u(0, TAU, ZT - HD, 120)))
    cap = Planar([circle(R - RTF, ZT), circle(RH, ZT)], (0, 0, 1))
    cap.lines3d.append(("cad", circle(RH, ZT, 128) + [circle(RH, ZT, 128)[0]]))
    if want_lines:
        for k in range(NB):
            th = TAU * k / NB
            hw.lines.append(("ln", polyline_v(th, ZT - HD, ZT, 2)))
            cap.lines3d.append(("ln", [(C[0] + RH * math.cos(th), C[1] + RH * math.sin(th), ZT),
                                       (C[0] + (R - RTF) * math.cos(th), C[1] + (R - RTF) * math.sin(th), ZT)]))
    return [side, fil, top, hb, hw, bf, cyl, tf, cap]


# ---------------------------------------------------------------- meshes of the part

def grid_faces(f, n, us, vs, wrap=False):
    polys = []
    nu = len(us) - (0 if wrap else 1)
    for i in range(nu):
        ua, ub = us[i], us[(i + 1) % len(us)]
        if wrap and i == len(us) - 1:
            ub = us[0] + TAU
        for j in range(len(vs) - 1):
            va, vb = vs[j], vs[j + 1]
            pts = [f(ua, va), f(ub, va), f(ub, vb), f(ua, vb)]
            polys.append((pts, n((ua + ub) / 2, (va + vb) / 2)))
    return polys


def tri_split(polys, flip=False):
    out = []
    for pts, nn in polys:
        if len(pts) == 4:
            if flip:
                out += [([pts[0], pts[1], pts[3]], nn), ([pts[1], pts[2], pts[3]], nn)]
            else:
                out += [([pts[0], pts[1], pts[2]], nn), ([pts[0], pts[2], pts[3]], nn)]
        else:
            out.append((pts, nn))
    return out


def part_quads():
    st = stations_plate() + [1.0]
    sf, sn = plate_side()
    ff, fn = plate_fillet()
    th = [TAU * k / NB for k in range(NB)]
    side = grid_faces(sf, sn, st, [0, 1])
    fil = grid_faces(ff, fn, st, [0, math.pi / 4, math.pi / 2])
    hole = grid_faces(*HOLE_WALL, th, [ZT - HD, ZT], wrap=True)
    zm = (H + RBF + ZT - RTF) / 2
    bf = grid_faces(*BASE_FILLET, th, [0, math.pi / 4, math.pi / 2], wrap=True)
    cyl = grid_faces(*BOSS_CYL, th, [H + RBF, zm, ZT - RTF], wrap=True)
    tf = grid_faces(*TOP_FILLET, th, [0, math.pi / 2], wrap=True)
    cap = []
    for k in range(NB):
        a, b = th[k], th[(k + 1) % NB] + (TAU if k == NB - 1 else 0)
        p = lambda r, t: (C[0] + r * math.cos(t), C[1] + r * math.sin(t), ZT)
        cap.append(([p(RH, a), p(RH, b), p(R - RTF, b), p(R - RTF, a)], (0, 0, 1)))
    # top face: polygons from the auto-fill chords (quads where both sides advance, else triangles)
    o, inn, ch = fill_chords()
    top = []
    ch = sorted(ch, key=lambda c: (c[0], c[1]))
    order = []
    for i, j in ch:
        order.append((i, j))
    # walk consecutive chords around the ring
    order.sort(key=lambda c: (math.atan2(o[c[0]][1] - C[1], o[c[0]][0] - C[0]) % TAU
                              + math.atan2(inn[c[1]][1] - C[1], inn[c[1]][0] - C[0]) % TAU))
    for k in range(len(order)):
        i0, j0 = order[k]
        i1, j1 = order[(k + 1) % len(order)]
        pts = [o[i0]]
        if i1 != i0:
            pts.append(o[i1])
        pts.append(inn[j1])
        if j1 != j0:
            pts.append(inn[j0])
        if len(pts) >= 3:
            top.append((pts, (0, 0, 1)))
    bottom = []
    return side, fil, top, hole, bf, cyl, tf, cap


def part_soup():
    """What a typical tessellated export looks like: few stations on flats, long slivers."""
    st = list(RR.starts) + [1.0]
    dense = []
    for k in range(len(st) - 1):
        seg = RR.segs[k][0]
        cnt = 5 if seg == "arc" else 1
        for i in range(cnt):
            dense.append(st[k] + (st[k + 1] - st[k]) * i / cnt)
    dense.append(1.0)
    sf, sn = plate_side()
    ff, fn = plate_fillet()
    nb = 40
    th = [TAU * k / nb for k in range(nb)]
    side = tri_split(grid_faces(sf, sn, dense, [0, 1]))
    fil = tri_split(grid_faces(ff, fn, dense, [0, 0.5, 1.05, math.pi / 2]), flip=True)
    hole = tri_split(grid_faces(*HOLE_WALL, th, [ZT - HD, ZT], wrap=True))
    bf = tri_split(grid_faces(*BASE_FILLET, th, [0, 0.8, math.pi / 2], wrap=True))
    cyl = tri_split(grid_faces(*BOSS_CYL, th, [H + RBF, ZT - RTF], wrap=True), flip=True)
    tf = tri_split(grid_faces(*TOP_FILLET, th, [0, math.pi / 2], wrap=True))
    # top face: zip the sparse outline to the boss circle -> fans and slivers
    outer = [top_inner(s) for s in dense[:-1]]
    inner = [(C[0] + (R + RBF) * math.cos(t), C[1] + (R + RBF) * math.sin(t), H) for t in th]
    top = zipper(outer, inner, (0, 0, 1))
    capo = [(C[0] + (R - RTF) * math.cos(t), C[1] + (R - RTF) * math.sin(t), ZT) for t in th]
    capi = [(C[0] + RH * math.cos(TAU * k / 18), C[1] + RH * math.sin(TAU * k / 18), ZT) for k in range(18)]
    cap = zipper(capo, capi, (0, 0, 1))
    return side, fil, top, hole, bf, cyl, tf, cap


def zipper(outer, inner, n):
    def ang(p):
        return math.atan2(p[1] - C[1], p[0] - C[0]) % TAU
    i0 = min(range(len(outer)), key=lambda k: ang(outer[k]))
    j0 = min(range(len(inner)), key=lambda k: ang(inner[k]))
    O = outer[i0:] + outer[:i0]
    I = inner[j0:] + inner[:j0]
    i = j = 0
    tris = []
    while i < len(O) or j < len(I):
        a_o = ang(O[(i + 1) % len(O)]) + (TAU if i + 1 >= len(O) else 0)
        a_i = ang(I[(j + 1) % len(I)]) + (TAU if j + 1 >= len(I) else 0)
        if j >= len(I) or (i < len(O) and a_o <= a_i):
            tris.append(([O[i % len(O)], O[(i + 1) % len(O)], I[j % len(I)]], n))
            i += 1
        else:
            tris.append(([O[i % len(O)], I[(j + 1) % len(I)], I[j % len(I)]], n))
            j += 1
    return tris


def mesh_items(parts, cls):
    side, fil, top, hole, bf, cyl, tf, cap = parts
    hb = Planar([circle(RH, ZT - HD)], (0, 0, 1))
    return [Faces(side, cls), Faces(fil, cls), Faces(top, cls), hb, Faces(hole, cls),
            Faces(bf, cls), Faces(cyl, cls), Faces(tf, cls), Faces(cap, cls)]


# ---------------------------------------------------------------- styling

def style(levels=LEVELS, anim=""):
    lv = "".join(
        f".l{i}{{--f:color-mix(in oklab,var(--hi) {round(100 * i / levels)}%,var(--lo))}}" for i in range(levels + 1))
    return f"""<style>
svg{{--lo:#161b20;--hi:#a7b2bc;--cad:#d8dee4;--ln:#57b6ee;--fe:#e7a65a;--edge:#0f1317;--edge-q:#5ab3e8;--soup:#1a2026}}
@media (prefers-color-scheme:light){{svg{{--lo:#6b7883;--hi:#e3e8ec;--cad:#20272d;--ln:#0b72b5;--fe:#b0620c;--edge:#e9edf0;--edge-q:#0b72b5;--soup:#3a434b}}}}
{lv}
[class^=l]{{fill:var(--f);stroke:var(--f);stroke-width:.7;stroke-linejoin:round}}
.cad,.ln,.fe,.mk{{fill:none;stroke-linecap:round;stroke-linejoin:round}}
.cad{{stroke:var(--cad);stroke-width:1.5}}
.ln{{stroke:var(--ln);stroke-width:1.15}}
.fe{{stroke:var(--fe);stroke-width:1.1;opacity:.95}}
.q [class^=l]{{stroke:var(--edge-q);stroke-width:.85}}
.t [class^=l]{{stroke:var(--soup);stroke-width:.6;stroke-opacity:.8}}
.mk{{stroke:var(--ln);stroke-width:2}}
.dot{{fill:var(--ln)}}
.dotc{{fill:var(--cad)}}
.cadf{{fill:var(--cad)}}
.ghost{{fill:none;stroke:var(--cad);stroke-width:1;stroke-dasharray:4 5;opacity:.6}}
{anim}
</style>"""


HERO_ANIM = """
@media (prefers-reduced-motion:no-preference){
.ln{stroke-dasharray:1;stroke-dashoffset:1;animation:draw 1.6s cubic-bezier(.2,.7,.2,1) forwards}
.fe{opacity:0;animation:fade .9s ease-out 1.5s forwards}
@keyframes draw{to{stroke-dashoffset:0}}
@keyframes fade{to{opacity:.95}}
}"""


def svg_doc(w, h, body, title, anim=""):
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{w}" height="{h}" role="img">'
            f"<title>{title}</title>{style(anim=anim)}{body}</svg>\n")


def write(name, content):
    os.makedirs(OUT, exist_ok=True)
    p = os.path.join(OUT, name)
    with open(p, "w") as fh:
        fh.write(content)
    print(f"{name}: {len(content) / 1024:.1f} KB")


# ---------------------------------------------------------------- small tool scenes

def cyl_scene(count=12):
    r, h = 1.0, 2.2
    f = lambda t, z: (r * math.cos(t), r * math.sin(t), z)
    n = lambda t, z: (math.cos(t), math.sin(t), 0)
    s = Surface(f, n, (0, TAU), (0, h), 96, 1)
    s.lines.append(("cad", polyline_u(0, TAU, 0, 120)))
    for k in range(count):
        s.lines.append(("ln", polyline_v(TAU * k / count + 0.2, 0, h, 2)))
    capc = [(r * math.cos(TAU * i / 96), r * math.sin(TAU * i / 96), h) for i in range(96)]
    cap = Planar([capc], (0, 0, 1))
    cap.lines3d.append(("cad", capc + [capc[0]]))
    return [s, cap]


def plate_hole_scene():
    a, b, t, hr, hc = 2.2, 1.5, 0.3, 0.6, (0.3, 0.05)
    items = []
    corners = [(a, -b), (a, b), (-a, b), (-a, -b)]
    for k in range(4):
        p0, p1 = corners[k], corners[(k + 1) % 4]
        nx, ny = p1[1] - p0[1], -(p1[0] - p0[0])
        nn = norm((nx, ny, 0))
        loop = [(p0[0], p0[1], 0), (p1[0], p1[1], 0), (p1[0], p1[1], t), (p0[0], p0[1], t)]
        pl = Planar([loop], nn)
        pl.lines3d.append(("cad", [(p0[0], p0[1], 0), (p1[0], p1[1], 0)]))
        items.append(pl)
    hw = Surface(lambda th, z: (hc[0] + hr * math.cos(th), hc[1] + hr * math.sin(th), z),
                 lambda th, z: (-math.cos(th), -math.sin(th), 0), (0, TAU), (0, t), 64, 1)
    items.insert(0, hw)
    top_loop = [(x, y, t) for x, y in corners]
    hole = [(hc[0] + hr * math.cos(TAU * i / 96), hc[1] + hr * math.sin(TAU * i / 96), t) for i in range(96)]
    top = Planar([top_loop, hole], (0, 0, 1))
    top.lines3d.append(("cad", top_loop + [top_loop[0]]))
    top.lines3d.append(("cad", hole + [hole[0]]))
    U, Vn = 9, 6
    def clip(p, q):
        segs, cur = [], []
        for i in range(81):
            x = p[0] + (q[0] - p[0]) * i / 80
            y = p[1] + (q[1] - p[1]) * i / 80
            if (x - hc[0]) ** 2 + (y - hc[1]) ** 2 > hr * hr:
                cur.append((x, y, t))
            else:
                if len(cur) > 1:
                    segs.append(cur)
                cur = []
        if len(cur) > 1:
            segs.append(cur)
        return segs
    for i in range(1, U + 1):
        x = -a + 2 * a * i / (U + 1)
        for sgm in clip((x, -b), (x, b)):
            top.lines3d.append(("ln", sgm))
    for j in range(1, Vn + 1):
        y = -b + 2 * b * j / (Vn + 1)
        for sgm in clip((-a, y), (a, y)):
            top.lines3d.append(("ln", sgm))
    return items + [top]


def contour(s: Surface, fn, level, nu=160, nv=24):
    """Marching squares on a surface's parameter grid; returns list of (u, v) segments."""
    u0, u1 = s.u
    v0, v1 = s.v
    g = [[fn(s.f(u0 + (u1 - u0) * i / nu, v0 + (v1 - v0) * j / nv)) - level for j in range(nv + 1)] for i in range(nu + 1)]
    segs = []
    for i in range(nu):
        for j in range(nv):
            cs = [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)]
            pts = []
            for k in range(4):
                (ia, ja), (ib, jb) = cs[k], cs[(k + 1) % 4]
                fa, fb = g[ia][ja], g[ib][jb]
                if (fa < 0) != (fb < 0):
                    t = fa / (fa - fb)
                    ui = ia + (ib - ia) * t
                    vj = ja + (jb - ja) * t
                    pts.append((u0 + (u1 - u0) * ui / nu, v0 + (v1 - v0) * vj / nv))
            if len(pts) == 2:
                segs.append(pts)
            elif len(pts) == 4:
                segs += [pts[:2], pts[2:]]
    return segs


def slice_scene():
    sf, sn = plate_side()
    ff, fn = plate_fillet()
    side = Surface(sf, sn, (0, 1), (0, 1), 240, 1)
    fil = Surface(ff, fn, (0, 1), (0, math.pi / 2), 240, 6)
    side.lines.append(("cad", polyline_u(0, 1, 0.0, 400)))
    side.lines.append(("cad", polyline_u(0, 1, 1.0, 400)))
    fil.lines.append(("cad", polyline_u(0, 1, 0.0, 400)))
    outer = [top_inner(i / 400) for i in range(400)]
    top = Planar([outer], (0, 0, 1))
    xs = [-A + 2 * A * (k + 0.5) / 9 for k in range(9)]
    for x in xs:
        for srf in (side, fil):
            for sg in contour(srf, lambda p: p[0], x, 320, 8):
                srf.lines.append(("ln", sg))
        lim = A - RF
        ys = [y for y in [(-B + RF) + (2 * (B - RF)) * i / 200 for i in range(201)]]
        inside = []
        for y in ys:
            # inside inner rounded rect?
            cx_, cy_ = max(abs(x) - (A - RC), 0), max(abs(y) - (B - RC), 0)
            if abs(x) <= lim and abs(y) <= B - RF and cx_ * cx_ + cy_ * cy_ <= (RC - RF) ** 2:
                inside.append((x, y, H))
        if len(inside) > 1:
            top.lines3d.append(("ln", inside))
    return [side, fil, top]


def iso_scene():
    # a patch of plate with the filleted boss; one through-iso loop on the base fillet
    pl = 1.7
    sq = [(-pl, -pl, H), (pl, -pl, H), (pl, pl, H), (-pl, pl, H)]
    sq = [(x + C[0], y + C[1], z) for x, y, z in sq]
    top = Planar([sq, circle(R + RBF, H)], (0, 0, 1))
    top.lines3d.append(("cad", circle(R + RBF, H, 128) + [circle(R + RBF, H, 128)[0]]))
    bf = Surface(*BASE_FILLET, (0, TAU), (0, math.pi / 2), 128, 8)
    cyl = Surface(*BOSS_CYL, (0, TAU), (H + RBF, ZT - RTF), 128, 1)
    tf = Surface(*TOP_FILLET, (0, TAU), (0, math.pi / 2), 128, 4)
    bf.lines.append(("cad", polyline_u(0, TAU, math.pi / 2, 160)))
    cyl.lines.append(("cad", polyline_u(0, TAU, ZT - RTF, 160)))
    tf.lines.append(("cad", polyline_u(0, TAU, 0, 160)))
    cap = Planar([circle(R - RTF, ZT)], (0, 0, 1))
    bf.lines.append(("ln", polyline_u(0, TAU, math.pi / 4, 200)))
    cyl.lines.append(("ln", polyline_u(0, TAU, H + 0.7, 200)))
    return [top, bf, cyl, tf, cap]


def connect_scene(count=16):
    # the filleted boss on a patch of plate: Connect lines run along the
    # cylinder and carry straight on across both fillets as one line each
    pl = 1.75
    sq = [(-pl, -pl, H), (pl, -pl, H), (pl, pl, H), (-pl, pl, H)]
    sq = [(x + C[0], y + C[1], z) for x, y, z in sq]
    top = Planar([sq, circle(R + RBF, H)], (0, 0, 1))
    top.lines3d.append(("cad", circle(R + RBF, H, 128) + [circle(R + RBF, H, 128)[0]]))
    bf = Surface(*BASE_FILLET, (0, TAU), (0, math.pi / 2), 128, 8)
    cyl = Surface(*BOSS_CYL, (0, TAU), (H + RBF, ZT - RTF), 128, 1)
    tf = Surface(*TOP_FILLET, (0, TAU), (0, math.pi / 2), 128, 4)
    bf.lines.append(("cad", polyline_u(0, TAU, math.pi / 2, 160)))
    cyl.lines.append(("cad", polyline_u(0, TAU, ZT - RTF, 160)))
    tf.lines.append(("cad", polyline_u(0, TAU, 0, 160)))
    cap = Planar([circle(R - RTF, ZT)], (0, 0, 1))
    for k in range(count):
        t = TAU * k / count + 0.1
        bf.lines.append(("ln", polyline_v(t, 0, math.pi / 2, 16)))
        cyl.lines.append(("ln", polyline_v(t, H + RBF, ZT - RTF, 2)))
        tf.lines.append(("ln", polyline_v(t, 0, math.pi / 2, 16)))
    return [top, bf, cyl, tf, cap]


def link_scene():
    a, b, t, hr = 3.0, 1.9, 0.35, 0.42
    centres = [(x, y) for y in (-0.85, 0.85) for x in (-1.9, 0.0, 1.9)]
    items = []
    for (hx, hy) in centres:
        hw = Surface(lambda th, z, hx=hx, hy=hy: (hx + hr * math.cos(th), hy + hr * math.sin(th), z),
                     lambda th, z: (-math.cos(th), -math.sin(th), 0), (0, TAU), (0, t), 48, 1)
        for k in range(8):
            hw.lines.append(("ln", polyline_v(TAU * k / 8 + 0.3, 0, t, 2)))
        items.append(hw)
    corners = [(a, -b), (a, b), (-a, b), (-a, -b)]
    for k in range(4):
        p0, p1 = corners[k], corners[(k + 1) % 4]
        nn = norm((p1[1] - p0[1], -(p1[0] - p0[0]), 0))
        pl = Planar([[(p0[0], p0[1], 0), (p1[0], p1[1], 0), (p1[0], p1[1], t), (p0[0], p0[1], t)]], nn)
        pl.lines3d.append(("cad", [(p0[0], p0[1], 0), (p1[0], p1[1], 0)]))
        items.append(pl)
    loops = [[(x, y, t) for x, y in corners]]
    for hx, hy in centres:
        loops.append([(hx + hr * math.cos(TAU * i / 64), hy + hr * math.sin(TAU * i / 64), t) for i in range(64)])
    top = Planar(loops, (0, 0, 1))
    for lp in loops:
        top.lines3d.append(("cad", lp + [lp[0]]))
    for hx, hy in centres:
        for k in range(8):
            th = TAU * k / 8 + 0.3
            top.lines3d.append(("ln", [(hx + hr * math.cos(th), hy + hr * math.sin(th), t),
                                       (hx + (hr + 0.16) * math.cos(th), hy + (hr + 0.16) * math.sin(th), t)]))
    items.append(top)
    return items, t


def cut_scene():
    s = 1.6
    faces = [
        ([(s, -s, 0), (s, s, 0), (s, s, 2 * s * .8), (s, -s, 2 * s * .8)], (1, 0, 0)),
        ([(-s, -s, 0), (s, -s, 0), (s, -s, 2 * s * .8), (-s, -s, 2 * s * .8)], (0, -1, 0)),
        ([(-s, -s, 2 * s * .8), (s, -s, 2 * s * .8), (s, s, 2 * s * .8), (-s, s, 2 * s * .8)], (0, 0, 1)),
    ]
    items = []
    for loop, n in faces:
        p = Planar([loop], n)
        p.lines3d.append(("cad", loop + [loop[0]]))
        items.append(p)
    zt = 2 * s * .8
    path = [(-0.7, -s, 0.5), (s * 0.15, -s, 1.6), (s * 0.55, -s, zt), (s, -0.3, zt), (s, 0.6, 1.1), (s, 1.1, 0.35)]
    # path goes front face -> top edge -> top face -> right edge -> right face
    path = [(-0.8, -s, 0.55), (0.2, -s, 1.85), (0.55, -s, zt), (1.1, -0.6, zt), (s, -0.1, zt), (s, 0.7, 1.4), (s, 1.15, 0.5)]
    items[-1].lines3d.append(("ln", path))
    return items, path


def build():
    # ---------- hero: the exact CAD surfaces, user lines, auto-fill chords
    cam = Camera(yaw_deg=-36, elev_deg=31, scale=118, cx=470, cy=360)
    body = render_items(cam, part_items("fill"))
    write("hero-part.svg", svg_doc(940, 640, body, "A filleted CAD part with lines drawn on its exact surfaces", HERO_ANIM))

    # ---------- loop states (same part, same camera, smaller)
    cam_s = Camera(yaw_deg=-36, elev_deg=31, scale=74, cx=300, cy=232)
    write("loop-import.svg", svg_doc(600, 400, render_items(cam_s, part_items("import")), "Imported CAD part with its edges"))
    write("loop-lines.svg", svg_doc(600, 400, render_items(cam_s, part_items("lines")), "Lines added on the exact surfaces"))
    write("loop-fill.svg", svg_doc(600, 400, render_items(cam_s, part_items("fill")), "Auto-fill connects the remaining polygons"))
    write("loop-preview.svg", svg_doc(600, 400, '<g class="q">' + render_items(cam_s, mesh_items(part_quads(), "q")) + "</g>",
                                      "Preview of the output mesh"))

    # ---------- before / after (same framing)
    cam_b = Camera(yaw_deg=-36, elev_deg=31, scale=112, cx=470, cy=345)
    write("compare-soup.svg", svg_doc(940, 620, render_items(cam_b, mesh_items(part_soup(), "t")),
                                      "Tessellated CAD export: long thin triangles"))
    write("compare-quads.svg", svg_doc(940, 620, render_items(cam_b, mesh_items(part_quads(), "q")),
                                       "newtopo output: quads that follow the surfaces"))

    # ---------- tool glyphs
    cam_t = Camera(yaw_deg=-34, elev_deg=28, scale=64, cx=200, cy=168)
    write("tool-connect.svg", svg_doc(520, 340, render_items(Camera(-36, 32, 96, 196, 262), connect_scene()),
                                      "Connect: lines along a boss and across both of its fillets"))
    write("tool-grid.svg", svg_doc(400, 280, render_items(Camera(-30, 38, 72, 200, 150), plate_hole_scene()),
                                   "Surface Grid: a grid that stops at a hole"))
    write("tool-slice.svg", svg_doc(400, 280, render_items(Camera(-34, 30, 52, 200, 150), slice_scene()),
                                    "Slice: evenly spaced loops across a body"))
    cam_i = Camera(-36, 30, 64, 160, 222)
    body = render_items(cam_i, iso_scene())
    write("tool-iso.svg", svg_doc(400, 280, body, "Iso: one exact line around a fillet"))
    items, path = cut_scene()
    cam_c = Camera(-38, 26, 52, 200, 195)
    dots = "".join(f'<circle class="dot" cx="{fmt(cam_c.proj(p)[0])}" cy="{fmt(cam_c.proj(p)[1])}" r="4"/>' for p in path)
    write("tool-cut.svg", svg_doc(400, 280, render_items(cam_c, items) + dots, "Cut: a path of points across three faces"))
    items, t = link_scene()
    cam_l = Camera(-24, 44, 50, 200, 132)
    p0, p1 = cam_l.proj((0.0, -2.35, t)), cam_l.proj((0.0, 2.35, t))
    plane = f'<path class="ghost" d="M{fmt(p0[0])} {fmt(p0[1])}L{fmt(p1[0])} {fmt(p1[1])}"/>'
    write("tool-cleanup.svg", cleanup_svg())
    write("tool-link.svg", svg_doc(400, 280, render_items(cam_l, items) + plane,
                                   "Link: one rule applied to six identical holes"))


def cleanup_svg():
    """Flat diagram: a quad strip, one vertex welded onto its neighbour."""
    def grid(ox, weld):
        cols, rows, w, h = 4, 2, 34, 34
        pts = {(i, j): (ox + i * w + (j * 6), 70 + j * h) for i in range(cols + 1) for j in range(rows + 1)}
        if weld:
            pts[(2, 1)] = pts[(3, 1)]
        out = []
        for i in range(cols):
            for j in range(rows):
                q = [pts[(i, j)], pts[(i + 1, j)], pts[(i + 1, j + 1)], pts[(i, j + 1)]]
                tri = weld and (i, j) in ((2, 0), (2, 1))
                out.append(f'<path class="{'l11' if tri else 'l6'}" d="{path_d(q)}"/>')
        return out, pts
    a, pa = grid(28, False)
    b, pb = grid(222, True)
    src, dst = pa[(2, 1)], pa[(3, 1)]
    body = '<g class="q">' + "".join(a) + "".join(b) + "</g>"
    body += f'<circle class="dot" cx="{fmt(src[0])}" cy="{fmt(src[1])}" r="5"/>'
    body += f'<circle class="dotc" cx="{fmt(dst[0])}" cy="{fmt(dst[1])}" r="5"/>'
    body += f'<path class="mk" d="M{fmt(src[0] + 7)} {fmt(src[1] - 9)}Q{fmt((src[0] + dst[0]) / 2)} {fmt(src[1] - 30)} {fmt(dst[0] - 6)} {fmt(dst[1] - 10)}"/>'
    w = pb[(3, 1)]
    body += f'<circle class="dotc" cx="{fmt(w[0])}" cy="{fmt(w[1])}" r="5"/>'
    body += '<path class="ghost" d="M190 60V170"/>'
    return svg_doc(400, 230, body, "Target Weld merges one vertex into another; a quad becomes a triangle")


if __name__ == "__main__":
    build()
