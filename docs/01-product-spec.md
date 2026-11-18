# 01 · Product spec

## 1. One-liner

Swirvo turns any file into the format you need with one gesture: start dragging, tap **Shift**, drop on a wedge. It runs entirely on your PC and works with Explorer, the desktop, browsers and mail clients. It also covers far more than conversion: **tools** (compress, trim, merge, redact, OCR, transcribe) and **actions** (share, shelf, copy path, terminal).

## 2. Who it's for

| Persona | Typical jobs | What they need from us |
|---|---|---|
| Everyday Windows user | iPhone HEIC photos → JPG, PDF → Word, shrinking a video for WhatsApp | No learning curve, safe defaults, originals never lost |
| Creator / marketer | WebP/AVIF for the web, GIFs from clips, resizing for social, subtitles | Quality presets, batches, size estimates, speed |
| Office / admin | DOCX/XLSX ↔ PDF, merging, splitting and redacting PDFs, scans → searchable PDF | Fidelity, true redaction, no uploads |
| Developer / power user | JSON ↔ YAML, CSV → Parquet, fonts → WOFF2, copying a WSL path, scripting | Keyboard first, CLI with JSON output, watch folders |
| Linux desktop user | The same jobs on GNOME or KDE, X11 or Wayland | Native integration, distro packages, no telemetry |

## 3. Landscape

| Product | Platform | Model | Gap Swirvo fills |
|---|---|---|---|
| File Converter (Tichau, GPL) | Windows | Explorer context menu, FFmpeg + ImageMagick | No gesture, thin document/PDF support, dated UI |
| PowerToys | Windows | Image Resizer, Peek, File Locksmith | Single-purpose utilities, no conversion hub |
| HandBrake, Shutter Encoder | Windows, Linux, macOS | Full windowed apps | Heavy UI, mostly video |
| XnConvert, Converseen | Windows, Linux | Batch image apps | Images only, window-first |
| GNOME apps (Switcheroo/Converter, Curtail, Video Trimmer) | Linux | Small single-purpose apps | Fragmented |
| Online converters | Web | Upload, convert, download | Privacy, size limits, speed |

No product on Windows or Linux offers a gesture-first, offline converter that covers images, media, documents, data and archives. That is the opening.

> Swirvo copies the *interaction pattern* (drag + modifier + radial menu, an HCI pattern dating back to pie menus in 1988 and marking menus in the early 1990s), never another product's name, assets, artwork or trade dress. Name, icons, illustrations and copy are original.

## 4. Product principles

1. **Zero-click by default.** The common path has no dialogs. Options are one dwell away.
2. **Originals are never modified.** Swirvo never changes, moves or deletes a source file unless the user picked an action that says so (Move, Replace original). Outputs appear next to the source under a new name.
3. **Instant should feel instant.** The wheel appears within one frame, every action gives feedback within 100 ms, and long jobs show progress and an ETA.
4. **Offline and private.** No uploads, and no telemetry unless the user opts in. The network is used only for explicit, visible actions.
5. **Native on each desktop.** Fluent on Windows 11, Adwaita- and Breeze-aware on Linux, following the system accent, fonts, dark mode and high contrast.
6. **Protect spatial memory.** Wedges don't jump around. Learning fills only unpinned slots, and slowly.
7. **Pick the best technique automatically.** Remux instead of re-encoding, go lossless when possible, and say in one line what was done.
8. **Every path has a keyboard and screen-reader equivalent.**

## 5. Ways to invoke Swirvo

