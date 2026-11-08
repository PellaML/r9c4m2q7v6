#!/bin/bash
# Reviewable source only: local installed tools, no installs/downloads/remote operations.
set -euo pipefail
umask 077
[[ $# -eq 0 ]] || exit 2
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
out=out-front-facing
[[ ! -e "$out" && ! -L "$out" ]] || { echo 'fresh output directory required' >&2; exit 2; }
mkdir -p "$out/.build"
printf 'label\tcommand_shell_quoted\texit\tcompleted_utc\n' > "$out/commands.tsv"
printf 'requested_runner_label=%s\n' "${RUNNER_LABEL:-local}" > "$out/runner.txt"
finish() {
  local rc=$?
  trap - EXIT
  printf '{"run_exit":%s,"native_result_requires":"7 completed GPU commands plus successful byte validation","cpu_fallback":false}\n' "$rc" > "$out/exit.json"
  (cd "$out" && find . -type f ! -path './.build/*' ! -name sha256.txt | LC_ALL=C sort |
    while IFS= read -r f; do shasum -a 256 "$f"; done) > "$out/sha256.txt" || rc=3
  exit "$rc"
}
trap finish EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
run() {
  local label="$1" rc
  shift
  if "$@" > "$out/$label.txt" 2>&1; then rc=0; else rc=$?; fi
  { printf '%s\t' "$label"; printf '%q ' "$@"; printf '\t%s\t%s\n' "$rc" "$(date -u +%Y-%m-%dT%H:%M:%SZ)"; } >> "$out/commands.tsv"
  return "$rc"
}
run platform uname -s
[[ $(cat "$out/platform.txt") == Darwin ]] || { echo 'Metal unavailable; NOT QUALIFIED, no CPU fallback' >&2; exit 3; }
run os sw_vers
run architecture uname -m
run xcode xcodebuild -version
run developer-dir xcode-select -p
run sdk-version xcrun --sdk macosx --show-sdk-version
run sdk-build xcrun --sdk macosx --show-sdk-build-version
run sdk-path xcrun --sdk macosx --show-sdk-path
run swift-version xcrun swiftc --version
run rust-version rustc -vV
(cd "$root" && shasum -a 256 scripts/front-facing/cases.rs scripts/front-facing/probe.swift scripts/front-facing/probe.metal scripts/front-facing/run.sh .github/workflows/front-facing.yml) > "$out/source-sha256.txt"
run rust-build rustc --edition=2021 -D warnings -O "$here/cases.rs" -o "$out/.build/cases"
run test-build rustc --edition=2021 -D warnings --test "$here/cases.rs" -o "$out/.build/tests"
run tests "$out/.build/tests"
run generate "$out/.build/cases" generate "$out/results"
run swift-build xcrun --sdk macosx swiftc -swift-version 5 -O "$here/probe.swift" -o "$out/.build/probe"
run capture "$out/.build/probe" "$here" "$out/results"
run validate "$out/.build/cases" validate "$out/results"
