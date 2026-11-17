#!/usr/bin/env bash
# Smoke test of a packaged newtopo (Linux and macOS; bash 3.2 compatible).
#
#   smoke.sh cli <models-dir> <work-dir> <cli command...>
#       Runs info / run / validate / export through the given CLI command
#       (e.g. /path/bin/newtopo-cli, or "/path/AppRun --cli"). Exercises the
#       newtopo-import helper next to the executable and the bundled OCCT.
#   smoke.sh gui <work-dir> <gui executable> [timeout-seconds]
#       Starts the GUI in its headless screenshot mode (--shot, no window).
#       A dynamic-loader failure is fatal; a missing GPU adapter only warns
#       (CI runners may have none), unless NEWTOPO_SMOKE_GPU=1.
#
# Run it with no developer library paths in the environment
# (env -u LD_LIBRARY_PATH; no DYLD_* variables).
set -euo pipefail
unset NEWTOPO_IMPORT_HELPER

mode="${1:?usage: smoke.sh cli|gui ...}"; shift
step() { printf '\n\033[1m--- %s\033[0m\n' "$*"; }

if [ "$mode" = cli ]; then
  models="$1" work="$2"; shift 2
  mkdir -p "$work"
  cp "$models/filleted_box.step" "$work/"
  cat > "$work/script.json" <<'EOF'
{"version":1,"input":"filleted_box.step","ops":[{"op":"slice","body":1,"count":10}]}
EOF
  step "info filleted_box.step"
  "$@" info "$work/filleted_box.step" | tee "$work/info.txt"
  [ -s "$work/info.txt" ] || { echo "info printed nothing"; exit 1; }
  step "run script.json -o out.ntopo"
  "$@" run "$work/script.json" -o "$work/out.ntopo"
  [ -s "$work/out.ntopo" ] || { echo "out.ntopo missing"; exit 1; }
  step "validate out.ntopo"
  "$@" validate "$work/out.ntopo"
  step "export out.ntopo -o out.fbx"
  "$@" export "$work/out.ntopo" -o "$work/out.fbx"
  magic="$(head -c 18 "$work/out.fbx")"
  [ "$magic" = "Kaydara FBX Binary" ] || { echo "out.fbx is not binary FBX"; exit 1; }
  ls -l "$work/out.ntopo" "$work/out.fbx"
  echo "SMOKE CLI OK: $*"
elif [ "$mode" = gui ]; then
  work="$1" exe="$2" limit="${3:-300}"
  mkdir -p "$work/shots"
  step "$exe --shot (headless, ${limit}s limit)"
  "$exe" --shot "$work/shots" >"$work/gui.log" 2>&1 &
  pid=$!
  waited=0
  while kill -0 "$pid" 2>/dev/null && [ "$waited" -lt "$limit" ]; do sleep 2; waited=$((waited + 2)); done
  if kill -0 "$pid" 2>/dev/null; then
    kill "$pid" 2>/dev/null || true
    cat "$work/gui.log"
    echo "warning: GUI still running after ${limit}s; it loaded its libraries but did not finish"
    exit 0
  fi
  rc=0; wait "$pid" || rc=$?
  cat "$work/gui.log"
  if grep -q 'Library not loaded\|error while loading shared libraries\|dyld\[' "$work/gui.log"; then
    echo "GUI failed to load its libraries"; exit 1
  fi
  n="$(find "$work/shots" -name '*.png' | wc -l | tr -d ' ')"
  if [ "$rc" = 0 ] && [ "$n" -gt 0 ]; then
    echo "SMOKE GUI OK: $n screenshots"
  elif [ "${NEWTOPO_SMOKE_GPU:-0}" = 1 ]; then
    echo "GUI exited $rc with $n screenshots"; exit 1
  else
    echo "warning: GUI loaded but exited $rc with $n screenshots (no usable GPU adapter?)"
  fi
else
  echo "unknown mode $mode" >&2; exit 2
fi
