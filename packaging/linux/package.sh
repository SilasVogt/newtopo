#!/usr/bin/env bash
# Build Linux x86-64 packages of newtopo: a relocatable .tar.gz, an AppImage
# and a .deb. Works locally and in CI (.github/workflows/release.yml).
#
# Usage: packaging/linux/package.sh [options]
#   --skip-build          use binaries already in the cargo target dir
#   --bin-dir DIR         take the executables from DIR (implies --skip-build)
#   --formats LIST        comma list of tar,appimage,deb (default: all three)
#   --out DIR             output directory (default: <repo>/dist)
#
# Environment:
#   OCCT_DIR              OCCT install prefix; every shared library the
#                         executables need (recursively) that lives under it is
#                         copied into lib/. Required when they link OCCT.
#   NEWTOPO_BUNDLE_LIB_DIRS  extra colon-separated dirs to bundle libraries from
#   NEWTOPO_VERSION       override the version (default: cargo metadata)
#   CARGO_TARGET_DIR      cargo target dir (default: <repo>/target)
#   APPIMAGETOOL          path to appimagetool (default: download a pinned one)
#
# Needs: cargo, python3, readelf, patchelf, ldd, tar; dpkg-deb for .deb.
set -euo pipefail
umask 022

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
pkgdir="$root/packaging"

build=1 bin_dir="" formats="tar,appimage,deb" out="$root/dist"
while [ $# -gt 0 ]; do
  case "$1" in
    --skip-build) build=0 ;;
    --bin-dir) bin_dir="$2"; build=0; shift ;;
    --formats) formats="$2"; shift ;;
    --out) out="$2"; shift ;;
    -h|--help) sed -n '2,22p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done
want() { [[ ",$formats," == *",$1,"* ]]; }
log() { printf '\033[1m==> %s\033[0m\n' "$*"; }
die() { echo "error: $*" >&2; exit 1; }

for tool in python3 readelf patchelf ldd tar; do
  command -v "$tool" >/dev/null || die "$tool not found (install it; patchelf: apt install patchelf)"
done

version="${NEWTOPO_VERSION:-$(python3 -I "$pkgdir/common/meta.py" version)}"
arch=x86_64
name="newtopo-$version-linux-$arch"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
work="$out/.work-linux"
rm -rf "$work"
mkdir -p "$work"

# ---------------------------------------------------------------- build
if [ "$build" = 1 ]; then
  log "cargo build --workspace --release"
  (cd "$root" && cargo build --workspace --release --locked)
fi
bin_dir="${bin_dir:-${CARGO_TARGET_DIR:-$root/target}/release}"
bins=()
for b in newtopo newtopo-cli newtopo-import; do
  if [ -f "$bin_dir/$b" ]; then bins+=("$b"); else echo "warning: $bin_dir/$b missing, not packaged" >&2; fi
done
[ -f "$bin_dir/newtopo" ] || die "$bin_dir/newtopo not found"

# ---------------------------------------------------------------- payload
# One relocatable tree, reused by all three formats:
#   bin/  newtopo newtopo-cli newtopo-import     (RUNPATH $ORIGIN/../lib)
#   lib/  bundled OCCT .so files                 (RUNPATH $ORIGIN)
#   share/{applications,icons,mime,doc/newtopo}
payload="$work/payload"
mkdir -p "$payload/bin" "$payload/lib"
for b in "${bins[@]}"; do install -m 0755 "$bin_dir/$b" "$payload/bin/$b"; done

lib_dirs=()
if [ -n "${OCCT_DIR:-}" ]; then
  for d in "$OCCT_DIR/lib" "$OCCT_DIR/lib64" "$OCCT_DIR/lib/x86_64-linux-gnu"; do
    [ -d "$d" ] && lib_dirs+=("$(cd "$d" && pwd -P)")
  done
