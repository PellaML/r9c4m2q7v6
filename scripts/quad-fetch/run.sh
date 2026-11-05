#!/usr/bin/env bash
set -euo pipefail
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
cd "$root"
[[ $# -le 1 ]] || exit 64
out="${1:-out-quad-fetch}"
[[ ! -e "$out" && ! -L "$out" ]] || exit 64
mkdir -p "$out/.build" "$out/logs" "$out/evidence"
out="$(cd -- "$out" && pwd -P)"
build="$out/.build"
phase=environment
finish() {
  status=$?
  trap - EXIT
  set +e
  printf '{"schema":1,"phase":"%s","command_exit_code":%s}\n' "$phase" "$status" > "$out/logs/command-exit.json"
  if [[ "$status" != 0 ]]; then
    for log in "$out"/logs/*.log; do
      [[ -f "$log" && ! -L "$log" ]] || continue
      printf '\n--- %s ---\n' "$(basename "$log")" >&2
      tail -n 80 "$log" >&2
    done
  fi
  (cd "$root" && shasum -a 256 scripts/quad-fetch/host.rs scripts/quad-fetch/bridge.m \
    scripts/quad-fetch/probe.metal scripts/quad-fetch/run.sh .github/workflows/quad-fetch.yml) > "$out/evidence/source-sha256.txt"
  source_status=$?
  (
    set -euo pipefail
    cd "$out"
    find evidence logs -type f -print | LC_ALL=C sort |
      while IFS= read -r file; do shasum -a 256 "$file"; done
  ) > "$build/manifest.tmp"
  manifest_status=$?
  if [[ "$source_status" == 0 && "$manifest_status" == 0 ]]; then
    mv "$build/manifest.tmp" "$out/evidence/SHA256SUMS" || status=1
  else
    printf 'Artifact/source seal failed.\n' > "$out/logs/seal-failure.txt"
    status=1
  fi
  exit "$status"
}
trap finish EXIT
logged() {
  local name="$1" code=0
  shift
  "$@" > "$out/logs/$name.log" 2>&1 || code=$?
  cat "$out/logs/$name.log"
  return "$code"
}
{
  uname -s
  uname -m
  sw_vers
  git rev-parse HEAD
  printf 'runner_label=%s\nrun_id=%s\nrun_attempt=%s\n' "${QUAD_RUNNER_LABEL:-unset}" "${GITHUB_RUN_ID:-unset}" "${GITHUB_RUN_ATTEMPT:-unset}"
} > "$out/logs/environment.log" 2>&1
[[ "$(uname -s)" == Darwin ]] || { echo 'Native Darwin required; no fallback.' >&2; exit 23; }
selected="${DEVELOPER_DIR:-$(/usr/bin/xcode-select -p)}"
export DEVELOPER_DIR="$selected"
version="$(/usr/bin/xcodebuild -version)"
os="$(sw_vers -productVersion)"
printf 'developer_dir=%s\n%s\n' "$selected" "$version" >> "$out/logs/environment.log"
case "${QUAD_RUNNER_LABEL:-}:$os" in
  macos-26:26.*|xcode-27:26.*|xcode-27:27.*) ;;
  *) echo 'Actual runner/OS outside existing bounded allowlist.' >&2; exit 23 ;;
esac
if [[ "${QUAD_RUNNER_LABEL:-}" == xcode-27 ]]; then [[ "$version" == Xcode\ 27* ]] || exit 23; fi
# No runner/toolchain installation, switching, or relabeling.
export MACOSX_DEPLOYMENT_TARGET=13.0
unset SDKROOT
xcrun --sdk macosx --show-sdk-version >> "$out/logs/environment.log" 2>&1
xcrun --sdk macosx clang --version >> "$out/logs/environment.log" 2>&1
rustc --version --verbose >> "$out/logs/environment.log" 2>&1
cat "$out/logs/environment.log"
phase=portable_build
logged portable-build rustc --edition=2021 --check-cfg 'cfg(test)' --check-cfg 'cfg(quad_native)' \
  -D warnings -O -C overflow-checks=on --remap-path-prefix "$root=/probe" scripts/quad-fetch/host.rs -o "$build/portable"
phase=test_build
logged test-build rustc --edition=2021 --test --check-cfg 'cfg(test)' --check-cfg 'cfg(quad_native)' \
  -D warnings -O -C overflow-checks=on --remap-path-prefix "$root=/probe" scripts/quad-fetch/host.rs -o "$build/tests"
phase=portable_tests
logged portable-tests "$build/tests" --test-threads=1
phase=portable_native_gate
gate=0
"$build/portable" observe "$out/evidence/must-not-exist" > "$out/logs/native-gate.log" 2>&1 || gate=$?
[[ "$gate" == 2 && ! -e "$out/evidence/must-not-exist" ]] || exit 2
grep -q 'no CPU-as-GPU fallback' "$out/logs/native-gate.log"
phase=bridge_build
logged bridge-build xcrun --sdk macosx clang -c scripts/quad-fetch/bridge.m -fobjc-arc -fblocks -std=c11 \
  -O2 -g0 -Wall -Wextra -Werror -Wno-deprecated-declarations -mmacosx-version-min=13.0 \
  -ffile-prefix-map="$root"=/probe -fdebug-prefix-map="$root"=/probe -o "$build/bridge.o"
phase=native_build
logged native-build rustc --edition=2021 --cfg quad_native --check-cfg 'cfg(test)' --check-cfg 'cfg(quad_native)' \
  -D warnings -O -C overflow-checks=on -C debuginfo=0 --remap-path-prefix "$root=/probe" scripts/quad-fetch/host.rs \
  -C "link-arg=$build/bridge.o" -l framework=Foundation -l framework=Metal -l objc -o "$build/probe"
phase=native_observe
logged native-observe "$build/probe" observe "$out/evidence/native"
phase=portable_recheck
logged portable-recheck "$build/portable" check "$out/evidence/native"
phase=complete
