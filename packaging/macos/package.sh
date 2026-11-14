#!/usr/bin/env bash
# Build the macOS app bundle (newtopo.app) and a .dmg with an Applications
# link. Ad-hoc signed, not notarised. Works locally and in CI
# (.github/workflows/release.yml). Written for macOS's bash 3.2.
#
# Usage: packaging/macos/package.sh [options]
#   --skip-build          use binaries already in the cargo target dir
#   --bin-dir DIR         take the executables from DIR (implies --skip-build)
#   --out DIR             output directory (default: <repo>/dist)
#
# Environment:
#   OCCT_DIR              OCCT install prefix; every dylib the executables need
#                         (recursively) that lives under it is copied into
#                         Contents/Frameworks and relinked via @rpath.
#   NEWTOPO_BUNDLE_LIB_DIRS  extra colon-separated dirs to bundle dylibs from
#   NEWTOPO_VERSION       override the version (default: cargo metadata)
#   MACOSX_DEPLOYMENT_TARGET  minimum macOS (default 11.0)
#   CARGO_TARGET_DIR      cargo target dir (default: <repo>/target)
#
# Needs: cargo, python3, otool, install_name_tool, codesign, hdiutil.
set -euo pipefail
umask 022

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
pkgdir="$root/packaging"

build=1 bin_dir="" out="$root/dist"
while [ $# -gt 0 ]; do
  case "$1" in
    --skip-build) build=0 ;;
    --bin-dir) bin_dir="$2"; build=0; shift ;;
    --out) out="$2"; shift ;;
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done
log() { printf '\033[1m==> %s\033[0m\n' "$*"; }
die() { echo "error: $*" >&2; exit 1; }

export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-11.0}"
version="${NEWTOPO_VERSION:-$(python3 -I "$pkgdir/common/meta.py" version)}"
arch="$(uname -m)"   # arm64 or x86_64
case "$arch" in arm64) triple=aarch64-apple-darwin ;; x86_64) triple=x86_64-apple-darwin ;; *) die "unsupported arch $arch" ;; esac
name="newtopo-$version-macos-$arch"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
work="$out/.work-macos"
rm -rf "$work"
mkdir -p "$work"

if [ "$build" = 1 ]; then
  log "cargo build --workspace --release"
  (cd "$root" && cargo build --workspace --release --locked)
fi
bin_dir="${bin_dir:-${CARGO_TARGET_DIR:-$root/target}/release}"
[ -f "$bin_dir/newtopo" ] || die "$bin_dir/newtopo not found"

# ---------------------------------------------------------------- bundle
app="$work/newtopo.app"
C="$app/Contents"
mkdir -p "$C/MacOS" "$C/Frameworks" "$C/Resources/licenses"
bins=""
for b in newtopo newtopo-cli newtopo-import; do
  if [ -f "$bin_dir/$b" ]; then
    install -m 0755 "$bin_dir/$b" "$C/MacOS/$b"; bins="$bins $b"
  else
    echo "warning: $bin_dir/$b missing, not packaged" >&2
  fi
done
sed -e "s/@VERSION@/$version/g" -e "s/@MIN_MACOS@/$MACOSX_DEPLOYMENT_TARGET/g" "$here/Info.plist" > "$C/Info.plist"
plutil -lint "$C/Info.plist" >/dev/null
printf 'APPL????' > "$C/PkgInfo"
cp "$pkgdir/icons/newtopo.icns" "$C/Resources/newtopo.icns"

lib_dirs=""
if [ -n "${OCCT_DIR:-}" ] && [ -d "$OCCT_DIR/lib" ]; then lib_dirs="$(cd "$OCCT_DIR/lib" && pwd -P)"; fi
old_ifs="$IFS"; IFS=':'
for d in ${NEWTOPO_BUNDLE_LIB_DIRS:-}; do
  if [ -d "$d" ]; then lib_dirs="$lib_dirs:$(cd "$d" && pwd -P)"; fi
