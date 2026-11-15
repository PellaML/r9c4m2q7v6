#!/usr/bin/env bash
set -euo pipefail
# Local execution only, deliberately no CI workflow, network, dispatch or upload.
[[ "$(uname -s)" == Darwin ]] || { echo 'Actual macOS MTLDevice required; no CPU fallback.' >&2; exit 23; }
[[ "${VALIDATION_REVIEWED:-}" == 1 ]] || { echo 'Main must review these exact bytes before execution.' >&2; exit 64; }
[[ $# == 1 ]] || { echo 'usage: bash run.sh NEW_OUTPUT_DIRECTORY' >&2; exit 64; }
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
[[ ! -e "$1" && ! -L "$1" ]] || { echo 'Output must be new; never replace evidence.' >&2; exit 64; }
mkdir -- "$1"
out="$(cd -- "$1" && pwd -P)"
mkdir "$out/.build" "$out/report"
export TMPDIR="$out/.build/"
phase=toolchain
finish() {
  code=$?
  trap - EXIT
  printf 'phase\t%s\nexit_code\t%s\nnative_qualification_claimed\t0\n' "$phase" "$code" > "$out/exit.tsv"
  (
    cd "$out"
    find . -type f ! -path './.build/*' ! -name artifacts.sha256 | LC_ALL=C sort |
      while IFS= read -r file; do shasum -a 256 "$file"; done
  ) > "$out/artifacts.sha256"
  exit "$code"
}
trap finish EXIT
cd "$root"
shasum -a 256 probe.metal bridge.m oracle.rs run.sh > "$out/source.sha256"
{
  printf 'machine_arch\t'; uname -m
  printf 'os_version\t'; sw_vers -productVersion
  printf 'os_build\t'; sw_vers -buildVersion
  printf 'sdk_version\t'; xcrun --sdk macosx --show-sdk-version
  printf 'clang\t'; xcrun --sdk macosx clang --version | sed -n '1p'
  printf 'rustc\t'; rustc --version
  xcodebuild -version | sed '1s/^/xcode_version\t/;2s/^/xcode_build\t/'
} > "$out/toolchain.tsv"
phase=parser_build_and_tests
rustc --edition=2021 -D warnings -C opt-level=1 -C debuginfo=0 \
  --remap-path-prefix "$root=/validation" oracle.rs -o "$out/.build/oracle" > "$out/.build/rust-build.log" 2>&1
rustc --edition=2021 -D warnings --test -C debuginfo=0 \
  --remap-path-prefix "$root=/validation" oracle.rs -o "$out/.build/parser-tests" > "$out/.build/rust-tests-build.log" 2>&1
"$out/.build/parser-tests" > "$out/parser-tests.txt"
"$out/.build/oracle" prepare "$out"
phase=native_bridge_build
xcrun --sdk macosx clang bridge.m -o "$out/.build/native" -fobjc-arc -fblocks -std=c11 -O1 -g0 \
  -Wall -Wextra -Werror -Wno-deprecated-declarations -mmacosx-version-min=13.0 \
  -ffile-prefix-map="$root"=/validation -fdebug-prefix-map="$root"=/validation \
  -framework Foundation -framework Metal > "$out/.build/objc-build.log" 2>&1
for profile in false true default; do
  for build in 0 1; do
    phase="native_${profile}_b${build}"
    "$out/.build/native" "$root/probe.metal" "$out/inputs.bin" "$out/$profile-b$build" "$profile" "$build" \
      > "$out/.build/$profile-b$build.stdout" 2> "$out/.build/$profile-b$build.stderr"
  done
done
phase=raw_integrity_and_observations
"$out/.build/oracle" check "$out" "$out/report" > "$out/check-summary.tsv"
# Verify source stability, not just the first source hash list.
shasum -a 256 -c "$out/source.sha256" > "$out/source-verified.txt"
phase=complete_observations_not_model_qualification
