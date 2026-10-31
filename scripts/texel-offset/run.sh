#!/usr/bin/env bash
set -euo pipefail
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
cd "$root"
[[ $# -eq 0 ]] || { echo 'no arguments' >&2; exit 64; }
[[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]] || { echo 'requires native Apple Silicon macOS' >&2; exit 2; }
case "$(sw_vers -productVersion)" in 26.*|27.*) ;; *) echo 'requires macOS26 or27' >&2; exit 2;; esac
unset MACOSX_DEPLOYMENT_TARGET SDKROOT
case "$(xcrun --sdk macosx --show-sdk-version)" in 26.*|27.*) ;; *) echo 'requires SDK26 or27' >&2; exit 2;; esac
out=out-texel-offset
[[ ! -e "$out" && ! -L "$out" ]] || { echo 'output directory must not exist' >&2; exit 3; }
mkdir "$out" "$out/.build"
work="$out/.build"
python3 scripts/texel-offset/corpus.py selftest
python3 scripts/texel-offset/corpus.py generate "$out"
{
  sw_vers
  uname -m
  xcodebuild -version
  xcrun --sdk macosx --show-sdk-version
  xcrun --sdk macosx metal --version | sed '/^InstalledDir:/d'
  xcrun --sdk macosx clang --version | sed '/^InstalledDir:/d'
  python3 --version
  git rev-parse HEAD
  printf '%s\n' 'MSL: -std=metal3.1 -O2 -g0 -ffast-math; compiler-default deployment; compute level(0)'
} > "$out/toolchain.txt"
xcrun --sdk macosx clang scripts/texel-offset/host.m -fobjc-arc -std=c11 -O2 -g0 \
  -Wall -Wextra -Werror -ffile-prefix-map="$root"=/probe -fdebug-prefix-map="$root"=/probe \
  -framework Metal -framework Foundation -o "$work/texel-offset-host"
cp shaders/texel-offset.metal "$work/texel-offset.metal"
(
  cd "$work"
  xcrun --sdk macosx metal -std=metal3.1 -O2 -g0 -ffast-math \
    -ffile-prefix-map="$root"=/probe -fdebug-prefix-map="$root"=/probe \
    -fdebug-compilation-dir=/probe -c texel-offset.metal -o texel-offset.air
  xcrun --sdk macosx metallib texel-offset.air -o texel-offset.metallib
)
cp "$work/texel-offset.air" "$work/texel-offset.metallib" "$out/"
set +e
"$work/texel-offset-host" "$out/texel-offset.metallib" "$out"
native=$?
set -e
if [[ $native -ne 0 ]]; then
  [[ ! -f "$out/native.json" ]] || cat "$out/native.json"
  echo 'native compute refused; no CPU or alternate-stage fallback' >&2
  exit "$native"
fi
set +e
python3 scripts/texel-offset/corpus.py check "$out"
check=$?
python3 scripts/texel-offset/corpus.py wrong-expected "$out"
control=$?
set -e
files=(corpus.json inputs.bin image-data.bin uploaded-images.bin results.bin native.json check.json wrong-expected.json toolchain.txt texel-offset.air texel-offset.metallib)
(cd "$out" && shasum -a 256 "${files[@]}" > SHA256SUMS)
[[ $check -eq 0 && $control -eq 1 ]] || { echo 'native integrity/anchor controls failed; raw data retained' >&2; exit 1; }
echo 'native image upload, explicit-LOD samples, complete readback and known controls passed; other arithmetic remains unjudged'
