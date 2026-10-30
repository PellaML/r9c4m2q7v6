#!/usr/bin/env bash
set -euo pipefail
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$root"
if [[ $# -gt 1 ]]; then
  printf '%s\n' 'usage: bash scripts/compile-pure-return.sh [new-output-directory]' >&2
  exit 64
fi
if [[ "$(uname -s)" != Darwin ]]; then
  printf '%s\n' 'requires macOS and installed Xcode; no fallback or download' >&2
  exit 1
fi
output="${1:-out-pure-return}"
host="$output.host"
if [[ -e "$output" || -L "$output" || -e "$host" || -L "$host" ]]; then
  printf '%s\n' 'output and host-build directories must not exist' >&2
  exit 1
fi
unset MACOSX_DEPLOYMENT_TARGET SDKROOT
mkdir -- "$host"
# Direct rustc avoids the numeric package's build.rs, bridge, tests and executable.
rustc --edition=2021 -C debuginfo=0 --remap-path-prefix="$root=/probe" \
  scripts/compile-pure-return.rs -o "$host/compile-pure-return"
exec "$host/compile-pure-return" "$output"
