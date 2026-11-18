# 12 · Board, page picker, UI toolkit and OS engines

This plan covers four things that were requested together:

1. The **Board**: one window where you drop many files and convert or process them in one go.
2. A **page picker** that pops up for PDF page tools, so you choose the pages first.
3. A **glassy, modern UI**, built in-house, with no web view and no UI framework.
4. **More formats**, without making the download bigger.

Everything here follows the non-negotiables in `PROJECT_GUIDE.md`. It supersedes ADR 003 (Tauri + React for the Hub); see §10.

---

## 1. Principles

- **Standalone.** One `Swirvo.exe` and nothing to install. The Board and the page picker are the same exe started with a flag (§6).
- **Few dependencies.** We add **no new third-party crates**. The UI toolkit is our own code on top of `tiny-skia` and `ab_glyph`, which the wheel already uses. New formats are our own parsers, except where the operating system already ships an engine.
- **Use the OS before bundling.** Windows ships OCR, HEIC/AVIF/RAW codecs and a libarchive-based `tar.exe`. Each costs 0 bytes in our download (§8).
- **Modular.** Each crate has one job and a one-way dependency. Portable crates never touch the OS, so they can be tested on Linux, including **pixel snapshots of the whole UI**.

```
swirvo-ui      (portable)  geometry · theme tokens · text · canvas · icons · input · widgets
     ▲
swirvo-board   (portable)  Board app · Page picker app · thumbnail service · batch runner
     ▲                       (depends on jobs, engines; never on the OS)
platform/win     (Windows)   window host: glass backdrop, custom title bar, DPI, drag-drop,
                             file dialogs, single instance → runs any `swirvo_board::App`
```

## 2. Size and selling: what new features cost

| Feature | How | Added to the download |
|---|---|---|
| OCR (image → text, scanned PDF → text/DOCX, "Copy text" on images) | **Windows.Media.Ocr**: the OCR engine built into Windows 10/11, using the user's installed language packs | **0 bytes** |
| OCR on Linux | `tesseract`, if installed, run as a child process (Apache-2.0) | 0 bytes |
| HEIC/HEIF, AVIF, camera RAW, JPEG XR, DDS | **WIC**, the Windows Imaging Component, with the codecs present on the PC. Windows 11 usually has HEIF, AV1 and Raw Image Extensions | **0 bytes** |
| 7Z, RAR, ISO, CAB → extract / ZIP | `C:\Windows\System32\tar.exe` (bsdtar/libarchive, shipped since Windows 10 1803); `bsdtar` on Linux | **0 bytes** |
| RAW without the Windows extension | our TIFF/IFD parser pulls the camera's embedded full-size JPEG | ~6 KB of code |
| PSD | our parser for the flattened composite image | ~5 KB |
| WOFF/WOFF2 ⇄ TTF/OTF, XML ⇄ JSON | our code; Brotli and quick-xml are already in the build | ~15 KB |
| Board + page picker + UI toolkit | our code | ~0.5–1 MB of the exe |

Today `Swirvo.exe` is 26 MB, or **11 MB compressed**. The next size wins are on the release checklist: ship one exe instead of exe + CLI, and turn on `opt-level="s"` for cold crates.

**Selling.** None of this needs an online account or a server. The planned commercial flow:
- A free tier counts actions in the local store.
- A license key is an offline-verified signature: Ed25519 via a vetted crate, never hand-rolled crypto.
- Code-signing removes the SmartScreen warning.
- An installer (MSIX or Velopack) handles updates.

Tracked in docs/08 P2. Optional engine packs (FFmpeg, LibreOffice) stay separate downloads, so the main installer stays small.

## 3. The Board

### 3.1 What it is
A window you open from the tray ("Open Board"), from the **Actions** wheel ("Board"), or by double-clicking `Swirvo.exe` while the agent is already running. It shows every file you drop in as a card. Then you:
1. pick a target **per source type**: all PNGs → JPG, all PDFs → DOCX, all HEICs → JPG;
2. set the quality (Best · Balanced · Smallest) and where the files go (next to the originals, or a folder you choose);
3. press **Convert**.

