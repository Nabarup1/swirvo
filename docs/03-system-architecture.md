# 03 · System architecture

## 1. Quality attributes, ranked

1. **Interaction latency**: the wheel paints within one frame; feedback within 100 ms.
2. **Safety of user data**: originals never modified; outputs atomic; crash-safe.
3. **Robustness**: one malformed file can't take the app down (crash isolation, watchdogs, circuit breakers).
4. **Footprint**: the always-on agent stays at ≤ 25 MB and 0 % idle CPU, with no timers firing while idle.
5. **Throughput**: batches use the hardware well (P-cores and E-cores, GPU encoders, I/O) without making the PC sluggish.
6. **Portability**: one core and thin platform layers (Windows, Linux X11, Linux Wayland).
7. **Maintainability**: pure, testable crates; platform code only at the edges.

## 2. Context

```text
 ┌────────────────────────── user's desktop session ───────────────────────────┐
 │                                                                             │
 │  Explorer / Files /     OLE drag · XDND · Wayland DnD   ┌────────────────┐  │
 │  browser / mail ───────────────────────────────────────►│ swirvo-agent │◄─┼── swirvo CLI
 │                                                         │ gesture        │  │
 │  context menu ─────────────── COM / D-Bus / CLI ───────►│ overlay        │  │
 │                                                         │ planner        │  │
 │                                                         │ scheduler      │  │
 │                                                         │ journal · IPC  │  │
 │                                                         └───┬──────▲─────┘  │
 │                          spawn + broker file handles        │      │ IPC    │
 │        ┌────────────────────────────────────────────────────▼─┐  ┌─┴──────┐ │
 │        │ swirvo-worker ×N   (sandboxed, no network)          │  │  hub   │ │
 │        │ libvips · PDFium · qpdf · libarchive · Arrow · ORT    │  │ Tauri, │ │
 │        │ child tools: ffmpeg · soffice (warm pool) · pandoc    │  │ on     │ │
 │        └───────────────────────────────────────────────────────┘  │ demand │ │
 │                                                                   └────────┘ │
 └─────────────────────────────────────────────────────────────────────────────┘
```

## 3. Process model

| Process | Lifetime | Responsibilities | Never does | Budget |
|---|---|---|---|---|
| `swirvo-agent` | Login → logout; restarted by the OS on crash | Gesture detection, wheel/shelf/receipt overlays, tray, notifications, planner, scheduler, journal, IPC server; on Windows also the shell-extension COM server (out of process) | Decode user files; host a webview | ≤ 25 MB private working set; 0 % idle CPU; no wakeups while idle |
| `swirvo-worker` | Built as `Swirvo.exe --worker`, one per run (§9); a warm pool is a later optimisation | Runs job steps; hosts in-process engines; spawns tool processes | Touch the network; open paths it wasn't given | Per-job memory cap (default min(2 GB, 25 % of RAM)) |
| Tool processes (`ffmpeg`, `soffice`, `pandoc`, `whisper`) | Per step, or a warm pool (`soffice`) | Heavy codec and document work | — | Inherit the worker's sandbox and job object/cgroup |
| `swirvo-hub` | On demand; can be kept warm (§4) | Settings, Queue, History, Recipes, Engines, tool editors | Conversion logic | ~80–150 MB while open; 0 when closed |
| `swirvo` (CLI) | Per command | Scripting: talks to the agent, or runs `--standalone` | — | — |
| Shell integration | Windows: COM server in the agent; Linux: file-manager extension scripts | Context-menu entries | Run heavy code inside `explorer.exe` or `nautilus` | `GetState` answers in < 1 ms, from the extension alone |

**Why the agent and the Hub are separate processes:** a webview costs 80–150 MB. The agent must be invisible in Task Manager, so the Hub exists only while it's needed.

## 4. Inside the agent

| Thread | Role | Rules |
|---|---|---|
| **UI** (main) | Win32 message loop or Wayland/X11 event loop: raw input, OLE drop target, overlay rendering, tray | No I/O, no blocking, no locks held over ~50 µs, no allocation in the hot path (pre-allocated scenes and glyph cache) |
| **Shell** (COM STA, Windows) | Explorer selection queries, toasts, share sheet, `IFileOperation` | May block on COM; never blocks UI |
| **Runtime** (tokio, 2 threads) | IPC server, worker supervision, file watchers, journal writes, engine-pack downloads | Async only; no CPU-heavy work |
| **Planner** | Runs inline on UI for wheel population (µs-scale, allocation-free) and on Runtime for full plans | Must finish in < 200 µs on UI |