fi
IFS=':' read -r -a extra_dirs <<< "${NEWTOPO_BUNDLE_LIB_DIRS:-}"
for d in "${extra_dirs[@]}"; do [ -n "$d" ] && [ -d "$d" ] && lib_dirs+=("$(cd "$d" && pwd -P)"); done

needed() { readelf -d "$1" 2>/dev/null | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p'; }

# Walk DT_NEEDED recursively. A soname found in one of $lib_dirs is copied
# into lib/ under its soname; anything else is left to the system.
log "bundling shared libraries from: ${lib_dirs[*]:-(none; OCCT_DIR unset)}"
queue=()
for b in "${bins[@]}"; do queue+=("$payload/bin/$b"); done
bundled=()
while [ ${#queue[@]} -gt 0 ]; do
  f="${queue[0]}"; queue=("${queue[@]:1}")
  while read -r so; do
    [ -n "$so" ] || continue
    [ -e "$payload/lib/$so" ] && continue
    for d in "${lib_dirs[@]}"; do
      if [ -e "$d/$so" ]; then
        install -m 0644 "$(readlink -f "$d/$so")" "$payload/lib/$so"
        bundled+=("$so"); queue+=("$payload/lib/$so")
        break
      fi
    done
  done < <(needed "$f")
done
echo "bundled ${#bundled[@]} libraries"

for b in "${bins[@]}"; do patchelf --set-rpath '$ORIGIN/../lib' "$payload/bin/$b"; done
for so in "${bundled[@]}"; do patchelf --set-rpath '$ORIGIN' "$payload/lib/$so"; done

# Check every packaged ELF resolves all its libraries, and none from outside
# the package that we meant to bundle.
log "checking library resolution (ldd)"
fail=0
for f in "$payload"/bin/* "$payload"/lib/*; do
  [ -f "$f" ] || continue
  res="$(env -u LD_LIBRARY_PATH ldd "$f" 2>&1 || true)"
  if grep -q 'not found' <<<"$res"; then echo "$f: unresolved:"; grep 'not found' <<<"$res"; fail=1; fi
  for d in "${lib_dirs[@]}"; do
    if grep -qF "=> $d/" <<<"$res"; then echo "$f: still loads from $d"; fail=1; fi
  done
done
[ "$fail" = 0 ] || die "library check failed"

# Desktop integration and docs.
share="$payload/share"
install -Dm0644 "$here/newtopo.desktop" "$share/applications/newtopo.desktop"
install -Dm0644 "$here/newtopo-mime.xml" "$share/mime/packages/newtopo.xml"
for s in 16 24 32 48 64 128 256 512; do
  install -Dm0644 "$pkgdir/icons/png/newtopo-$s.png" "$share/icons/hicolor/${s}x$s/apps/newtopo.png"
  install -Dm0644 "$pkgdir/icons/png/newtopo-$s.png" "$share/icons/hicolor/${s}x$s/mimetypes/application-x-newtopo-scene.png"
done
install -Dm0644 "$root/assets/icon.svg" "$share/icons/hicolor/scalable/apps/newtopo.svg"
doc="$share/doc/newtopo"
install -Dm0644 "$pkgdir/common/THIRD-PARTY-NOTICES.txt" "$doc/THIRD-PARTY-NOTICES.txt"
python3 -I "$pkgdir/common/meta.py" rust-notices "$doc/THIRD-PARTY-RUST.txt" x86_64-unknown-linux-gnu
occt_lic=""
for d in "${OCCT_DIR:-/nonexistent}/share/doc/opencascade" "$pkgdir/common/licenses"; do  # prefer the texts that came with the shipped OCCT
  if [ -f "$d/LICENSE_LGPL_21.txt" ]; then occt_lic="$d"; break; fi
done
[ -n "$occt_lic" ] || die "OCCT licence texts not found (set OCCT_DIR to an OCCT install)"
install -Dm0644 "$occt_lic/LICENSE_LGPL_21.txt" "$doc/licenses/LICENSE_LGPL_21.txt"
install -Dm0644 "$occt_lic/OCCT_LGPL_EXCEPTION.txt" "$doc/licenses/OCCT_LGPL_EXCEPTION.txt"

# ---------------------------------------------------------------- tar.gz
if want tar; then
  log "tar.gz"
  t="$work/tar/$name"
  mkdir -p "$t"
  cp -a "$payload/." "$t/"
  cat > "$t/README.txt" <<EOF
newtopo $version for Linux x86-64

Run bin/newtopo (app) or bin/newtopo-cli (command line). Keep the folder
together: the executables find their libraries in lib/ and each other in bin/.

Optional desktop integration for the current user:
  mkdir -p ~/.local/share/applications ~/.local/share/mime/packages
  sed "s|^Exec=newtopo|Exec=\$PWD/bin/newtopo|" share/applications/newtopo.desktop > ~/.local/share/applications/newtopo.desktop
  cp share/mime/packages/newtopo.xml ~/.local/share/mime/packages/ && update-mime-database ~/.local/share/mime
  cp -r share/icons ~/.local/share/

Licences: share/doc/newtopo/THIRD-PARTY-NOTICES.txt
EOF
  tar -C "$work/tar" --owner=0 --group=0 --numeric-owner -czf "$out/$name.tar.gz" "$name"
fi

# ---------------------------------------------------------------- AppImage
if want appimage; then
  log "AppImage"
  appdir="$work/newtopo.AppDir"
  mkdir -p "$appdir/usr"
  cp -a "$payload/." "$appdir/usr/"
  install -m0755 "$here/AppRun" "$appdir/AppRun"
  sed "/^Version=/a X-AppImage-Version=$version" "$here/newtopo.desktop" > "$appdir/newtopo.desktop"
  install -m0644 "$pkgdir/icons/png/newtopo-256.png" "$appdir/newtopo.png"
  ln -s newtopo.png "$appdir/.DirIcon"

  tools="${XDG_CACHE_HOME:-$HOME/.cache}/newtopo-packaging"
  mkdir -p "$tools"
  fetch() { # url sha256 dest
    if [ ! -f "$3" ] || ! echo "$2  $3" | sha256sum -c --status; then
      curl -fsSL --retry 3 -o "$3.part" "$1"
      echo "$2  $3.part" | sha256sum -c --status || die "checksum mismatch for $1"
      mv "$3.part" "$3"
    fi
  }
  tool="${APPIMAGETOOL:-}"
  if [ -z "$tool" ]; then
    tool="$tools/appimagetool-1.9.1-x86_64.AppImage"
    fetch https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage \
      ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0 "$tool"
    chmod +x "$tool"
  fi
  runtime="$tools/type2-runtime-20251108-x86_64"
  fetch https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64 \
    2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d "$runtime"
  # Extract-and-run so appimagetool works without FUSE (CI, containers).
  ARCH=$arch APPIMAGE_EXTRACT_AND_RUN=1 VERSION="$version" \
    "$tool" --no-appstream --runtime-file "$runtime" "$appdir" "$out/$name.AppImage"
  chmod +x "$out/$name.AppImage"
fi

# ---------------------------------------------------------------- .deb
if want deb; then
  log ".deb"
  command -v dpkg-deb >/dev/null || die "dpkg-deb not found (needed for the .deb)"
  deb="$work/deb"
  mkdir -p "$deb/opt/newtopo" "$deb/usr/bin" "$deb/DEBIAN"
  cp -a "$payload/bin" "$payload/lib" "$deb/opt/newtopo/"
  ln -s ../../opt/newtopo/bin/newtopo "$deb/usr/bin/newtopo"
  [ -f "$payload/bin/newtopo-cli" ] && ln -s ../../opt/newtopo/bin/newtopo-cli "$deb/usr/bin/newtopo-cli"
  mkdir -p "$deb/usr/share"
  cp -a "$share/applications" "$share/icons" "$share/mime" "$deb/usr/share/"
  mkdir -p "$deb/usr/share/doc/newtopo"
  cp -a "$doc/." "$deb/usr/share/doc/newtopo/"
  cat > "$deb/usr/share/doc/newtopo/copyright" <<'EOF'
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: newtopo
Source: https://github.com/SilasVogt/newtopo

Files: *
Copyright: Silas Vogt
License: proprietary
 All rights reserved.

Files: opt/newtopo/lib/libTK*
Copyright: Open CASCADE SAS
License: LGPL-2.1 with Open CASCADE Exception 1.0
 See /usr/share/doc/newtopo/licenses/LICENSE_LGPL_21.txt and
 /usr/share/doc/newtopo/licenses/OCCT_LGPL_EXCEPTION.txt.

Files: opt/newtopo/bin/*
Comment: Statically linked Rust crates are listed with their licences in
 /usr/share/doc/newtopo/THIRD-PARTY-RUST.txt.
EOF

  # Runtime dependencies of the executables and bundled libraries. Prefer
  # dpkg-shlibdeps (Debian/Ubuntu); fall back to a mapping of common sonames.
  depends=""
  if command -v dpkg-shlibdeps >/dev/null; then
    sd="$work/shlibdeps"
    mkdir -p "$sd/debian"
    printf 'Source: newtopo\n\nPackage: newtopo\nArchitecture: amd64\n' > "$sd/debian/control"
    elfs=()
    for f in "$deb"/opt/newtopo/bin/* "$deb"/opt/newtopo/lib/*; do [ -f "$f" ] && elfs+=("$f"); done
    if out_sd="$(cd "$sd" && dpkg-shlibdeps -O --ignore-missing-info -l"$deb/opt/newtopo/lib" "${elfs[@]}" 2>"$sd/err")"; then
      depends="$(sed -n 's/^shlibs:Depends=//p' <<<"$out_sd")"
    else
      cat "$sd/err" >&2
    fi
  fi
  if [ -z "$depends" ]; then
    declare -A map=([libc.so.6]=libc6 [libm.so.6]=libc6 [libstdc++.so.6]=libstdc++6 [libgcc_s.so.1]=libgcc-s1
                    [libgtk-3.so.0]=libgtk-3-0 [libdbus-1.so.3]=libdbus-1-3 [libz.so.1]=zlib1g)
    declare -A seen=()
    for f in "$deb"/opt/newtopo/bin/* "$deb"/opt/newtopo/lib/*; do
      [ -f "$f" ] || continue
      while read -r so; do [ -n "${map[$so]:-}" ] && seen[${map[$so]}]=1; done < <(needed "$f")
    done
    depends="$(printf '%s\n' "${!seen[@]}" | sort | paste -sd, | sed 's/,/, /g')"
  fi
  debver="${version//-/\~}"
  size="$(du -sk --exclude=DEBIAN "$deb" | cut -f1)"
  cat > "$deb/DEBIAN/control" <<EOF
Package: newtopo
Version: $debver
Architecture: amd64
Maintainer: Silas Vogt <SilasVogt@users.noreply.github.com>
Installed-Size: $size
Depends: $depends
Recommends: libvulkan1, mesa-vulkan-drivers | vulkan-icd, libxkbcommon0, libwayland-client0, libx11-6, libxcursor1, libxi6, libxrandr2, xdg-desktop-portal
Section: graphics
Priority: optional
Homepage: https://github.com/SilasVogt/newtopo
Description: Turn CAD solids into clean, game-ready polygon meshes
 newtopo imports STEP and IGES CAD models and builds clean quad-dominant
 meshes from them for games and real-time use. Installs to /opt/newtopo
 with newtopo and newtopo-cli in /usr/bin.
EOF
  find "$deb" -type d -exec chmod 0755 {} +
  dpkg-deb --root-owner-group -Zxz --build "$deb" "$out/${name/linux-$arch/linux-amd64}.deb" >/dev/null
fi

rm -rf "$work"
log "done"
ls -l "$out" | sed -n '2,$p'