| # | Surface | How | Platforms | Phase |
|---|---|---|---|---|
| 1 | Drag + modifier | Start dragging a file anywhere, then tap **Shift** (Convert), **Ctrl** (Tools) or **Alt** (Actions) | Windows, Linux X11 | P1 |
| 2 | Drag + dwell | Hold still ~600 ms while dragging (touch, pen, no keyboard) | Windows, X11 | P2 |
| 3 | Hot edge | Drag a file to a chosen screen edge or corner; the wheel opens out from the edge | All, including Wayland | P1 Windows (opt-in), P3 Wayland |
| 4 | Shortcut on a selection | Select files in Explorer, press **Ctrl+Alt+Space** (remappable) | Windows; Linux via file-manager extension or copied files | P2 |
| 5 | Context menu | Right-click → *Convert with Swirvo* ▸ top formats / More… | Windows 11 modern + classic menus; Nautilus, Dolphin, Nemo, Thunar | P1 classic, P2 modern |
| 6 | Drop zone | Drop files on the tray flyout or the Hub window | All | P1 |
| 7 | Command bar | Global shortcut, then type `webp`, `compress 25mb`, `merge pdf` | All | P3 |
| 8 | CLI and watch folders | `swirvo convert …`; hot folders bound to recipes | All | P1 CLI, P3 watch folders |

## 6. The three wheels

| Wheel | Default trigger | Contents | Color |
|---|---|---|---|
| **Convert** | Shift | Target formats for the dragged files, best first. A dwell opens a preset sub-ring: Best · Balanced · Smallest · Custom… | Orange (accent) |
| **Tools** | Ctrl | Tools for the file's category (compress, trim, merge, redact…), plus your recipes | Teal |
| **Actions** | Alt | Share, send to device, shelf, copy/move to, copy path, terminal… Works on any file, folder or mixed selection | Indigo |

While a wheel is open you can switch by pressing another modifier, pressing Tab, scrolling, or resting on the tab pill for 250 ms. Triggers can be remapped. **Double-tap mode** avoids clashing with Explorer's own drag modifiers (Shift = move, Ctrl = copy) for people who use them.

## 7. Conversion catalog

A *conversion option* is one (input format → output format) pair offered in the Convert wheel. v1.0 ships **300+**.

| Category | Inputs | Outputs | Notes |
|---|---|---|---|
| Images | JPG, PNG, WebP, HEIC/HEIF, AVIF, JPEG XL, TIFF (multi-page), BMP, GIF and APNG (animated), ICO, TGA, QOI, PSD¹, SVG¹, camera RAW¹ (CR2, CR3, NEF, ARW, DNG, RAF, ORF, RW2) | JPG, PNG, WebP, AVIF, HEIC, JXL, TIFF, BMP, GIF, ICO (multi-size), PDF, data URI to the clipboard | ¹ Input only. JPEG → JXL is lossless and reversible. |
| Video | MP4, MOV, MKV, WebM, AVI, WMV, FLV, M4V, TS/M2TS, 3GP, OGV, GIF | MP4 (H.264/HEVC/AV1), MOV, MKV, WebM (VP9/AV1), AVI, GIF, animated WebP; audio track as MP3, M4A, WAV, FLAC or Opus | Remuxed when the codecs fit: seconds instead of minutes, and lossless. |
| Audio | MP3, M4A/AAC, ALAC, WAV, FLAC, OGG, Opus, AIFF, WMA, AMR, AC3, CAF, MKA | MP3, M4A (AAC), ALAC, WAV, FLAC, OGG, Opus, AIFF, M4R (ringtone) | Tags and cover art carry over. |
| PDF | PDF | DOCX, PPTX, XLSX, ODT, EPUB, TXT, Markdown, HTML, JPG/PNG/WebP (300 DPI), searchable PDF (OCR, P3) | PDF → Office uses layout reconstruction (labelled beta at v1.0). |
| Documents | DOCX, DOC, ODT, RTF, TXT, Markdown, HTML, EPUB, Pages² | PDF, DOCX, ODT, RTF, TXT, Markdown (GFM), HTML, EPUB | ² Apple iWork import through LibreOffice. |
| Slides | PPTX, PPT, ODP, Keynote², PDF | PDF, PPTX, ODP, JPG/PNG per slide | |
| Spreadsheets | XLSX, XLSM, XLS, ODS, CSV, TSV, Numbers² | XLSX, ODS, CSV, TSV, PDF, JSON, Parquet, HTML or Markdown table | One CSV per sheet, or pick a sheet. |
| Data | JSON, NDJSON, YAML, TOML, XML, CSV, Parquet | Each other, where the data's shape allows | Pretty-print, minify and validate on the way. |
| Ebooks | EPUB, FB2, MOBI³, AZW3³ | EPUB, PDF, DOCX, TXT, Markdown, HTML | ³ Through the optional Calibre pack. |
| Subtitles | SRT, VTT, ASS/SSA, TTML, SBV, tracks embedded in MKV/MP4 | SRT, VTT, ASS, TTML, TXT transcript | Encoding and timing fixed on the way. |
| Archives | ZIP, 7z, TAR, TAR.GZ/XZ/ZST/BZ2, GZ, XZ, ZST, RAR¹, ISO¹, CAB¹ | ZIP, 7z, TAR, TAR.GZ, TAR.XZ, TAR.ZST | Entries are streamed and repacked without a temporary extraction. |
| Fonts | TTF, OTF, WOFF, WOFF2 | WOFF2, WOFF, TTF/OTF (container only) | |
| Text | Any text file | UTF-8, UTF-16 or legacy code pages; LF ⇄ CRLF; BOM on or off | Encoding is auto-detected. |

