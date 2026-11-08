#!/bin/bash
# Local toolchains only. No downloads, package installs, or remote operations.
set -uo pipefail
umask 077
[[ $# -eq 0 ]] || { echo 'expected no arguments' >&2; exit 2; }
out=out-rg16add
[[ ! -e "$out" && ! -L "$out" ]] || { echo 'out-rg16add must be new' >&2; exit 2; }
mkdir -p "$out/results" || exit 2
here="$(cd "$(dirname "$0")" && pwd)" || exit 2

metadata=99; source_hashes=99; rust_build=99; test_build=99; tests=99
generation=99; swift_build=99; probe=99; validation=99; packaging=99; truncated_logs=0
printf 'schema=1\nstarted_utc=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" > "$out/run-receipt.txt"
: > "$out/source-sha256.txt"

write_exit() {
  printf '{"schema":1,"metadata_exit":%s,"source_hash_exit":%s,"rust_build_exit":%s,"test_build_exit":%s,"tests_exit":%s,"generation_exit":%s,"swift_build_exit":%s,"probe_exit":%s,"validation_exit":%s,"packaging_exit":%s,"run_exit":%s,"not_run_sentinel":99,"truncated_logs":%s}\n' \
    "$metadata" "$source_hashes" "$rust_build" "$test_build" "$tests" "$generation" \
    "$swift_build" "$probe" "$validation" "$packaging" "$1" "$truncated_logs" > "$out/command-exit.json"
}

hash_outputs() {
  (cd "$out" && shasum -a 256 command-exit.json run-receipt.txt source-sha256.txt \
    results/*.json results/*.bin results/*.tsv results/*.txt) > "$out/sha256.txt"
}

bounded() {
  local file size actual total=0 count=0
  [[ -d "$out/results" && ! -L "$out" && ! -L "$out/results" ]] || return 1
  [[ -z "$(find "$out" ! -type f ! -type d -print -quit)" ]] || return 1
  [[ -z "$(find "$out" -mindepth 1 -type d ! -path "$out/results" -print -quit)" ]] || return 1
  local files=("$out/command-exit.json" "$out/run-receipt.txt" "$out/source-sha256.txt" "$out/sha256.txt"
               "$out/results/"*.json "$out/results/"*.bin "$out/results/"*.tsv "$out/results/"*.txt)
  for file in "${files[@]}"; do
    [[ -f "$file" && ! -L "$file" ]] || return 1
    size=$(wc -c < "$file") || return 1
    (( size <= 1048576 )) || return 1
    total=$((total + size)); count=$((count + 1))
  done
  actual=$(find "$out" -type f | wc -l) || return 1
  (( count == actual && count >= 6 && count <= 64 && total <= 4194304 ))
}

finish() {
  local rc="$1" file size
  trap - EXIT
  shopt -s nullglob
  # Exact temporary executables, all inside the fresh output directory.
  rm -f "$out/.cases" "$out/.tests" "$out/.probe"
  for file in "$out/results/"*.txt; do
    size=$(wc -c < "$file") || { rc=3; continue; }
    if (( size > 65536 )); then
      head -c 65536 "$file" > "$out/.log-slice"
      printf '\n[log truncated at 65536 bytes]\n' >> "$out/.log-slice"
      mv "$out/.log-slice" "$file"
      truncated_logs=$((truncated_logs + 1))
    fi
  done
  if [[ ! -f "$out/results/gpu.json" ]]; then
    printf '{"schema":1,"status":"FAILED_NOT_PASS","reason":"native capture did not produce a summary"}\n' > "$out/results/gpu.json"
  fi
  if [[ ! -f "$out/results/validation.json" ]]; then
    printf '{"schema":1,"status":"NOT_QUALIFIED","reason":"ordinary validation did not complete"}\n' > "$out/results/validation.json"
  fi
  printf 'finished_utc=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >> "$out/run-receipt.txt"
  packaging=0
  write_exit "$rc"
  if ! hash_outputs || ! bounded; then
    packaging=3; rc=3
    write_exit "$rc"
    hash_outputs || true
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
  "$@" >> "$out/run-receipt.txt" 2>&1
  rc=$?
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
record swift xcrun swiftc --version
record rust rustc -vV
(( metadata == 0 )) || exit 3

(cd "$here/../.." && shasum -a 256 scripts/rg16add/cases.rs scripts/rg16add/probe.swift \
  scripts/rg16add/probe.metal scripts/rg16add/run.sh \
  .github/workflows/rg16add.yml) > "$out/source-sha256.txt" 2> "$out/results/source-hash.txt"
source_hashes=$?
(( source_hashes == 0 )) || exit 3

rustc --edition=2021 -D warnings -O "$here/cases.rs" -o "$out/.cases" > "$out/results/build-rust.txt" 2>&1
rust_build=$?
(( rust_build == 0 )) || exit 3
rustc --edition=2021 -D warnings --test "$here/cases.rs" -o "$out/.tests" > "$out/results/build-tests.txt" 2>&1
test_build=$?
(( test_build == 0 )) || exit 3
"$out/.tests" > "$out/results/tests.txt" 2>&1
tests=$?
(( tests == 0 )) || exit 3
"$out/.cases" generate "$out/results" > "$out/results/generation.txt" 2>&1
generation=$?
(( generation == 0 )) || exit 3

xcrun --sdk macosx swiftc -swift-version 5 -O "$here/probe.swift" -o "$out/.probe" > "$out/results/build-swift.txt" 2>&1
swift_build=$?
(( swift_build == 0 )) || exit 3
"$out/.probe" "$here" "$out/results" > "$out/results/stdout.txt" 2> "$out/results/stderr.txt"
probe=$?
(( probe == 0 )) || exit 3
"$out/.cases" validate "$out/results" > "$out/results/validator.txt" 2>&1
validation=$?
(( validation == 0 )) || exit 3
exit 0
