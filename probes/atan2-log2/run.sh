#!/usr/bin/env bash
set -euo pipefail
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
cd "$root"
probe=probes/atan2-log2
[[ $# -eq 0 ]] || { echo 'no arguments' >&2; exit 64; }
[[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]] || { echo 'requires native Apple Silicon macOS' >&2; exit 2; }
case "${EXPECTED_OS_MAJOR:-}" in 26|27) ;; *) echo 'explicit expected OS major required' >&2; exit 2;; esac
[[ "$(sw_vers -productVersion)" == "$EXPECTED_OS_MAJOR".* ]] || { echo 'native OS major mismatch' >&2; exit 2; }
unset MACOSX_DEPLOYMENT_TARGET SDKROOT
[[ "$(xcrun --sdk macosx --show-sdk-version)" == "$EXPECTED_OS_MAJOR".* ]] || { echo 'native SDK major mismatch' >&2; exit 2; }
out=out-atan2-log2
[[ ! -e "$out" && ! -L "$out" ]] || { echo 'output directory must not exist' >&2; exit 3; }
mkdir "$out" "$out/.build"
work="$out/.build"
files=(corpus.json inputs.bin toolchain.txt)
for p in legacy modern; do
  files+=("$p.air" "$p.metallib" "$p.ll" "$p-native.json" "$p-uploaded-inputs.bin" "$p-results.bin"
          "$p-check.json" "$p-wrong-expected.json" "$p-abi.json")
done
finish() {
  status=$?
  trap - EXIT
  # Hash even partial failure evidence; never discard or replace a raw result.
  (cd "$out"; for file in "${files[@]}"; do [[ ! -f "$file" ]] || shasum -a 256 "$file"; done) > "$out/SHA256SUMS"
  exit "$status"
}
trap finish EXIT
export CARGO_NET_OFFLINE=true CARGO_TARGET_DIR="$work/target"
cargo build --offline --locked --release --manifest-path "$probe/Cargo.toml"
tool="$CARGO_TARGET_DIR/release/atan2-log2-cohort"
"$tool" selftest
"$tool" generate "$out"
{
  sw_vers
  uname -m
  xcodebuild -version
  xcrun --sdk macosx --show-sdk-version
  xcrun --sdk macosx metal --version | sed '/^InstalledDir:/d'
  xcrun --sdk macosx clang --version | sed '/^InstalledDir:/d'
  rustc --version
  git rev-parse HEAD
  printf '%s\n' 'legacy: -std=macos-metal2.2 -mmacosx-version-min=10.15 -O2 -g0 -ffast-math'
  printf '%s\n' 'modern: -std=metal4.0 -O2 -g0 -ffast-math (compiler-default deployment)'
  printf '%s\n' 'one compute dispatch per profile; identical runtime inputs'
} > "$out/toolchain.txt"
xcrun --sdk macosx clang "$probe/host.m" -fobjc-arc -std=c11 -O2 -g0 \
  -Wall -Wextra -Werror -ffile-prefix-map="$root"=/probe -fdebug-prefix-map="$root"=/probe \
  -framework Metal -framework Foundation -o "$work/host"
cp "$probe/probe.metal" "$work/probe.metal"
compile() {
  local name=$1; shift
  (
    cd "$work"
    local common=(-O2 -g0 -ffast-math -ffile-prefix-map="$root"=/probe -fdebug-prefix-map="$root"=/probe
                  -fdebug-compilation-dir=/probe "$@")
    xcrun --sdk macosx metal "${common[@]}" -c probe.metal -o "$name.air"
    xcrun --sdk macosx metallib "$name.air" -o "$name.metallib"
    # Textual AIR from the same flags; observation only, never executed.
    xcrun --sdk macosx metal "${common[@]}" -S -emit-llvm probe.metal -o "$name.ll"
  )
  cp "$work/$name.air" "$work/$name.metallib" "$out/"
  sed '/^; ModuleID/d;/^source_filename/d' "$work/$name.ll" > "$out/$name.ll"
}
compile legacy -std=macos-metal2.2 -mmacosx-version-min=10.15
compile modern -std=metal4.0
status=0
for p in legacy modern; do
  set +e
  "$work/host" "$out/$p.metallib" "$out/inputs.bin" "$out" "$p"
  native=$?
  set -e
  if [[ $native -ne 0 ]]; then
    [[ ! -f "$out/$p-native.json" ]] || cat "$out/$p-native.json"
    echo "$p native execution refused; no CPU or alternate implementation fallback" >&2
    status=1
  fi
done
[[ $status -eq 0 ]] || exit 1
set +e
for p in legacy modern; do
  "$tool" check "$out" "$p"; check=$?
  "$tool" wrong-expected "$out" "$p"; control=$?
  "$tool" abi "$out" "$p"; abi=$?
  [[ $check -eq 0 && $control -eq 1 && $abi -eq 0 ]] || { echo "$p integrity/control/ABI check failed; raw data retained" >&2; status=1; }
done
"$tool" audit "$out" || status=1
set -e
[[ $status -eq 0 ]] || exit 1
echo 'raw native atan2/log2 results retained; integrity, ABI and control checks passed; bound findings recorded, not gated'