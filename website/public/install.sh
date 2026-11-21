#!/bin/sh
# Install the Swirvo command-line tool on Linux.
#
#   curl -fsSL https://raw.githubusercontent.com/Nabarup1/swirvo/main/install.sh | sh
#
# Installs `swirvo` into ~/.local/bin (or /usr/local/bin when run as root).
# Options (environment variables):
#   SWIRVO_VERSION=0.1.2       install this version instead of the latest
#   SWIRVO_INSTALL_DIR=/path   install into this folder
# Uninstall: delete the `swirvo` file it reports, and the swirvo folder in ~/.local/share.
set -eu

repo="Nabarup1/swirvo"

say() { printf '%s\n' "$*"; }
fail() { printf 'swirvo install: %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || fail "needs '$1'; install it and try again"; }

[ "$(uname -s)" = "Linux" ] || fail "this script is for Linux. On Windows, get Swirvo from the Microsoft Store: https://apps.microsoft.com/detail/9NPGWFBVTDJQ"

case "$(uname -m)" in
    x86_64 | amd64) arch="x64" ;;
    aarch64 | arm64) arch="arm64" ;;
    *) fail "no build for $(uname -m) yet; build from source: https://github.com/$repo#build-from-source" ;;
esac

need tar
need gzip
need uname
if command -v curl >/dev/null 2>&1; then
    fetch() { curl -fsSL --proto '=https' --tlsv1.2 -o "$2" "$1"; }
    final_url() { curl -fsSLI --proto '=https' --tlsv1.2 -o /dev/null -w '%{url_effective}' "$1"; }
elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -q --https-only -O "$2" "$1"; }
    final_url() { wget -q --https-only --max-redirect=5 -S --spider "$1" 2>&1 | sed -n 's/^ *[Ll]ocation: *//p' | tail -n 1 | tr -d '\r'; }
else
    fail "needs curl or wget"
fi

version="${SWIRVO_VERSION:-}"
if [ -z "$version" ]; then
    latest="$(final_url "https://github.com/$repo/releases/latest")" || fail "couldn't reach GitHub"
    version="${latest##*/v}"
    case "$version" in
        "" | */* | *" "*) fail "couldn't find the latest version; set SWIRVO_VERSION" ;;
    esac
fi
version="${version#v}"

if [ -n "${SWIRVO_INSTALL_DIR:-}" ]; then
    bin_dir="$SWIRVO_INSTALL_DIR"
elif [ "$(id -u)" = "0" ]; then
    bin_dir="/usr/local/bin"
else
    bin_dir="$HOME/.local/bin"
fi

name="swirvo-cli-$version-linux-$arch"
base="https://github.com/$repo/releases/download/v$version"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

say "Downloading Swirvo $version for Linux $arch..."
fetch "$base/$name.tar.gz" "$tmp/$name.tar.gz" || fail "download failed: $base/$name.tar.gz"
fetch "$base/$name.tar.gz.sha256" "$tmp/$name.tar.gz.sha256" || fail "checksum download failed"

expected="$(cut -d ' ' -f 1 "$tmp/$name.tar.gz.sha256")"
if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$tmp/$name.tar.gz" | cut -d ' ' -f 1)"
elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$tmp/$name.tar.gz" | cut -d ' ' -f 1)"
else
    fail "needs sha256sum or shasum to check the download"
fi
[ "$expected" = "$actual" ] || fail "checksum mismatch; the download is damaged, try again"

tar -xzf "$tmp/$name.tar.gz" -C "$tmp"
mkdir -p "$bin_dir"
install -m 755 "$tmp/$name/swirvo" "$bin_dir/swirvo.new"
mv -f "$bin_dir/swirvo.new" "$bin_dir/swirvo"

doc_dir="${XDG_DATA_HOME:-$HOME/.local/share}/swirvo"
[ "$(id -u)" = "0" ] && [ -z "${SWIRVO_INSTALL_DIR:-}" ] && doc_dir="/usr/local/share/doc/swirvo"
mkdir -p "$doc_dir"
cp "$tmp/$name/EULA.md" "$tmp/$name/PRIVACY.md" "$tmp/$name/THIRD-PARTY-NOTICES.txt" "$doc_dir/"

say "Installed $("$bin_dir/swirvo" --version 2>/dev/null || echo swirvo) to $bin_dir/swirvo"
say "Licence and notices: $doc_dir"

case ":$PATH:" in
    *":$bin_dir:"*) say "Try it: swirvo convert photo.png --to webp" ;;
    *)
        say ""
        say "$bin_dir is not on your PATH yet. Add this line to ~/.bashrc or ~/.profile, then open a new terminal:"
        say "  export PATH=\"$bin_dir:\$PATH\""
        ;;
esac
