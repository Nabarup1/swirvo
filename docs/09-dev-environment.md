# 09 · Dev environment

## 1. What's on this machine now (checked 2026-11-07)

| Item | WSL2 (Ubuntu 24.04.1, kernel 6.18) | Windows 11 (build 26200) |
|---|---|---|
| Rust | ✅ rustc/cargo 1.98.0 | ❌ not installed |
| C/C++ | ✅ GCC 13.3, CMake 3.28 · ❌ clang, lld, mold, ninja, meson | ❌ no MSVC toolset, no Windows SDK (the VS 2022 folders are empty) |
| Node | ✅ Node 22.22.2, npm 10.9.7, pnpm 12.18.0 | ✅ Node, pnpm |
| Git | ✅ 2.43 | ✅ Git for Windows |
| Python | ✅ 3.12.3 | — |
| Media/doc tools | ❌ ffmpeg, LibreOffice, Pandoc | ❌ |
| Package manager | apt | ✅ winget |
| Runtime bits | WSLg (Wayland + X11) | ✅ WebView2 runtime 156/157 |
| Hardware | Intel i5-9300H (4 cores/8 threads), 24 GB, GTX 1650 Ti (driver 610.62), UHD 630 | same |

## 2. Recommended workflow

- **The repo stays in WSL** (`~/swirvo` on ext4), so builds are fast. Never build from `/mnt/c`; I/O over 9P is roughly 10× slower.
- **Linux builds and tests** run natively in WSL. WSLg can display windows for smoke tests, but it is **not** a real Linux desktop: it has no layer-shell and no drag-and-drop between Windows and Linux apps. For the Linux UX, use VMs: Ubuntu 24.04 GNOME and Fedora KDE in Hyper-V, with a dedicated Wayland test VM.
- **Fast Windows loop from WSL:** `cargo xwin build --target x86_64-pc-windows-msvc -p swirvo-agent`, then run the `.exe` straight from WSL. Interop launches it on the Windows desktop, where Raw Input and drag-and-drop behave normally. This works for pure-Rust crates (`windows-rs` included).
- **Full Windows builds** (C dependencies through vcpkg, Tauri bundling, MSIX, signing) use the native MSVC toolchain in a Windows clone (`C:\dev\swirvo`, synced with git) and in CI. CI is the source of truth for release builds.
- **Hub development on Windows:** run `pnpm tauri dev` on the Windows side so it uses WebView2.
- **Disk use.** `target/` is a build cache, never part of the repo, and always safe to delete (`cargo clean`). The dev profile keeps it small (line tables only, no debug info for dependencies). It was 44 GB before that change; expect 5–8 GB with Linux, Windows and release builds. Deleting files inside WSL does **not** return space to Windows: the distro's `ext4.vhdx` grows but never shrinks by itself. Run `scripts/compact-wsl-disk.ps1` from an elevated PowerShell to compact it (it shuts WSL down, attaches the disk read-only and drops unused blocks; diskpart on Windows Home).
- **PDF → DOCX quality loop.** `cargo run --release -p swirvo-engines --example pdf_layout -- dump in.pdf out.docx` prints the analysed layout and writes the DOCX. `compare a.pdf b.pdf out.png` renders two PDFs side by side; convert the DOCX back to PDF with LibreOffice to compare against the original (docs/13 §6).
- **Licences.** `scripts/third-party-notices.py` regenerates `THIRD-PARTY-NOTICES.txt`; the Windows build runs it automatically.

## 3. Setup scripts

Read them before running. Both are idempotent.

```bash
bash scripts/setup-wsl.sh
```

```powershell
powershell -ExecutionPolicy Bypass -File scripts\setup-windows.ps1
```

Run the PowerShell script from an elevated Windows PowerShell, from a Windows copy of the repo or `\\wsl.localhost\Ubuntu\home\nabarup\swirvo\scripts\setup-windows.ps1`.