You can also run **Tools** on the selection (Resize, Compress, Strip info, Merge PDFs…).

### 3.2 Layout (logical px, 4 px grid)

```
┌───────────────────────────────────────────────────────────────────────────── ─ □ × ┐  title bar 44 (drag area)
│ 🍑 Swirvo Board      [＋ Add files]  [Select all] [Clear done]                   │
├──────────────────────────────┬─────────────────────────────────────────────────────┤
│  CONVERT     TOOLS           │  ┌──────┐ ┌──────┐ ┌──────┐ ┌──────┐               │
│                              │  │thumb │ │thumb │ │thumb │ │thumb │   cards 168 w │
│  PNG · 8 files     → JPG ▾   │  │      │ │      │ │      │ │      │   grid, gap 12│
│  PDF · 2 files     → DOCX ▾  │  │name  │ │name  │ │name  │ │name  │               │
│  HEIC · 3 files    → JPG ▾   │  │PNG→JPG│ │…    │ │ ✓ −62%│ │ ✗    │               │
│                              │  └──────┘ └──────┘ └──────┘ └──────┘               │
│  Quality  [Best|Balanced|Small]                                                     │
│  Save to  Next to originals ▾│                                                     │
│                              │                                                     │
│  [      Convert 13 files    ]│                                                     │
├──────────────────────────────┴─────────────────────────────────────────────────────┤
│ 13 files · 48.2 MB   ·   Media ✓  Office ✓  OCR ✓                       status 28  │
└─────────────────────────────────────────────────────────────────────────────────────┘
```

- **Empty state.** A large dashed drop zone: "Drop files here", plus an "Add files…" button.
- **Cards.**
  - Each card shows a thumbnail (images, SVG, and the first page of a PDF or of a Word, PowerPoint, Excel, ODT or RTF file; other files get a category glyph tinted per docs/02 §14), a type badge, the name (ellipsized) and the size.
  - The status line reads "PNG → JPG", a spinner, "✓ 1.2 MB · −62%", or "✗ reason".
  - Hovering shows a select checkbox and a remove ×. Click selects; Ctrl/Shift+click extends; Ctrl+A selects all; Delete removes.
  - A finished card offers **Show in folder**.
- **Sidebar, Convert tab.**
  - One row per source format, with a target dropdown. The dropdown lists every reachable target, ranked the same way as the wheel: frecency plus priors.
  - A row the graph can't convert shows "No conversions".
  - **Convert** acts on the selection when there is one, otherwise on every pending file.
  - While it runs, the button becomes **Cancel**.
- **Sidebar, Tools tab.** The tools that apply to the selection (`tools_for`), each as a row with **Run**.
- **Status bar.** File count and total size, plus which packs and OS engines were found.

