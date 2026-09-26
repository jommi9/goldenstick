#!/usr/bin/env bash
# Build boothready-helper in release mode and put it where the release
# config's externalBin expects it: app/src-tauri/binaries/
# boothready-helper-<target>[.exe]. Tauri then ships it next to the app's
# executable, which is where the app looks for it.
#
# Usage: scripts/build-helper-sidecar.sh [target-triple | universal-apple-darwin]
set -euo pipefail
cd "$(dirname "$0")/.."

target="${1:-$(rustc -vV | sed -n 's/^host: //p')}"
out=app/src-tauri/binaries
mkdir -p "$out"

if [ "$target" = universal-apple-darwin ]; then
  # Tauri compiles each architecture on its own and looks for that
  # architecture's sidecar each time, then bundles the universal one.
  for t in aarch64-apple-darwin x86_64-apple-darwin; do
    cargo build --release --locked -p boothready-helper --target "$t"
    cp "target/$t/release/boothready-helper" "$out/boothready-helper-$t"
  done
  lipo -create -output "$out/boothready-helper-$target" \
    "$out/boothready-helper-aarch64-apple-darwin" "$out/boothready-helper-x86_64-apple-darwin"
else
  ext=""
  case "$target" in *windows*) ext=".exe" ;; esac
  cargo build --release --locked -p boothready-helper --target "$target"
  cp "target/$target/release/boothready-helper$ext" "$out/boothready-helper-$target$ext"
fi
echo "helper sidecar ready in $out"
