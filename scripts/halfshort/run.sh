#!/bin/bash
# Existing local toolchains only. No installation, network, or dynamic-library fallback.
set -uo pipefail
umask 077
[[ $# -eq 0 ]] || { echo 'expected no arguments' >&2; exit 2; }
out=out-halfshort
[[ ! -e "$out" && ! -L "$out" ]] || { echo 'out-halfshort must be new' >&2; exit 2; }
mkdir -p "$out/results" || exit 2
here="$(cd "$(dirname "$0")" && pwd)" || exit 2
results="$(cd "$out/results" && pwd)" || exit 2
metadata=99; source_hashes=99; rust_build=99; test_build=99; tests=99; generation=99
swift_build=99; probe=99; validation=99; packaging=99; truncated_logs=0
compile_nofast=99; inspect_nofast=99; air_nofast=99; link_nofast=99
compile_fast=99; inspect_fast=99; air_fast=99; link_fast=99
printf 'schema=1\nstarted_utc=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" > "$out/run-receipt.txt"
: > "$out/source-sha256.txt"

write_exit() {
  printf '{"schema":1,"metadata_exit":%s,"source_hash_exit":%s,"rust_build_exit":%s,"test_build_exit":%s,"tests_exit":%s,"generation_exit":%s,"compile_nofast_exit":%s,"inspect_nofast_exit":%s,"air_nofast_exit":%s,"link_nofast_exit":%s,"compile_fast_exit":%s,"inspect_fast_exit":%s,"air_fast_exit":%s,"link_fast_exit":%s,"swift_build_exit":%s,"probe_exit":%s,"validation_exit":%s,"packaging_exit":%s,"run_exit":%s,"not_run_sentinel":99,"truncated_logs":%s}\n' \
    "$metadata" "$source_hashes" "$rust_build" "$test_build" "$tests" "$generation" \
    "$compile_nofast" "$inspect_nofast" "$air_nofast" "$link_nofast" \
    "$compile_fast" "$inspect_fast" "$air_fast" "$link_fast" \
    "$swift_build" "$probe" "$validation" "$packaging" "$1" "$truncated_logs" > "$out/command-exit.json"
}
hash_outputs() {
  (cd "$out" && shasum -a 256 command-exit.json run-receipt.txt source-sha256.txt \
    results/*.json results/*.bin results/*.tsv results/*.txt results/*.air results/*.ll results/*.metallib) > "$out/sha256.txt"
}
bounded() {
  local f size actual total=0 count=0
  [[ -d "$out/results" && ! -L "$out" && ! -L "$out/results" ]] || return 1
  [[ -z "$(find "$out" ! -type f ! -type d -print -quit)" ]] || return 1
  [[ -z "$(find "$out" -mindepth 1 -type d ! -path "$out/results" -print -quit)" ]] || return 1
  local files=("$out/command-exit.json" "$out/run-receipt.txt" "$out/source-sha256.txt" "$out/sha256.txt"
    "$out/results/"*.json "$out/results/"*.bin "$out/results/"*.tsv "$out/results/"*.txt
    "$out/results/"*.air "$out/results/"*.ll "$out/results/"*.metallib)
  for f in "${files[@]}"; do
    [[ -f "$f" && ! -L "$f" ]] || return 1
    size=$(wc -c < "$f") || return 1
    (( size <= 1048576 )) || return 1
    total=$((total+size)); count=$((count+1))
  done
  actual=$(find "$out" -type f | wc -l) || return 1
  (( count == actual && count >= 6 && count <= 64 && total <= 8388608 ))
}
finish() {
  local rc="$1" f size qualified=false status=UNQUALIFIED
  trap - EXIT
  shopt -s nullglob
  rm -f "$out/.cases" "$out/.tests" "$out/.probe"
  for f in "$out/results/"*.txt; do
    size=$(wc -c < "$f") || { rc=3; continue; }
    if (( size > 65536 )); then
      head -c 65536 "$f" > "$out/.log-slice"
      printf '\n[log truncated at 65536 bytes]\n' >> "$out/.log-slice"
      mv "$out/.log-slice" "$f"; truncated_logs=$((truncated_logs+1))
    fi
  done
  [[ -f "$out/results/gpu.json" ]] || printf '{"schema":1,"status":"UNQUALIFIED_NOT_COMPLETE","cpu_fallback":false}\n' > "$out/results/gpu.json"
  [[ -f "$out/results/validation.json" ]] || printf '{"schema":1,"status":"UNQUALIFIED_NOT_COMPLETE"}\n' > "$out/results/validation.json"
  if (( rc == 0 && air_nofast == 0 && air_fast == 0 && validation == 0 && probe == 0 )); then
    qualified=true; status=QUALIFIED_OFFLINE_OBSERVATIONS
  fi
  printf '{"schema":1,"qualified":%s,"status":"%s","scope":"exact retained offline AIR and metallib, typed live conversion calls, native coverage/guards/negative controls; no blanket out-of-range or NaN language contract","dynamic_source_fallback":false}\n' "$qualified" "$status" > "$out/results/qualification.json"
  printf 'finished_utc=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >> "$out/run-receipt.txt"
  packaging=0; write_exit "$rc"
  if ! hash_outputs || ! bounded; then
    packaging=3; rc=3
    printf '{"schema":1,"qualified":false,"status":"UNQUALIFIED_ARTIFACT_BOUNDS_OR_HASH_FAILURE"}\n' > "$out/results/qualification.json"
    write_exit "$rc"; hash_outputs || true
  fi
  cat "$out/command-exit.json"
  exit "$rc"
}
trap 'finish "$?"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

record() {
  local label="$1" rc
  shift
  printf '\n[%s]\n' "$label" >> "$out/run-receipt.txt"
  "$@" >> "$out/run-receipt.txt" 2>&1; rc=$?
  printf 'command_exit=%s\n' "$rc" >> "$out/run-receipt.txt"
  if (( rc != 0 )); then metadata=3; fi
}
[[ $(uname -s) == Darwin ]] || { metadata=3; exit 3; }
metadata=0
record os sw_vers
record architecture uname -m
record xcode xcodebuild -version
record sdk-version xcrun --sdk macosx --show-sdk-version
record sdk-build xcrun --sdk macosx --show-sdk-build-version
record metal xcrun --sdk macosx metal --version
record swift xcrun --sdk macosx swiftc --version
record rust rustc -vV
(( metadata == 0 )) || exit 3
(cd "$here/../.." && shasum -a 256 scripts/halfshort/cases.rs scripts/halfshort/probe.metal \
  scripts/halfshort/probe.swift scripts/halfshort/run.sh .github/workflows/halfshort.yml) > "$out/source-sha256.txt" 2> "$out/results/source-hash.txt"
source_hashes=$?; (( source_hashes == 0 )) || exit 3
rustc --edition=2021 -D warnings -O "$here/cases.rs" -o "$out/.cases" > "$out/results/build-rust.txt" 2>&1
rust_build=$?; (( rust_build == 0 )) || exit 3
rustc --edition=2021 -D warnings --test "$here/cases.rs" -o "$out/.tests" > "$out/results/build-tests.txt" 2>&1
test_build=$?; (( test_build == 0 )) || exit 3
"$out/.tests" > "$out/results/tests.txt" 2>&1
tests=$?; (( tests == 0 )) || exit 3
"$out/.cases" generate "$out/results" > "$out/results/generation.txt" 2>&1
generation=$?; (( generation == 0 )) || exit 3

offline() {
  local mode="$1" flag="$2" fast="$3" compile inspect=99 proof=99 link=99 route=none before after
  local ir_hash=unavailable proof_hash=unavailable library_hash=unavailable
  # The AIR compiled here is the very AIR passed to metallib below. IR inspection
  # is from that binary, NEVER a separate source-to-IR sibling compilation.
  (cd "$here" && xcrun --sdk macosx metal -std=metal4.0 -O2 -g0 "$flag" -c probe.metal -o "$results/$mode.air") > "$out/results/compile-$mode.txt" 2>&1
  compile=$?
  if (( compile == 0 )); then
    before=$(shasum -a 256 "$results/$mode.air" | cut -d' ' -f1) || return 3
    : > "$out/results/inspect-$mode.txt"
    if xcrun --sdk macosx --find llvm-dis >> "$out/results/inspect-$mode.txt" 2>&1; then
      xcrun --sdk macosx llvm-dis "$results/$mode.air" -o "$results/$mode.ll" >> "$out/results/inspect-$mode.txt" 2>&1
      inspect=$?
      printf 'llvm_dis_exit=%s\n' "$inspect" >> "$out/results/inspect-$mode.txt"
      if (( inspect == 0 )); then route=llvm-dis-exact-air; fi
    fi
    if (( inspect != 0 )); then
      # Existing Metal compiler's IR reader; -O0 re-emission, not hardware ISA
      # disassembly. Retain the route and both original AIR and inspected text.
      xcrun --sdk macosx metal -x ir -S -emit-llvm -O0 "$flag" "$results/$mode.air" -o "$results/$mode.ll" >> "$out/results/inspect-$mode.txt" 2>&1
      inspect=$?
      printf 'metal_ir_reemit_exit=%s\n' "$inspect" >> "$out/results/inspect-$mode.txt"
      if (( inspect == 0 )); then route=metal-ir-reemit-O0-from-exact-air; fi
    fi
    if (( inspect == 0 )); then
      ir_hash=$(shasum -a 256 "$results/$mode.ll" | cut -d' ' -f1) || return 3
      "$out/.cases" verify-air "$results/$mode.ll" "$results/$mode-air-proof.json" > "$out/results/air-check-$mode.txt" 2>&1
      proof=$?
      if [[ -f "$results/$mode-air-proof.json" ]]; then
        proof_hash=$(shasum -a 256 "$results/$mode-air-proof.json" | cut -d' ' -f1) || return 3
      fi
      # Even a signature-unqualified library may be captured for diagnostics;
      # final qualification stays false. There is no source-library fallback.
      xcrun --sdk macosx metallib "$results/$mode.air" -o "$results/$mode.metallib" > "$out/results/link-$mode.txt" 2>&1
      link=$?
      if (( link == 0 )); then
        library_hash=$(shasum -a 256 "$results/$mode.metallib" | cut -d' ' -f1) || return 3
      fi
    fi
    after=$(shasum -a 256 "$results/$mode.air" | cut -d' ' -f1) || return 3
    [[ "$before" == "$after" ]] || { echo 'AIR changed during inspection/link' >&2; return 3; }
  else
    before=unavailable
  fi
  printf '{"schema":1,"mode":"%s","fast_math_requested":%s,"language":"metal4.0","compile_flags":["-std=metal4.0","-O2","-g0","%s","-c"],"compile_exit":%s,"inspection_route":"%s","inspection_math_flag":"%s","inspect_exit":%s,"air_proof_exit":%s,"link_exit":%s,"air_sha256":"%s","ir_sha256":"%s","proof_sha256":"%s","library_sha256":"%s","link_input":"%s.air","runtime_library":"%s.metallib","sibling_source_ir_used":false}\n' \
    "$mode" "$fast" "$flag" "$compile" "$route" "$flag" "$inspect" "$proof" "$link" "$before" "$ir_hash" "$proof_hash" "$library_hash" "$mode" "$mode" > "$out/results/$mode-compile.json"
  if [[ "$mode" == nofast ]]; then
    compile_nofast=$compile; inspect_nofast=$inspect; air_nofast=$proof; link_nofast=$link
  else
    compile_fast=$compile; inspect_fast=$inspect; air_fast=$proof; link_fast=$link
  fi
  (( compile == 0 && inspect == 0 && link == 0 ))
}
offline nofast -fno-fast-math false || exit 3
offline fast -ffast-math true || exit 3
xcrun --sdk macosx swiftc -swift-version 5 -O "$here/probe.swift" -o "$out/.probe" > "$out/results/build-swift.txt" 2>&1
swift_build=$?; (( swift_build == 0 )) || exit 3
"$out/.probe" "$here" "$out/results" > "$out/results/stdout.txt" 2> "$out/results/stderr.txt"
probe=$?; (( probe == 0 )) || exit 3
"$out/.cases" validate "$out/results" > "$out/results/validator.txt" 2>&1
validation=$?; (( validation == 0 )) || exit 3
(( air_nofast == 0 && air_fast == 0 )) || exit 3
exit 0
