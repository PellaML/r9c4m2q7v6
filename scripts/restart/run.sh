#!/bin/bash
# Installed local tools only. No downloads, installs, remote operations, or CPU image fallback.
set -euo pipefail
umask 077
[[ $# -eq 0 ]] || exit 2
here="$(cd "$(dirname "$0")" && pwd)"; root="$(cd "$here/../.." && pwd)"
out=out-restart
[[ ! -e "$out" && ! -L "$out" ]] || { echo 'fresh output directory required' >&2; exit 2; }
mkdir -p "$out/.build"
printf 'label\tcommand_shell_quoted\texit\tcompleted_utc\n' > "$out/commands.tsv"
printf 'runner_label=%s\nexpected_os_build=%s\n' "${RUNNER_LABEL:-local}" "${EXPECTED_OS_BUILD:-record-only}" > "$out/runner.txt"
seal() { (cd "$out" && find . -type f ! -path './.build/*' ! -name sha256.txt | LC_ALL=C sort | while IFS= read -r f; do shasum -a 256 "$f"; done) > "$out/sha256.txt"; }
finish() {
  local rc=$?; trap - EXIT
  printf '{"run_exit":%s,"cpu_fallback":false,"qualification_requires":"complete GPU receipts and successful exact positive/negative/list-model validation"}\n' "$rc" > "$out/exit.json"
  if ! seal; then
    rc=3; printf '{"run_exit":3,"packaging_failed":true,"cpu_fallback":false}\n' > "$out/exit.json"
    seal || true
  fi
  exit "$rc"
}
trap finish EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
run() {
  local label="$1" rc; shift
  if "$@" > "$out/$label.txt" 2>&1; then rc=0; else rc=$?; fi
  { printf '%s\t' "$label"; printf '%q ' "$@"; printf '\t%s\t%s\n' "$rc" "$(date -u +%Y-%m-%dT%H:%M:%SZ)"; } >> "$out/commands.tsv"
  return "$rc"
}
hash() { shasum -a 256 "$1" | cut -d' ' -f1; }
run platform uname -s
[[ $(cat "$out/platform.txt") == Darwin ]] || { echo 'Metal unavailable; NOT QUALIFIED' >&2; exit 3; }
run os sw_vers
run os-build sw_vers -buildVersion
[[ -z "${EXPECTED_OS_BUILD:-}" || $(cat "$out/os-build.txt") == "${EXPECTED_OS_BUILD:-}" ]] || { echo 'OS build drift; NOT QUALIFIED' >&2; exit 3; }
run architecture uname -m
run xcode xcodebuild -version
run developer-dir xcode-select -p
run sdk-version xcrun --sdk macosx --show-sdk-version
run sdk-build xcrun --sdk macosx --show-sdk-build-version
run sdk-path xcrun --sdk macosx --show-sdk-path
run metal-version xcrun --sdk macosx metal --version
run swift-version xcrun swiftc --version
run rust-version rustc -vV
(cd "$root" && shasum -a 256 scripts/restart/cases.rs scripts/restart/probe.swift scripts/restart/probe.metal scripts/restart/run.sh .github/workflows/restart.yml) > "$out/source-sha256.txt"
run rust-build rustc --edition=2021 -D warnings -O "$here/cases.rs" -o "$out/.build/cases"
run test-build rustc --edition=2021 -D warnings --test "$here/cases.rs" -o "$out/.build/tests"
run tests "$out/.build/tests"
run generate "$out/.build/cases" generate "$out/results"
run metal-compile xcrun --sdk macosx metal -std=metal3.1 -O2 -g0 -fno-fast-math -c "$here/probe.metal" -o "$out/results/probe.air"
air=$(hash "$out/results/probe.air")
run metallib-link xcrun --sdk macosx metallib "$out/results/probe.air" -o "$out/results/probe.metallib"
[[ "$air" == "$(hash "$out/results/probe.air")" ]] || exit 3
printf '{"source_sha256":"%s","air_sha256":"%s","metallib_sha256":"%s","link_input":"probe.air","runtime_library":"probe.metallib","compile_exit":0,"link_exit":0,"language":"metal3.1","flags":["-O2","-g0","-fno-fast-math"],"source_compile_fallback":false}\n' \
  "$(hash "$here/probe.metal")" "$air" "$(hash "$out/results/probe.metallib")" > "$out/results/compile-pair.json"
run swift-build xcrun --sdk macosx swiftc -swift-version 5 -O "$here/probe.swift" -o "$out/.build/probe"
run capture "$out/.build/probe" "$here" "$out/results"
run validate "$out/.build/cases" validate "$out/results"
