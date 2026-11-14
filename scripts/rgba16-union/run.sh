#!/bin/bash
# Existing runner tools only. One finite batch; no installs, downloads or retries.
set -euo pipefail
umask 077
export LC_ALL=C
cd "$(dirname "$0")/../.."
s=scripts/rgba16-union
out=out-rgba16-union
r=$out/results
sources=("$s/probe.metal" "$s/probe.swift" "$s/cases.json" "$s/audit.py" "$s/run.sh" .github/workflows/rgba16-union.yml)
if [[ $# == 1 && $1 == verify ]]; then
  python3 -I -B "$s/audit.py" verify "$out"
  shasum -a 256 -c "$r/source.sha256"
  exit 0
fi
[[ $# == 0 && $(uname -s) == Darwin ]] || { echo 'Native macOS only; no fallback' >&2; exit 2; }
[[ ! -e "$out" && ! -L "$out" ]] || { echo 'Output must be new; nothing deleted' >&2; exit 2; }
mkdir -p "$r/precise" "$r/fast" "$out/build" "$out/tmp"
export TMPDIR="$PWD/$out/tmp"
finish() {
  local rc=$?; trap - EXIT
  python3 -I -B - "$r/status.json" "$rc" <<'PY'
import json, sys
rc = int(sys.argv[2])
with open(sys.argv[1], "x", encoding="utf-8") as f:
    json.dump(dict(status="OBSERVED_INTEGRITY_PASS_PRIVATE_COMPARISON_PENDING" if rc == 0 else "UNQUALIFIED",
                   run_exit=rc, cpu_fallback=False, private_model_compared=False), f, indent=2)
    f.write("\n")
PY
  if ! python3 -I -B "$s/audit.py" seal "$out"; then
    echo 'UNQUALIFIED: artifact bounds/seal failure; no upload permitted' >&2
    rc=3
  fi
  exit "$rc"
}
trap finish EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
: > "$r/commands.txt"; : > "$r/build.txt"
step() {
  local seconds=$1 log=$2 rc=0; shift 2
  printf '%q ' "$@" >> "$r/commands.txt"; printf '\n' >> "$r/commands.txt"
  python3 -I -B - "$r/$log" "$seconds" "$@" <<'PY' || rc=$?
import os, signal, subprocess, sys
with open(sys.argv[1], "ab", buffering=0) as log:
    try:
        p = subprocess.Popen(sys.argv[3:], stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            rc = p.wait(timeout=int(sys.argv[2]))
        except subprocess.TimeoutExpired:
            os.killpg(p.pid, signal.SIGKILL)
            p.wait()
            log.write(b"\nUNQUALIFIED: bounded command timeout; no retry\n")
            rc = 124
    except OSError as error:
        log.write((str(error) + "\n").encode())
        rc = 127
sys.exit(rc if rc >= 0 else 128 - rc)
PY
  return "$rc"
}

# Save the actual OS before enforcing the historic environment pins.
export ACTUAL_OS_BUILD
ACTUAL_OS_BUILD=$(sw_vers -buildVersion)
{
  printf 'runner_label=%s\nexpected_os_build=%s\nactual_os_build=%s\n' "${RUNNER_LABEL:-unset}" "${EXPECTED_OS_BUILD:-unset}" "$ACTUAL_OS_BUILD"
  printf 'architecture=%s\nrunner_environment=%s\nreviewed_commit=%s\n' "$(uname -m)" "${RUNNER_ENVIRONMENT:-local}" "${REVIEWED_COMMIT:-unset}"
  sw_vers
} > "$r/environment.txt"
shasum -a 256 "${sources[@]}" > "$r/source.sha256"
cp "$s/cases.json" "$r/cases.json"
[[ $(uname -m) == arm64 && "$ACTUAL_OS_BUILD" == "${EXPECTED_OS_BUILD:?}" ]]
[[ "${RUNNER_LABEL:?}:$ACTUAL_OS_BUILD" == macos-26:25G332 || "$RUNNER_LABEL:$ACTUAL_OS_BUILD" == xcode-27:26A434 ]]
[[ ${REVIEWED_COMMIT:?} =~ ^[0-9a-f]{40}$ && $(git rev-parse HEAD) == "$REVIEWED_COMMIT" ]]
git diff --quiet HEAD -- "${sources[@]}"
if [[ ${GITHUB_ACTIONS:-false} == true ]]; then
  [[ ${RUNNER_ENVIRONMENT:-} == github-hosted && ${FREE_RUNNERS_CONFIRMED:-false} == true ]]
  [[ ${GITHUB_REPOSITORY:-} == PellaML/r9c4m2q7v6 && ${GITHUB_REF:-} == refs/heads/research/probe && ${GITHUB_RUN_ATTEMPT:-} == 1 ]]
fi
step 30 environment.txt xcodebuild -version
step 30 environment.txt xcrun --sdk macosx --show-sdk-version
step 30 environment.txt xcrun --sdk macosx --show-sdk-build-version
step 30 environment.txt xcrun --sdk macosx metal --version
step 30 environment.txt xcrun --sdk macosx swiftc --version

# Same compiler flags as the reference. No runtime makeLibrary(source:) route.
# The retained AIR is the linker INPUT, not an extraction of a captured library.
for mode in precise fast; do
  flag=-fno-fast-math; [[ $mode == precise ]] || flag=-ffast-math
  m=$r/$mode
  step 45 build.txt xcrun --sdk macosx metal -std=metal4.0 -O2 -g0 "$flag" -c "$s/probe.metal" -o "$m/program.air"
  step 45 build.txt xcrun --sdk macosx metallib "$m/program.air" -o "$m/program.metallib"
  step 45 build.txt xcrun --sdk macosx metal -x ir -S -emit-llvm -O0 "$flag" "$m/program.air" -o "$m/program.ll"
  step 10 build.txt python3 -I -B "$s/audit.py" profile "$m" "$mode"
done
step 60 build.txt xcrun --sdk macosx swiftc -swift-version 5 -O "$s/probe.swift" -o "$out/build/probe"
shasum -a 256 -c "$r/source.sha256" >> "$r/build.txt"

# Preserve the other mode if a native mode fails; never retry a capture.
rc=0
for mode in precise fast; do
  if step 70 "$mode/native.log" "$out/build/probe" "$r/$mode" "$mode"; then
    if step 10 "$mode/native.log" python3 -I -B "$s/audit.py" validate "$r/$mode" "$mode"; then :; else rc=3; fi
  else rc=3; fi
done
if (( rc == 0 )); then step 10 build.txt python3 -I -B "$s/audit.py" complete "$r"; fi
shasum -a 256 -c "$r/source.sha256" >> "$r/build.txt"
exit "$rc"
