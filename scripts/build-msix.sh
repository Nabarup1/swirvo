#!/usr/bin/env bash
# Build the Microsoft Store package: dist/Swirvo-<version>-x64.msix (unsigned; the Store
# signs it when you submit). For a local test install, sign it with
# scripts/sign-msix.ps1 first.
#
# Usage: scripts/build-msix.sh [--build] [input-dir]
#   --build    run scripts/build-windows-exe.sh first (otherwise the input must exist)
#   input-dir  the built app folder (default: dist/Swirvo)
#
# Inputs: dist/Swirvo/ (the exes and text files), packaging/msix/Assets/ (from
# `cargo run -p swirvo-render --example icon_assets`), packaging/msix/AppxManifest.xml.in
# and packaging/msix/identity.env (your Partner Center identity: Product management > Product identity).
# Needs the Windows SDK (makeappx.exe, makepri.exe) on the Windows side.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
build=0
if [[ "${1:-}" == "--build" ]]; then build=1; shift; fi
dist="${1:-$root/dist/Swirvo}"
if (( build )); then "$root/scripts/build-windows-exe.sh" "$dist"; fi

[[ -f "$dist/Swirvo.exe" ]] || { echo "no $dist/Swirvo.exe: run scripts/build-windows-exe.sh (or pass --build)" >&2; exit 1; }
sdk="$(ls -d "/mnt/c/Program Files (x86)/Windows Kits/10/bin/"10.*/x64 2>/dev/null | sort -V | tail -1 || true)"
[[ -x "$sdk/makeappx.exe" && -x "$sdk/makepri.exe" ]] || { echo "Windows SDK makeappx/makepri not found" >&2; exit 1; }

# shellcheck disable=SC1091
source "$root/packaging/msix/identity.env"
ver="$(grep -m1 '^version' "$root/Cargo.toml" | sed -E 's/.*"([0-9]+\.[0-9]+\.[0-9]+)".*/\1/')"
msix_ver="$ver.0" # the Store wants the fourth number to be 0

# The SDK tools are Windows programs: stage in a Windows temp folder.
wtemp="$(cmd.exe /c 'echo %TEMP%' 2>/dev/null | tr -d '\r')"
stage_w="$wtemp\\swirvo-msix"
stage="$(wslpath -u "$stage_w")"
rm -rf "$stage" && mkdir -p "$stage/layout/Assets"
cp "$dist/Swirvo.exe" "$dist/swirvo-cli.exe" "$dist/THIRD-PARTY-NOTICES.txt" "$dist/How to use.txt" "$stage/layout/"
cp "$root/packaging/msix/Assets/"*.png "$stage/layout/Assets/"
sed -e "s|@IDENTITY_NAME@|$IDENTITY_NAME|" \
    -e "s|@PUBLISHER@|$PUBLISHER|" \
    -e "s|@PUBLISHER_DISPLAY_NAME@|$PUBLISHER_DISPLAY_NAME|" \
    -e "s|@VERSION@|$msix_ver|" \
    "$root/packaging/msix/AppxManifest.xml.in" > "$stage/layout/AppxManifest.xml"

# resources.pri indexes the scale-/targetsize- variants of each image.
(
  cd "$stage"
  "$sdk/makepri.exe" createconfig /cf priconfig.xml /dq en-US /pv 10.0.0 /o > /dev/null
  # One resources.pri for a single package: drop the per-scale split meant for bundles.
  sed -i '/<packaging>/,/<\/packaging>/d' priconfig.xml
  "$sdk/makepri.exe" new /pr layout /cf priconfig.xml /mn layout\\AppxManifest.xml /of layout\\resources.pri /o > /dev/null
)

out_name="Swirvo-$ver-x64.msix"
(cd "$stage" && "$sdk/makeappx.exe" pack /d layout /p "$out_name" /o > makeappx.log) || {
  cat "$stage/makeappx.log" >&2
  exit 1
}
cp "$stage/$out_name" "$root/dist/$out_name"
rm -rf "$stage"
echo "built dist/$out_name ($(du -h "$root/dist/$out_name" | cut -f1)), identity $IDENTITY_NAME, version $msix_ver"
case "$PUBLISHER" in
  *"Test Publisher"*) echo "note: placeholder identity; replace packaging/msix/identity.env with your Partner Center values before uploading" ;;
esac
