#!/bin/bash
# One finite acquisition; no installs, downloads, cleanup, retries, or expected-byte rewrites.
set -euo pipefail
umask 077
hash_results() {
  (cd "$results" && for f in *; do
    [[ "$f" == SHA256SUMS ]] || { shasum -a 256 "$f" || exit 1; }
  done) > "$out/build/SHA256SUMS" && mv "$out/build/SHA256SUMS" "$results/SHA256SUMS"
}
if [[ "${1:-}" == --check-artifacts ]]; then
  out="${2:?output root}"; results="$out/results"
  "$out/build/check" bounds "$results"
  # Also seals partial buffers after step timeout; incomplete status stays failed.
  hash_results
  exec "$out/build/check" bounds "$results"
fi
out="${1:?new output basename required}"
[[ $# == 1 && "$out" =~ ^[a-zA-Z0-9_-]+$ && ! -e "$out" && ! -L "$out" ]] || exit 64
mkdir -p "$out/build" "$out/results"
out="$(cd "$out" && pwd)"; results="$out/results"
here="$(cd "$(dirname "$0")" && pwd)"
native=99; verified=99
finish() {
  rc=$?; trap - EXIT; set +e
  printf 'run_exit\tnative_exit\tcapture_check_exit\n%s\t%s\t%s\n' "$rc" "$native" "$verified" > "$results/command-exit.tsv"
  final=3
  if [[ -x "$out/build/check" ]]; then
    "$out/build/check" status "$results" "$rc" "$native" "$verified"; final=$?
  else
    printf 'FAILED_CHECKER_BUILD_NO_NATIVE_EXECUTION\n' > "$results/status.txt"
  fi
  hash_results; hashes=$?
  [[ $rc == 0 && $final == 0 && $hashes == 0 ]] || exit 3
  exit 0
}
trap finish EXIT
for f in .github/workflows/fetch-sampling.yml scripts/fetch-edge/sampling{.metal,.swift,-check.rs,.sh}; do
  shasum -a 256 "$f"
done > "$results/source-sha256.txt"
(cd "$here" && rustc --edition=2021 -O sampling-check.rs -o "$out/build/check") 2>&1 | head -c 65536 > "$results/rust-build.txt"
"$out/build/check" preflight "$results" > "$results/preflight.txt" 2>&1
(cd "$here" && swiftc -swift-version 5 -O sampling.swift -o "$out/build/native") 2>&1 | head -c 65536 > "$results/swift-build.txt"
set +e
"$out/build/native" "$here/sampling.metal" "$results" 2>&1 | head -c 65536 > "$results/native-stdio.txt"
native=${PIPESTATUS[0]}
# Check framing and basic controls even on partial/error acquisitions, retaining every byte and per-case diagnostic.
"$out/build/check" check "$results" > "$results/check.txt" 2>&1
verified=$?
set -e
[[ $native == 0 && $verified == 0 ]]
