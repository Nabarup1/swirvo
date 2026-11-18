# 08 · Roadmap and risks

**Assumption:** one full-time developer. Durations scale linearly with fewer hours. Windows ships first, because it is the larger audience and holds the riskiest technology. Linux X11 follows; Linux Wayland comes after that.

## Phase 0: Prove the risky parts (weeks 1–3)

| Spike | Question | Pass criteria | Estimate |
|---|---|---|---|
| **S1 · Windows drag probe** | Can we reliably detect *file drag + Shift* and get `DragEnter` on an invisible probe window, for drags from Explorer, the desktop, Edge/Chrome, Outlook, VS Code and 7-Zip? | ≥ 99 % detection in 200 scripted trials; p95 ≤ 25 ms to the first frame; normal drops unaffected; Defender scan clean | 4–5 days |
| **S2 · Overlay compositing** | Composition overlay with host-backdrop glass plus a tiny-skia content layer | First frame ≤ 1 vsync after show; 120 Hz animations without dropped frames; Windows 10 Solid fallback works | 3 days |
| **S3 · Linux X11 + Wayland** | XI2 + XDND probe on GNOME X11 and KDE X11; layer-shell hot edge on KDE Wayland and Sway | Drags from Nautilus, Dolphin and Firefox land; the edge grows within one frame | 4 days |
| **S4 · Native dependencies on Windows** | libvips + libheif + libavif + libjxl + PDFium through vcpkg with MSVC | Reproducible CI build ≤ 25 min with cache; core ≤ 30 MB | 3 days |
| **S5 · Minimal LGPL FFmpeg** | Custom build with `h264_mf`/`hevc_mf`, NVENC/QSV/AMF, libopus, libvpx, dav1d, SVT-AV1, LAME | ≤ 25 MB per architecture; hardware probe correct on the GTX 1650 Ti and UHD 630 | 3 days |
| **S6 · LibreOffice warm pool** | UNO conversion latency, crash recovery, profile isolation | Warm DOCX → PDF ≤ 2 s; recovers after `kill -9` | 2 days |
| **S7 · Hub shell + IPC** | Tauri 2 window ⇄ agent over named pipe/UDS; progress streaming | 1k events/s coalesced with no lost terminal states; idle Hub ≤ 120 MB | 2 days |

**Go/no-go:** if S1 fails, the Windows hero gesture changes to **hot edge + context menu + selection shortcut** (the UX spec already covers all three), and the product still ships.

## Phase 1: Windows MVP / alpha (weeks 4–13)

- Agent: raw input, probe, Convert and Actions wheels, receipts, tray flyout, toasts, journal, history and undo
- Engines: libvips family, PDFium, qpdf/lopdf, libarchive, data formats, subtitles, text; Media pack (FFmpeg) with remux fast paths
- Conversions: images, audio, video, PDF ⇄ images, archives, data, subtitles (≈ 200 options)
- Tools (P1 list in [01](01-product-spec.md) §8): strip metadata, extract audio, mute, PDF merge/split/rotate/extract text, extract and compress archives
- Classic context menu, Send To, `swirvo` CLI (`convert`, `formats`, `plan`)
- Settings basics, Velopack installer, nightly channel

**Exit criteria:** all gated budgets met on the reference laptop; corpus pass rate ≥ 99 %; 10 alpha users use it daily for 2 weeks with no data-loss bugs.

## Phase 2: v1.0, Windows stable + Linux X11 beta (weeks 14–23)

- Tools wheel and the P2 tools; editors: Trim, Crop, PDF Organizer, Annotate, Metadata
- Office pack (LibreOffice) and Docs pack (Pandoc); 300+ conversions; PDF → DOCX/XLSX reconstruction (beta)
- Presets and sub-rings, frecency with pinned slots, size estimates, the selection shortcut, double-tap mode, drag-dwell
- Windows 11 modern context menu (sparse package), Share, Shelf v1, onboarding playground
- Microsoft Store + winget; L1 sandbox on both OSes
- Linux X11: overlay, XDND, tray, notifications, Nautilus/Dolphin/Nemo/Thunar integrations, deb/rpm/AppImage

**Exit criteria:** 99.9 % crash-free sessions on beta; Store certification passed; Linux X11 parity checklist green.

## Phase 3: v1.5, Wayland, AI, automation (weeks 24–33)

- Linux Wayland: layer-shell hot edge, GlobalShortcuts portal, clipboard path, Flatpak; GNOME Shell extension (research)
- AI packs: OCR (Windows.Media.Ocr / Tesseract), transcription and auto-subtitles, background removal, upscaling, face and PII redaction, word bleeping
- Smart compress (target size, VMAF-targeted presets), chunked parallel AV1
- Recipes, watch folders, command bar, Send to nearby device (LocalSend v2), Shelf v2
- True PDF redaction, OCR to searchable PDF

## Phase 4: v2 and beyond

