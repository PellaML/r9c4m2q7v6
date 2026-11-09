#!/bin/bash
# Existing local toolchains only. No downloads, installs, retries, SSH or cleanup.
set -euo pipefail
umask 077
cd "$(dirname "$0")/../.."
out=out-scalar-varying
r=$out/results
s=scripts/scalar-varying
sources=("$s/probe.metal" "$s/probe.swift" "$s/accept.rs" "$s/check.rs" "$s/run.sh" .github/workflows/scalar-varying.yml)

bounded() {
  [[ -d "$r" && ! -L "$out" && ! -L "$r" ]] || return 1
  [[ -z "$(find "$r" -mindepth 1 ! -type f -print -quit)" ]] || return 1
  local allowed='metadata.tsv toolchain.txt source.sha256 commands.txt build.txt tests.txt generation.txt bindings.tsv compiler.sha256 gpu.json gpu.tsv half.bin vertices.bin vertices.tsv expected-u16.tsv measured-u16.tsv measured-varying.tsv metrics.tsv validation.txt status.txt'
  local mode i j f n bytes total=0 count=0
  for mode in precise fast; do
    allowed+=" $mode.air $mode.ll $mode.metallib $mode-air-proof.txt $mode-inspect.txt $mode-u16.bin"
    for i in 0 1; do for j in 0 1; do allowed+=" $mode-v$i-$j.bin"; done; done
  done
  for f in "$r"/*; do
    [[ -f "$f" && ! -L "$f" ]] || return 1
    n=${f##*/}; [[ " $allowed " == *" $n "* ]] || return 1
    bytes=$(wc -c < "$f"); (( bytes <= 1048576 )) || return 1
    total=$((total+bytes)); count=$((count+1))
  done
  (( count <= 40 && total <= 4194304 && count == $(find "$r" -type f | wc -l) ))
}
if [[ $# == 1 && $1 == verify ]]; then
  bounded
  # Recompute exactly the allowed regular files; never follow manifest paths.
  [[ -f "$out/sha256.txt" && ! -L "$out/sha256.txt" ]]
  [[ "$(cd "$out" && shasum -a 256 results/*)" == "$(cat "$out/sha256.txt")" ]]
  exit 0
fi
[[ $# == 0 && $(uname -s) == Darwin ]] || { echo 'macOS runner, no arguments required' >&2; exit 2; }
[[ ! -e "$out" && ! -L "$out" ]] || { echo 'output must be new; nothing deleted' >&2; exit 2; }
mkdir -p "$r" "$out/build" "$out/tmp"
export TMPDIR="$PWD/$out/tmp"
finish() {
  local rc=$?
  trap - EXIT
  if (( rc == 0 )); then
    printf 'BOUNDED_REFERENCE_OBSERVATIONS_PASS\nrun_exit=0\n' > "$r/status.txt"
  else
    printf 'UNQUALIFIED\nrun_exit=%s\n' "$rc" > "$r/status.txt"
  fi
  if bounded; then (cd "$out" && shasum -a 256 results/*) > "$out/sha256.txt" || rc=3; else rc=3; fi
  exit "$rc"
}
trap finish EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
: > "$r/commands.txt"; : > "$r/build.txt"
step() {
  local log=$1; shift
  printf '%q ' "$@" >> "$r/commands.txt"; printf '\n' >> "$r/commands.txt"
  "$@" >> "$r/$log" 2>&1
}
space() {
  # No new compiler invocation below 2 GiB available on this filesystem.
  local available; available=$(df -Pk . | awk 'END {print $4}')
  (( available >= 2097152 )) || { echo 'insufficient free space; no new build' >&2; exit 3; }
}
actual=$(sw_vers -buildVersion)
printf 'runner\t%s\nos_build\t%s\narchitecture\t%s\n' "${RUNNER_LABEL:?}" "$actual" "$(uname -m)" > "$r/metadata.tsv"
[[ $(uname -m) == arm64 && "$actual" == "${EXPECTED_OS_BUILD:?}" ]]
[[ "$RUNNER_LABEL:$actual" == macos-26:25G332 || "$RUNNER_LABEL:$actual" == xcode-27:26A434 ]]
step toolchain.txt sw_vers
step toolchain.txt xcodebuild -version
step toolchain.txt xcrun --sdk macosx --show-sdk-version
step toolchain.txt xcrun --sdk macosx --show-sdk-build-version
step toolchain.txt xcrun --sdk macosx metal --version
step toolchain.txt xcrun --sdk macosx swiftc --version
step toolchain.txt rustc -vV
shasum -a 256 "${sources[@]}" > "$r/source.sha256"
space; step build.txt rustc --edition=2021 -D warnings --crate-name scalar_accept --crate-type rlib "$s/accept.rs" -o "$out/build/libscalar_accept.rlib"
space; step build.txt rustc --edition=2021 -D warnings --test "$s/accept.rs" -o "$out/build/accept-tests"
space; step build.txt rustc --edition=2021 -D warnings --test "$s/check.rs" --extern "scalar_accept=$out/build/libscalar_accept.rlib" -o "$out/build/check-tests"
step tests.txt "$out/build/accept-tests"
step tests.txt "$out/build/check-tests"
space; step build.txt rustc --edition=2021 -D warnings "$s/check.rs" --extern "scalar_accept=$out/build/libscalar_accept.rlib" -o "$out/build/check"
step generation.txt "$out/build/check" generate "$r"
printf 'mode\tair_sha256\tll_sha256\tmetallib_sha256\tinspection_route\n' > "$r/bindings.tsv"
hash() { shasum -a 256 "$1" | cut -d' ' -f1; }
for mode in precise fast; do
  flag=-fno-fast-math; [[ $mode == precise ]] || flag=-ffast-math
  space; step build.txt xcrun --sdk macosx metal -std=metal4.0 -O2 -g0 "$flag" -c "$s/probe.metal" -o "$r/$mode.air"
  before=$(hash "$r/$mode.air")
  route=metal-ir-reader-exact-air
  if xcrun --sdk macosx --find llvm-dis >/dev/null 2>&1 &&
     step "$mode-inspect.txt" xcrun --sdk macosx llvm-dis "$r/$mode.air" -o "$r/$mode.ll"; then
    route=llvm-dis-exact-air
  else
    space; step "$mode-inspect.txt" xcrun --sdk macosx metal -x ir -S -emit-llvm -O0 "$flag" "$r/$mode.air" -o "$r/$mode.ll"
  fi
  # Never a separately source-compiled .ll sibling. Failure stops BEFORE any GPU run.
  step "$mode-air-proof.txt" "$out/build/check" air "$r/$mode.ll"
  space; step build.txt xcrun --sdk macosx metallib "$r/$mode.air" -o "$r/$mode.metallib"
  [[ "$before" == "$(hash "$r/$mode.air")" ]]
  printf '%s\t%s\t%s\t%s\t%s\n' "$mode" "$before" "$(hash "$r/$mode.ll")" "$(hash "$r/$mode.metallib")" "$route" >> "$r/bindings.tsv"
done
shasum -a 256 "$r/"*.air "$r/"*.ll "$r/"*.metallib "$r/"*-air-proof.txt "$r/bindings.tsv" "$r/half.bin" "$r/vertices.bin" "$r/metadata.tsv" > "$r/compiler.sha256"
space; step build.txt xcrun --sdk macosx swiftc -swift-version 5 -O "$s/probe.swift" -o "$out/build/probe"
shasum -a 256 -c "$r/source.sha256" >> "$r/build.txt"
shasum -a 256 -c "$r/compiler.sha256" >> "$r/build.txt"
step build.txt "$out/build/probe" "$r"
shasum -a 256 -c "$r/compiler.sha256" >> "$r/build.txt"
step validation.txt "$out/build/check" validate "$r"
shasum -a 256 -c "$r/source.sha256" >> "$r/build.txt"
