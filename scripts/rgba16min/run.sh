#!/usr/bin/env bash
# Public synthetic observations only. Local toolchains; no installs, retries or cleanup.
# Adapted from rg16max; reviewed-HEAD binding matches the pixel/half workflows.
set -euo pipefail
umask 077
if [[ "${1:-}" == --seal-artifacts ]]; then
  [[ $# == 2 && "$2" =~ ^[a-zA-Z0-9_-]+$ && ! -L "$2" ]] || exit 64
  out="$2"
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
free_kb="$(df -Pk . | awk 'NR == 2 {print $4}')"
[[ "$free_kb" =~ ^[0-9]+$ && "$free_kb" -ge 2097152 ]] || exit 64
root="$PWD"; out="$1"; here=scripts/rgba16min
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
trap 'exit 130' INT
trap 'exit 143' TERM
rustc --edition=2021 -Dwarnings -O -C debuginfo=0 --remap-path-prefix "$root=/probe" \
  "$here/cases.rs" -o "$out/build/check" > "$out/build/rust.log" 2>&1
rustc --edition=2021 --test -Dwarnings -O -C debuginfo=0 --remap-path-prefix "$root=/probe" \
  "$here/cases.rs" -o "$out/build/tests" > "$out/build/test-build.log" 2>&1
phase=tests
"$out/build/tests" --test-threads=1 > "$out/results/tests.txt"
phase=bind
"$out/build/check" bind "$out/results" "$EXPECTED_HEAD" "$root"
{
  sw_vers; uname -m; xcodebuild -version
  xcrun --sdk macosx --show-sdk-version
  xcrun --sdk macosx --show-sdk-build-version
  xcrun swiftc --version; rustc -Vv
  printf '%s\n' 'Swift: -swift-version 5 -O -target arm64-apple-macos26.0'
  printf '%s\n' 'MSL: makeLibrary(source:options:), explicit language 2.4, fastMathEnabled=false'
  printf '%s\n' 'No device addresses, captured sources, AIR extraction, CPU output fallback or production adoption.'
} > "$out/results/baseline.txt"
phase=generate
"$out/build/check" generate "$out/results"
phase=build
xcrun --sdk macosx swiftc -swift-version 5 -O -target arm64-apple-macos26.0 \
  "$here/probe.swift" -o "$out/build/native" > "$out/build/native-build.log" 2>&1
phase=native
"$out/build/native" "$here" "$out/results" > "$out/build/native.log" 2>&1
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
used_kb="$(du -sk "$out" | awk '{print $1}')"
free_kb="$(df -Pk . | awk 'NR == 2 {print $4}')"
[[ "$used_kb" =~ ^[0-9]+$ && "$used_kb" -le 307200 && "$free_kb" =~ ^[0-9]+$ && "$free_kb" -ge 2097152 ]]
