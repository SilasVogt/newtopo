#!/usr/bin/env python3
"""Small helpers shared by the per-OS packaging scripts.

    meta.py version                 print the version of the `newtopo` package
    meta.py rust-notices OUT [TRIPLE]
                                    write the list of third-party Rust crates
                                    (name, version, licence, source) to OUT

Both read `cargo metadata` from the workspace this file lives in.
"""

import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BINARY_PACKAGES = ("newtopo", "newtopo-cli", "newtopo-import")


def cargo_metadata(extra):
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked", *extra],
        cwd=ROOT,
        check=True,
        stdout=subprocess.PIPE,
    ).stdout
    return json.loads(out)


def version():
    meta = cargo_metadata(["--no-deps"])
    for pkg in meta["packages"]:
        if pkg["name"] == "newtopo":
            return pkg["version"]
    sys.exit("package `newtopo` not found in cargo metadata")


def rust_notices(out_path, triple=None):
    extra = ["--filter-platform", triple] if triple else []
    meta = cargo_metadata(extra)
    pkgs = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    workspace = set(meta["workspace_members"])

    # Walk normal (non-dev, non-build) dependencies from the shipped binaries.
    roots = [p["id"] for p in meta["packages"] if p["name"] in BINARY_PACKAGES and p["id"] in workspace]
    seen, stack = set(), list(roots)
    while stack:
        pid = stack.pop()
        if pid in seen:
            continue
        seen.add(pid)
        for dep in nodes[pid]["deps"]:
            kinds = {k.get("kind") for k in dep["dep_kinds"]}
            if None in kinds:  # None == normal dependency
                stack.append(dep["pkg"])

    rows = []
    for pid in seen - workspace:
        p = pkgs[pid]
        rows.append((p["name"], p["version"], p.get("license") or p.get("license_file") or "see crate", p.get("repository") or ""))
    rows.sort(key=lambda r: (r[0].lower(), r[1]))

    lines = [
        "Third-party Rust crates statically linked into newtopo",
        "=" * 56,
        "",
        f"{len(rows)} crates. Licence expressions are the SPDX values from each",
        "crate's Cargo.toml. The licence, copyright and NOTICE files each crate",
        "ships follow the list, as their licences require.",
        "",
    ]
    w = max((len(f"{n} {v}") for n, v, _, _ in rows), default=10)
    for n, v, lic, repo in rows:
        lines.append(f"{(n + ' ' + v).ljust(w)}  {lic}  {repo}".rstrip())

    missing, printed = [], {}
    for pid in sorted(seen - workspace, key=lambda i: (pkgs[i]["name"].lower(), pkgs[i]["version"])):
        p = pkgs[pid]
        crate = f"{p['name']} {p['version']}"
        texts = licence_files(p)
        if not texts:
            missing.append(f"{crate}  ({p.get('license') or 'see crate'})")
            continue
        for name, text in texts:
            key = " ".join(text.split())
            lines += ["", "", "-" * 72, f"{crate}: {name}", "-" * 72, ""]
            if key in printed:
                lines.append(f"(Identical to the text printed above for {printed[key]}.)")
            else:
                printed[key] = f"{crate}: {name}"
                lines.append(text.rstrip())
    if missing:
        lines += [
            "",
            "",
            "-" * 72,
            "Crates without a licence file in their published source",
            "-" * 72,
            "",
            "These crates name their licence only in Cargo.toml. They are used under",
            "the licence listed for each, copyright their respective authors; the",
            "standard texts of those licences follow.",
            "",
            *missing,
            "",
            STANDARD_MIT,
        ]
        apache = next((t for k, t in printed.items() if "Apache License" in k and "Version 2.0" in k), None)
        if apache is None:
            lines.append("Apache-2.0: https://www.apache.org/licenses/LICENSE-2.0")
        else:
            lines.append(f"Apache-2.0: the full text is printed above ({apache}).")
    Path(out_path).write_text("\n".join(lines) + "\n", encoding="utf-8")


LICENCE_PREFIXES = ("license", "licence", "copying", "copyright", "notice", "unlicense")
LICENCE_DIRS = ("fonts", "licenses", "licences", "license", "licence")
STANDARD_MIT = """MIT licence (standard text):

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
"""


def licence_files(pkg):
    """(file name, text) for the licence/notice files at the crate's root,
    plus the Cargo.toml `license-file` if it lives elsewhere."""
    root = Path(pkg["manifest_path"]).parent
    found = {}

    def wanted(f, in_licence_dir):
        n = f.name.lower()
        if not f.is_file() or f.stat().st_size >= 512 * 1024:
            return False
        if n.startswith(LICENCE_PREFIXES) or "license" in n or "licence" in n:
            return True
        # Licence texts shipped next to bundled assets (e.g. fonts/OFL.txt).
        return in_licence_dir and n.endswith((".txt", ".md"))

    for f in sorted(root.iterdir()):
        if wanted(f, False):
            found[f.name] = f
        elif f.is_dir() and f.name.lower() in LICENCE_DIRS:
            for g in sorted(f.iterdir()):
                if wanted(g, True):
                    found[f"{f.name}/{g.name}"] = g
    lf = pkg.get("license_file")
    if lf:
        f = (root / lf).resolve()
        if f.is_file():
            found.setdefault(f.name, f)
    out = []
    for name, f in found.items():
        try:
            out.append((name, f.read_text(encoding="utf-8", errors="replace")))
        except OSError:
            pass
    return out


def main(argv):
    if len(argv) >= 2 and argv[1] == "version":
        print(version())
    elif len(argv) in (3, 4) and argv[1] == "rust-notices":
        rust_notices(argv[2], argv[3] if len(argv) == 4 else None)
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main(sys.argv)
