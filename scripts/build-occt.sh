#!/usr/bin/env bash
# Build Open CASCADE Technology from source and install it to a prefix.
# CI copy of the OCCT recipe (Linux and macOS). Usage:
#   scripts/build-occt.sh <install-prefix>
# Env: OCCT_VERSION (default 8.0.1), OCCT_JOBS (default: all cores),
#      MACOSX_DEPLOYMENT_TARGET (macOS, default 11.0)
# Changing this file changes the CI cache key, so OCCT is rebuilt.
set -euo pipefail

prefix="${1:?usage: build-occt.sh <install-prefix>}"
version="${OCCT_VERSION:-8.0.1}"
jobs="${OCCT_JOBS:-$(getconf _NPROCESSORS_ONLN 2>/dev/null || sysctl -n hw.ncpu)}"
work="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/occt-build-$version"
mkdir -p "$work" "$prefix"
prefix="$(cd "$prefix" && pwd)"

if [ ! -d "$work/src" ]; then
  curl -fsSL --retry 3 -o "$work/occt.tar.gz" \
    "https://github.com/Open-Cascade-SAS/OCCT/archive/refs/tags/V$version.tar.gz"
  mkdir -p "$work/src"
  tar -xzf "$work/occt.tar.gz" -C "$work/src" --strip-components=1
fi

gen=()
command -v ninja >/dev/null && gen=(-G Ninja)
extra=()
if [ "$(uname -s)" = Darwin ]; then
  extra+=(-DCMAKE_OSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-11.0}")
fi

cmake -S "$work/src" -B "$work/build" "${gen[@]}" \
  -DCMAKE_BUILD_TYPE=Release \
  -DBUILD_LIBRARY_TYPE=Shared \
  -DINSTALL_DIR="$prefix" \
  -DINSTALL_DIR_LAYOUT=Unix \
  -DBUILD_MODULE_Draw=OFF \
  -DBUILD_MODULE_Visualization=OFF \
  -DUSE_TCL=OFF -DUSE_TK=OFF \
  -DUSE_FREETYPE=OFF \
  -DUSE_OPENGL=OFF -DUSE_GLES2=OFF \
  -DUSE_XLIB=OFF \
  -DBUILD_DOC_Overview=OFF \
  -DBUILD_GTEST=OFF \
  "${extra[@]}"
cmake --build "$work/build" --parallel "$jobs"
cmake --install "$work/build"
echo "OCCT $version installed to $prefix"
