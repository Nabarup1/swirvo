# 07 · Performance, testing and release

## 1. Reference hardware

Budgets are set on **your current laptop**, a deliberately modest 2019 machine. If it's fast there, it's fast everywhere.

| | Reference laptop | Low-end check (CI, ×2 slack) |
|---|---|---|
| CPU | Intel Core i5-9300H, 4 cores / 8 threads | Intel N100-class, 4 cores |
| GPU | NVIDIA GTX 1650 Ti (NVENC H.264/HEVC, no AV1) + Intel UHD 630 (Quick Sync H.264/HEVC) | iGPU only |
| RAM | 24 GB | 8 GB |
| OS | Windows 11 (build 26200) + Ubuntu 24.04 | Windows 11 |

## 2. Budgets (CI gates where marked ●)

| Metric | Budget | How it's measured |
|---|---|---|
| ● Trigger key → first wheel frame on screen | p50 ≤ 12 ms · p95 ≤ 25 ms · p99 ≤ 40 ms | ETW: `WM_INPUT` timestamp → probe shown → DragEnter → composition commit → DWM present (frame statistics) |
| ● DragEnter → labelled wedges | ≤ 2 ms (extension) · ≤ 8 ms with sniff | `tracing` span |
| Hover change → repaint | ≤ 1 frame | Composition frame statistics |
| ● `Drop()` returns (source unblocked) | ≤ 5 ms | ETW |
| Drop → receipt or progress visible | ≤ 100 ms | ETW |
| ● Agent idle | 0.0 % CPU · 0 timer wakeups · ≤ 25 MB private working set | WPR/WPA + `powercfg /energy`; Linux `powertop`, `smem` |
| Agent ready after login | ≤ 150 ms | ETW |
| Hub first paint | ≤ 900 ms cold · ≤ 350 ms warm | Performance API + ETW |
| ● 12 MP HEIC → JPG | ≤ 250 ms | Corpus benchmark |
| 20 MB PNG screenshot → lossless WebP | ≤ 600 ms | |
| 1 GB MOV → MP4 (remux) | ≤ 3 s (I/O bound) | |
| 1080p60, 1 min H.264 → smaller H.264 (NVENC) | ≥ 6× real time | |
| 30-page DOCX → PDF (warm LibreOffice) | ≤ 2 s | |
| ● 500 JPEG → WebP batch | ≥ 25 files/s | |
| Peak memory per image job | ≤ 3× decoded size, capped | Job-object accounting |
| Crash-free sessions | ≥ 99.9 % | Local crash counters (plus opt-in reports) |
| ● Corpus success rate | ≥ 99.5 % | Golden tests |

A CI gate fails the build when the p95 of a gated metric regresses by more than 10 % against the main branch's rolling baseline.

## 3. Measuring latency the right way

- **Timestamps start at the input event, not at our code:** use the `WM_INPUT` message time (`GetMessageTime`, aligned with QPC) and Linux XI2 event times.
- **Count frames on screen, not frames drawn:** DWM frame statistics (`DwmGetCompositionTimingInfo`) and PresentMon on Windows; `wp_presentation` feedback on Wayland.
- **Automated rig:** a Windows test harness drives real drags with `SendInput` from an Explorer window (window and coordinates from UI Automation), taps Shift, and records the ETW trace. 200 trials per run produce p50/p95/p99. The same harness checks detection reliability (≥ 99 %).
- **Latency HUD** (debug builds and a hidden setting) shows trigger-to-frame time in milliseconds on the wheel itself.

## 4. Test strategy

