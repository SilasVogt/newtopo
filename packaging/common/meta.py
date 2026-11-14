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
        "crate's Cargo.toml; full texts are in each crate's source on crates.io.",
        "",
    ]
    w = max((len(f"{n} {v}") for n, v, _, _ in rows), default=10)
    for n, v, lic, repo in rows:
        lines.append(f"{(n + ' ' + v).ljust(w)}  {lic}  {repo}".rstrip())
    Path(out_path).write_text("\n".join(lines) + "\n", encoding="utf-8")


def main(argv):
    if len(argv) >= 2 and argv[1] == "version":
        print(version())
    elif len(argv) in (3, 4) and argv[1] == "rust-notices":
        rust_notices(argv[2], argv[3] if len(argv) == 4 else None)
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main(sys.argv)
