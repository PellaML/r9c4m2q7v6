#!/usr/bin/env bash
set -euo pipefail
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
cd "$root"
[[ $# -le 1 ]] || { echo 'one math mode only' >&2; exit 64; }
mode="${1:-strict}"
case "$mode" in strict) math=-fno-fast-math;; fast) math=-ffast-math;; *) echo 'unknown math mode' >&2; exit 64;; esac
[[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]] || { echo 'requires native Apple Silicon macOS' >&2; exit 2; }
case "$(sw_vers -productVersion)" in 26.*|27.*) ;; *) echo 'requires macOS 26 or 27' >&2; exit 2;; esac
unset MACOSX_DEPLOYMENT_TARGET SDKROOT
case "$(xcrun --sdk macosx --show-sdk-version)" in 26.*|27.*) ;; *) echo 'requires SDK 26 or 27' >&2; exit 2;; esac
out=out-unpack
[[ ! -e "$out" && ! -L "$out" ]] || { echo 'out-unpack must not exist' >&2; exit 3; }
mkdir "$out"
work="$out/.build"
mkdir "$work"
rustc --edition=2021 --check-cfg 'cfg(test)' --check-cfg 'cfg(unpack_native)' --test -O -D warnings scripts/unpack/oracle.rs -o "$work/tests"
"$work/tests"
xcrun --sdk macosx clang -c scripts/unpack/bridge.m -fobjc-arc -std=c11 -O2 -g0 \
  -Wall -Wextra -Werror -ffile-prefix-map="$root"=/probe -fdebug-prefix-map="$root"=/probe \
  -o "$work/bridge.o"
rustc --edition=2021 --check-cfg 'cfg(test)' --check-cfg 'cfg(unpack_native)' --cfg unpack_native -D warnings \
  -C opt-level=2 -C debuginfo=0 --remap-path-prefix "$root=/probe" \
  scripts/unpack/oracle.rs -C "link-arg=$work/bridge.o" \
  -l framework=Metal -l framework=Foundation -o "$work/unpack-oracle"
cp shaders/unpack.metal "$work/unpack.metal"
(
  cd "$work"
  xcrun --sdk macosx metal -std=metal4.0 -O2 -g0 "$math" \
    -ffile-prefix-map="$root"=/probe -fdebug-prefix-map="$root"=/probe \
    -fdebug-compilation-dir=/probe -c unpack.metal -o unpack.air
  xcrun --sdk macosx metallib unpack.air -o unpack.metallib
)
set +e
"$work/unpack-oracle" run "$work/unpack.metallib" "$work/results" "$mode"
status=$?
set -e
if [[ $status -ne 0 && $status -ne 1 ]]; then
  [[ ! -f "$work/results/native.json" ]] || cat "$work/results/native.json"
  echo 'native execution refused; no CPU fallback' >&2
  exit "$status"
fi
set +e
"$work/unpack-oracle" check "$work/results"
check=$?
"$work/unpack-oracle" wrong-expected "$work/results"
control=$?
set -e
cp "$work/unpack.air" "$work/unpack.metallib" "$work/results/"
{
  sw_vers
  uname -m
  xcodebuild -version
  xcrun --sdk macosx --show-sdk-version
  xcrun --sdk macosx metal --version | sed '/^InstalledDir:/d'
  rustc --version --verbose
  printf '%s\n' "MSL: -std=metal4.0 -O2 -g0 $math; deployment: compiler default"
} > "$work/results/toolchain.txt"
"$work/unpack-oracle" audit "$work/results"
files=(inputs.bin expected-half.bin samples.bin native.json check.json wrong-expected.json manifest.json toolchain.txt unpack.air unpack.metallib)
# Only a completely executed, numerically checked and path-audited run reaches
# the artifact allowlist. Retain actual observations even on numeric rejection;
# that remains a failed job, never an adjusted or silently successful oracle.
# No executables, objects, source or scratch are uploaded.
for file in "${files[@]}"; do cp "$work/results/$file" "$out/$file"; done
(cd "$out" && shasum -a 256 "${files[@]}" > SHA256SUMS)
if [[ $status -ne 0 || $check -ne 0 || $control -ne 1 ]]; then
  echo 'raw native observations retained, but numeric/control checks failed' >&2
  exit 1
fi
echo '1040 native cases / 4160 half cells passed; wrong expected control rejected exactly one cell'