- **Search** (in the sidebar under the Convert | Tools tabs, Ctrl+F; Esc clears). Every word typed must match, in any case. It narrows the cards to files whose names match (when no name matches, it's a search for tools or formats and every file stays), the Convert list to formats whose name or targets match, and the Tools list to tools whose name or caption match. While searching, tools that match but don't apply to the files on the Board are listed under "Not for these files", dimmed, so a tool can be found before its file is added. When no format matches but tools do, the Convert list points to the Tools tab.

### 3.3 Batch runner (system design)
- On Convert, the runner hands the whole batch, each file with its own target, to `swirvo_jobs::convert_each` on **one coordinator thread**. That runs **one pool of workers (one per core)** for all of it, so a batch where some files become PDF and others JPG keeps every core busy, with one no-replace name reservation across the batch.
- **Tools** run over several files at once too (`tools::par_map`, results in file order): up to 8 images, 6 PDFs or Word files, 3 PDFs for redaction, OCR and compression (they render pages at high resolution), and 2 videos (encoders already use every core).
- **Engines that can't share** stay one at a time whatever the pool size: LibreOffice (one profile) and Microsoft Office (each conversion starts its own Word, Excel or PowerPoint, and a hung one is found by the processes that appeared, which a second conversion would muddle). Signing uses the certificate store one file at a time.
- Events (Done/Failed/Skipped) go through an `mpsc` channel to the UI thread, keyed by input index. The coordinator then calls the **waker**, which on Windows posts `WM_APP` to the window.
- The UI thread drains the channel at the start of each frame. It never blocks and never does I/O.
- Cancel sets a `CancelToken` that every running step checks.
- After a run, frecency is updated and saved on a background thread.

### 3.4 Thumbnail service
- **Workers.** Two of them (one on 2-core machines). The job queue is guarded by a `Mutex`, with a `Condvar` to wake workers. Each job carries a *generation*: removing or clearing files bumps it, and workers drop stale jobs.
- **Priority.** The UI enqueues only cards that are visible, in visual order, so the first screen of thumbnails arrives first.
- **Decoders.**
  - Images: `image` + EXIF orientation, downscaled to at most 2× the card size. Uses the Triangle filter: quality is fine at this size and it runs about 4× faster than Lanczos.
  - SVG: resvg. PDFs: hayro, page 1 only.
  - Word, PowerPoint, Excel, ODT and RTF (`engines::preview`): page 1 is laid out by our own engines (docs/16) into a one-page PDF in memory and rendered by hayro. Nothing is written to disk and no Office suite is needed. The layout runs on its own thread with a 6 s cap; past it the card keeps its glyph and the thumbnail worker moves on. Files over 40 MB aren't previewed.
  - HEIC/RAW: through the conversion engines when available.
- **Cache.** An LRU of premultiplied RGBA thumbnails, keyed by (path, mtime, length), with a **32 MB budget**. It is a `HashMap` plus a recency `VecDeque`; eviction pops from the front, skipping stale entries.

### 3.5 Tool previews
When a PDF tool that changes how pages look has its options open (watermark, page numbers, header and footer, Bates, crop, trim, resize, N-up, booklet, grayscale, redact, compress, and Sign for placing its box), the file grid gives way to a **preview** of the first PDF in scope (`board/src/preview.rs`). The × hides it; a Preview button in the form brings it back.
- **What it shows.** The page as the tool will leave it (*After*), or as it is (*Before*), with ◀ Page n of N ▶. N-up and booklets step through the printed sheets instead.
- **How it runs.** A worker thread keeps the PDF open for *Before* renders. A quarter second after the options stop changing, it runs `pdf_ops::preview`: tools that change pages one at a time run on a lopdf copy of just that page (so a 500-page file costs one page). Page numbers, headers and Bates numbers do too: the page is stamped alone with the number and total it gets in the whole run. Only N-up and booklets, which re-lay pages out, run on a copy of the whole file. Both copies sit in a temporary folder, so nothing is written next to the PDF. A newer request cancels the one running; the result is rendered at up to 1400 px.
- **Set by pointing.**
  - **Crop:** the box is drawn on the page and the part cut off is dimmed. Drag an edge, a corner or the middle, or press outside the box to draw a new one. *Every page* edits the shared margins; *Just this page* gives the page its own box (the `custom` option, `3: 0, 0, 20, 20; 5-7: …` in mm). *Fit to content* sets the box to the page's ink plus 6 pt, and *Use the shared box* drops a page's own box. The form's numbers follow the drag, and the drag follows typed numbers.
  - **Watermark:** a crosshair marks its centre; drag it anywhere (the `place` option, `50, 20` in % from the top-left) and scroll over the page to make it bigger or smaller (`size`, % of the page width). Colour and angle (diagonal, flat, upright, falling) are options.
  - **Page numbers and Bates:** six dots on the page; clicking one sets the position. **Header and footer:** the dots set the alignment.
  - **Sign:** nothing runs (signing needs the certificate), but the signature box (220 × 64 pt) is drawn where it will go and can be dragged, or put on any page by pressing there. It sets the `place` option (`2: 120, 240`, the box's top-left corner in mm), and the preview follows the box to its page.
- **Permissions.** The preview checks the PDF's permissions like the tool does, so a locked file shows why instead of a preview.

## 4. Page picker

### 4.1 When it appears
These PDF page tools open the picker when **one PDF with more than one page** is dropped on them:
- Pages → JPG
- Pages → PNG
- Split
- Rotate
- New: **Pages…**, which opens it on "Extract".

The page count is checked on the job thread, never on the wheel thread. Single-page PDFs and multi-file drops run as before. All pages start selected, so the old behavior is one **Enter** away.

### 4.2 Layout
```
┌ Report.pdf · 24 pages ─────────────────────────────────────────────── × ┐
│ [All] [None] [Odd] [Even] [Invert]     Pages  [ 1-3, 7, 10-        ]    │
│ ┌────┐ ┌────┐ ┌────┐ ┌────┐ ┌────┐ ┌────┐                               │
│ │ ✓1 │ │ ✓2 │ │ ✓3 │ │  4 │ │  5 │ │  6 │   page thumbnails (A4 ratio)  │
│ └────┘ └────┘ └────┘ └────┘ └────┘ └────┘   click toggles; Shift = range │
│ …                                                                         │
├───────────────────────────────────────────────────────────────────────────┤
│ Do  [Images JPG][Images PNG][New PDF][Split][Rotate][Delete]  150 dpi ▾  │
│                                              [ Export 4 pages as JPG ]   │
└───────────────────────────────────────────────────────────────────────────┘
```
- The **range field** and the grid stay in sync both ways. The grammar is `1-3, 7, 10-` (a trailing `-` means "to the end"), plus the words `odd`, `even`, `all`. Invalid input turns the field red and keeps the last valid selection.
- **Operations.** All of them write new files; the original is never changed.

| Operation | Output | Implementation |
|---|---|---|
| Images JPG / PNG | `stem (images)/stem p03.jpg` (one page → a single file next to the PDF) | hayro at 150 or 300 dpi |
| New PDF | `stem (pages 1-3, 7).pdf` | krilla `embed_pdf_pages` with the selection |
| Split · Each page | `stem (pages)/stem p03.pdf` per selected page | krilla |
| Split · Groups | `stem (parts)/stem 1 (p1-15, 16, 18-20).pdf`, one per group | krilla, `assemble::split_groups` |
| Rotate | `stem (rotated).pdf`, only the selected pages turned 90° | lopdf `/Rotate` (lossless) |
| Delete | `stem (edited).pdf` without the selected pages | krilla with the complement |

- **Split into groups.** With Split chosen, *Each page | Groups* sits beside the operations. In Groups mode the toolbar has a chip per output PDF (*PDF 1 · 17*), *+ PDF*, and the whole split as text: `1-15, 16, 18-20; 25-40, 42`, where a `;` (or `|`, or a new line) starts the next PDF. Clicking pages adds them to the active PDF or takes them out (Shift for a range, or press and sweep across pages). Every page shows a numbered dot in its group's colour for each PDF it's in, since groups may share pages. The text and the pages stay in sync both ways, and an error names the group ("PDF 2: page 61 doesn't exist"). Switching to Groups starts PDF 1 from the current selection. The same split is the `pdf-split-ranges` tool (`--set ranges="1-15, 16; 25-40"`).
- **Sweep to select.** Pressing on a page flips it; dragging on across other pages gives them all the same state (the run from the first page to the one under the pointer, in page order), in both the normal selection and Groups mode.
- **Thumbnails.** One worker thread parses the PDF **once**. It renders the unrendered page nearest the visible range first, so scrolling a 500-page PDF stays smooth.
- **After export**, an inline result bar appears: "Saved 4 images → Report (images)", with [Show in folder] and [Close]. Esc closes the picker.

### 4.3 Organise mode
*Built 2026-11-10. Code: `crates/board/src/pages/organise.rs`; the job is `swirvo_jobs::organise`, the engine `pdfedit::organise` and `pdfedit::annotate` (docs/17 §2).* The last segment of the operation bar, **Organise**, turns the grid into tiles to rearrange by hand. Nothing is written until **Save as a new PDF**, which writes `stem (organised).pdf`.

- **Grid.**
  - Drag tiles to reorder them; an accent bar shows the gap they'll land in, and the view scrolls when the pointer is held near an edge.
  - Dragging a selected tile moves the whole selection. Click selects one page, Ctrl+click adds pages, Shift+click selects a range.
  - Toolbar: Undo (Ctrl+Z, 50 steps), Rotate, Duplicate (Ctrl+D), Delete (Del), Blank page (the size of its neighbour), Insert PDF… (after the selection; dropping PDFs on the window does the same), Edit page (Enter or double-click).
  - Tiles show the page turned as it will be, its marks and crop, and a badge counting them. Pages from inserted PDFs are labelled with their file name.
- **Page editor.** The page is shown large (a 1400 px render made on the file's worker thread, ahead of thumbnails), with a side panel:
  - **Comments:** Highlight, Note, Text, Draw (ink), Box, in five colours. They become real PDF annotations.
  - **Sign:** pick a PNG or JPEG of a signature (read and decoded off the UI thread; dropping a picture also works), then drag a box. The picture keeps its aspect ratio; a click places it 150 pt wide.
  - **Black out:** boxes whose content is removed for good (the page is rebuilt from pixels on save).
  - **Crop:** drag the part to keep; the outside is dimmed and can be cleared.
  - Rotate this page, previous and next page, and a list of the page's marks with a remove button each. Turning a page turns its marks and crop with it.
- **Saving** runs on a background thread with Cancel, like the other operations.
  - When the pages stay in their order, the PDF is edited in place, so links, form fields and bookmarks survive and a protected PDF stays protected.
  - Otherwise the pages are re-embedded, which needs a PDF that restricts nothing (docs/17 §4); the result says links and form fields weren't kept.

## 5. The UI toolkit (`swirvo-ui`)

An **immediate-mode** toolkit. Each frame the app calls widget functions that read input and draw at once. Per-widget memory (hover animation, scroll offset, text-field caret) lives in a map keyed by a 64-bit id. We picked this approach because it is small (a few thousand lines), has no retained tree to keep in sync, and is trivially testable: feed synthetic input, render, compare pixels.

| Module | Contents |
|---|---|
| `geom` | `Rect`, `Point`; RectCut layout (`cut_top`, `cut_left`, `inset`, `grid`). The layout is just rectangles, with no constraint solver |
| `theme` | docs/02 §14 tokens plus glass layers: backdrop tint, panel fill, hairline stroke, top highlight, shadows. Light and dark |
| `text` | a font set (regular + semibold, from Segoe UI Variable's `wght` axis), kerning, a glyph cache keyed by (font, glyph, size, ¼-px offset), measure / ellipsize / wrap, and a fallback font for missing glyphs |
| `canvas` | rounded rects, strokes, layered soft shadows, gradients, images, a rectangular clip stack, and text blending with a contrast curve |
| `icons` | about 40 stroke icons drawn as paths (Fluent-like 1.5 px, round caps), plus file-type badges |
| `input` | pointer, buttons, wheel, keys, text, modifiers, dropped files, drag hover |
| `ui` | the `Ui` context: hot/active/focus ids, animation springs, cursor, caption hit zones for the host, and "repaint at" deadlines |
| `widgets` | button (primary, secondary, ghost), icon button, chip, checkbox, segmented control, dropdown (a popup layer drawn last), text field, progress bar, spinner, scroll area, tooltip, window controls (only where the OS draws none) |

**Rendering budget.**
- **Event-driven.** A frame is drawn only on input, on a waker, or before an animation deadline. Idle CPU is 0.
- **Full redraw.** A 1280×800 logical window at 150 % scale is 2.3 MP. That takes about 3–6 ms on CPU, plus 1–2 ms to blit. Damage tracking is a later optimization if profiling asks for it.
- **No glyph re-rasterization.** Glyphs are rasterized once per (size, offset) and reused.

**Glass.** On Windows 11 the window gets the system **Acrylic** backdrop: a real blur of what's behind it, done by DWM with no cost to us. We extend the frame into the whole client area and paint premultiplied alpha. Panels are translucent white (light) or white at 5–8 % (dark), with a 1 px hairline and a brighter top edge. Without a backdrop (Windows 10, snapshots), a soft orange-tinted gradient stands in for it.

**Testing.**
- Widget logic gets unit tests with synthetic input (click, drag, keys).
- Each Board and picker state gets a **pixel snapshot**: an example renders PNGs we can review, and tests assert that a frame renders and that key regions are where they should be.
- The range parser gets proptest round trips.

## 6. Windows host and process model

- **Same exe, different roles.** `Swirvo.exe` with no flag is the agent. `Swirvo.exe --board [files…]` is the Board. `Swirvo.exe --pages <tool> <file>` is the picker. The agent starts them with argument vectors (rule 7) after `AllowSetForegroundWindow`, so the new window gets focus.
- **The Board is single-instance.** The `Local\Swirvo.Board` mutex marks a running Board. A second launch finds the window and hands over its files with `WM_COPYDATA` (UTF-16, NUL-separated), then exits.
- **Window.**
  - Style is `WS_OVERLAPPEDWINDOW`. `WM_NCCALCSIZE` removes the caption and keeps the resize borders.
  - `DwmExtendFrameIntoClientArea(-1)` plus `DWMWA_SYSTEMBACKDROP_TYPE = Acrylic` give the glass, and `DWMWA_USE_IMMERSIVE_DARK_MODE` follows the system theme.
  - We draw our own title bar. The caption buttons are Windows' native ones, which DWM draws in the extended frame: `DwmDefWindowProc` runs first in the window procedure, so their hover, press and Windows 11 Snap Layouts behave natively. Their rectangle (`DWMWA_CAPTION_BUTTON_BOUNDS`) is passed to the UI, which keeps it free. `WM_NCHITTEST` then returns `HTCAPTION` for the drag zones the UI recorded in its last frame. Hosts without native buttons (snapshots, a future Linux host) get our drawn buttons and `HTMINBUTTON`/`HTMAXBUTTON`/`HTCLOSE` zones instead.
  - Per-monitor DPI v2, with `WM_DPICHANGED` handled.
- **Paint.** The frame is drawn into a reused 32-bpp top-down DIB, then blitted with `BitBlt`, which keeps per-pixel alpha for DWM.
- **Input.**
  - Mouse: move, buttons, wheel and leave. Keyboard: `WM_KEYDOWN`, plus `WM_CHAR` for text.
  - Files dropped from Explorer arrive through an OLE `IDropTarget`.
  - The `IFileOpenDialog` file and folder pickers run after the frame, never inside it.
- **Background work** wakes the window with `PostMessageW(WM_APP)`.

## 7. Agent changes
- The tray menu gets **Open Board…**.
- The Actions wheel gets **Board** ("Open these in the Board"). The Convert wheel's "More…" opens the Board with the files, so every target is reachable.
- The PDF page tools open the picker as described in §4.1.

## 8. OS engines and new formats

| Engine | Capabilities | Where |
|---|---|---|
| `os-wic` (Windows) | {heic, avif, dng, cr2, cr3, nef, arw, …} → png, **only for codecs WIC lists on this PC** | `engines/src/os/wic.rs` |
| `os-ocr` (Windows) / `tesseract` (Linux) | jpg, png, webp, bmp, tiff, gif → txt, so png → txt → docx is planned automatically. Also used by the PDF reader when a PDF has no text layer, and by **Copy text** on images | `engines/src/os/ocr.rs`, `engines/src/ocr.rs` (portable trait) |
| `os-media` (Windows Media Foundation) | video {mp4, mov, mkv, avi, wmv, webm, …} → mp4 (H.264 + AAC, hardware encoders when the GPU has them) or an animated gif (480 px, 12 fps, first 20 s); video or audio → m4a, aac, mp3, wav. Codecs are Windows' own (HEVC, VP9, AV1 decode when their Store extensions are installed), so we ship and license none. Encoder output types are picked from the list the encoder itself reports. FFmpeg, when installed, still wins for formats Windows can't write. Also the video tools' backend ([11](11-tools-and-features.md) "Video and audio"): duration and size from the source reader, frames decoded to RGB at full size (up to 4K wide), with a seek per frame unless the next one is less than 2 s ahead, and H.264 at a given bitrate and size for **Compress to a size** | `engines/src/os/mediafoundation.rs`, `engines/src/video.rs` |
| `ms-office` (Windows, when Office is installed) | Word {docx, doc, odt, rtf}, Excel {xlsx, xls, ods}, PowerPoint {pptx, ppt, odp} → pdf, and doc → docx, xls → xlsx, ppt → pptx. Office's own export, so it matches what the user sees in Office; preferred over LibreOffice when both are installed. Detected from the registered ProgIDs; no app starts until a file is converted. Macros are force-disabled (`AutomationSecurity`), alerts off, files open read-only, password-protected files fail instead of prompting. Each job has a deadline; a hung Word or Excel instance (the one that appeared when we started it) is ended; PowerPoint, being single-instance, is never ended. **Not yet tested against a real Office install**: the late-binding layer is tested against `Scripting.Dictionary` | `engines/src/os/msoffice.rs` |
| `certsign` (Windows) | Not a conversion: the PDF tools **Sign** and **Signatures** use the user's certificates in the Windows certificate store (Personal) through CryptoAPI: detached CMS signatures with SHA-256, the certificate chain and a signed signing time; verification with the chain engine's trust verdict at the signing time. Keys stay in the store; smart cards ask for their PIN in Windows' dialog. No crypto crates. See docs/17 §2 | `engines/src/os/certsign.rs` |
| `systar` (bsdtar) | {7z, rar, iso, cab} → zip, tar via `bsdtar -a -cf out @in`; **Extract here** for those types | `engines/src/os/systar.rs` |
| `raw` (built in) | {dng, cr2, nef, arw, …} → jpg: the largest embedded JPEG preview, EXIF-oriented | `engines/src/raw.rs` + `tiff.rs` (IFD walker) |
| `psd` (built in) | psd → png (flattened composite; 8- and 16-bit, RGB/gray/CMYK, raw and RLE) | `engines/src/psd.rs` |
| `font` (built in) | ttf/otf ⇄ woff ⇄ woff2 (WOFF2 decode includes the glyf/loca and hmtx transforms; encoding uses the null transform) | `engines/src/font.rs` |
| `data` (extended) | xml ⇄ json (attributes as `@name`, text as `#text`, repeated children as arrays) | `engines/src/data.rs` |

Security: every OS engine runs on a job thread, like the rest of S1 (rule 3's known gap). Child processes get argument vectors only and `CREATE_NO_WINDOW`, and are killed on cancel.

## 9. Delivery order and acceptance

| Step | Done when |
|---|---|
| 1. Page operations in `pdf_tools` (subset render/extract/rotate/delete) and the range parser | Unit and proptest pass; outputs open in hayro |
| 2. `swirvo-ui` | Widget tests pass; snapshot PNGs look right in light and dark |
| 3. `swirvo-board`: Board and picker apps, thumbnails, runner | Headless tests convert a mixed batch and export a page subset; snapshots reviewed |
| 4. Windows host and agent wiring | `--board` and `--pages` open with glass on the user's PC; drop, convert and export work |
| 5. OS engines (WIC, OCR, bsdtar) | Verified on the user's PC through `swirvo-cli.exe` |
| 6. PSD, RAW preview, fonts, XML ⇄ JSON | Round-trip tests |
| 7. Docs (10, 11, README), clippy, fmt, deny, Windows build, redeploy | All green |

Not in this round (🔜): drag-to-reorder pages, page thumbnails in the wheel hub, a Linux window host (the toolkit is ready for it), searchable-PDF output (OCR text layer), encoding HEIC/AVIF, a license/trial flow, an installer.

## 11. Settings window and install

**Settings** (`board/src/settings.rs`, `Swirvo.exe --settings`, single-instance): from the tray menu ("Settings…") or the gear in the Board's title bar. Sections on the left, settings on the right; every change applies at once (no Save button).

| Section | Settings |
|---|---|
| General | Start with Windows · a tip when Swirvo starts · theme (System, Light, Dark) · Install / Uninstall Swirvo (or "Installed from the Microsoft Store") |
| Output | Save new files next to the original (default) or in a chosen folder · quality (Best, Balanced, Smallest) · show the new file in its folder after a wheel conversion |
| Wheel | Which key opens which wheel (Shift, Ctrl, Alt) · Pause the wheel (also a checkable item in the tray menu) |
| Engines | Use Microsoft Office / LibreOffice / FFmpeg when installed, each with what was found on this PC (detected on a background thread) · files at once (Auto = one per core, or 1–16) |
| Privacy | Offline mode (on by default: nothing in the app uses the network; future update checks only run with it off) · clear the recent-format history |
| About | Version · open-source licences · privacy summary |

**Storage** (`jobs/src/settings.rs`): `settings.json` in the data dir, versioned; unknown fields are ignored and missing ones take defaults, so older and newer builds share it. `settings::current()` is a cached `Arc` (a read lock, no I/O), so the overlay can check "Pause the wheel" on every trigger. Saving is atomic (temp file + rename). After a save the host broadcasts the registered message `Swirvo.SettingsChanged`; every Swirvo window and the agent re-read the file on a background thread and re-apply the theme. Clearing the history broadcasts `Swirvo.HistoryCleared` so the agent drops its in-memory copy too.

**Where settings apply:** `Options::from_settings()` (preset, folder, files at once) for the Board and the wheel; `tools::workers` caps tools at "files at once"; `Toolkit::new()` leaves out the engine packs turned off (new Board windows at once, the wheel on its next start); the window hosts and the overlay take the theme; the agent skips triggers while the wheel is paused and shows the startup tip only if it's on.

**Install** (`platform/win/src/install.rs`): the first time a copied `Swirvo.exe` starts, it offers once to install for this user, no admin. The tray menu keeps "Install Swirvo…" until it's done.

## 10. ADR 009: native Board on our own toolkit (supersedes 003)

| | |
|---|---|
| Decision | The Board and editors use `swirvo-ui` (immediate mode, tiny-skia), hosted by a thin per-OS window layer |
| Why | No web view or Node toolchain. One exe, ~1 MB of UI code, a cold start of about 50 ms, and about 30 MB while open (against 80–150 MB for Tauri). Pixel-testable on Linux. The same drawing code as the wheel |
| Cost | We own text input, accessibility (UI Automation is 🔜) and complex editors. The timeline and canvas editors (docs/02 §12) will be custom widgets |
| Rejected | Tauri + React (Node build chain, web view memory); egui or Slint (a large dependency we'd still need to theme heavily) |
