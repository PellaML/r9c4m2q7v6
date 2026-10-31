#!/usr/bin/env bash
set -euo pipefail
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
cd "$root"
[[ $# -eq 0 ]] || { echo 'no arguments' >&2; exit 64; }
[[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]] || { echo 'requires native Apple Silicon macOS' >&2; exit 2; }
case "${EXPECTED_OS_MAJOR:-}" in 26|27) ;; *) echo 'explicit expected OS major required' >&2; exit 2;; esac
[[ "$(sw_vers -productVersion)" == "$EXPECTED_OS_MAJOR".* ]] || { echo 'native OS major mismatch' >&2; exit 2; }
unset MACOSX_DEPLOYMENT_TARGET SDKROOT
[[ "$(xcrun --sdk macosx --show-sdk-version)" == "$EXPECTED_OS_MAJOR".* ]] || { echo 'native SDK major mismatch' >&2; exit 2; }
out=out-math-cohort
[[ ! -e "$out" && ! -L "$out" ]] || { echo 'output directory must not exist' >&2; exit 3; }
mkdir "$out" "$out/.build"
work="$out/.build"
files=(corpus.json inputs.bin uploaded-inputs.bin results.bin native.json check.json wrong-expected.json toolchain.txt math-cohort.air math-cohort.metallib)
finish() {
  status=$?
  trap - EXIT
  # Hash even partial failure evidence; never discard or replace a raw result.
  (cd "$out"; for file in "${files[@]}"; do [[ ! -f "$file" ]] || shasum -a 256 "$file"; done) > "$out/SHA256SUMS"
  exit "$status"
}
trap finish EXIT
python3 scripts/math-cohort/corpus.py selftest
python3 scripts/math-cohort/corpus.py generate "$out"
{
  sw_vers
  uname -m
  xcodebuild -version
  xcrun --sdk macosx --show-sdk-version
  xcrun --sdk macosx metal --version | sed '/^InstalledDir:/d'
  xcrun --sdk macosx clang --version | sed '/^InstalledDir:/d'
  python3 --version
  git rev-parse HEAD
  printf '%s\n' 'MSL: -std=metal4.0 -O2 -g0 -ffast-math; compiler-default deployment; one compute dispatch'
} > "$out/toolchain.txt"
xcrun --sdk macosx clang scripts/math-cohort/host.m -fobjc-arc -std=c11 -O2 -g0 \
  -Wall -Wextra -Werror -ffile-prefix-map="$root"=/probe -fdebug-prefix-map="$root"=/probe \
  -framework Metal -framework Foundation -o "$work/math-host"
cp shaders/math-cohort.metal "$work/math-cohort.metal"
(
  cd "$work"
  xcrun --sdk macosx metal -std=metal4.0 -O2 -g0 -ffast-math \
    -ffile-prefix-map="$root"=/probe -fdebug-prefix-map="$root"=/probe \
    -fdebug-compilation-dir=/probe -c math-cohort.metal -o math-cohort.air
  xcrun --sdk macosx metallib math-cohort.air -o math-cohort.metallib
)
cp "$work/math-cohort.air" "$work/math-cohort.metallib" "$out/"
set +e
"$work/math-host" "$out/math-cohort.metallib" "$out/inputs.bin" "$out"
native=$?
set -e
if [[ $native -ne 0 ]]; then
  [[ ! -f "$out/native.json" ]] || cat "$out/native.json"
  echo 'native execution refused; no CPU or alternate implementation fallback' >&2
  exit "$native"
fi
set +e
python3 scripts/math-cohort/corpus.py check "$out"
check=$?
python3 scripts/math-cohort/corpus.py wrong-expected "$out"
control=$?
python3 scripts/math-cohort/corpus.py audit "$out"
audit=$?
set -e
[[ $check -eq 0 && $control -eq 1 && $audit -eq 0 ]] || { echo 'integrity/control/privacy check failed; raw data retained' >&2; exit 1; }
echo 'raw native math results retained; exact controls passed; no SFU parity or full-domain claim'
