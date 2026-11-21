#!/bin/sh
# Package a built Linux `swirvo` binary as a tarball and a .deb, each with a .sha256.
#
#   scripts/package-linux.sh <binary> <version> <x64|arm64> <notices.txt> [out-dir]
#
# Used by .github/workflows/release.yml; runs on any Linux with tar, gzip and dpkg-deb.
set -eu

bin="$1"
version="$2"
arch="$3"
notices="$4"
out="${5:-.}"

case "$arch" in
    x64) deb_arch="amd64" ;;
    arm64) deb_arch="arm64" ;;
    *) echo "unknown arch: $arch" >&2; exit 1 ;;
esac

root="$(cd "$(dirname "$0")/.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$out"

# Tarball: swirvo-cli-<version>-linux-<arch>/{swirvo, EULA.md, PRIVACY.md, THIRD-PARTY-NOTICES.txt}
name="swirvo-cli-$version-linux-$arch"
mkdir -p "$work/$name"
install -m 755 "$bin" "$work/$name/swirvo"
cp "$root/EULA.md" "$root/PRIVACY.md" "$work/$name/"
cp "$notices" "$work/$name/THIRD-PARTY-NOTICES.txt"
tar -C "$work" --owner=0 --group=0 -czf "$out/$name.tar.gz" "$name"

# Debian package: /usr/bin/swirvo and /usr/share/doc/swirvo
pkg="$work/deb"
mkdir -p "$pkg/DEBIAN" "$pkg/usr/bin" "$pkg/usr/share/doc/swirvo"
install -m 755 "$bin" "$pkg/usr/bin/swirvo"
cp "$root/EULA.md" "$root/PRIVACY.md" "$pkg/usr/share/doc/swirvo/"
cp "$notices" "$pkg/usr/share/doc/swirvo/THIRD-PARTY-NOTICES.txt"
cat > "$pkg/usr/share/doc/swirvo/copyright" <<EOF
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: swirvo
Source: https://github.com/Nabarup1/swirvo

Files: *
Copyright: 2026 Nabarup Ghosh
License: Swirvo-EULA
 This binary package is the official Swirvo build, free to use under
 /usr/share/doc/swirvo/EULA.md. The source code is available under the
 PolyForm Noncommercial License 1.0.0: https://github.com/Nabarup1/swirvo.
 Bundled open-source components and their licences are listed in
 /usr/share/doc/swirvo/THIRD-PARTY-NOTICES.txt.
EOF
size_kb="$(du -sk "$pkg/usr" | cut -f 1)"
cat > "$pkg/DEBIAN/control" <<EOF
Package: swirvo
Version: $version
Architecture: $deb_arch
Maintainer: Nabarup Ghosh <51172739+Nabarup1@users.noreply.github.com>
Installed-Size: $size_kb
Depends: libc6 (>= 2.28), libgcc-s1 | libgcc1
Section: utils
Priority: optional
Homepage: https://github.com/Nabarup1/swirvo
Description: Private file converter and PDF toolkit (command line)
 Swirvo converts and edits files on your own computer: about 96 formats
 (images, PDF, Word, Excel, PowerPoint, EPUB, Markdown, data, fonts,
 subtitles, archives) and tools such as PDF merge, split, compress,
 watermark, protect and redaction. Nothing is uploaded, and originals are
 never changed. Also runs as a local MCP server (swirvo mcp) for AI
 assistants.
EOF
find "$pkg" -type d -exec chmod 755 {} +
find "$pkg/usr/share" -type f -exec chmod 644 {} +
dpkg-deb --root-owner-group -Zxz --build "$pkg" "$out/swirvo_${version}_${deb_arch}.deb" >/dev/null

cd "$out"
for f in "$name.tar.gz" "swirvo_${version}_${deb_arch}.deb"; do
    sha256sum "$f" > "$f.sha256"
done
ls -l "$name.tar.gz" "swirvo_${version}_${deb_arch}.deb"
