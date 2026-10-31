#!/usr/bin/env bash
set -euo pipefail
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
cd "$root"
[[ $# -eq 0 ]] || { echo 'no arguments' >&2; exit 64; }
[[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]] || { echo 'requires native Apple Silicon macOS' >&2; exit 2; }
case "$(sw_vers -productVersion)" in 26.*|27.*) ;; *) echo 'requires macOS26 or27' >&2; exit 2;; esac
unset MACOSX_DEPLOYMENT_TARGET SDKROOT
case "$(xcrun --sdk macosx --show-sdk-version)" in 26.*|27.*) ;; *) echo 'requires SDK26 or27' >&2; exit 2;; esac
out=out-fract
[[ ! -e "$out" && ! -L "$out" ]] || { echo 'out-fract must not exist' >&2; exit 3; }
mkdir "$out"
work="$out/.build"
mkdir "$work"
python3 scripts/fract/oracle.py selftest
python3 scripts/fract/oracle.py generate "$work/results"
xcrun --sdk macosx clang scripts/fract/host.m -fobjc-arc -std=c11 -O2 -g0 \
  -Wall -Wextra -Werror -ffile-prefix-map="$root"=/probe -fdebug-prefix-map="$root"=/probe \
  -framework Metal -framework Foundation -o "$work/fract-host"
cp shaders/fract.metal "$work/fract.metal"
(
  cd "$work"
  xcrun --sdk macosx metal -std=metal4.0 -O2 -g0 -ffast-math \
    -ffile-prefix-map="$root"=/probe -fdebug-prefix-map="$root"=/probe \
    -fdebug-compilation-dir=/probe -c fract.metal -o fract.air
  xcrun --sdk macosx metallib fract.air -o fract.metallib
)
set +e
"$work/fract-host" "$work/fract.metallib" "$work/results/inputs.bin" "$work/results"
native=$?
set -e
if [[ $native -ne 0 ]]; then
  [[ ! -f "$work/results/native.json" ]] || cat "$work/results/native.json"
  echo 'native execution refused; no CPU fallback' >&2
  exit "$native"
fi
set +e
python3 scripts/fract/oracle.py check "$work/results"
check=$?
python3 scripts/fract/oracle.py wrong-expected "$work/results"
control=$?
set -e
cp "$work/fract.air" "$work/fract.metallib" "$work/results/"
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
python3 scripts/fract/oracle.py audit "$work/results"
files=(inputs.bin samples.bin corpus.json native.json check.json wrong-expected.json toolchain.txt fract.air fract.metallib)
# Only generic raw observations and metadata reach the artifact allowlist.
# Numeric disagreement remains failure; it never substitutes CPU output.
for file in "${files[@]}"; do cp "$work/results/$file" "$out/$file"; done
(cd "$out" && shasum -a 256 "${files[@]}" > SHA256SUMS)
[[ $check -eq 0 && $control -eq 1 ]] || { echo 'numeric/model/control checks failed; raw observations retained' >&2; exit 1; }
echo '126976 float cells and190464 half cells checked; one wrong expectation rejected'
