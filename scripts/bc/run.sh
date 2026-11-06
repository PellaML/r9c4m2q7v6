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
# Optional offline IR of the synthetic clamp variants only (best effort; not a GPU result).
ir="$out/results/ir.txt"; : > "$ir"
work="$(mktemp -d)"
for mode in -fno-fast-math -ffast-math; do
  for v in 0 1 2 3 4; do
    echo "=== variant $v mode $mode" >> "$ir"
    if xcrun -sdk macosx metal $mode -DVARIANT=$v -S -emit-llvm "$here/clamp.metal" -o "$work/c.ll" > "$work/log" 2>&1; then
      grep -E 'call|declare' "$work/c.ll" | grep -E '@air\.|@llvm\.' | sed -E 's/#[0-9]+//g' | sort -u | head -n 40 >> "$ir"
    else
      echo "IR_UNAVAILABLE (offline compiler exit nonzero; not a GPU result):" >> "$ir"; head -c 600 "$work/log" >> "$ir"; echo >> "$ir"
    fi
  done
done
rm -rf "$work"
swiftc -swift-version 5 -O "$here/probe.swift" -o "$out/probe" > "$out/results/build.txt" 2>&1
build=$?
probe=99
if [ "$build" -eq 0 ]; then
  "$out/probe" "$here" "$out/results" > "$out/results/stdout.txt" 2> "$out/results/stderr.txt"
  probe=$?
fi
rm -f "$out/probe"
echo "{\"build_exit\": $build, \"probe_exit\": $probe}" > "$out/command-exit.json"
cat "$out/command-exit.json"; head -c 3000 "$out/results/summary.json" 2>/dev/null; head -c 3000 "$ir"
[ "$build" -eq 0 ] && [ "$probe" -eq 0 ]