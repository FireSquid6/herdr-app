#!/usr/bin/env bash
# Cached standalone rustc only. Never launches on the inherited desktop.
set -euo pipefail
umask 077
# Resolve the caller's normal-user HOME before env.sh or the private harness.
ORIGINAL_HOME=${HOME:?HOME must identify the normal-user home}
P="$ORIGINAL_HOME/.cache/herdr-browser-prototype"
EVIDENCE_ROOT="$P/clipboard-transport-proof"
# shellcheck source=/dev/null
source /home/firesquid/.local/share/herdr-fork-tools/env.sh
HERE=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
mkdir -p "$EVIDENCE_ROOT"
RUN="$EVIDENCE_ROOT/legacy-evidence-$(date +%s%N)"
mkdir -m700 "$RUN"
printf '%s\n' "$RUN"
rustc --edition=2024 "$HERE/legacy_probe.rs" \
  -L "dependency=$P/target-app-mvp/debug/deps" \
  -L "native=$P/runtime-156.0.26" \
  --extern "cef=$P/target-app-mvp/debug/deps/libcef-ad1108621cfb5a08.rlib" \
  --extern "serde_json=$P/target-app-mvp/debug/deps/libserde_json-9f3941d56b23a919.rlib" \
  -C debuginfo=0 -o "$RUN/legacy-read-proof" 2>"$RUN/build.log"
rustc --edition=2024 --test "$HERE/arguments.rs" -o "$RUN/arguments-tests"
"$RUN/arguments-tests" >"$RUN/arguments-tests.log"
export LD_LIBRARY_PATH="$P/execution-stage-156.0.26"
export HERDR_BROWSER_RUNTIME="$P/execution-stage-156.0.26"
export CHROME_DEVEL_SANDBOX=/opt/google/chrome/chrome-sandbox
export VK_DRIVER_FILES=/usr/share/vulkan/icd.d/lvp_icd.json
export NATIVE_WAYLAND_HEADLESS=1
python3 /home/firesquid/worktrees/herdr-forks/browser-tabs-and-clipboard/prototypes/native-wayland-browser/run-headless.py \
  --output "$RUN/run" --timeout 45 -- "$RUN/legacy-read-proof"
