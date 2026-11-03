#!/usr/bin/env bash
set -euo pipefail
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
cd "$root"
[[ $# -le 2 ]] || { echo 'usage: bash scripts/rounding/run.sh [observe|inspect] [NEW_OUTPUT_DIRECTORY]' >&2; exit 64; }
kind="${1:-observe}"
case "$kind" in observe|inspect) ;; *) echo 'unknown collection kind' >&2; exit 64;; esac
out="${2:-out-rounding}"
[[ ! -e "$out" && ! -L "$out" ]] || { echo 'output directory must not exist (no stale results)' >&2; exit 64; }
mkdir -p "$out/.build"
out="$(cd -- "$out" && pwd -P)"
work="$out/.build"
export MACOSX_DEPLOYMENT_TARGET=13.0
unset SDKROOT

record() {
  local label="$1"; shift
  printf '\n[%s]\n' "$label"
  if "$@"; then printf 'exit_status=0\n'; else printf 'exit_status=%s\n' "$?"; fi
}
clang_version() { xcrun --sdk macosx clang --version | sed '/^InstalledDir:/d'; }
{
  record utc date -u '+%Y-%m-%dT%H:%M:%SZ'
  record platform uname -s
  record machine uname -m
  record process_arch arch
  record sw_vers sw_vers
  record translated sysctl -n sysctl.proc_translated
  record xcode xcodebuild -version
  record sdk_version xcrun --sdk macosx --show-sdk-version
  record sdk_build xcrun --sdk macosx --show-sdk-build-version
  record clang clang_version
  record rustc rustc --version --verbose
} > "$out/identity.txt" 2> "$work/identity-stderr.log"
shasum -a 256 scripts/rounding/host.rs scripts/rounding/bridge.m \
  scripts/rounding/write.metal scripts/rounding/run.sh > "$out/SOURCE_SHA256SUMS"
if [[ -f .github/workflows/rounding.yml ]]; then
  shasum -a 256 .github/workflows/rounding.yml >> "$out/SOURCE_SHA256SUMS"
fi
cat > "$out/build-options.txt" <<'OPTIONS'
Rust: edition=2021; opt-level=2; debuginfo=0; no dependencies; native host target
Objective-C: ARC; C11; -O2 -g0; deployment target 13.0; Foundation + Metal + objc
MSL: runtime newLibraryWithSource; fastMathEnabled=YES plus NO control; SDK default language version
Input: raw binary32 bits, no shader arithmetic; output: raw texture blit, no texture read conversion
Paths: compiler remap to /probe; compiler object/executable/stderr are not publication artifacts
OPTIONS
[[ "$(uname -s)" == Darwin ]] || { echo 'native bridge requires macOS; no GPU inference' >&2; exit 2; }
rustc --edition=2021 --check-cfg 'cfg(test)' --check-cfg 'cfg(rounding_native)' \
  --test -O -D warnings --remap-path-prefix "$root=/probe" \
  scripts/rounding/host.rs -o "$work/tests"
"$work/tests" > "$out/reference-tests.txt" 2>&1
cat "$out/reference-tests.txt"
xcrun --sdk macosx clang -c scripts/rounding/bridge.m -fobjc-arc -std=c11 -O2 -g0 \
  -Wall -Wextra -Werror -Wno-deprecated-declarations -mmacosx-version-min=13.0 \
  -ffile-prefix-map="$root"=/probe -fdebug-prefix-map="$root"=/probe -o "$work/bridge.o"
rustc --edition=2021 --check-cfg 'cfg(test)' --check-cfg 'cfg(rounding_native)' --cfg rounding_native \
  -D warnings -C opt-level=2 -C debuginfo=0 --remap-path-prefix "$root=/probe" \
  scripts/rounding/host.rs -C "link-arg=$work/bridge.o" \
  -l framework=Foundation -l framework=Metal -l objc -o "$work/rounding-probe"
set +e
"$work/rounding-probe" "$kind" "$out/results"
status=$?
set -e
(
  cd "$out"
  shopt -s nullglob
  files=(identity.txt SOURCE_SHA256SUMS build-options.txt reference-tests.txt results/*.json results/*.bin)
  shasum -a 256 "${files[@]}" > SHA256SUMS
)
echo "collection exit=$status; consult results/summary.json, not job color"
exit "$status"