| Layer | What | Tools |
|---|---|---|
| Unit | Pure crates: formats, graph, scheduler, wheel, store | `cargo nextest` |
| Property | Planner (every path valid; optimal against brute force on small graphs), gesture FSM invariants ([04](04-algorithms.md) §5), hit-test angles, rename cycles, config migrations | `proptest` |
| Snapshot | IPC schemas, plan explanations, CLI `--json` output | `insta` |
| Golden corpus | ~600 fixtures across categories; per-output property checks ([05](05-conversion-engines.md) §12) | `tests/corpus` + a runner |
| Fuzzing | Format detection, subtitle parsers, rename templates, IPC decoder, CSV sniffer, PDF layout, ZIP name decoding | `cargo-fuzz` (libFuzzer), nightly for 1 h; corpus kept in the repo |
| Sandbox | A worker tries network, file and process access and must fail | Integration tests on both OSes |
| Overlay rendering | The wheel scene rendered to PNG with tiny-skia, then a pixel diff against references (light/dark, S/M/L, 2–8 wedges) | Image diff with tolerance |
| Hub UI | Components and flows against the Vite dev server; end-to-end through `tauri-driver` (WebDriver) | Playwright, WebdriverIO |
| Windows end-to-end | Real Explorer drags with `SendInput`, toasts and context menu via UI Automation | Custom harness on a GitHub Windows runner (verify interactive-desktop support) or a self-hosted VM |
| Linux end-to-end | X11 path on Xvfb/Xephyr; Wayland path on headless Sway (`WLR_BACKENDS=headless`); drags with `xdotool`/`ydotool` | CI containers |
| Accessibility | Windows: Accessibility Insights FastPass on the overlay and Hub; axe-core in the Hub; Orca smoke test | Manual + automated |
| Manual matrix | Windows 11 x64 and ARM64, Windows 10, Ubuntu GNOME (X11/Wayland), Fedora KDE Wayland, Arch Hyprland; mixed DPI, HDR monitor, touch | Release checklist |

**Corpus rules:** only files we generated ourselves or that are clearly public-domain or permissively licensed. Each fixture has a sidecar `.expect.toml` (expected format, dimensions, duration, pages, tolerance). Large fixtures live in Git LFS or are generated during CI with FFmpeg's synthetic sources (`testsrc2`, `sine`).

## 5. CI/CD (GitHub Actions)

```text
on: push, pull_request
jobs:
  lint:        fmt · clippy -D warnings · typos · cargo deny (licenses, bans, advisories) · eslint/tsc for apps/hub
  test-linux:  ubuntu-22.04 (oldest glibc) · nextest · proptest · corpus subset · fuzz smoke (60 s per target)
  test-win:    windows-2025 · nextest · corpus subset · overlay pixel tests · end-to-end gesture smoke
  build:       matrix { x86_64-pc-windows-msvc, aarch64-pc-windows-msvc, x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu }
               sccache · vcpkg binary cache · artifacts
  bench:       criterion/divan micro-benchmarks + corpus macro-benchmarks → compare with the baseline (gate ●)
  package:     (tags) MSIX · Velopack · AppImage · Flatpak · deb · rpm · SBOM · THIRD-PARTY-NOTICES
  sign:        (tags) Windows Authenticode (managed signing service) · Linux repo GPG · pack signatures (ed25519)
  release:     nightly → beta → stable channels; staged rollout 5 % → 25 % → 100 %
nightly:       full corpus · 1 h fuzzing per target · full end-to-end matrix · soak test (10k-file batch, memory flat)
```

## 6. Signing and SmartScreen

- **Microsoft Store** builds are signed by the Store; this is the simplest path to trust.
- **Direct downloads:** sign with a managed cloud signing service (for example Microsoft's Azure Trusted Signing; check its current name, price and eligibility rules for individual developers in your country) or an OV certificate held in a hardware token or cloud HSM. SmartScreen reputation builds over time whichever you choose.
- Sign every PE file: the agent, worker, CLI, Hub and the DLLs we build. Timestamp every signature.
- Linux: GPG-signed apt/dnf repositories; Flathub signs Flatpaks.
- Engine packs: ed25519 signatures checked against a key compiled into the agent; that key is stored offline and the signing is done in CI with an HSM-backed secret.

## 7. Versioning and channels

- App: SemVer. Packs: the upstream version plus our build number (`7.1.2+swirvo.3`).
- **nightly** (every main commit, Velopack channel), **beta** (weekly), **stable** (every 4–6 weeks, plus hotfixes).
- Automatic rollout pause when the crash rate on a new version exceeds 2× the previous version's.

## 8. Crash reporting and support

- Minidumps are captured out of process (`crash-handler` + `minidumper`), stored locally, and offered for sending at the next start (opt-in; the dump is shown before it's sent).
- Symbols (PDBs, split DWARF) are uploaded to a private symbol store per release.
- The **diagnostics bundle** ([03](03-system-architecture.md) §15) is the only support artifact we ask for.