| Script | Installs |
|---|---|
| `setup-wsl.sh` | clang/lld/mold/ninja/meson/nasm; Tauri 2 Linux dependencies (WebKitGTK 4.1 …); dev packages for libvips, libheif, libavif, libjxl, libwebp, LibRaw, lcms2, libarchive and qpdf; FFmpeg and its dev libraries; LibreOffice (no GUI), Pandoc, Tesseract; X11/Wayland dev libraries and test tools (Xvfb, xdotool, weston); rustup components and Windows targets; cargo tools |
| `setup-windows.ps1` | VS 2022 Build Tools (C++ workload + Windows 11 SDK), rustup (MSVC), CMake, Ninja, LLVM, Python, FFmpeg, LibreOffice, Pandoc (all dev only), Sysinternals, Accessibility Insights, vcpkg, cargo tools |

Not covered by the scripts (install by hand when needed): Windows Performance Toolkit (from the Windows ADK) for WPR/WPA, PresentMon, and the MSIX Packaging Tool (Store).

## 4. Rust and JS tooling

| Tool | Why |
|---|---|
| `cargo-nextest` | Faster, isolated test runs |
| `cargo-deny`, `cargo-about`, `cargo-cyclonedx` | License policy, notices, SBOM |
| `cargo-xwin` | Windows MSVC cross-builds from WSL (set `XWIN_ACCEPT_LICENSE=1` after reading Microsoft's license) |
| `cargo-fuzz` (nightly) | Fuzz targets |
| `tauri-cli` 2.x | Hub dev and bundling (Tauri 3 is in alpha; stay on 2.x) |
| `just` | Task runner (`just dev`, `just bench`, `just corpus`) |
| `bacon` | Background `cargo check` / clippy |
| `typos-cli` | Spell check in CI |
| `hyperfine` | Command-line benchmarks |
| pnpm | Hub frontend |

Crates checked on crates.io on 2026-11-07 (pin exact versions at scaffold time): `windows` 0.62 · `tokio` 1.54 · `rayon` 1.12 · `crossbeam` 0.8 · `rusqlite` 0.41 · `interprocess` 2.4 · `tiny-skia` 0.12 · `smithay-client-toolkit` 0.21 · `x11rb` 0.14 · `ksni` 0.3 · `libvips` 2.3 · `pdfium-render` 0.9 · `lopdf` 0.47 · `resvg` 0.49 · `oxipng` 10.2 · `ravif` 0.13 · `libheif-rs` 3.0 · `image` 0.25 · `fast_image_resize` 6.1 · `calamine` 0.38 · `rust_xlsxwriter` 0.102 · `zip` 9.4 · `moka` 0.12 · `nucleo` 0.5 · `blake3` 1.8 · `ort` 2.0-rc (ORT 1.31) · `whisper-rs` 0.16 · `velopack` 1.3 · `notify` 8.x stable (9.0 is an RC).

## 5. Editor

VS Code (on Windows, connected to WSL with *WSL: Connect to WSL*) with rust-analyzer, Even Better TOML, CodeLLDB, the Tauri extension, ESLint + Prettier (or Biome), Error Lens, and the GitHub Actions extension. Keep rust-analyzer's `cargo.target` on Linux by default and switch to `x86_64-pc-windows-msvc` when working in `platform/win`.

## 6. Automation worth adding

| Script | Does |
|---|---|
| `add-converter` | Adds a capability: registry entry, engine adapter, cost priors, corpus fixtures and expectations, docs table row |
| `add-tool` | Adds a Tools-wheel tool: params schema, step implementation, editor hook (if any), tests |
| `latency-check` | Runs the latency rig, compares with the baseline, explains regressions |
| `license-audit` | `cargo deny` + `cargo about` + the native library list → a report |
| `release` | Changelog, version bump, packaging, signing checklist |

**Editor hooks worth adding:** `cargo fmt` after Rust edits; block adding a dependency without `cargo deny check`.
