"""3ds Max add-on: smoothing-group assignment from hard edges. Plain
Python 3 (pymxs is stubbed; no 3ds Max needed):

    python3 tests/blender/test_max_smoothing.py

Max smooths across an edge when the two faces share any smoothing-group
bit, so the rule checked here is: faces across every smooth edge share a
bit, faces across every hard edge share none (unless reported unresolved).
"""

import os
import random
import sys
import types
import unittest

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
sys.dont_write_bytecode = True
sys.modules.setdefault("pymxs", types.ModuleType("pymxs"))
sys.modules["pymxs"].runtime = types.SimpleNamespace()
sys.modules["pymxs"].undo = None
sys.path.insert(0, os.path.join(ROOT, "addons", "max"))
from newtopo_bridge.build import smoothing_groups  # noqa: E402


def edges_of(sizes, verts):
    """{(a, b): [faces]} for undirected edges."""
    out, at = {}, 0
    for f, n in enumerate(sizes):
        for k in range(n):
            a, b = verts[at + k], verts[at + (k + 1) % n]
            out.setdefault((min(a, b), max(a, b)), []).append(f)
        at += n
    return out


def check(test, sizes, verts, hard_pairs, masks, unresolved):
    hard = {(min(a, b), max(a, b)) for a, b in zip(hard_pairs[0::2], hard_pairs[1::2])}
    unresolved = set(unresolved)
    for e, fs in edges_of(sizes, verts).items():
        for x in range(len(fs)):
            for y in range(x + 1, len(fs)):
                f, g = fs[x], fs[y]
                shared = masks[f] & masks[g]
                if e in hard:
                    test.assertEqual(shared, 0, "hard edge %s smoothed (faces %d, %d: %s)" % (e, f, g, masks))
                elif (min(f, g), max(f, g)) not in unresolved:
                    test.assertNotEqual(shared, 0, "smooth edge %s not smoothed (faces %d, %d: %s)" % (e, f, g, masks))
    for m in masks:
        test.assertLess(m, 1 << 32)


def cylinder(n, seams):
    """n quads around a tube; vertical edges at the indices in `seams` hard."""
    sizes, verts, hard = [], [], []
    for i in range(n):
        j = (i + 1) % n
        sizes.append(4)
        verts += [i, j, n + j, n + i]
    for i in seams:
        hard += [i, n + i]
    return sizes, verts, hard


class SmoothingGroupTests(unittest.TestCase):
    def test_cylinder_with_one_hard_seam(self):
        # The review case: 4 quads, seam between face 3 and face 0. The old
        # component-based code gave [1, 1, 1, 1] and lost the seam.
        sizes, verts, hard = cylinder(4, [0])
        masks, unresolved = smoothing_groups(sizes, verts, hard)
        self.assertEqual(unresolved, [])
        self.assertEqual(masks[3] & masks[0], 0, masks)
        self.assertNotEqual(masks, [1, 1, 1, 1])
        check(self, sizes, verts, hard, masks, unresolved)

    def test_cylinders_with_several_seams(self):
        for n, seams in [(12, [0]), (12, [0, 6]), (12, [0, 3, 6, 9]), (5, [0, 1, 2, 3, 4]), (3, [1])]:
            sizes, verts, hard = cylinder(n, seams)
            masks, unresolved = smoothing_groups(sizes, verts, hard)
            self.assertEqual(unresolved, [], (n, seams))
            check(self, sizes, verts, hard, masks, unresolved)

    def test_cube_all_hard_is_faceted(self):
        cube = [0, 3, 2, 1, 4, 5, 6, 7, 0, 1, 5, 4, 2, 3, 7, 6, 0, 4, 7, 3, 1, 2, 6, 5]
        hard = [a for e in [(0, 1), (1, 2), (2, 3), (3, 0), (4, 5), (5, 6), (6, 7), (7, 4), (0, 4), (1, 5), (2, 6), (3, 7)] for a in e]
        masks, unresolved = smoothing_groups([4] * 6, cube, hard)
        self.assertEqual((masks, unresolved), ([0] * 6, []))

    def test_random_grids(self):
        rng = random.Random(7)
        for case in range(200):
            nx, ny = rng.randint(1, 8), rng.randint(1, 8)
            vid = lambda i, j: j * (nx + 1) + i  # noqa: E731
            sizes, verts = [], []
            for j in range(ny):
                for i in range(nx):
                    sizes.append(4)
                    verts += [vid(i, j), vid(i + 1, j), vid(i + 1, j + 1), vid(i, j + 1)]
            edges = list(edges_of(sizes, verts))
            hard = [a for e in edges if rng.random() < 0.3 for a in e]
            masks, unresolved = smoothing_groups(sizes, verts, hard)
            self.assertEqual(unresolved, [], case)
            check(self, sizes, verts, hard, masks, unresolved)

    def test_impossible_cases_are_reported(self):
        # Too few bits: the seamed cylinder needs two.
        sizes, verts, hard = cylinder(4, [0])
        masks, unresolved = smoothing_groups(sizes, verts, hard, max_bits=1)
        self.assertTrue(unresolved)
        check(self, sizes, verts, hard, masks, unresolved)
        # Two faces sharing a hard edge and smooth edges: the hard edge wins
        # and the pair is reported.
        sizes, verts = [4, 6], [0, 1, 2, 3, 3, 2, 5, 4, 1, 0]
        masks, unresolved = smoothing_groups(sizes, verts, [0, 1])
        self.assertEqual(unresolved, [(0, 1)])
        self.assertEqual(masks[0] & masks[1], 0)


if __name__ == "__main__":
    unittest.main(verbosity=2)
