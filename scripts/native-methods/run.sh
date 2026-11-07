#!/bin/bash
set -euo pipefail
umask 077
root="${RUNNER_TEMP:?}/bounded-native-methods"
mkdir -p "$root/private" out-native-methods
status=0
{
  sw_vers
  uname -m
  xcodebuild -version
  xcrun --sdk macosx --show-sdk-version
  printf 'ImageOS=%s\nImageVersion=%s\n' "${ImageOS:-}" "${ImageVersion:-}"
} > "$root/private/identity.txt" 2>&1
if xcrun clang -arch arm64 -fobjc-arc -fblocks -O2 -Wno-deprecated-declarations \
    -framework Foundation scripts/native-methods/collect.m -o "$root/collect" \
    > "$root/private/compiler.txt" 2>&1; then
  "$root/collect" "$root/private/native.json" > "$root/private/stdout.txt" 2> "$root/private/stderr.txt" || status=$?
else
  status=$?
fi
printf '%s\n' "$status" > "$root/private/status.txt"
tar -C "$root/private" -cf "$root/evidence.tar" .
# Reuse the established recipient and CMS envelope. No plaintext artifact path.
openssl cms -encrypt -binary -aes-256-cbc -in "$root/evidence.tar" \
  -out out-native-methods/evidence.cms -outform DER scripts/native-service/recipient.cert.pem \
  > "$root/encryption.stdout" 2> "$root/encryption.stderr"
printf 'Encrypted bounded native-method evidence prepared. Collector exit: %s\n' "$status"
exit "$status"
