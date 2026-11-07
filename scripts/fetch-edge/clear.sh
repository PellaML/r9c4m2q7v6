#!/bin/bash
set -uo pipefail
out="$1"; mkdir -p "$out/results"
here="$(cd "$(dirname "$0")" && pwd)"
{
  echo "{"
  echo "\"sw_vers\": \"$(sw_vers -productVersion) $(sw_vers -buildVersion)\","
  echo "\"uname\": \"$(uname -m)\","
  echo "\"sdk\": \"$(xcrun --show-sdk-version 2>/dev/null)\","
  echo "\"swiftc\": \"$(swiftc --version 2>&1 | head -n1 | tr -d '"')\","
  echo "\"hw_model\": \"$(sysctl -n hw.model 2>/dev/null)\","
  echo "\"cpu\": \"$(sysctl -n machdep.cpu.brand_string 2>/dev/null)\""
  echo "}"
} > "$out/run-receipt.json"
system_profiler SPDisplaysDataType > "$out/results/displays.txt" 2>&1 || true
swiftc -swift-version 5 -O "$here/clear.swift" -o "$out/probe" > "$out/results/build.txt" 2>&1
build=$?
probe=99
if [ "$build" -eq 0 ]; then
  "$out/probe" "$here/probe.metal" "$out/results" > "$out/results/stdout.txt" 2> "$out/results/stderr.txt"
  probe=$?
fi
rm -f "$out/probe"
echo "{\"build_exit\": $build, \"probe_exit\": $probe}" > "$out/command-exit.json"
cat "$out/command-exit.json"; cat "$out/results/summary.json" 2>/dev/null | head -c 4000
[ "$build" -eq 0 ] && [ "$probe" -eq 0 ]
