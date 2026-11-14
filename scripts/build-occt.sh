#!/usr/bin/env bash
# Downloads Open CASCADE 8.0.1 (GitHub tag V8.0.1) and builds/installs it into
# a prefix with the configuration newtopo uses everywhere (SPEC §3.4):
# Release, shared libraries, no Draw, no Visualization module, no Tcl/Tk,
# FreeType, OpenGL, GLES2 or Xlib. Linux and macOS.
#
# Usage: scripts/build-occt.sh <install-prefix> [work-dir]
#   OCCT_ARCHIVE=<file.tar.gz>  use a local source archive instead of downloading
#   OCCT_CONFIGURE_ONLY=1       stop after the CMake configure step (testing)
#   OCCT_FORCE=1                rebuild even if the prefix already has 8.0.1
#   JOBS=<n>                    parallel build jobs (default: all cores)
#
# Then build newtopo with: OCCT_DIR=<install-prefix> cargo build
set -euo pipefail

VERSION=8.0.1
TAG=V${VERSION}
URL="https://github.com/Open-Cascade-SAS/OCCT/archive/refs/tags/${TAG}.tar.gz"

if [[ $# -lt 1 ]]; then
  echo "usage: $0 <install-prefix> [work-dir]" >&2
  exit 2
fi
mkdir -p "$1"
PREFIX=$(cd "$1" && pwd)
WORK=${2:-"$PREFIX/../occt-build-${VERSION}"}
mkdir -p "$WORK"
WORK=$(cd "$WORK" && pwd)

if [[ -z "${OCCT_FORCE:-}" ]] && grep -qs "OCC_VERSION_COMPLETE \"${VERSION}\"" "$PREFIX/include/opencascade/Standard_Version.hxx"; then
  echo "OCCT ${VERSION} already installed in $PREFIX (set OCCT_FORCE=1 to rebuild)"
  exit 0
fi

for tool in cmake tar; do
  command -v "$tool" >/dev/null || { echo "error: $tool is required" >&2; exit 1; }
done

ARCHIVE=${OCCT_ARCHIVE:-"$WORK/occt-${VERSION}.tar.gz"}
if [[ ! -f "$ARCHIVE" ]]; then
  echo "downloading $URL"
  curl -fL --retry 3 -o "$ARCHIVE.part" "$URL"
  mv "$ARCHIVE.part" "$ARCHIVE"
fi

SRC="$WORK/OCCT-${VERSION}"
if [[ ! -f "$SRC/CMakeLists.txt" ]]; then
  rm -rf "$SRC"
  tar -xzf "$ARCHIVE" -C "$WORK"
fi
[[ -f "$SRC/CMakeLists.txt" ]] || { echo "error: $SRC/CMakeLists.txt missing after extraction" >&2; exit 1; }

GEN=()
if command -v ninja >/dev/null; then GEN=(-G Ninja); fi

BUILD="$WORK/build"
cmake -S "$SRC" -B "$BUILD" "${GEN[@]}" \
  -DCMAKE_BUILD_TYPE=Release \
  -DINSTALL_DIR="$PREFIX" \
  -DCMAKE_INSTALL_PREFIX="$PREFIX" \
  -DBUILD_LIBRARY_TYPE=Shared \
  -DBUILD_MODULE_Draw=OFF \
  -DBUILD_MODULE_Visualization=OFF \
  -DBUILD_MODULE_DETools=OFF \
  -DBUILD_DOC_Overview=OFF \
  -DBUILD_SAMPLES_QT=OFF \
  -DBUILD_GTEST=OFF \
  -DUSE_TCL=OFF \
  -DUSE_TK=OFF \
  -DUSE_FREETYPE=OFF \
  -DUSE_OPENGL=OFF \
  -DUSE_GLES2=OFF \
  -DUSE_XLIB=OFF

if [[ -n "${OCCT_CONFIGURE_ONLY:-}" ]]; then
  echo "configured in $BUILD (OCCT_CONFIGURE_ONLY set; not building)"
  exit 0
fi

if [[ -z "${JOBS:-}" ]]; then
  JOBS=$(getconf _NPROCESSORS_ONLN 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 4)
fi
cmake --build "$BUILD" --config Release --parallel "$JOBS"
cmake --install "$BUILD" --config Release
echo "OCCT ${VERSION} installed in $PREFIX"
echo "build newtopo with: OCCT_DIR=$PREFIX cargo build"