Messages between threads go through bounded lock-free queues (`crossbeam::ArrayQueue`, MPSC for job submission). The UI thread never waits on another thread.

**Adaptive Hub warm-up:** if an editor was opened in the last 30 minutes, the agent keeps a hidden Hub process alive for 10 more minutes, so editors open in about 300 ms instead of about 900 ms.

## 5. Workspace layout

```text
swirvo/
├─ crates/
│  ├─ core/            # ids, Format, Category, Capability, JobSpec, Plan, Preset, errors (no I/O)
│  ├─ formats/         # static format registry (build.rs → phf), detection trie, container probes
│  ├─ graph/           # conversion graph, Dijkstra planner, reachability bitsets, cost model
│  ├─ scheduler/       # queues, resource governor, backfilling, DAG executor, estimators
│  ├─ engines/         # one crate per engine, feature-gated
│  │  ├─ vips/  pdfium/  pdfops/  ffmpeg/  office/  pandoc/  archive/
│  │  ├─ data/  subtitles/  fonts/  text/  ocr/  ai/
│  ├─ worker/          # worker main, sandbox bootstrap, step runner
│  ├─ store/           # SQLite (history, frecency, journal, perf models), config (TOML), migrations
│  ├─ ipc/             # framing, messages, versioning, transports (named pipe / UDS)
│  ├─ wheel/           # wheel model: layout, hit-testing, gesture FSM, animation curves (pure)
│  ├─ render/          # tiny-skia renderer + cosmic-text glyph cache → RGBA layers
│  ├─ platform/
│  │  ├─ win/          # raw input, OLE probe/drop target, composition overlay, shell bridge,
│  │  │                # toasts, tray, share, IFileOperation, power/QoS, job objects, AppContainer
│  │  └─ linux/        # XI2, XDND, layer-shell, SNI tray, notifications, portals, Landlock/seccomp
│  ├─ agent/           # binary: wires everything together
│  ├─ cli/             # binary: `swirvo`
│  └─ shellext-win/    # IExplorerCommand implementation used by the agent's COM server
├─ crates/ui/          # swirvo-ui: immediate-mode toolkit (docs/12 §5)
├─ crates/board/       # the Board and the page picker, as swirvo-ui apps
├─ integrations/linux/ # nautilus-python, Dolphin service menu, Nemo action, Thunar UCA
├─ packaging/          # msix/, velopack/, flatpak/, appimage/, deb/, rpm/, aur/
├─ tests/corpus/       # golden-file fixtures + expected properties
├─ docs/  scripts/
```

