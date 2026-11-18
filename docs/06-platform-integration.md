# 06 · Platform integration

## Part A: Windows

**Targets:** Windows 11 24H2+ on x64 and ARM64 (Snapdragon X). Windows 10 22H2 is best effort only: its consumer extended security updates ended in October 2026, so it gets the Solid wheel and no modern context menu.

### A1. Detecting the gesture without global hooks

The OS gives no public notification when another process starts a drag. Swirvo infers the drag from cheap signals and then *proves* it by becoming a drop target.

1. **Keyboard Raw Input only.** Register `RAWINPUTDEVICE{usUsagePage=0x01, usUsage=0x06, dwFlags=RIDEV_INPUTSINK, hwndTarget=hidden}` and receive `WM_INPUT` for keys even in the background. Raw input is **asynchronous** (unlike `WH_KEYBOARD_LL`, the system never waits on us) and low-frequency, because we deliberately do **not** register the mouse: that would mean 1,000 wakeups per second with a gaming mouse.
2. **On a trigger key down** (Shift / Ctrl / Alt, or a double tap in that mode):
   - Check `GetAsyncKeyState(VK_LBUTTON)` (or `VK_RBUTTON`, since Explorer also supports right-drag). If no button is held, ignore it.
   - Show the **probe window** at `GetCursorPos()`. It is already created and simply hidden. It's 64 × 64 px, `WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE`, alpha 1/255 (invisible but still hit-testable; *not* `WS_EX_TRANSPARENT`, which would make OLE skip it), and registered with `RegisterDragDrop` on the STA UI thread.
   - **Nudge:** OLE's `DoDragDrop` loop re-hit-tests the window under the cursor whenever the mouse or key state changes. The key-up of the tap usually does it. To be sure, send a 1 px there-and-back absolute move with `SendInput` (to be verified in spike S1).
3. **`IDropTarget::DragEnter`** on the probe proves an OLE drag is in progress and hands us the `IDataObject`:
   - `CF_HDROP` → file paths (`DragQueryFileW`)
   - `CFSTR_SHELLIDLISTARRAY` → shell items (zip folders, phones over MTP, libraries) via `SHCreateShellItemArrayFromDataObject`
   - `CFSTR_FILEDESCRIPTORW` + `CFSTR_FILECONTENTS` → virtual files (Outlook attachments, images dragged from browsers, files inside archives). Read only the descriptors now; read contents on drop.
   - No files → hide the probe and do nothing. A text drag or a drawing app's Shift-drag is left alone.
4. **Reveal:** classify by extension (instant), resize the window to the wheel's size centered on the cursor, and render (≤ 1 frame). Sniff content asynchronously ([04](04-algorithms.md) §1).
5. **`DragOver`** gives the pointer in screen coordinates: hit-test ([04](04-algorithms.md) §4), set `*pdwEffect = DROPEFFECT_COPY` (or `NONE` over the hub), and set a **drop description** (`CFSTR_DROPDESCRIPTION`, `DROPIMAGE_COPY`, "Convert to %1" with "JPG"). The cursor tooltip then shows the action natively.
6. Forward `DragEnter`, `DragOver`, `DragLeave` and `Drop` to **`IDropTargetHelper`** (CLSID_DragDropHelper). Without it, Explorer's drag image disappears while over our window.
7. **`Drop`:** build the `JobSpec`, enqueue it, return `DROPEFFECT_COPY` **within 5 ms**. The source app is blocked until `Drop` returns. For virtual files, use **`IDataObjectAsyncCapability`** when offered (Explorer and Outlook offer it), so contents are pulled asynchronously without freezing the source. Otherwise read the streams inside `Drop`, as Explorer does.
8. **Never return `DROPEFFECT_MOVE`.** For a move, Explorer deletes the source after the drop. We always return Copy.

**Modifier semantics conflict:** during Explorer drags, Shift means move, Ctrl copy and Alt link. Our taps don't change what happens when the user drops elsewhere, because the wheel fades if they leave it. Users who rely on those modifiers can switch to **double-tap mode** (chosen during onboarding).

**Alt side effect:** releasing Alt can activate the source window's menu bar. Use the *menu-mask* trick: inject an unassigned virtual key (`vkE8`) with `SendInput` while Alt is still down, so the Alt-up is not a "lone Alt".

