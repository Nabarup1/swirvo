#!/usr/bin/env bash
# Swirvo: WSL2 / Ubuntu 24.04 developer setup.
# Idempotent. Read it before running. Needs sudo for apt.
set -euo pipefail

APT_PACKAGES=(
  # Toolchain
  build-essential pkg-config clang lld mold cmake ninja-build meson nasm git curl unzip jq sqlite3 hyperfine
  # Tauri 2 on Linux (WebKitGTK 4.1)
  libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev
  libsoup-3.0-dev libjavascriptcoregtk-4.1-dev libssl-dev
  # Image engines
  libvips-dev libheif-dev libavif-dev libjxl-dev libwebp-dev libraw-dev liblcms2-dev libexif-dev
  # Audio/video (dev and testing; releases ship our own LGPL FFmpeg build)
  ffmpeg libavcodec-dev libavformat-dev libavutil-dev libavfilter-dev libswscale-dev libswresample-dev
  # Documents, archives, OCR
  libreoffice-nogui pandoc libarchive-dev libqpdf-dev qpdf tesseract-ocr libtesseract-dev
  # X11 / Wayland client development and test tools
  libx11-dev libxi-dev libxtst-dev libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev wayland-protocols
  xvfb xdotool weston dbus-x11
  # File-manager integration testing
  python3-nautilus
)

echo "==> apt packages"
sudo apt-get update
sudo apt-get install -y --no-install-recommends "${APT_PACKAGES[@]}"

echo "==> Rust components and targets"
rustup component add rustfmt clippy llvm-tools-preview
rustup target add x86_64-pc-windows-msvc aarch64-pc-windows-msvc
rustup toolchain install nightly --profile minimal   # only for cargo-fuzz

echo "==> cargo tools"
cargo install --locked cargo-nextest cargo-deny cargo-about cargo-cyclonedx cargo-xwin just bacon typos-cli
cargo install --locked tauri-cli --version '^2'
cargo +nightly install --locked cargo-fuzz

cat <<'EOF'

Done.
Next:
  * cargo-xwin downloads the MSVC CRT and Windows SDK on first use. Read Microsoft's
    license, then set XWIN_ACCEPT_LICENSE=1 in your shell profile.
  * Optional linker speed-up for Linux builds, in ~/.cargo/config.toml:
      [target.x86_64-unknown-linux-gnu]
      linker = "clang"
      rustflags = ["-C", "link-arg=-fuse-ld=mold"]
EOF
