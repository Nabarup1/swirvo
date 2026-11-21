---
title: "Linux"
description: "Install the swirvo command-line tool on Linux (x64 and ARM64), what it can do there, and how to remove it."
section: Power users
order: 22
---

On Linux, Swirvo is the `swirvo` command-line tool: convert, run tools, detect types, and the MCP server for AI assistants. The desktop app (the wheel and the Board) isn't available on Linux yet.

It runs on **x64 and ARM64** with **glibc 2.28 or newer**: Ubuntu 18.10+, Debian 10+, RHEL, Rocky and Alma 8+, Fedora, Mint 20+, and most other distributions from 2018 on.

## Install with one line

```bash
curl -fsSL https://raw.githubusercontent.com/Nabarup1/swirvo/main/install.sh | sh
```

It downloads the latest release for your machine, checks its SHA-256 checksum, and installs `swirvo` into `~/.local/bin` (or `/usr/local/bin` when run as root). No root needed. If `~/.local/bin` isn't on your PATH yet, it tells you the line to add.

Options, as environment variables before `sh`:

```bash
curl -fsSL https://raw.githubusercontent.com/Nabarup1/swirvo/main/install.sh | SWIRVO_VERSION=0.1.2 sh
curl -fsSL https://raw.githubusercontent.com/Nabarup1/swirvo/main/install.sh | SWIRVO_INSTALL_DIR=~/bin sh
```

The installer works with `curl` or `wget`. Want to read it first? It's [install.sh](https://github.com/Nabarup1/swirvo/blob/main/install.sh) in the repository.

## Debian, Ubuntu and Mint

Download `swirvo_<version>_amd64.deb` (or `_arm64.deb`) from the [download page](/download), then:

```bash
sudo apt install ./swirvo_*.deb
```

## Any distribution

Download `swirvo-cli-<version>-linux-x64.tar.gz` (or `-arm64`), unpack it, and put `swirvo` somewhere on your PATH:

```bash
tar -xzf swirvo-cli-*-linux-x64.tar.gz
install -m 755 swirvo-cli-*/swirvo ~/.local/bin/
```

## What works on Linux

Everything Swirvo does with its own engines works the same: images, PDF (including PDF to Word and Excel, compress, split, stamps, protect, redact, compare, PDF/A), documents, Office files to PDF, spreadsheets and data, subtitles, e-books, fonts, archives (and `tar.zst`).

What uses a Windows engine works differently:

- **OCR** (reading scans and photos) uses `tesseract` if it's installed.
- **Audio and video** beyond WAV and FLAC need `ffmpeg` installed.
- **HEIC, AVIF and a full RAW develop**, **Microsoft Office**, and **signing and checking signatures** need Windows.

`swirvo formats --supported` shows exactly what this machine can do.

## Remove it

- Installed with the script: delete `~/.local/bin/swirvo` and the folder `~/.local/share/swirvo` (licence notices).
- Installed from the `.deb`: `sudo apt remove swirvo`.
