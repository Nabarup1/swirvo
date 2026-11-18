#!/usr/bin/env bash
# Build standalone Windows .exe files from WSL, without Rust on Windows.
#
# Links with Rust's bundled linker (rust-lld) against the MSVC + Windows SDK libraries of
# the Visual Studio Build Tools installed on the Windows side. The C runtime is linked
# statically, so the .exe needs no Visual C++ redistributable.
#
# Limitation: no C compiler for Windows here, so the zstd feature (tar.zst) is left out.
#
# Usage: scripts/build-windows-exe.sh [output-dir]      (default: dist/Swirvo)
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-$root/dist/Swirvo}"

vs_root="/mnt/c/Program Files (x86)/Microsoft Visual Studio"
msvc="$(ls -d "$vs_root"/*/*/VC/Tools/MSVC/*/lib/x64 2>/dev/null | sort -V | tail -1 || true)"
sdk="$(ls -d "/mnt/c/Program Files (x86)/Windows Kits/10/Lib/"*/ 2>/dev/null | sort -V | tail -1 || true)"
if [[ -z "$msvc" || -z "$sdk" ]]; then
  echo "Visual Studio Build Tools (C++ workload + Windows SDK) not found under C:\\." >&2
  echo "Install them on Windows first: see docs/09-dev-environment.md." >&2
  exit 1
fi
sdk="${sdk%/}"

rustup target add x86_64-pc-windows-msvc >/dev/null
sysroot="$(rustc --print sysroot)"

export LIB="$msvc;$sdk/ucrt/x64;$sdk/um/x64"
export CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER="$sysroot/lib/rustlib/x86_64-unknown-linux-gnu/bin/rust-lld"
export CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS="-C target-feature=+crt-static"

echo "MSVC libs: $msvc"
echo "SDK libs:  $sdk"
cargo build --release --target x86_64-pc-windows-msvc --no-default-features \
  -p swirvo-platform-win -p swirvo-cli --manifest-path "$root/Cargo.toml"

bin="${CARGO_TARGET_DIR:-$root/target}/x86_64-pc-windows-msvc/release"
mkdir -p "$out"
cp "$bin/swirvo-agent.exe" "$out/Swirvo.exe"
cp "$bin/swirvo.exe" "$out/swirvo-cli.exe"
# Licences of everything compiled in travel with the binaries.
python3 -I "$root/scripts/third-party-notices.py" "$out/THIRD-PARTY-NOTICES.txt"
sed -i 's/$/\r/' "$out/THIRD-PARTY-NOTICES.txt"
cp "$root/packaging/windows/How to use.txt" "$out/How to use.txt"
cp "$root/EULA.md" "$root/PRIVACY.md" "$out/"
sed -i 's/$/\r/' "$out/How to use.txt"   # Notepad-friendly line endings
echo "Done: $out"
ls -la "$out"