## 8. Tools catalog

Phase: **P1** MVP · **P2** v1.0 · **P3** v1.5 (see [08](08-roadmap-risks.md)).

**Images.** Compress to a quality or a target size (P2) · Resize: 50 %, 1920 px, 1080p, 4K, social presets (P2) · Crop with aspect presets (P2) · Rotate/flip, lossless for JPEG (P2) · Strip metadata (EXIF, GPS, XMP; keeps orientation and ICC profile) (P1) · Edit metadata: date taken, remove location, author (P2) · Annotate: arrows, boxes, text, highlight, numbered steps, blur (P2) · Redact: manual (P2), automatic for faces and text (P3) · Remove background (local AI, P3) · Upscale ×2/×4 (local AI, P3) · Extract text with OCR (P2 Windows, P3 Linux) · Read QR codes and barcodes (P2) · Make an icon set: ICO plus PNG sizes (P2) · Combine into a PDF, grid or GIF (P2) · Watermark (P3)

**Video.** Compress to a target size (10/25/50/100 MB) or quality (P2) · Trim: smart cut, lossless in the middle (P2) · Cut out several segments (P3) · Crop (P2) · Resize (P2) · Change speed 0.25×–4×, pitch-corrected (P2) · Rotate/flip, lossless metadata rotation when possible (P2) · Extract audio, by stream copy when possible (P1) · Mute (P1) · Extract frames or a thumbnail sheet (P2) · Merge, lossless when codecs match (P2) · Burn in or add subtitles (P3) · Auto-subtitles with local Whisper (P3) · Normalize loudness (P2) · Stabilize (P3) · Blur faces with tracking (P3) · HDR → SDR tone mapping (P3)

**Audio.** Compress (P2) · Trim (P2) · Fade in/out (P2) · Normalize to −14/−16/−23 LUFS (P2) · Denoise with RNNoise (P3) · Bleep words using word timestamps (P3) · Merge (P2) · Split on silence (P3) · Change speed/pitch (P3) · Transcribe to TXT/SRT/VTT (P3) · Edit tags and cover art (P2) · Change channels and sample rate (P2)

**PDF.** Merge (P1) · Split by page, range, every N pages or size (P1) · Organize: reorder, rotate, delete, insert (P2) · Compress (P2) · OCR to a searchable PDF (P3) · True redaction (P3) · Protect/unlock with AES-256 (P2) · Visible signature (P3) · Watermark/stamp (P3) · Page numbers (P3) · Extract images (P2) · Extract text (P1) · Flatten forms and annotations (P3) · Rotate (P1)

**Archives.** Extract here (P1) · Extract to folder (P1) · Compress to ZIP/7z, with optional AES-256 password (P1/P2) · Test integrity (P2)

**Any file.** Checksums (SHA-256, BLAKE3, MD5, CRC32), copied to the clipboard or verified against it (P2) · Batch rename with templates and preview (P2) · Base64 encode/decode (P2) · Change created/modified dates (P3) · Split/join large files (P3)

## 9. Actions catalog

Actions work on files, folders and mixed selections, and they never count against a trial allowance.

