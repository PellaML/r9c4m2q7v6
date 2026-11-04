#!/usr/bin/env bash
set -euo pipefail
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
cd "$root"
[[ $# -le 1 ]] || { echo 'usage: bash scripts/unary/run.sh [NEW_OUTPUT_DIRECTORY]' >&2; exit 64; }
out="${1:-out-unary}"
[[ ! -e "$out" && ! -L "$out" ]] || { echo 'output directory must be new' >&2; exit 64; }
mkdir -p "$out/.build" "$out/air"
out="$(cd -- "$out" && pwd -P)"
work="$out/.build"
export MACOSX_DEPLOYMENT_TARGET=13.0
unset SDKROOT
phase=portable_build

finish() {
  status=$?
  trap - EXIT
  printf '{"schema":1,"phase":"%s","command_exit_code":%s}\n' "$phase" "$status" > "$out/command-exit.json"
  if [[ -x "$work/portable" ]]; then
    if ! "$work/portable" seal "$out" "$status"; then
      echo 'receipt sealing failed; run is not qualified' >&2
      exit 1
    fi
  fi
  exit "$status"
}
trap finish EXIT
rustc --edition=2021 --check-cfg 'cfg(test)' --check-cfg 'cfg(unary_native)' \
  -D warnings -C opt-level=2 -C debuginfo=0 --remap-path-prefix "$root=/probe" \
  scripts/unary/host.rs -o "$work/portable" > "$work/portable-build.log" 2>&1
phase=portable_test_build
rustc --edition=2021 --check-cfg 'cfg(test)' --check-cfg 'cfg(unary_native)' \
  --test -O -D warnings --remap-path-prefix "$root=/probe" \
  scripts/unary/host.rs -o "$work/tests" > "$work/test-build.log" 2>&1
phase=portable_tests
"$work/tests" > "$out/reference-tests.txt" 2>&1
phase=portable_reference
"$work/portable" reference "$out/reference"
for variant in swap zero-cos; do
  phase="portable_negative_$variant"
  set +e
  "$work/portable" "reference-negative-$variant" "$out/reference-negative-$variant"
  code=$?
  set -e
  [[ "$code" == 2 ]] || { echo 'portable negative did not fail as expected' >&2; exit 2; }
done
phase=native_platform_check
[[ "$(uname -s)" == Darwin ]] || { echo 'native macOS required; no CPU-as-GPU fallback' >&2; exit 2; }

# Offline outputs are from this fresh generic shader only. They are not the
# runtime library, not a hardware disassembly, and not evidence of GPU execution.
phase=offline_compile_observation
for fast in 0 1; do
  flag=-fno-fast-math
  [[ "$fast" == 0 ]] || flag=-ffast-math
  for variant in 0 1 2; do
    name="fast-$fast-variant-$variant"
    set +e
    xcrun --sdk macosx metal -std=metal3.0 "$flag" -DPROBE_VARIANT="$variant" \
      -c scripts/unary/probe.metal -o "$out/air/$name.air" > "$work/$name.air.log" 2>&1
    air_code=$?
    xcrun --sdk macosx metal -std=metal3.0 "$flag" -DPROBE_VARIANT="$variant" \
      -S -emit-llvm scripts/unary/probe.metal -o "$out/air/$name.ll" > "$work/$name.ll.log" 2>&1
    ir_code=$?
    set -e
    # Only bounded signature/attribute lines from our own emitted IR are exported.
    if [[ "$ir_code" == 0 && -f "$out/air/$name.ll" ]]; then
      LC_ALL=C grep -E '(^target |^define |^declare |^attributes |air\.(fast_)?(sin|cos)|air\.compile_options|fast_math)' \
        "$out/air/$name.ll" | head -c 32768 > "$out/air/$name.signatures.txt" || true
    fi
    printf '{"schema":1,"fast_math_requested":%s,"variant":%s,"air_exit_code":%s,"ir_exit_code":%s,"language":"metal3.0","math_flag":"%s","execution_proof":false,"scope":"owned generic offline outputs; runtime uses SDK default language version"}\n' \
      "$fast" "$variant" "$air_code" "$ir_code" "$flag" > "$out/air/$name.compile.json"
  done
done
phase=native_bridge_build
xcrun --sdk macosx clang -c scripts/unary/bridge.m -fobjc-arc -std=c11 -O2 -g0 \
  -Wall -Wextra -Werror -Wno-deprecated-declarations -mmacosx-version-min=13.0 \
  -ffile-prefix-map="$root"=/probe -fdebug-prefix-map="$root"=/probe \
  -o "$work/bridge.o" > "$work/bridge-build.log" 2>&1
phase=native_host_build
rustc --edition=2021 --check-cfg 'cfg(test)' --check-cfg 'cfg(unary_native)' --cfg unary_native \
  -D warnings -C opt-level=2 -C debuginfo=0 --remap-path-prefix "$root=/probe" \
  scripts/unary/host.rs -C "link-arg=$work/bridge.o" \
  -l framework=Foundation -l framework=Metal -l objc -o "$work/unary-probe" > "$work/native-build.log" 2>&1
phase=native_observe
"$work/unary-probe" observe "$out/results"
