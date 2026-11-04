#!/usr/bin/env bash
set -euo pipefail
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
cd "$root"
[[ $# -le 1 ]] || exit 64
out="${1:-out-boundary}"
[[ ! -e "$out" && ! -L "$out" ]] || { echo 'output directory must be new' >&2; exit 64; }
mkdir -p "$out/.build"
out="$(cd -- "$out" && pwd -P)"
work="$out/.build"
export MACOSX_DEPLOYMENT_TARGET=13.0
unset SDKROOT
phase=portable_build
finish() {
  status=$?
  trap - EXIT
  if [[ "$status" != 0 ]]; then
    # These logs contain only this repository's generic probe compilation.
    # Keep failed compiler diagnostics visible without publishing build outputs.
    for log in portable-build.log test-build.log bridge-build.log native-build.log; do
      if [[ -f "$work/$log" && ! -L "$work/$log" ]]; then
        printf '\n--- %s ---\n' "$log" >&2
        tail -n 80 "$work/$log" >&2
      fi
    done
  fi
  printf '{"schema":1,"phase":"%s","command_exit_code":%s}\n' "$phase" "$status" > "$out/command-exit.json"
  if [[ -x "$work/portable" ]]; then
    if ! "$work/portable" seal "$out" "$status"; then
      printf '{"schema":1,"phase":"seal_failed","observation_exit_code":%s,"command_exit_code":1}\n' "$status" > "$out/command-exit.json"
      exit 1
    fi
  fi
  exit "$status"
}
trap finish EXIT
rustc --edition=2021 --check-cfg 'cfg(test)' --check-cfg 'cfg(boundary_native)' \
  -D warnings -O --remap-path-prefix "$root=/probe" scripts/boundary/host.rs -o "$work/portable" > "$work/portable-build.log" 2>&1
phase=portable_test_build
rustc --edition=2021 --test --check-cfg 'cfg(test)' --check-cfg 'cfg(boundary_native)' \
  -D warnings -O --remap-path-prefix "$root=/probe" scripts/boundary/host.rs -o "$work/tests" > "$work/test-build.log" 2>&1
phase=portable_tests
"$work/tests" > "$out/reference-tests.txt" 2>&1
phase=portable_reference
"$work/portable" reference "$out/reference"
phase=portable_negative
set +e
"$work/portable" reference-negative "$out/reference-negative"
code=$?
set -e
[[ "$code" == 2 ]] || { echo 'portable negative did not fail as expected' >&2; exit 2; }
phase=native_platform_check
[[ "$(uname -s)" == Darwin ]] || { echo 'native macOS required; no CPU-as-GPU fallback' >&2; exit 2; }
phase=native_bridge_build
xcrun --sdk macosx clang -c scripts/boundary/bridge.m -fobjc-arc -std=c11 -O2 -g0 \
  -Wall -Wextra -Werror -Wno-deprecated-declarations -mmacosx-version-min=13.0 \
  -ffile-prefix-map="$root"=/probe -fdebug-prefix-map="$root"=/probe \
  -o "$work/bridge.o" > "$work/bridge-build.log" 2>&1
phase=native_host_build
rustc --edition=2021 --check-cfg 'cfg(test)' --check-cfg 'cfg(boundary_native)' --cfg boundary_native \
  -D warnings -O -C debuginfo=0 --remap-path-prefix "$root=/probe" \
  scripts/boundary/host.rs -C "link-arg=$work/bridge.o" \
  -l framework=Foundation -l framework=Metal -l objc -o "$work/boundary-probe" > "$work/native-build.log" 2>&1
phase=native_observe
"$work/boundary-probe" observe "$out/results"
