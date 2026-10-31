#!/usr/bin/env bash
set -euo pipefail
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
cd "$root"
[[ $# -eq 0 ]] || { echo 'no arguments' >&2; exit 64; }
[[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]] || { echo 'requires native Apple Silicon macOS' >&2; exit 2; }
case "$(sw_vers -productVersion)" in 26.*|27.*) ;; *) echo 'requires macOS26 or27' >&2; exit 2;; esac
unset MACOSX_DEPLOYMENT_TARGET SDKROOT
case "$(xcrun --sdk macosx --show-sdk-version)" in 26.*|27.*) ;; *) echo 'requires SDK26 or27' >&2; exit 2;; esac
out=out-derivative
[[ ! -e "$out" && ! -L "$out" ]] || { echo 'out-derivative must not exist' >&2; exit 3; }
mkdir "$out"
work="$out/.build"
mkdir "$work"
python3 scripts/derivative/oracle.py selftest
python3 scripts/derivative/oracle.py generate "$work/results"
xcrun --sdk macosx clang scripts/derivative/host.m -fobjc-arc -std=c11 -O2 -g0 \
  -Wall -Wextra -Werror -ffile-prefix-map="$root"=/probe -fdebug-prefix-map="$root"=/probe \
  -framework Metal -framework Foundation -o "$work/derivative-host"
cp shaders/derivative.metal "$work/derivative.metal"
(
  cd "$work"
  xcrun --sdk macosx metal -std=metal4.0 -O2 -g0 -ffast-math \
    -ffile-prefix-map="$root"=/probe -fdebug-prefix-map="$root"=/probe \
    -fdebug-compilation-dir=/probe -c derivative.metal -o derivative.air
  xcrun --sdk macosx metallib derivative.air -o derivative.metallib
)
set +e
"$work/derivative-host" "$work/derivative.metallib" "$work/results/inputs.bin" "$work/results"
native=$?
set -e
if [[ $native -ne 0 ]]; then
  [[ ! -f "$work/results/native.json" ]] || cat "$work/results/native.json"
  echo 'native render refused; no CPU or compute fallback' >&2
  exit "$native"
fi
set +e
python3 scripts/derivative/oracle.py check "$work/results"
check=$?
python3 scripts/derivative/oracle.py wrong-expected "$work/results"
control=$?
set -e
cp "$work/derivative.air" "$work/derivative.metallib" "$work/results/"
{
  sw_vers
  uname -m
  xcodebuild -version
  xcrun --sdk macosx --show-sdk-version
  xcrun --sdk macosx metal --version | sed '/^InstalledDir:/d'
  xcrun --sdk macosx clang --version | sed '/^InstalledDir:/d'
  python3 --version
  printf '%s\n' 'MSL: -std=metal4.0 -O2 -g0 -ffast-math; deployment: compiler default'
} > "$work/results/toolchain.txt"
python3 scripts/derivative/oracle.py audit "$work/results"
files=(inputs.bin corpus.json trace.bin coverage.bin textures.bin native.json check.json wrong-expected.json toolchain.txt derivative.air derivative.metallib)
for file in "${files[@]}"; do cp "$work/results/$file" "$out/$file"; done
(cd "$out" && shasum -a 256 "${files[@]}" > SHA256SUMS)
[[ $check -eq 0 && $control -eq 1 ]] || { echo 'render/coverage/known control checks failed; raw data retained' >&2; exit 1; }
echo 'real fragment render, full quad coverage and nonzero F32 controls passed; half models are diagnostic only'
