#!/usr/bin/env bash
# Manual, source-bound, single finite batch. No installs, downloads, retries or cleanup.
set -euo pipefail
umask 077
if [[ "${1:-}" == --seal-artifacts ]]; then
  out="${2:?output root}"
  [[ $# == 2 && "$out" =~ ^[a-zA-Z0-9_-]+$ && ! -L "$out" ]] || exit 64
  "$out/build/check" bounds "$out/results"
  exec "$out/build/check" seal "$out/results"
fi
[[ $# == 1 && "$1" =~ ^[a-zA-Z0-9_-]+$ && ! -e "$1" && ! -L "$1" ]] || exit 64
[[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]] || exit 64
[[ "${GITHUB_EVENT_NAME:-}" == workflow_dispatch && "${GITHUB_REPOSITORY:-}" == PellaML/r9c4m2q7v6 &&
   "${GITHUB_REF:-}" == refs/heads/research/probe && "${GITHUB_RUN_ATTEMPT:-}" == 1 &&
   "${EXPECTED_HEAD:-}" =~ ^[0-9a-f]{40}$ && "${GITHUB_SHA:-}" == "$EXPECTED_HEAD" &&
   "$(git rev-parse HEAD)" == "$EXPECTED_HEAD" ]] || exit 64
[[ -z "$(git status --porcelain)" ]] || exit 64
root="$PWD"; out="$1"; here=scripts/fetch-edge
mkdir -p "$out/build" "$out/results"
phase=build
finish() {
  rc=$?; trap - EXIT
  if [[ -f "$out/build/native.log" ]]; then tail -c 8192 "$out/build/native.log"; fi
  if [[ "$rc" != 0 && "$phase" == build && -f "$out/build/native-build.log" ]]; then
    tail -c 8192 "$out/build/native-build.log" >&2
  fi
  printf '{"exit":%s,"phase":"%s"}\n' "$rc" "$phase" > "$out/results/command-exit.json"
  exit "$rc"
}
trap finish EXIT
rustc --edition=2021 -Dwarnings -O -C debuginfo=0 --remap-path-prefix "$root=/probe" \
  "$here/half-check.rs" -o "$out/build/check" > "$out/build/rust.log" 2>&1
rustc --edition=2021 --test -Dwarnings -O -C debuginfo=0 --remap-path-prefix "$root=/probe" \
  "$here/half-check.rs" -o "$out/build/tests" > "$out/build/test-build.log" 2>&1
"$out/build/tests" --test-threads=1 > "$out/build/tests.log"
"$out/build/check" bind "$out/results" "$EXPECTED_HEAD" "$root"
"$out/build/check" prepare "$out/results"
# Existing public Swift platform shim retained; Rust is dependency-free.
xcrun --sdk macosx swiftc -swift-version 5 -O -target arm64-apple-macos26.0 \
  "$here/half.swift" -o "$out/build/native" > "$out/build/native-build.log" 2>&1
phase=native
"$out/build/native" "$here/half.metal" "$out/results" > "$out/build/native.log" 2>&1
phase=check
"$out/build/check" check "$out/results" "$EXPECTED_HEAD" "$root"
phase=expected_negative
set +e
"$out/build/check" negative "$out/results" "$EXPECTED_HEAD" "$root"
rc=$?
set -e
[[ "$rc" == 2 ]]
phase=complete
"$out/build/check" bounds "$out/results"