**Limits:** drags from elevated (admin) apps can't reach a non-elevated target (UIPI). This is documented, and nothing works around it. While a full-screen exclusive app is running (`SHQueryUserNotificationState` = `QUNS_RUNNING_D3D_FULL_SCREEN` or `QUNS_BUSY`), the wheel is suppressed.

### A2. Overlay and compositing

- The window has `WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE`, so it has no GDI redirection surface, never takes focus and stays out of Alt+Tab. `DWMWA_WINDOW_CORNER_PREFERENCE = DWMWCP_DONOTROUND`.
- **Windows.UI.Composition** through `windows-rs`: `CreateDispatcherQueueController` → `Compositor` → `ICompositorDesktopInterop::CreateDesktopWindowTarget(hwnd, topmost=true)`.
  - **Glass layer** (Windows 11): `DwmSetWindowAttribute(DWMWA_USE_HOSTBACKDROPBRUSH, TRUE)`, then `Compositor.CreateHostBackdropBrush()` under a tint, clipped to the ring with `CreateGeometricClip(CreateEllipseGeometry())`. This is real blur of whatever is behind the wheel, done by DWM.
  - **Content layer:** our tiny-skia renderer draws premultiplied RGBA into a **composition swap chain** (`CreateSwapChainForComposition`, `DXGI_ALPHA_MODE_PREMULTIPLIED`), which is wrapped as a surface brush. Static layers (wedges, labels) are cached per wheel content; hover repaints only change one wedge layer, so frames take under a millisecond.
  - **Animations:** `ScalarKeyFrameAnimation` on Scale and Opacity run on DWM's compositor thread, so they stay smooth even when the agent is busy.
- **Pre-warmed at startup:** the window, device, swap chain and glyph atlas all exist before the first gesture. Showing the wheel is a resize, a present and a commit.
- **Windows 10 fallback:** the same window and swap chain with an opaque Solid style; no host backdrop.
- **DPI:** per-monitor v2 in the manifest. On `WM_DPICHANGED`, re-rasterize the glyph atlas for that scale.

### A3. Shortcut on a selection

- `RegisterHotKey` (default **Ctrl+Alt+Space**; if registration fails, the user is told the shortcut is taken and picks another).
- Find the target: foreground window class `CabinetWClass` (an Explorer window) or `Progman`/`WorkerW` (the desktop).
- **Selection:** `IShellWindows` → the matching `IWebBrowserApp` → `IServiceProvider` → `IShellBrowser` → `IShellView` → `IFolderView2::GetSelection` → `IShellItemArray`. **Explorer tabs (Windows 11):** pick the active tab by matching the visible `ShellTabWindowClass` child of that `CabinetWClass` window. The desktop uses `FindWindowSW(SWC_DESKTOP)`.
- Show the wheel at the cursor with keyboard focus. Receiving `WM_HOTKEY` grants the right to call `SetForegroundWindow`.
- **Fallback:** if nothing is selected, use the files on the clipboard (`CF_HDROP`), so *copy files, then press the shortcut* works anywhere.

### A4. Explorer integration

| Integration | Mechanism | Notes |
|---|---|---|
| Modern context menu (Windows 11) | `IExplorerCommand` (with `IEnumExplorerCommand` for submenus) served **out of process** by the agent's COM server; registered by a signed **sparse MSIX package** (package with external location) using `desktop4:FileExplorerContextMenus` and `com:ExeServer` | `GetState` decides from the extension only, never with I/O. Submenu: top 5 targets by frecency, *More…* opens the wheel at the cursor, *Swirvo tools ▸* |
| Classic menu (Windows 10, "Show more options") | `HKCU\Software\Classes\*\shell\Swirvo` with `ExtendedSubCommandsKey`; static verbs that run `swirvo.exe wheel --files …` | No DLL at all |
| Send To | A shortcut in `shell:sendto` | Trivial; one more way in |
| Package identity (from the sparse package) | Also gives a stable AUMID for toasts, toast activation, the share target and the Windows AI APIs | Needs code signing |

### A5. Notifications, tray and share

