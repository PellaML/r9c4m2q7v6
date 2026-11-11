#!/usr/bin/env bash
# Manual bounded native observation only; no installs, downloads, retries or cleanup.
set -euo pipefail
umask 077
if [[ "${1:-}" == --check-artifacts ]]; then
  out="${2:?output root}"
  "$out/build/check" bounds "$out/results"
  (cd "$out/results"; for f in *; do [[ "$f" == SHA256SUMS ]] || shasum -a 256 "$f"; done) > "$out/build/SHA256SUMS"
  [[ ! -e "$out/results/SHA256SUMS" ]]
  cp "$out/build/SHA256SUMS" "$out/results/SHA256SUMS"
  exec "$out/build/check" bounds "$out/results"
fi
[[ $# == 1 && "$1" =~ ^[a-zA-Z0-9_-]+$ && ! -e "$1" && ! -L "$1" ]] || exit 64
[[ "$(uname -s)" == Darwin ]] || exit 64
[[ "${GITHUB_EVENT_NAME:-}" == workflow_dispatch && "${GITHUB_REPOSITORY:-}" == PellaML/r9c4m2q7v6 &&
   "${GITHUB_REF:-}" == refs/heads/research/probe && "${GITHUB_RUN_ATTEMPT:-}" == 1 &&
   "${EXPECTED_HEAD:-}" =~ ^[0-9a-f]{40}$ && "${GITHUB_SHA:-}" == "$EXPECTED_HEAD" &&
   "$(git rev-parse HEAD)" == "$EXPECTED_HEAD" ]] || exit 64
[[ -z "$(git status --porcelain)" ]] || exit 64
free_kb="$(df -Pk . | awk 'NR == 2 {print $4}')"
[[ "$free_kb" =~ ^[0-9]+$ && "$free_kb" -ge 2097152 ]] || exit 64
out="$1"
mkdir -p "$out/build" "$out/results"
root="$PWD"; out="$(cd "$out" && pwd)"; results="$out/results"
phase=build
finish() {
  rc=$?; trap - EXIT
  if [[ "$rc" != 0 && "$phase" == build && -f "$out/build/native-build.log" ]]; then
    tail -c 8192 "$out/build/native-build.log" >&2
  fi
  printf '{"exit":%s,"phase":"%s","native_evidence_only_if_all_checks_pass":true}\n' "$rc" "$phase" > "$results/command-exit.json"
  exit "$rc"
}
trap finish EXIT
here=scripts/pixel-query
for f in .github/workflows/fetch-pixel-query.yml "$here"/{bridge.mm,probe.metal,check.rs,run.sh}; do
  shasum -a 256 "$f"
done > "$results/source-sha256.txt"
{
  sw_vers; xcodebuild -version; xcrun --sdk macosx --show-sdk-version; rustc -Vv
  printf '%s\n' 'host: clang++ -std=c++17 -fobjc-arc -O2 -g0 -Wall -Wextra -Werror -Wno-deprecated-declarations -mmacosx-version-min=13.0'
  printf '%s\n' 'MSL: runtime newLibraryWithSource; language 2.4; fastMathEnabled=false; no AIR extraction'
} > "$results/baseline.txt"
rustc --edition=2021 -Dwarnings -O -C debuginfo=0 --remap-path-prefix "$root=/probe" "$here/check.rs" -o "$out/build/check" > "$out/build/rust.log" 2>&1
rustc --edition=2021 --test -Dwarnings -O -C debuginfo=0 --remap-path-prefix "$root=/probe" "$here/check.rs" -o "$out/build/tests" > "$out/build/test-build.log" 2>&1
"$out/build/tests" > "$out/build/tests.log"
"$out/build/check" prepare "$results"
xcrun --sdk macosx clang++ -std=c++17 -fobjc-arc -O2 -g0 -Wall -Wextra -Werror -Wno-deprecated-declarations \
  -mmacosx-version-min=13.0 -ffile-prefix-map="$root"=/probe -fdebug-prefix-map="$root"=/probe \
  "$here/bridge.mm" -framework Foundation -framework Metal -o "$out/build/native" > "$out/build/native-build.log" 2>&1
phase=native
"$out/build/native" "$here/probe.metal" "$results" > "$out/build/native-stdio.log" 2>&1
phase=check
"$out/build/check" check "$results"
phase=expected_negative
set +e
"$out/build/check" negative "$results"
rc=$?
set -e
[[ "$rc" == 2 ]]
phase=complete
"$out/build/check" bounds "$results"
