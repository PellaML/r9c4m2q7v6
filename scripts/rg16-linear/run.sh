#!/bin/bash
# Generic owned corpus only. Caller owns review/publication/scheduling.
set -euo pipefail
umask 077
export LC_ALL=C
cd "$(dirname "$0")/../.."
s=scripts/rg16-linear
out=out-rg16-linear
r=$out/results
modes=(precise fast)
entries=(triangle sample_f32 sample_f16 read_exact)
sources=("$s/air.rs" "$s/probe.rs" "$s/probe.metal" "$s/probe.swift" "$s/run.sh" .github/workflows/rg16-linear.yml)
bounded() {
  [[ -d "$r" && ! -L "$out" && ! -L "$r" ]] || return 1
  [[ -d "$r/precise" && -d "$r/fast" && ! -L "$r/precise" && ! -L "$r/fast" ]] || return 1
  [[ -z "$(find "$r" -mindepth 1 ! -type f ! -type d -print -quit)" ]] || return 1
  (( $(find "$r" -mindepth 1 -type d | wc -l) == 2 )) || return 1
  local allowed='toolchain.txt source.sha256 commands.txt build.txt tests.txt compiler.sha256 status.txt mode-comparison.txt compare.txt'
  local common='metadata.tsv generation.txt extraction.txt inspection.tsv bindings.tsv gpu.tsv gpu.json plan.json corpus.tsv candidate-preflight.tsv coordinates-0.bin coordinates-1.bin program.metallib negative-skip.bin validation.txt observations.tsv candidate-matches.tsv format-comparison.tsv return-precision.tsv negatives.tsv acceptance.txt'
  local mode e c norm bits f n bytes total=0 count=0
  for mode in "${modes[@]}"; do
    for n in $common; do allowed+=" $mode/$n"; done
    for e in "${entries[@]}"; do allowed+=" $mode/$e.air $mode/$e.ll $mode/$e-proof.txt $mode/$e-inspect.txt"; done
    for c in {0..7}; do
      allowed+=" $mode/c$c-rg-upload.bin $mode/c$c-control-upload.bin $mode/c$c-rg-copy.bin $mode/c$c-control-copy.bin $mode/c$c-read.bin $mode/c$c-red-upload.bin $mode/c$c-red-copy.bin"
      for norm in 0 1; do for bits in 16 32; do allowed+=" $mode/c$c-s$norm-f$bits.bin"; done; done
    done
  done
  for f in "$r"/* "$r/precise"/* "$r/fast"/*; do
    [[ -e "$f" ]] || continue
    if [[ -d "$f" ]]; then [[ "$f" == "$r/precise" || "$f" == "$r/fast" ]] || return 1; continue; fi
    [[ -f "$f" && ! -L "$f" ]] || return 1
    n=${f#"$r"/}; [[ " $allowed " == *" $n "* ]] || return 1
    bytes=$(wc -c < "$f"); (( bytes <= 2097152 )) || return 1
    total=$((total+bytes)); count=$((count+1))
  done
  (( count <= 288 && total <= 8388608 && count == $(find "$r" -type f | wc -l) ))
}
result_hashes() {
  (cd "$out"; files=(); for f in results/* results/precise/* results/fast/*; do
    if [[ -f "$f" && ! -L "$f" ]]; then files+=("$f"); fi
  done; shasum -a 256 "${files[@]}")
}
if [[ $# == 1 && $1 == verify ]]; then
  bounded
  [[ -f "$out/sha256.txt" && ! -L "$out/sha256.txt" ]]
  [[ "$(result_hashes)" == "$(cat "$out/sha256.txt")" ]]
  exit 0
fi
[[ $# == 0 && $(uname -s) == Darwin ]] || { echo 'macOS runner, no arguments required' >&2; exit 2; }
[[ ! -e "$out" && ! -L "$out" ]] || { echo 'output must be new; nothing deleted' >&2; exit 2; }
actual=$(sw_vers -buildVersion)
[[ $(uname -m) == arm64 && "$actual" == "${EXPECTED_OS_BUILD:?}" ]]
[[ "${RUNNER_LABEL:?}:$actual" == macos-26:25G332 || "$RUNNER_LABEL:$actual" == xcode-27:26A434 ]]
mkdir -p "$r/precise" "$r/fast" "$out/build" "$out/tmp"
export TMPDIR="$PWD/$out/tmp"
finish() {
  local rc=$?; trap - EXIT
  if (( rc == 0 )); then printf 'CAPTURE_INTEGRITY_PASS_PROFILE_UNSELECTED\nrun_exit=0\n' > "$r/status.txt"
  else printf 'UNQUALIFIED_CAPTURE_OR_GATE_FAILURE\nrun_exit=%s\n' "$rc" > "$r/status.txt"; fi
  if bounded; then
    if ! result_hashes > "$out/sha256.txt"; then printf 'UNQUALIFIED_HASH_FAILURE\n' > "$r/status.txt"; rc=3; fi
  else printf 'UNQUALIFIED_ARTIFACT_BOUNDS\n' > "$r/status.txt"; rc=3; fi
  exit "$rc"
}
trap finish EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
: > "$r/commands.txt"; : > "$r/build.txt"
step() {
  local log=$1 rc=0; shift
  printf '%q ' "$@" >> "$r/commands.txt"; printf '\n' >> "$r/commands.txt"
  "$@" >> "$r/$log" 2>&1 || rc=$?
  bounded || return 3
  return "$rc"
}
space() { local available; available=$(df -Pk . | awk 'END {print $4}'); (( available >= 2097152 )) || { echo 'insufficient free space' >&2; exit 3; }; }
step toolchain.txt sw_vers
step toolchain.txt xcodebuild -version
step toolchain.txt xcrun --sdk macosx --show-sdk-version
step toolchain.txt xcrun --sdk macosx --show-sdk-build-version
step toolchain.txt xcrun --sdk macosx metal --version
step toolchain.txt xcrun --sdk macosx swiftc --version
step toolchain.txt rustc -vV
shasum -a 256 "${sources[@]}" > "$r/source.sha256"
space; step build.txt rustc --edition=2021 -O -D warnings --test "$s/probe.rs" -o "$out/build/tests"
step tests.txt "$out/build/tests"
space; step build.txt rustc --edition=2021 -O -D warnings "$s/probe.rs" -o "$out/build/check"
: > "$r/compiler.sha256"
# Compile/link/extract/inspect BOTH binaries before any native GPU command.
for mode in "${modes[@]}"; do
  m=$r/$mode; flag=-fno-fast-math; [[ $mode == precise ]] || flag=-ffast-math
  printf 'runner\t%s\nos_build\t%s\narchitecture\tarm64\nmode\t%s\n' "$RUNNER_LABEL" "$actual" "$mode" > "$m/metadata.tsv"
  step "$mode/generation.txt" "$out/build/check" generate "$m"
  space; step build.txt xcrun --sdk macosx metal -std=metal4.0 -O2 -g0 "$flag" -c "$s/probe.metal" -o "$out/build/$mode.air"
  space; step build.txt xcrun --sdk macosx metallib "$out/build/$mode.air" -o "$m/program.metallib"
  step "$mode/extraction.txt" "$out/build/check" extract "$m"
  printf 'entry\troute\n' > "$m/inspection.tsv"
  for e in "${entries[@]}"; do
    if xcrun --sdk macosx --find llvm-dis >/dev/null 2>&1; then
      step "$mode/$e-inspect.txt" xcrun --sdk macosx llvm-dis "$m/$e.air" -o "$m/$e.ll"
      route=llvm-dis-extracted-air
    else
      step "$mode/$e-inspect.txt" xcrun --sdk macosx metal -x ir -S -emit-llvm -O0 "$flag" "$m/$e.air" -o "$m/$e.ll"
      route=metal-ir-reader-extracted-air
    fi
    printf '%s\t%s\n' "$e" "$route" >> "$m/inspection.tsv"
  done
  step build.txt "$out/build/check" bind "$m"
  shasum -a 256 "$m/program.metallib" "$m/"*.air "$m/"*.ll "$m/"*-proof.txt "$m/bindings.tsv" "$m/inspection.tsv" "$m/"*-upload.bin "$m/coordinates-0.bin" "$m/coordinates-1.bin" "$m/plan.json" "$m/corpus.tsv" "$m/candidate-preflight.tsv" "$m/metadata.tsv" >> "$r/compiler.sha256"
done
space; step build.txt xcrun --sdk macosx swiftc -swift-version 5 -O "$s/probe.swift" -o "$out/build/probe"
shasum -a 256 -c "$r/source.sha256" >> "$r/build.txt"
shasum -a 256 -c "$r/compiler.sha256" >> "$r/build.txt"
# Retain both modes before host observation/model classification.
for mode in "${modes[@]}"; do step build.txt "$out/build/probe" "$r/$mode" "$mode"; done
shasum -a 256 -c "$r/compiler.sha256" >> "$r/build.txt"
rc=0
for mode in "${modes[@]}"; do if step "$mode/validation.txt" "$out/build/check" validate "$r/$mode"; then :; else rc=3; fi; done
if step compare.txt "$out/build/check" compare "$r"; then :; else rc=3; fi
shasum -a 256 -c "$r/source.sha256" >> "$r/build.txt"
exit "$rc"
