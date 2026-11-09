#!/bin/bash
# Existing local tools only. Precise+fast are BOTH mandatory on each selected
# reference host. Scheduling the two hosts belongs to the caller, not this file.
set -euo pipefail
umask 077
export LC_ALL=C
cd "$(dirname "$0")/../.."
s=scripts/r16f-render
out=out-r16f-render
r=$out/results
modes=(precise fast)
entries=(triangle emit_f32 emit_f16 observe arithmetic)
captures=(seed clear clear-zero clear-negative-zero load f32-replace f16-replace f32-none f16-none f32-blend f16-blend f32-red f32-absent negative-skip clear-tie clear-tie-below clear-tie-above clear-subnormal clear-underflow clear-overflow clear-odd-tie)
sources=("$s/accept.rs" "$s/air.rs" "$s/check.rs" "$s/probe.metal" "$s/probe.swift" "$s/run.sh")

bounded() {
  [[ -d "$r" && ! -L "$out" && ! -L "$r" ]] || return 1
  [[ -d "$r/precise" && -d "$r/fast" && ! -L "$r/precise" && ! -L "$r/fast" ]] || return 1
  [[ -z "$(find "$r" -mindepth 1 ! -type f ! -type d -print -quit)" ]] || return 1
  (( $(find "$r" -mindepth 1 -type d | wc -l) == 2 )) || return 1
  local allowed='toolchain.txt source.sha256 commands.txt build.txt tests.txt compiler.sha256 status.txt mode-differences.tsv math-mode-differences.tsv compare.txt'
  local common='metadata.tsv generation.txt extraction.txt inspection.tsv bindings.tsv gpu.tsv gpu.json source.bin seed.bin plan.json selection.tsv scope.tsv program.metallib validation.txt observations.tsv negatives.tsv hypotheses.tsv model-matches.tsv clear-observations.tsv native-math.tsv source-precision.tsv math.tsv math-f32.bin math-f16.bin'
  local mode e c f n bytes total=0 count=0
  for mode in "${modes[@]}"; do
    for n in $common; do allowed+=" $mode/$n"; done
    for e in "${entries[@]}"; do allowed+=" $mode/$e.air $mode/$e.ll $mode/$e-proof.txt $mode/$e-inspect.txt"; done
    for c in "${captures[@]}"; do allowed+=" $mode/$c-raw.bin $mode/$c-trace.bin $mode/$c-read.bin"; done
  done
  for f in "$r"/* "$r/precise"/* "$r/fast"/*; do
    [[ -e "$f" ]] || continue
    if [[ -d "$f" ]]; then [[ "$f" == "$r/precise" || "$f" == "$r/fast" ]] || return 1; continue; fi
    [[ -f "$f" && ! -L "$f" ]] || return 1
    n=${f#"$r"/}; [[ " $allowed " == *" $n "* ]] || return 1
    bytes=$(wc -c < "$f"); (( bytes <= 1048576 )) || return 1
    total=$((total+bytes)); count=$((count+1))
  done
  (( count <= 224 && total <= 4194304 && count == $(find "$r" -type f | wc -l) ))
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
  if (( rc == 0 )); then printf 'CAPTURE_INTEGRITY_PASS_MATH_PROFILE_UNSELECTED\nrun_exit=0\n' > "$r/status.txt"
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
space() {
  local available; available=$(df -Pk . | awk 'END {print $4}')
  (( available >= 2097152 )) || { echo 'insufficient free space; no new build' >&2; exit 3; }
}
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
: > "$r/compiler.sha256"
# Prove both linked binaries before any GPU command. program.metallib is neutral;
# no fast binary is ever renamed to, or reported as, precise.
for mode in "${modes[@]}"; do
  m=$r/$mode
  flag=-fno-fast-math; [[ $mode == precise ]] || flag=-ffast-math
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
      space; step "$mode/$e-inspect.txt" xcrun --sdk macosx metal -x ir -S -emit-llvm -O0 "$flag" "$m/$e.air" -o "$m/$e.ll"
      route=metal-ir-reader-extracted-air
    fi
    printf '%s\t%s\n' "$e" "$route" >> "$m/inspection.tsv"
  done
  step build.txt "$out/build/check" bind "$m"
  shasum -a 256 "$m/program.metallib" "$m/"*.air "$m/"*.ll "$m/"*-proof.txt "$m/bindings.tsv" "$m/inspection.tsv" "$m/source.bin" "$m/seed.bin" "$m/plan.json" "$m/selection.tsv" "$m/scope.tsv" "$m/metadata.tsv" >> "$r/compiler.sha256"
done
space; step build.txt xcrun --sdk macosx swiftc -swift-version 5 -O "$s/probe.swift" -o "$out/build/probe"
shasum -a 256 -c "$r/source.sha256" >> "$r/build.txt"
shasum -a 256 -c "$r/compiler.sha256" >> "$r/build.txt"
# Capture both modes before any numerical observations can fail a later gate.
for mode in "${modes[@]}"; do step build.txt "$out/build/probe" "$r/$mode" "$mode"; done
shasum -a 256 -c "$r/compiler.sha256" >> "$r/build.txt"
rc=0
for mode in "${modes[@]}"; do
  if step "$mode/validation.txt" "$out/build/check" validate "$r/$mode"; then :; else rc=3; fi
done
if step compare.txt "$out/build/check" compare "$r"; then :; else rc=3; fi
shasum -a 256 -c "$r/source.sha256" >> "$r/build.txt"
exit "$rc"