- **Toasts:** `Windows.UI.Notifications` with an AUMID; a progress bar via data binding (`NotificationData`, updated ≤ 1 Hz); buttons via protocol activation (`swirvo://job/…`) or a COM activator. Batches reuse one toast `Tag`.
- **Tray:** `Shell_NotifyIconW` with `NIF_GUID` (stable across updates); the flyout is our own overlay window placed above the tray. Re-add the icon on the `TaskbarCreated` message after Explorer restarts. Light and dark icon variants.
- **Share:** `IDataTransferManagerInterop::ShowShareUIForWindow` opens the system share sheet (Nearby Share, Phone Link, mail, apps).
- **Terminal:** `wt.exe -d "<dir>"` when Windows Terminal is installed, otherwise PowerShell; WSL with `wsl.exe --cd "<dir>"`.
- **Copy path variants:** Win32, quoted, forward slashes, UNC for mapped drives (`WNetGetUniversalNameW`), `file:///` URI, WSL `/mnt/c/...`, Markdown link.

### A6. File operations

- Copy to / Move to through **`IFileOperation`**, for native conflict dialogs, progress, and an undo record (`FOFX_ADDUNDORECORD`) that works with Explorer's Ctrl+Z.
- Undoing an output: `IFileOperation::DeleteItem` with `FOFX_RECYCLEONDELETE`, so it goes to the Recycle Bin.
- Locked files: the **Restart Manager** (`RmGetList`) names the app holding the lock.
- **Cloud files** (OneDrive Files On-Demand): `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS` / `RECALL_ON_OPEN` → show *Downloading from OneDrive…*. Reading the file triggers hydration; the cfapi gives progress.
- **Long paths:** `longPathAware` in the manifest, `\\?\` internally, shell APIs only for the parts that need them.

### A7. Power, QoS and containment

- **EcoQoS** (`SetProcessInformation(ProcessPowerThrottling, EXECUTION_SPEED)`) plus `PROCESS_MODE_BACKGROUND_BEGIN` for batch and background workers. Interactive workers opt *out* of throttling explicitly.
- `RegisterPowerSettingNotification` for `GUID_ACDC_POWER_SOURCE` and `GUID_POWER_SAVING_STATUS` (energy saver) feeds the scheduler's capacity policy ([04](04-algorithms.md) §6).
- **Job objects:** `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, a memory limit, and `JOB_OBJECT_CPU_RATE_CONTROL` for throttle-style preemption. Every worker and tool process lives in one, so nothing is orphaned if the agent dies.
- Manifest: per-monitor-v2 DPI, `longPathAware`, `activeCodePage = UTF-8` (helps C libraries with paths), Common Controls v6.

### A8. Packaging and signing

| Channel | Format | Updates | Notes |
|---|---|---|---|
| Microsoft Store | MSIX (full trust) | Store | The Store signs it; easiest SmartScreen story |
| Direct download | Velopack `Setup.exe` + sparse MSIX for identity | Velopack delta updates | Code-signed (§07) |
| winget | Manifest pointing at the direct installer | winget | Community repository PR |

