#!/bin/bash
# Installed tools only. No downloads, installs, remote operations or substitutions.
set -euo pipefail
umask 077
[[ $# -eq 0 ]] || exit 2
here="$(cd "$(dirname "$0")" && pwd)"
cd "$here/../.."
exec python3 -B "$here/runner.py" run
