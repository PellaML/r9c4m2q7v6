#!/bin/bash
# Existing local toolchains only. No network, installation, dispatch or cleanup.
# One precise build on one explicitly identified existing reference host.
set -euo pipefail
umask 077
export LC_ALL=C
cd "$(dirname "$0")/../.."
s=scripts/r16f-render
out=out-r16f-render
r=$out/results
entries=(triangle emit_f32 emit_f16 observe)
captures=(seed clear clear-zero clear-negative-zero load f32-replace f16-replace f32-none f16-none f32-blend f16-blend f32-red f32-absent negative-skip)
sources=("$s/accept.rs" "$s/air.rs" "$s/check.rs" "$s/probe.metal" "$s/probe.swift" "$s/run.sh")

bounded() {
  [[ -d "$r" && ! -L "$out" && ! -L "$r" ]] || return 1
  [[ -z "$(find "$r" -mindepth 1 ! -type f -print -quit)" ]] || return 1
  local allowed='metadata.tsv toolchain.txt source.sha256 commands.txt build.txt tests.txt generation.txt extraction.txt inspection.tsv bindings.tsv compiler.sha256 gpu.tsv gpu.json source.bin seed.bin plan.json precise.metallib validation.txt observations.tsv negatives.tsv status.txt'
  local e c f n bytes total=0 count=0
  for e in "${entries[@]}"; do allowed+=" $e.air $e.ll $e-proof.txt $e-inspect.txt"; done
  for c in "${captures[@]}"; do allowed+=" $c-raw.bin $c-trace.bin $c-read.bin"; done
  for f in "$r"/*; do
    [[ -f "$f" && ! -L "$f" ]] || return 1
    n=${f##*/}; [[ " $allowed " == *" $n "* ]] || return 1
    bytes=$(wc -c < "$f"); (( bytes <= 1048576 )) || return 1
    total=$((total+bytes)); count=$((count+1))
  done
  (( count <= 79 && total <= 4194304 && count == $(find "$r" -type f | wc -l) ))
}
if [[ $# == 1 && $1 == verify ]]; then
  bounded
  [[ -f "$out/sha256.txt" && ! -L "$out/sha256.txt" ]]
  # Recompute only whitelisted regular files; never trust paths in a manifest.
  [[ "$(cd "$out" && shasum -a 256 results/*)" == "$(cat "$out/sha256.txt")" ]]
  exit 0
fi
[[ $# == 0 && $(uname -s) == Darwin ]] || { echo 'macOS runner, no arguments required' >&2; exit 2; }
[[ ! -e "$out" && ! -L "$out" ]] || { echo 'output must be new; nothing deleted' >&2; exit 2; }
actual=$(sw_vers -buildVersion)
[[ $(uname -m) == arm64 && "$actual" == "${EXPECTED_OS_BUILD:?}" ]]
[[ "${RUNNER_LABEL:?}:$actual" == macos-26:25G332 || "$RUNNER_LABEL:$actual" == xcode-27:26A434 ]]
mkdir -p "$r" "$out/build" "$out/tmp"
export TMPDIR="$PWD/$out/tmp"
finish() {
  local rc=$?
  trap - EXIT
  if (( rc == 0 )); then printf 'BOUNDED_R16F_OBSERVATIONS_PASS\nrun_exit=0\n' > "$r/status.txt"
  else printf 'UNQUALIFIED\nrun_exit=%s\n' "$rc" > "$r/status.txt"; fi
  if bounded; then
    if ! (cd "$out" && shasum -a 256 results/*) > "$out/sha256.txt"; then
      printf 'UNQUALIFIED\nhash_manifest_failed=1\n' > "$r/status.txt"; rc=3
    fi
  else
    printf 'UNQUALIFIED\nartifact_bounds_failed=1\n' > "$r/status.txt"; rc=3
  fi
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
  bounded
}
space() {
  local available; available=$(df -Pk . | awk 'END {print $4}')
  (( available >= 2097152 )) || { echo 'insufficient free space; no new build' >&2; exit 3; }
}
printf 'runner\t%s\nos_build\t%s\narchitecture\tarm64\nmode\tprecise\n' "$RUNNER_LABEL" "$actual" > "$r/metadata.tsv"
step toolchain.txt sw_vers
step toolchain.txt xcodebuild -version
step toolchain.txt xcrun --sdk macosx --show-sdk-version
step toolchain.txt xcrun --sdk macosx --show-sdk-build-version
step toolchain.txt xcrun --sdk macosx metal --version
step toolchain.txt xcrun --sdk macosx swiftc --version
step toolchain.txt rustc -vV
shasum -a 256 "${sources[@]}" > "$r/source.sha256"
space; step build.txt rustc --edition=2021 -D warnings --crate-name r16f_accept --crate-type rlib "$s/accept.rs" -o "$out/build/libr16f_accept.rlib"
space; step build.txt rustc --edition=2021 -D warnings --test "$s/accept.rs" -o "$out/build/accept-tests"
space; step build.txt rustc --edition=2021 -D warnings --test "$s/check.rs" --extern "r16f_accept=$out/build/libr16f_accept.rlib" -o "$out/build/check-tests"
step tests.txt "$out/build/accept-tests"
step tests.txt "$out/build/check-tests"
space; step build.txt rustc --edition=2021 -D warnings "$s/check.rs" --extern "r16f_accept=$out/build/libr16f_accept.rlib" -o "$out/build/check"
step generation.txt "$out/build/check" generate "$r"

# The source-compiled intermediate stays in build/. Evidence comes ONLY from
# the linked metallib's checked module ranges, each with its embedded HASH tag.
space; step build.txt xcrun --sdk macosx metal -std=metal4.0 -O2 -g0 -fno-fast-math -c "$s/probe.metal" -o "$out/build/precise.air"
space; step build.txt xcrun --sdk macosx metallib "$out/build/precise.air" -o "$r/precise.metallib"
step extraction.txt "$out/build/check" extract "$r"
printf 'entry\troute\n' > "$r/inspection.tsv"
for e in "${entries[@]}"; do
  # Select an available reader, not a retry after a reader/compiler failure.
  if xcrun --sdk macosx --find llvm-dis >/dev/null 2>&1; then
    step "$e-inspect.txt" xcrun --sdk macosx llvm-dis "$r/$e.air" -o "$r/$e.ll"
    route=llvm-dis-extracted-air
  else
    space; step "$e-inspect.txt" xcrun --sdk macosx metal -x ir -S -emit-llvm -O0 -fno-fast-math "$r/$e.air" -o "$r/$e.ll"
    route=metal-ir-reader-extracted-air
  fi
  printf '%s\t%s\n' "$e" "$route" >> "$r/inspection.tsv"
done
step build.txt "$out/build/check" bind "$r"
shasum -a 256 "$r/precise.metallib" "$r/"*.air "$r/"*.ll "$r/"*-proof.txt "$r/bindings.tsv" "$r/inspection.tsv" "$r/source.bin" "$r/seed.bin" "$r/plan.json" "$r/metadata.tsv" > "$r/compiler.sha256"
space; step build.txt xcrun --sdk macosx swiftc -swift-version 5 -O "$s/probe.swift" -o "$out/build/probe"
shasum -a 256 -c "$r/source.sha256" >> "$r/build.txt"
shasum -a 256 -c "$r/compiler.sha256" >> "$r/build.txt"
step build.txt "$out/build/probe" "$r"
shasum -a 256 -c "$r/compiler.sha256" >> "$r/build.txt"
step validation.txt "$out/build/check" validate "$r"
shasum -a 256 -c "$r/source.sha256" >> "$r/build.txt"