| Action | Windows | Linux | Phase |
|---|---|---|---|
| Share | System share sheet (Nearby Share, Phone Link, mail, apps) | KDE Connect / GSConnect, or the portal | P2 |
| Send to nearby device | LocalSend v2 protocol over the LAN (opt-in network), reaching phones and PCs on any OS | Same | P3 |
| Float on shelf | Floating shelf panel to drag files out of later | Same | P2 |
| Copy to… / Move to… | Recent and pinned destinations; native conflict UI and undo | Same | P1 |
| Copy path | Windows, quoted, forward-slash, WSL `/mnt/c/…`, UNC, `file://` URI, Markdown link | POSIX, quoted, URI, Markdown | P1 |
| Open terminal here | Drop opens the first shell found: Windows Terminal, else PowerShell (7 before 5.1). Resting on the wedge shows the others found on this PC: Cmd, each WSL distro (default first), Git Bash. Console shells follow Windows' "default terminal app" setting. **Built.** | Default terminal | P1 |
| Open with… | App picker | Portal / `gio` | P2 |
| Copy as image | Puts the bitmap on the clipboard, ready to paste into chat | Same | P1 |
| Copy contents | Text files go to the clipboard | Same | P2 |
| Show in folder | Explorer with the item selected | File manager over D-Bus (`org.freedesktop.FileManager1`) | P1 |
| Email as attachment | Simple MAPI | `xdg-email --attach` | P2 |

## 10. Automation

- **Recipes**: named multi-step pipelines, for example *Web-ready image*: resize 2048 px → WebP quality 80 → strip GPS → save to `…\Web`. A recipe can sit in a Tools wheel slot. Steps can ask for a value when run ("Ask me: width"). Recipes import and export as TOML.
- **Watch folders**: a folder bound to a recipe, with include patterns, an optional subfolder scan and a policy for originals (keep by default, or move them to an `Originals` subfolder). Status and counters appear in the Hub.
- **CLI**: scriptable, with stable exit codes and JSON Lines progress.
  ```text
  swirvo convert *.heic --to jpg --preset balanced
  swirvo tool compress talk.mp4 --target 25MB --json
  swirvo recipe run "Web-ready image" ./shots/*.png
  swirvo formats --from heic            # what can HEIC become?
  swirvo plan scan.pdf --to docx --explain
  ```
- **Command bar**: a global shortcut opens a search-style bar that works on the current Explorer selection, or on the files you last copied.

## 11. Privacy promise

- File contents and names never leave the device.
- The network is used in exactly three places, each visible and each with a switch: the update check, engine-pack downloads, and Send to nearby device (LAN only). **Offline mode** turns all three off.
- Worker processes are blocked from the network by the OS sandbox, not merely by policy.
- Crash reports stay on disk unless the user chooses to send one.

## 12. Non-goals (v1)

Cloud conversion, accounts and sync, mobile apps, full video editing, removing DRM (never), downloading from streaming sites (never), a macOS build.

## 13. Business model: your decision

| Option | Shape | Licensing consequence |
|---|---|---|
| A · Open source | GPL-3.0, free, sponsorships | Can bundle GPL encoders (x264/x265) and libimagequant |
| B · Open core | Permissive core (MPL-2.0/Apache-2.0) plus a paid Pro tier (AI tools, recipes, watch folders) | Core must avoid GPL; Pro is closed |
| C · Paid app with trial | A free allowance, then a one-time licence | No GPL linked; GPL tools only as optional separate executables |

**Recommendation:** build under option C's constraints from day one. That keeps A and B open, and the code doesn't change if you choose later.

## 14. Success metrics

| Metric | Target |
|---|---|
| Time from install to first successful conversion | median < 90 s |
| Wheel open latency | p95 ≤ 25 ms on the reference laptop (see [07](07-performance-testing-release.md)) |
| Conversion success on the test corpus | ≥ 99.5 % |
| Crash-free sessions | ≥ 99.9 % |
| Week-1 activation | ≥ 60 % of installs do 3+ conversions |
| "Where did my file go?" support tickets | ~0: every result links to *Show in folder* |
