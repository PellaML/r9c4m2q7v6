#!/usr/bin/env bash
set -euo pipefail

# Run from the bundle root. No installation, download, or CPU sampling fallback.
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$root"
if [[ $# -gt 1 ]]; then
  printf '%s\n' '{"status":"usage_error","usage":"bash scripts/run.sh [new-output-directory]"}' >&2
  exit 64
fi
output="${1:-out}"
if [[ -e "$output" || -L "$output" ]]; then
  printf '%s\n' '{"status":"output_directory_must_not_exist"}' >&2
  exit 1
fi
export CARGO_NET_OFFLINE=true
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target}"
export MACOSX_DEPLOYMENT_TARGET=13.0
host="$(rustc --version --verbose | sed -n 's/^host: //p')"
if [[ -z "$host" ]]; then
  printf '%s\n' '{"status":"rust_host_target_unavailable"}' >&2
  exit 1
fi
cargo test --offline --locked --target "$host"
cargo build --offline --locked --release --target "$host"
probe="$CARGO_TARGET_DIR/$host/release/volume-sample-probe"

set +e
"$probe" run --out "$output"
status=$?
set -e
if [[ $status -ne 0 ]]; then
  # Exit 2 is explicit native unavailability, not a successful qualification.
  exit "$status"
fi

set +e
"$probe" wrong-oracle --out "$output" > "$output/invalid-control.json"
control_status=$?
set -e
if [[ $control_status -ne 1 ]]; then
  printf '%s\n' '{"status":"wrong_oracle_did_not_fail_with_the_required_mismatch_exit"}' >&2
  exit 1
fi