The plugin SDK (WASM components), a recipe gallery, the L2 sandbox (AppContainer + in-worker libav*), a Media Foundation engine, a PowerToys Command Palette extension, a KRunner plugin and GNOME search provider, touch gestures, localization into 10+ languages, and a decision on a macOS build.

## First ten tickets (after you approve the plan)

1. Scaffold the Cargo workspace and `apps/hub` with the layout from [03](03-system-architecture.md) §5; CI lint and test jobs; `cargo-deny` configuration.
2. `crates/core`: ids, `Format`, `Capability`, `JobSpec`, `Plan`, errors.
3. `crates/formats`: format table → `build.rs` → phf + signature trie; fuzz target.
4. `crates/graph`: Dijkstra, reachability bitsets, explanations; property tests against brute force.
5. `crates/wheel`: geometry, hit-testing, FSM; property tests; a PNG renderer for snapshots.
6. **S1 spike** in `crates/platform/win`: raw input, probe window, `IDropTarget`, latency ETW events.
7. **S2 spike**: composition overlay plus the tiny-skia layer.
8. `crates/store`: SQLite schema, migrations, frecency, journal.
9. `crates/worker` + the vips engine: the first end-to-end HEIC → JPG through the agent.
10. Latency rig ([07](07-performance-testing-release.md) §3) running in CI.

**Progress (7 Nov 2026).**
- Done: 1 (workspace, CI, `cargo-deny`; `apps/hub` not yet), 2, 3 (the format table is a Rust array plus a runtime trie rather than `build.rs` + phf; same lookups, simpler build), 4, 5 (PNG renderer via `swirvo wheel --png`).
- 6 (S1) code complete: Raw Input triggers, alpha-1 probe with a 1 px cursor nudge, `IDropTarget` + `IDropTargetHelper`, layered wheel via `UpdateLayeredWindow`, drop → job. Trigger→wheel latency prints to the console; ETW events and the 200-trial rig are still to do. Needs its first run on real Windows.
- 7 (S2) partly: the tiny-skia layer exists with two cached layers (background; wheel at rest) so a hover repaint costs ≈ 0.5 ms at 150 %. The composition (DirectComposition/Windows.UI.Composition) path is not started; the spike presents with `UpdateLayeredWindow`.
- 8 interim: frecency persists to a JSON file (atomic replace) until the SQLite store lands.
- 9 interim: Core engines are pure Rust and run in-process in a `jobs` crate; the sandboxed worker and the vips engine are next.

## Risk register

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| R1 | The Windows drag probe is unreliable for some drag sources | Medium | High | Spike S1 first; fallbacks are hot edge, context menu and the selection shortcut; a per-app allow/deny list |
| R2 | Antivirus or SmartScreen flags the agent | Medium | High | Raw input instead of hooks; signing; Microsoft submission; Store distribution |
| R3 | Codec patents and licensing | Medium | High | OS and hardware codecs, royalty-free defaults, opt-in GPL pack, legal review before charging ([05](05-conversion-engines.md) §10) |
| R4 | LibreOffice size and fidelity | High | Medium | Optional pack, warm pool, metric-compatible fonts, webview printing for HTML/MD |
| R5 | PDF → DOCX quality falls short of expectations | High | Medium | Layout engine built (docs/13) with a visual round-trip harness ●; next: stream tables, scans with OCR boxes; compare against references; let users report a file locally |
| R6 | Wayland limits (no global input, GNOME has no layer-shell) | Certain | Medium | Hot-edge design, portal shortcuts, clipboard path, Shell extension, honest docs |
| R7 | Scope creep (40+ tools) | High | High | Phase gates; tools ship only with corpus coverage; P3/P4 stay parked |
| R8 | Performance regressions creep in | Medium | Medium | CI gates ●, latency rig, a perf review for every PR touching the hot path |
| R9 | WebKitGTK quirks in the Hub on Linux | Medium | Low | Keep the Hub UI simple; no `<video>` (frames come from our engine); test on Linux every sprint |
| R10 | Native build complexity (vcpkg, MSVC, cross-arch) | Medium | Medium | Spike S4; binary caching; prebuilt PDFium; pinned baselines |
| R11 | Explorer or Windows updates change behavior | Low | Medium | Rig in CI against Insider builds monthly; feature flags for each integration |
| R12 | Solo-developer bandwidth | High | High | Windows first; scripts for repetitive work (adding converters, packs); a strict "first ten tickets" focus |
| R13 | Name conflict with an existing brand. **Done:** named Swirvo after a clearance search | High | High | Rename before any public listing, with a clearance search and a Store name reservation; a distinct icon and accent colour |
| R14 | Store review: startup task, full trust, runtime downloads | Medium | Medium | MSIX with a user-controllable startup task, no self-update and no engine-pack downloads in the Store build |
| R15 | Missing licence notices or codec royalties in a paid product | Medium | High | Generated notices shipped with every build ●; Media Foundation encoders instead of bundled H.264/HEVC/AAC encoders |
