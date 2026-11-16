#!/usr/bin/env bash
set -euo pipefail
[[ $# == 1 ]] || { echo 'UNQUALIFIED: usage: run.sh NEW_OUTPUT_DIRECTORY' >&2; exit 2; }
exec python3 "$(dirname "$0")/review.py" run "$1"

