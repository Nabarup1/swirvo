---
title: "Build from source"
description: "Build the swirvo command line and the Windows app from the Rust workspace, and run the checks."
section: Help
order: 33
---

Swirvo is a Rust workspace (2024 edition, stable toolchain) with its own engines and its own UI toolkit; there's no web view. The code is on [GitHub](https://github.com/Nabarup1/swirvo) under the [PolyForm Noncommercial licence](/license).

## The command line (Windows or Linux)

```bash
git clone https://github.com/Nabarup1/swirvo
cd swirvo
cargo build --release -p swirvo-cli
```

Then try it:

```bash
cargo run -p swirvo-cli -- convert photo.png --to webp
```

## The Windows app

The app (`swirvo-agent`, shipped as `Swirvo.exe`) builds natively on Windows with the MSVC toolchain:

```bash
cargo build --release -p swirvo-platform-win -p swirvo-cli
```

From WSL, `scripts/build-windows-exe.sh` builds a standalone `dist/Swirvo/` using the Windows-side Visual Studio Build Tools.

## Checks

Before sending a change:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo deny check
```

## Where things are

| Crate | What it does |
|---|---|
| `crates/formats` | Format registry and detection from content |
| `crates/graph` | The conversion planner: shortest routes per quality preset |
| `crates/wheel` | Wheel geometry, hit-testing, the gesture state machine, stable slots |
| `crates/engines` | The engines: images, PDF, PDF → DOCX, the DOCX/XLSX/PPTX → PDF layout engines, data, fonts, archives, and the Windows OS engines |
| `crates/jobs` | Parallel batches, the tools, naming, atomic writes, the sandboxed worker |
| `crates/render`, `crates/ui` | The wheel renderer and a small immediate-mode UI toolkit |
| `crates/board` | The Board, the page picker and the live previews |
| `crates/platform/win` | The Windows app: input, the drag probe, the wheel window, tray, installer |
| `crates/cli` | The `swirvo` command and the MCP server |

Contributions are accepted under the terms in [CONTRIBUTING.md](https://github.com/Nabarup1/swirvo/blob/main/CONTRIBUTING.md).