The C runtime is linked statically (`+crt-static`) where licenses allow, so no VC++ redistributable is needed. WebView2 is preinstalled on Windows 11 (the Evergreen bootstrapper is used if it's missing). Builds: `x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc`.

### A9. Antivirus and SmartScreen hygiene

No global hooks (A1). Signed binaries. Submit each release to Microsoft's malware-analysis portal. No code injection and no memory scanning. The keyboard sink only ever reads modifier keys plus Esc and Tab while a wheel is open, and that is stated in the privacy page.

---

## Part B: Linux

**Targets:** Ubuntu 24.04+, Debian 13, Fedora 41+, Arch; GNOME 46+ and KDE Plasma 6 as primary desktops; X11 and Wayland.

### B1. What works where

| Environment | Drag + modifier | Hot edge | Shortcut on a selection | Context menu |
|---|---|---|---|---|
| X11 (any desktop) | ✅ XI2 raw keys + XDND probe | ✅ | ✅ global key grab + file manager or clipboard | ✅ |
| KDE Plasma Wayland | ➖ (no global input on Wayland) | ✅ layer-shell | ✅ GlobalShortcuts portal | ✅ Dolphin service menu |
| wlroots (Sway, Hyprland), COSMIC | ➖ | ✅ layer-shell | ✅ portal / compositor bind | depends on the file manager |
| GNOME Wayland | ➖ | ⚠ needs the optional GNOME Shell extension (no layer-shell) | ✅ GlobalShortcuts portal (GNOME 48+; verify per distro) | ✅ Nautilus extension |

On Wayland, modifier state isn't delivered to a client that doesn't have focus, even during a drag. So the wheel there is pointer-driven: rest on the tab pill to switch wheels, and dwell for presets. That is the same pointer-only path touch users get on Windows.

### B2. X11 pipeline

1. Use **XInput2** raw events (`XI_RawKeyPress` / `XI_RawKeyRelease` on the root window) through `x11rb`. Raw key events only, no pointer stream.
2. On a trigger with `Button1Mask` set (`XQueryPointer`), map the probe: a 64 × 64 **override-redirect ARGB window** advertising `XdndAware` (version 5), with an `XTest` motion nudge.
3. **XDND:** `XdndEnter` (check `text/uri-list` in the type list, or via `XdndTypeList`) → `XdndPosition` (reply with `XdndStatus`, accept, action `XdndActionCopy`) → `XdndDrop` → `XConvertSelection(XdndSelection, text/uri-list)` → parse `file://` URIs (percent-decoded into raw bytes) → `XdndFinished`.
4. Render with tiny-skia into a shared-memory `XImage` (MIT-SHM). Transparency needs a compositor (all mainstream desktops have one). Without a compositor, fall back to an XShape mask with the Solid style.
5. Blur on KDE X11 via `_KDE_NET_WM_BLUR_BEHIND_REGION`.

### B3. Wayland: hot-edge design

- A **layer-shell** surface (`zwlr_layer_shell_v1`, through smithay-client-toolkit) in the overlay layer: a 6 px strip on the user's chosen edge or corner, `keyboard_interactivity = none`, `exclusive_zone = -1`.
- On **`wl_data_device.enter`**, if the offer includes `text/uri-list`, the strip grows into the wheel, opening out from that edge (resize is a commit plus a new buffer within one frame). Hit-testing uses `motion` events.
- Drop: `wl_data_offer.accept`, `set_actions(copy)`, `receive("text/uri-list")` → read from the pipe → parse → `finish`.
- **KDE blur** through `org_kde_kwin_blur`; **fractional scaling** through `wp_fractional_scale_v1` plus `wp_viewporter`.
- **GNOME Wayland** has no layer-shell. Options, in order: (1) an optional **GNOME Shell extension** (JavaScript) that provides the edge zone and talks to the agent over D-Bus (a research item); (2) the Nautilus extension plus the selection shortcut; (3) the Hub window as a drop zone.

### B4. Selection and the global shortcut

- **Shortcut:** X11 `XGrabKey`; on Wayland the **GlobalShortcuts portal** (`org.freedesktop.portal.GlobalShortcuts`), where the user confirms the binding.
- **Selection:** Linux has no universal API for the current selection. So: (1) file-manager extensions pass the selection directly (below); (2) **Nautilus scripts with accelerators** (`~/.config/nautilus/scripts-accels`) bind a key to `swirvo wheel --at-pointer -- "$@"`; (3) the **clipboard fallback**: read `x-special/gnome-copied-files` or `text/uri-list` from the clipboard. The command bar takes focus briefly to do it, since Wayland only allows clipboard reads with focus.

### B5. File-manager integrations (in `integrations/linux/`)

| File manager | Mechanism |
|---|---|
| Nautilus (GNOME Files) | `nautilus-python` `MenuProvider` (package `python3-nautilus`): *Convert with Swirvo ▸* top formats, *More…* |
| Dolphin (KDE) | Service menu `~/.local/share/kio/servicemenus/swirvo.desktop` with `X-KDE-Submenu` and MIME filters |
| Nemo (Cinnamon) | `.nemo_action` files |
| Thunar (Xfce) | Custom actions in `uca.xml` |
| Any | A `.desktop` entry with a `MimeType=` list, so *Open With → Swirvo* opens the wheel |

Every one of them shells out to the CLI (`swirvo convert …` or `swirvo wheel …`) with argument vectors. Nothing heavy runs inside the file manager.

### B6. Tray, notifications, autostart

- **Tray:** StatusNotifierItem through `ksni` (native on KDE; on GNOME it needs the AppIndicator extension, which Ubuntu ships). Without a tray, the Hub is the home.
- **Notifications:** `org.freedesktop.Notifications` with actions. KDE shows progress (the `value` hint); GNOME shows the percentage in the text. Flatpak uses Notification portal v2.
- **Autostart:** a **systemd user service** `swirvo-agent.service` (`Restart=on-failure`, `MemoryMax=`, `Slice=app.slice`), falling back to XDG autostart. Workers are launched in transient scopes (`systemd-run --user --scope -p MemoryMax=… -p CPUWeight=20`), or in a delegated cgroup v2 subtree.

### B7. Sandbox

Landlock (file-system rules), seccomp-bpf (no `socket(AF_INET*)`, `ptrace` or `mount`), `PR_SET_NO_NEW_PRIVS`, `PR_SET_PDEATHSIG`, rlimits, and network namespaces where unprivileged user namespaces are allowed (Ubuntu 24.04 needs an AppArmor profile). Inputs and outputs are passed as file descriptors over `SCM_RIGHTS`.

### B8. Hardware acceleration

VA-API for Intel and AMD (probed through libva), NVENC/NVDEC with the proprietary NVIDIA driver, Vulkan Video where FFmpeg supports it. The same one-frame test-encode probe as on Windows.

### B9. Packaging

| Format | Pros | Caveats |
|---|---|---|
| **.deb / .rpm** (`cargo-deb`, `cargo-generate-rpm`) + apt/dnf repository | Best integration; uses system FFmpeg, LibreOffice and Pandoc | Build against the oldest supported glibc |
| **AppImage** | One file, runs everywhere; Velopack or zsync updates | No automatic file-manager integration (offer to install it) |
| **Flatpak (Flathub)** | Discoverability, sandbox | XI2 needs the X11 socket; outputs next to the source need `--filesystem=home`; FFmpeg through `org.freedesktop.Platform.ffmpeg-full`; no global input on Wayland anyway |
| **AUR** | Arch users expect it | Community-maintained PKGBUILD |

### B10. Linux pitfalls

File names are bytes (`OsString`, never `String`). NFC vs NFD names. Case-sensitive file systems. Symlinks and bind mounts. `fs.inotify.max_user_watches` limits (watch only the configured folders). Mixed and fractional HiDPI. Font discovery through fontconfig. Testing on WSLg isn't representative: it has no layer-shell and no drag-and-drop between Windows and Linux apps, so real testing happens in VMs ([09](09-dev-environment.md)).

---

## Part C: Cross-platform seams (traits in `crates/platform`)

```rust
pub trait GestureSource { fn start(&mut self, sink: GestureSink) -> Result<()>; }        // raw keys → Events
pub trait DropProbe     { fn arm(&self, at: Point); fn disarm(&self); }                   // emits DragEnter/Over/Leave/Drop
pub trait OverlayHost   { fn show(&self, at: Point, scene: &Scene); fn update(&self, scene: &Scene); fn hide(&self); }
pub trait ShellBridge   { fn selection(&self) -> Result<Vec<PathBuf>>; fn reveal(&self, p: &Path) -> Result<()>;
                          fn share(&self, files: &[PathBuf]) -> Result<()>; fn open_terminal(&self, dir: &Path) -> Result<()>; }
pub trait Notifier      { fn receipt(&self, r: &Receipt); fn progress(&self, id: JobId, p: Progress); fn finish(&self, o: &Outcome); }
pub trait Sandbox       { fn spawn_worker(&self, spec: &WorkerSpec) -> Result<WorkerHandle>; }
pub trait FileOps       { fn commit_atomic(&self, tmp: &Path, dst: &Path) -> Result<()>;
                          fn recycle(&self, p: &Path) -> Result<()>; fn clone_or_copy(&self, s: &Path, d: &Path) -> Result<CopyStats>; }
```

The wheel model, planner, scheduler and storage never see an OS type. Each platform crate implements these traits, and tests use in-memory fakes.