**Dependency rule (enforced with `cargo-deny`'s ban list and a CI script):** `core ← formats ← graph ← scheduler ← agent`. `engines/*` depend only on `core`. `platform/*` is used only by `agent`, `worker` and `hub`. Nothing in `core`, `formats`, `graph`, `scheduler` or `wheel` may depend on an OS crate, so they all build and test on any host.

## 6. Domain model

```rust
pub struct FormatId(u16);                 // dense index → bitsets
pub struct Format {
    pub id: FormatId, pub key: &'static str,          // "heic"
    pub category: Category, pub mimes: &'static [&'static str],
    pub exts: &'static [&'static str], pub traits: FormatTraits, // LOSSY|ALPHA|ANIMATION|MULTIPAGE|HDR|CONTAINER
}
pub struct Capability {
    pub engine: EngineId, pub from: FormatId, pub to: FormatId, pub op: OpKind,
    pub loss: Loss,                         // None | Lossless | Lossy { typical_q }
    pub cost: CostModel,                    // per-unit time model + fixed overhead
    pub guards: GuardSet,                   // e.g. HAS_AUDIO, NO_ALPHA_OR_FLATTEN, PAGES<=N
    pub fast_path: Option<FastPath>,        // Remux | DctTransform | ObjectCopy | LosslessRecompress
}
pub struct JobSpec {
    pub id: JobId, pub inputs: SmallVec<[InputRef; 4]>,
    pub intent: Intent,                     // Convert{to, preset} | Tool{tool, params} | Action{..} | Recipe{id}
    pub origin: Origin,                     // Wheel | Menu | Cli | Watch | Hub
    pub priority: Priority,                 // Interactive | Batch | Background
}
pub struct Plan { pub steps: Vec<Step>, pub edges: Vec<(StepIx, StepIx)>, pub estimate: Estimate }
pub struct Step { pub op: OpKind, pub engine: EngineId, pub params: Params,
                  pub inputs: Vec<Artifact>, pub outputs: Vec<Artifact>, pub demand: ResourceVec }

pub trait Engine: Send + Sync {
    fn id(&self) -> EngineId;
    fn capabilities(&self) -> &'static [Capability];
    fn probe(&self, input: &InputHandle) -> Result<ProbeInfo>;           // cheap, optional
    fn run(&self, step: &Step, io: &mut StepIo, ctl: &StepControl) -> Result<StepOutput>;
}
pub struct StepControl { pub cancel: CancellationToken, pub progress: ProgressSink, pub budget: ResourceBudget }
```

## 7. Job lifecycle

| # | Stage | Where | Budget | What happens |
|---|---|---|---|---|
| 1 | Drop | UI thread | ≤ 5 ms until `Drop()` returns | Build a `JobSpec` from the paths and the wedge, push it to an MPSC queue, return `DROPEFFECT_COPY`. The drag source is unblocked straight away |
| 2 | Resolve | Runtime | ≤ 5 ms per file (local) | `stat`, FileKey, cloud-placeholder check, magic-byte sniff if the wheel didn't already do one |
| 3 | Plan | Runtime | µs | Dijkstra with edge guards for each input; build the Plan DAG; estimate time and size |
| 4 | Admit | Scheduler | µs | Pre-flight checks (disk space, permissions); **reserve output names** in a concurrent set so parallel jobs can't collide; queue by priority |
| 5 | Dispatch | Scheduler → worker | ≤ 1 ms with a warm worker | Lease a worker; broker file handles; send `RunStep` |
| 6 | Execute | Worker | — | Engine runs; progress streams back, coalesced to ≤ 15 Hz |
| 7 | Verify | Worker | ≤ 5 % of the step | Output decodes; dimensions, duration and page count match; size > 0 |
| 8 | Commit | Agent | ≤ 1 ms | Atomic no-replace rename from temp to final name (`MoveFileExW` without `REPLACE_EXISTING` / `renameat2(RENAME_NOREPLACE)`); dates; journal entry with its inverse |
| 9 | Notify | UI / Shell | — | Receipt or toast; history; frecency bump; perf-model update |

The name reservation set prevents collisions *between Swirvo jobs*. Final creation still uses `CREATE_NEW`/`O_EXCL`, so collisions with other programs are caught too. On Windows, case-insensitive comparison follows NTFS rules.

## 8. Workers

- **Protocol** over a private pipe: `Hello{ver, caps}` · `RunStep{step, handles}` · `Progress{done, total}` · `Log` · `StepResult{ok | err, outputs, metrics}` · `Cancel` · `Shutdown`.
- **Heartbeat** every 1 s. **Watchdog**: no progress for max(30 s, 3 × expected interval) → kill the process tree (job object / process group), retry once with safe flags (hardware acceleration off), then fail with diagnostics.
- **Circuit breaker**: three crashes for an (engine, format) pair within 10 minutes → mark it unhealthy for the session and route around it if the graph has an alternative.
- **Lazy engines**: native libraries are delay-loaded, so a worker that only resizes images never maps FFmpeg or PDFium.
- **Process priority**: interactive steps run at normal priority with high QoS. Batch and background steps run with EcoQoS and `PROCESS_MODE_BACKGROUND_BEGIN` on Windows, or nice 10 plus `SCHED_IDLE` I/O class on Linux. On hybrid Intel CPUs that puts background work on the E-cores, which keeps the fans quiet.

## 9. Sandboxing (staged)

**Built (2026-11-12): L0+, the worker process** (`crates/jobs/src/worker.rs`). Conversions and tools from the wheel, the Board and the page picker run in `Swirvo.exe --worker` (the CLI: `swirvo --worker`), one worker per run, talking line-delimited JSON over its stdin/stdout:
- **Protocol.** One request line (`convert` with each input's target key and the options, `tool`, `page_op`, `organise`), then `cancel` or stdin closing to cancel. Replies: `started` per job, each job's event, then `converted`, `finished` (a tool's result or message) or `error`. Starting a worker, engine discovery included, costs about 0.1 s.
- **Crashes.** If the worker's output ends without a final reply, the jobs it had started fail with "the converter stopped unexpectedly on this file" and the jobs it hadn't reached run again in a fresh worker (up to 3 restarts per run). A crashed tool run reports "the tool stopped unexpectedly". Cancel asks the worker to stop and kills it after 3 s.
- **Windows sandbox.**
  - A Job Object with kill-on-close (workers die with the app), a per-process memory cap (the lesser of 4 GB and ¾ of RAM), and no crash dialogs.
  - Below-normal priority, no console window.
  - Process mitigations set by the worker at start: no images from remote shares or low-integrity files, System32 first, extension points (AppInit DLLs, legacy hooks) disabled, strict handle checks.
  - LibreOffice, FFmpeg and Word/Excel/PowerPoint run as the worker's children: they inherit the job and its memory cap, not the mitigations.
- **Not isolated yet: network and file system.** The worker runs with the user's token, so it *could* open other files or connect out; our engines don't. L1 and L2 below close that.
- **Still in the calling process:** thumbnails and the Board's tool previews (they decode one page or picture; a crash there takes down the Board window, not the agent), and the CLI unless it's asked to use a worker.
- **Linux:** the same worker runs when configured, without restrictions yet (L1 below).

| Level | Windows | Linux | Phase |
|---|---|---|---|
| **L1** | Job object (kill on close, memory cap, no breakaway); restricted token at Low integrity; outputs go to a Low-labelled `.swirvo-tmp-*` folder in the destination that we create; outbound network blocked by a firewall rule added at install | `PR_SET_NO_NEW_PRIVS`, `PR_SET_PDEATHSIG`, rlimits, **Landlock** (read inputs, write only the temp folder), **seccomp** (no `socket(AF_INET*)`, `ptrace` or `mount`), cgroup v2 limits through a systemd scope | P1–P2 |
| **L2** | **AppContainer** with no capabilities, so there's no network by construction. The agent brokers file handles (`DuplicateHandle`). In-process engines read through those handles; FFmpeg moves into the worker as libav* with a custom `AVIOContext` over brokered handles, so it stays seekable without path access | User + network namespace (`unshare`), where distro policy allows (Ubuntu 24.04 needs an AppArmor profile for unprivileged user namespaces) | P3–P4 |

## 10. IPC

- **Transport:** Windows `\\.\pipe\swirvo-{user SID}` with a DACL for the current user only and `PIPE_REJECT_REMOTE_CLIENTS`. Linux `$XDG_RUNTIME_DIR/swirvo/agent.sock` with mode 0600, checked with `SO_PEERCRED`.
- **Framing:** a `u32` length prefix followed by a `postcard`-encoded message enum. The `Hello` handshake carries the protocol's semantic version. A CLI with a mismatched version falls back to standalone mode.
- **Server push:** job events go out through per-job *latest-value slots*, flushed at ≤ 15 Hz. Terminal events (Done, Failed) are never coalesced away.
- **Hub ↔ agent:** the Hub's Rust side keeps one connection open and forwards events to the web UI through Tauri channels.

## 11. Storage

| Data | Windows | Linux | Technology |
|---|---|---|---|
| Config | `%APPDATA%\Swirvo\config.toml` | `$XDG_CONFIG_HOME/swirvo/config.toml` | TOML, schema-versioned, hot-reloaded |
| History, frecency, recipes, presets, journal, perf models | `%LOCALAPPDATA%\Swirvo\swirvo.db` | `$XDG_DATA_HOME/swirvo/swirvo.db` | SQLite in WAL mode, FTS5, daily `VACUUM INTO` backup |
| Engine packs | `%LOCALAPPDATA%\Swirvo\engines\{pack}\{version}` | `$XDG_DATA_HOME/swirvo/engines/…` | Signed archives, versions side by side |
| Thumbnail and probe cache | `%LOCALAPPDATA%\Swirvo\Cache` | `$XDG_CACHE_HOME/swirvo` | Content-addressed files plus an index; LRU capped at 512 MB |
| Logs | `%LOCALAPPDATA%\Swirvo\logs` | `$XDG_STATE_HOME/swirvo/logs` | Rolling JSON, 7 days or 50 MB |
| Crash dumps | `…\crashes` | `$XDG_STATE_HOME/swirvo/crashes` | Minidumps, local only |
| In-flight outputs | `.swirvo-tmp-*` next to the destination | same | Swept at startup using the journal |

```sql
CREATE TABLE job      (id INTEGER PRIMARY KEY, created_at INTEGER, origin TEXT, intent TEXT,
                       status TEXT, priority INTEGER, error TEXT);
CREATE TABLE job_io   (job_id INTEGER, role TEXT CHECK(role IN ('in','out')), path BLOB,
                       file_key BLOB, bytes INTEGER, fmt TEXT);
CREATE TABLE journal  (seq INTEGER PRIMARY KEY, job_id INTEGER, op TEXT, args BLOB,
                       inverse BLOB, state TEXT);                        -- undo/redo + crash recovery
CREATE TABLE frecency (ctx TEXT, target TEXT, score REAL, t_last INTEGER,
                       PRIMARY KEY (ctx, target)) WITHOUT ROWID;
CREATE TABLE perf     (key TEXT PRIMARY KEY, mean REAL, m2 REAL, n INTEGER) WITHOUT ROWID; -- Welford
CREATE VIRTUAL TABLE job_fts USING fts5(name, tokenize = 'unicode61');
```

**Config layering:** built-in defaults < system < user < environment < CLI flags. On a file change the config is validated, then swapped into an `ArcSwap<Config>`. Readers never lock. An invalid edit is rejected with a notification, and the last good config stays active.

## 12. Engine packs

```toml
[pack]
id        = "media"
version   = "7.1.2+swirvo.3"
license   = "LGPL-2.1-or-later"
bytes     = 39_845_120
sha256    = "…"
signature = "ed25519:…"            # verified against a key compiled into the agent
[[pack.provides]]
engine       = "ffmpeg"
binaries     = ["ffmpeg.exe", "ffprobe.exe"]
capabilities = "capabilities.toml"
self_test    = "selftest/clip.mkv"
```

**Flow:** consent → resumable download (HTTP Range, mirrors) → verify SHA-256 and the signature → extract to a versioned folder → **self-test** (convert a tiny fixture) → atomically switch the `current` pointer → recompute reachability → notify. A failed self-test rolls back, and old versions are removed after 7 days. On Linux, compatible system binaries are found first (ffmpeg ≥ 6, soffice ≥ 7.6, pandoc ≥ 3), with an offer to *use the system FFmpeg*, which means nothing to download. An offline installer bundles every pack.

| Pack | Contents | Approx. size |
|---|---|---|
| Core (built in) | libvips + codecs, PDFium, qpdf, libarchive, data, subtitles, fonts, text | 25–30 MB |
| Media | FFmpeg (LGPL, trimmed build) | 25–40 MB |
| Office | LibreOffice (headless subset) | 300–350 MB |
| Docs | Pandoc | ~100 MB |
| OCR (Linux) | Tesseract plus language data | 15–40 MB |
| AI Speech | whisper.cpp plus a model (base 142 MB … large-v3-turbo ~1.6 GB) | variable |
| AI Image | Background removal, upscaler, face detector (ONNX) | 60–250 MB |
| GPL codecs (opt-in) | x264/x265 FFmpeg build, run as a separate executable | ~40 MB |

## 13. Updates

- **App:** Velopack (delta updates, Windows and Linux) for direct downloads; MSIX through the Microsoft Store or App Installer; Flathub, AppImage (zsync) and distro repositories on Linux. Channels: nightly, beta, stable. Staged rollout of 5 % → 25 % → 100 %, paused automatically on a crash-rate regression.
- **Packs** update independently of the app, with the same verification.

## 14. Security model

| Threat | Mitigation |
|---|---|
| Malicious media or document exploits a C decoder | Parsing only in sandboxed workers; pixel and size caps; decompression-bomb limits; fuzzing; fast updates for engine CVEs |
| Archive path traversal (zip-slip), symlink tricks | Canonicalize and confine every entry; reject absolute paths, `..` and device names; extract symlinks as plain files unless the user allows them |
| Office macros, PDF JavaScript | LibreOffice with macros disabled and no network; PDFium built without V8/XFA |
| DLL planting | `SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32 \| …APPLICATION_DIR)`; signed binaries; absolute paths for tool executables |
| Tampered engine pack | SHA-256 plus an ed25519 signature checked against a key pinned in the binary |
| Local attacker using IPC | Pipe DACL / socket mode for the current user; peer credential check; no remote clients |
| Privacy leaks | No telemetry by default; logs redact paths; Offline mode; workers can't reach the network |
| Command injection | Argument vectors only; no shell; file names starting with `-` passed after `--` |

## 15. Observability

- `tracing` spans for every stage in §7, written as rolling JSON logs.
- On Windows, TraceLogging/ETW provider `Swirvo` marks the latency path (raw input → probe shown → DragEnter → first frame committed), so WPA and PresentMon can show real on-screen times.
- A hidden *latency HUD* (Settings → Advanced) prints trigger-to-frame time on the wheel.
- Minidumps through `crash-handler` + `minidumper`, symbolized offline with our symbol store.
- *Diagnostics bundle*: config, logs with paths redacted, hardware probe, pack versions. The user saves it and decides whether to share it.

## 16. Failure handling

| Failure | Detection | Recovery |
|---|---|---|
| Engine error on a file | The step returns an error (not a cancel) | **Built.** Plan again without that engine (`Graph::route_avoiding`), up to two fallbacks. Example: a Word file our layout engine refuses goes to LibreOffice if installed, else the typesetter. The output carries a note ("a fallback was used, check the result"), shown in the tray toast, on the Board card and by the CLI |
| Worker crash | Pipe EOF or exit code | Retry once on a fresh worker; circuit breaker (§8) |
| Hung tool | Watchdog | Kill the job object or process group; retry with safe flags |
| Agent crash | OS restart (Task Scheduler restart-on-failure, systemd `Restart=on-failure`) | Replay the journal: resume queued jobs, delete temp files, mark in-flight jobs as interrupted, offer to retry |
| GPU driver reset | Encoder error codes | Fall back to software; re-probe after the next driver event |
| Disk full mid-write | `ERROR_DISK_FULL` / `ENOSPC` | Delete the temp file, explain, pause the queue |
| Power loss | — | Journal plus atomic rename means no partial outputs are visible |
| Corrupt database | `PRAGMA quick_check` at startup | Restore the most recent `VACUUM INTO` backup |

## 17. Extensibility (P4)

- **WASM component plugins** (Wasmtime, WIT interface `convert(input: stream, params) -> stream`). A plugin declares capabilities in a manifest, has no file-system or network access unless granted, and gets CPU and memory limits from fuel and memory caps.
- **External-tool plugins**: a manifest that wraps a native executable (for example, a 3D converter built on assimp), run inside the worker sandbox.

## 18. Architecture decisions (summaries)

| ADR | Decision | Why | Rejected |
|---|---|---|---|
| 001 | Rust for all core and platform code | Memory safety while parsing hostile files; performance; first-class Windows bindings (`windows-rs`); one language for both OSes | C++ (safety), .NET (Linux story, footprint), Go (FFI and GUI) |
| 002 | Native overlay for the wheel; no webview | One-frame latency, ≤ 25 MB footprint, composition-thread animations | Tauri or Electron overlay: 100–300 ms cold show, 100+ MB |
| 003 | ~~Tauri 2 + React for the Hub and editors~~ (superseded by 009) | | |
| 004 | FFmpeg as a subprocess first; in-worker libav* later | Crash isolation and easy progress first; full sandboxing (custom AVIO) later | Linking libav* from day one: slower to build, and a crash still kills the worker |
| 005 | SQLite for state | Proven, transactional, FTS5 for history search, a single file | sled/redb (no SQL and no FTS) |
| 006 | Keyboard Raw Input plus an OLE probe window instead of global hooks | Asynchronous (adds no input latency), less suspicious to antivirus, keyboard only (no mouse-move flood), so idle CPU stays at 0 | `WH_KEYBOARD_LL` / `WH_MOUSE_LL`: synchronous, timeouts, AV flags |
| 007 | Out-of-process shell extension (COM server in the agent) | A bug in our code can never crash Explorer | In-process DLL handler |
| 008 | Engine packs | Small installer; users download only what they use; GPL isolation | One 500 MB installer |
| 009 | Native Board and editors on our own toolkit (`swirvo-ui`, immediate mode on tiny-skia) in the same exe; OS engines before bundling (docs/12) | No web view or Node chain; ~1 MB of UI code; ~50 ms cold start; pixel-testable on Linux; same drawing code as the wheel | Tauri + React (memory, build chain); egui/Slint (large dependency we'd still theme heavily) |