done
IFS="$old_ifs"
lib_dirs="${lib_dirs#:}"

# find_lib NAME -> path of NAME in one of $lib_dirs, or nothing
find_lib() {
  local IFS=':' d
  for d in $lib_dirs; do
    if [ -e "$d/$1" ]; then echo "$d/$1"; return 0; fi
  done
  return 0
}
in_lib_dirs() {
  local IFS=':' d p
  p="$(cd "$(dirname "$1")" 2>/dev/null && pwd -P)/$(basename "$1")"
  for d in $lib_dirs; do case "$p" in "$d"/*) return 0 ;; esac; done
  return 1
}
# deps FILE -> install names FILE loads (excluding its own id)
deps() {
  local self
  self="$(otool -D "$1" | tail -n +2)"
  otool -L "$1" | tail -n +2 | awk '{print $1}' | while read -r ref; do
    [ "$ref" = "$self" ] || echo "$ref"
  done
}
rpaths() { otool -l "$1" | awk '/cmd LC_RPATH/{f=1} f&&/ path /{print $2; f=0}'; }

log "bundling dylibs from: ${lib_dirs:-(none; OCCT_DIR unset)}"
queue=""
for b in $bins; do queue="$queue $C/MacOS/$b"; done
nbundled=0
while [ -n "${queue// /}" ]; do
  set -- $queue; f="$1"; shift; queue="$*"
  chmod u+w "$f"
  for ref in $(deps "$f"); do
    case "$ref" in
      /usr/lib/*|/System/*) continue ;;
      @rpath/*|@loader_path/*|@executable_path/*) src="$(find_lib "$(basename "$ref")")" ;;
      /*) if in_lib_dirs "$ref"; then src="$ref"
          else
            case "$ref" in /opt/homebrew/*|/usr/local/*|/opt/local/*)
              die "$f links $ref, which is outside the system and outside OCCT_DIR; add its dir to NEWTOPO_BUNDLE_LIB_DIRS" ;;
            esac
            src=""
          fi ;;
      *) src="" ;;
    esac
    if [ -z "$src" ]; then
      case "$ref" in @*) die "$f needs $ref but it was not found in: $lib_dirs" ;; esac
      continue
    fi
    base="$(basename "$ref")"
    dst="$C/Frameworks/$base"
    if [ ! -e "$dst" ]; then
      cp "$(python3 -I -c 'import os,sys;print(os.path.realpath(sys.argv[1]))' "$src")" "$dst"
      chmod 0644 "$dst"; chmod u+w "$dst"
      install_name_tool -id "@rpath/$base" "$dst" 2>/dev/null
      queue="$queue $dst"; nbundled=$((nbundled + 1))
    fi
    if [ "$ref" != "@rpath/$base" ]; then install_name_tool -change "$ref" "@rpath/$base" "$f" 2>/dev/null; fi
  done
done
echo "bundled $nbundled dylibs"

# rpaths: executables search Contents/Frameworks; dylibs search their own dir.
# Absolute rpaths (e.g. into OCCT_DIR from the build) are removed.
fix_rpaths() { # file wanted
  for rp in $(rpaths "$1"); do
    case "$rp" in /*) install_name_tool -delete_rpath "$rp" "$1" 2>/dev/null ;; esac
  done
  if ! rpaths "$1" | grep -qxF "$2"; then install_name_tool -add_rpath "$2" "$1" 2>/dev/null; fi
  return 0
}
for b in $bins; do fix_rpaths "$C/MacOS/$b" "@executable_path/../Frameworks"; done
for l in "$C"/Frameworks/*.dylib; do
  if [ -e "$l" ]; then fix_rpaths "$l" "@loader_path"; fi
done

log "checking load commands (otool -L)"
fail=0
for f in "$C"/MacOS/* "$C"/Frameworks/*.dylib; do
  [ -e "$f" ] || continue
  for ref in $(deps "$f"); do
    case "$ref" in
      /usr/lib/*|/System/*) ;;
      @rpath/*) [ -e "$C/Frameworks/${ref#@rpath/}" ] || { echo "$f: $ref not in Frameworks"; fail=1; } ;;
      *) echo "$f: unexpected reference $ref"; fail=1 ;;
    esac
  done
done
[ "$fail" = 0 ] || die "dylib check failed"
for b in $bins; do echo "--- otool -L $b"; otool -L "$C/MacOS/$b" | tail -n +2; done

# Licences.
install -m 0644 "$pkgdir/common/THIRD-PARTY-NOTICES.txt" "$C/Resources/THIRD-PARTY-NOTICES.txt"
python3 -I "$pkgdir/common/meta.py" rust-notices "$C/Resources/THIRD-PARTY-RUST.txt" "$triple"
lic="$pkgdir/common/licenses"
if [ -n "${OCCT_DIR:-}" ] && [ -f "$OCCT_DIR/share/doc/opencascade/LICENSE_LGPL_21.txt" ]; then
  lic="$OCCT_DIR/share/doc/opencascade"
fi
install -m 0644 "$lic/LICENSE_LGPL_21.txt" "$lic/OCCT_LGPL_EXCEPTION.txt" "$C/Resources/licenses/"
# newtopo's own licence (MIT), from the repo root.
if [ -f "$root/LICENSE" ]; then
  install -m 0644 "$root/LICENSE" "$C/Resources/LICENSE"
else
  echo "warning: $root/LICENSE missing; bundle has no newtopo licence file" >&2
fi

# ---------------------------------------------------------------- sign
# install_name_tool invalidates signatures; Apple silicon refuses to run
# unsigned code, so re-sign ad hoc: libraries, helper executables, bundle.
log "ad-hoc codesign"
for l in "$C"/Frameworks/*.dylib; do
  if [ -e "$l" ]; then codesign --force --sign - --timestamp=none "$l"; fi
done
for b in $bins; do
  if [ "$b" != newtopo ]; then codesign --force --sign - --timestamp=none "$C/MacOS/$b"; fi
done
codesign --force --deep --sign - --timestamp=none "$app"
codesign --verify --deep --strict --verbose=2 "$app"

# Smoke test: dyld must resolve every library of the non-GUI executables.
for b in newtopo-cli newtopo-import; do
  [ -x "$C/MacOS/$b" ] || continue
  if "$C/MacOS/$b" --help >"$work/smoke.log" 2>&1; then :; fi
  if grep -q 'Library not loaded\|dyld' "$work/smoke.log"; then cat "$work/smoke.log"; die "$b failed to load its libraries"; fi
done

# ---------------------------------------------------------------- dmg
log "dmg"
stage="$work/dmg"
mkdir -p "$stage"
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
cat > "$stage/Read me first.txt" <<EOF
newtopo $version for macOS ($arch)

Install: drag newtopo onto Applications.

This build is not notarised by Apple. The first time you open it, macOS
says it cannot check the app for malicious software. To open it anyway:
  - Control-click (right-click) newtopo in Applications, choose Open, then
    Open again; or
  - System Settings > Privacy & Security > "Open Anyway"; or
  - in Terminal: xattr -dr com.apple.quarantine /Applications/newtopo.app

Command line: /Applications/newtopo.app/Contents/MacOS/newtopo-cli
EOF
dmg="$out/$name.dmg"
rm -f "$dmg"
for i in 1 2 3 4 5; do
  if hdiutil create -volname "newtopo $version" -srcfolder "$stage" -ov -format UDZO -fs HFS+ "$dmg"; then break; fi
  [ "$i" = 5 ] && die "hdiutil failed"
  echo "hdiutil failed, retrying ($i)"; sleep 5
done
hdiutil verify "$dmg"

rm -rf "$work"
log "done"
ls -l "$out"
