# Swirvo: project guide

Swirvo is a native Windows + Linux file converter. You start dragging a file, tap a modifier key and drop it on a radial wheel. The repo has **early code** (core crates, CLI, Windows overlay spike); the plan in `docs/` stays the source of truth, and code that disagrees with it needs a doc update in the same change. README "What works today" tracks what exists.

## Read first
- Architecture and process model: `docs/03-system-architecture.md`
- Wheel behavior, timings and design tokens: `docs/02-ux-ui-spec.md`
- Performance budgets (these are CI gates): `docs/07-performance-testing-release.md`
- Dependency licensing rules: `docs/05-conversion-engines.md` §9
- PDF → DOCX layout engine: `docs/13-pdf-reconstruction.md`
- DOCX → PDF layout engine: `docs/16-docx-to-pdf.md`
- PDF editing tools, tool options and the protection/permission rules: `docs/17-pdf-and-document-tools.md`
- Review, legal, Store and business notes are kept locally in `private/` (git-ignored, never published).

## Non-negotiables
1. **Never touch originals.** Write outputs to a temp file in the destination folder, then rename it into place atomically without replacing anything. Never overwrite, delete or move a user file unless the user picked an action that says so (Move, Replace original).
2. **Never block the UI/overlay thread.** No I/O, no lock held longer than ~50 µs, no allocation in the hot path. The wheel must paint within one frame of the trigger.
3. **The agent never parses user content.** Files are decoded and encoded only in sandboxed `swirvo-worker` processes, and workers have no network access.
4. **Offline by default.** Use the network only for explicit, visible actions: update check, engine-pack download, send to nearby device. All three obey Offline mode.
5. **Licensing.** No AGPL anywhere, and no GPL code linked into our binaries. GPL tools (x264/x265 builds, Pandoc, Calibre) are optional, separately downloaded executables run as child processes. Run `cargo deny check` before adding a dependency. Model weights must allow commercial use (no CC BY-NC). Shipped builds include `THIRD-PARTY-NOTICES.txt` from `scripts/third-party-notices.py` (it also covers assets bundled inside crates, such as hayro's fonts). Never bundle Microsoft fonts, and never use another product's name, icon or marketing copy.
6. **Paths are not strings.** Use `PathBuf`/`OsString` end to end. Support Windows long paths (`\\?\`), Unicode normalization differences and non-UTF-8 names on Linux.
7. **No shell strings.** Spawn processes with argument vectors only. Never build a command line by interpolating strings.
8. **Privacy in logs.** Log file names and paths only at `debug` level; the diagnostics bundle redacts them.

## Layout (docs/03 §5)
Built: `crates/{core,formats,graph,wheel,engines,jobs,render,ui,board,cli,platform/win}`. `ui` is our immediate-mode toolkit and `board` holds the Board and page-picker apps (docs/12); both are portable and snapshot-tested on Linux. `jobs` currently stands in for the planned scheduler + store (JSON frecency file until SQLite lands). `platform/win` has a portable `model.rs` (unit-tested on Linux) and a thin Win32 `agent.rs`.
Planned: scheduler, worker, store, ipc, platform/linux (also hosts the Board), agent, shellext-win; `integrations/linux`, `packaging/`, `tests/corpus/`.

## Known gaps vs. the non-negotiables
- Rule 3: conversions and tools run in a worker process (`Swirvo.exe --worker`, `crates/jobs/src/worker.rs`) inside a Job Object with a memory cap and process mitigations (docs/03 §9). Still open: the worker isn't network- or file-isolated (AppContainer, L2), and thumbnails and tool previews still decode inside the Board process.
- ETW latency events are not wired yet; the agent prints trigger→wheel latency to the console.
- The product name is **Swirvo** . The old working name must not appear anywhere: code, docs, UI, commits, packaging.

## Conventions
- Rust 2024 edition, stable toolchain. Run `cargo fmt` and `cargo clippy --all-targets --all-features -- -D warnings`.
- Errors: `thiserror` in libraries, `anyhow` only in binaries. No `unwrap()`/`expect()` outside tests unless a comment states the invariant.
- Tracing: a `tracing` span around every job step. On Windows, latency-critical spans are also emitted as ETW events.
- Tests: `cargo nextest run`. Use `proptest` for the planner, wheel hit-testing, rename resolution and the gesture state machine. Golden-file conversion tests live in `tests/corpus/`.
- UI (Board, page picker, future editors): `swirvo-ui` widgets and the tokens in `crates/ui/src/theme.rs` only (docs/02 §14). No web view. Review layouts with `cargo run -p swirvo-board --example snapshot -- <dir>`.
- OS engines first: prefer what Windows/Linux already ship (OCR, WIC codecs, tar.exe) over bundling a library (docs/12 §2).
- Commits: Conventional Commits (`feat(graph): …`). Commit only when asked.

## Dev environment
Windows 11 + WSL2 Ubuntu 24.04. The repo lives in WSL and Linux builds are native. Windows builds use `cargo xwin` for fast iteration and a native MSVC toolchain for packaging. See `docs/09-dev-environment.md` and `scripts/`.

Without MSVC/xwin, compile-check Windows code from WSL with `--no-default-features` (drops the C-built zstd): `cargo clippy -p swirvo-platform-win --all-targets --no-default-features --target x86_64-pc-windows-msvc -- -D warnings`. To produce a runnable, standalone `dist/Swirvo/Swirvo.exe` from WSL (rust-lld + the Windows-side MSVC/SDK libs, static CRT): `scripts/build-windows-exe.sh`. Render budget: `cargo test --release -p swirvo-render repaint_budget -- --ignored --nocapture --test-threads=1`. `target/` is a cache: `cargo clean` is always safe when it grows (docs/09 §2).
