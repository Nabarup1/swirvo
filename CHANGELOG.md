# Changelog

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

## 0.1.2 (2026-11-21)

- **Linux: runs on far more systems.** The `swirvo` command-line tool is now built against glibc 2.28, so it runs on Ubuntu 18.10+, Debian 10+, RHEL/Rocky/Alma 8+, Fedora and Mint 20+ (0.1.1 needed Ubuntu 24.04 or newer).
- **Linux on ARM64** (Raspberry Pi 4/5 with a 64-bit OS, ARM servers): new `linux-arm64` downloads.
- **One-line install on Linux:** `curl -fsSL https://raw.githubusercontent.com/Nabarup1/swirvo/main/install.sh | sh`, no root needed; it verifies the checksum.
- **`.deb` packages** for Debian, Ubuntu and Mint: `sudo apt install ./swirvo_<version>_amd64.deb`.
- Swirvo is on the [Microsoft Store](https://apps.microsoft.com/detail/9NPGWFBVTDJQ).
- The Claude plugin is now a skill that drives the `swirvo` command line (no server to start); add the MCP server separately if you want it.

## 0.1.1 (2026-11-18)

- **`swirvo` in every terminal.** Installing Swirvo now puts the command line on your user PATH (`%LOCALAPPDATA%\Programs\Swirvo\bin\swirvo.exe`), so `swirvo convert …` and `swirvo mcp` work anywhere. Uninstall takes it off again; nothing else in your PATH is touched.
- **Claude Desktop extension:** `Swirvo-<version>.mcpb` in every release; double-click it to add Swirvo to Claude Desktop.
- **Claude plugin** in [`plugin/`](plugin): the MCP server plus a skill that tells Claude which Swirvo tool fits which task, for Claude Code and Cowork.
- **Listed in the official MCP Registry** as `io.github.Nabarup1/swirvo`, published automatically with each release.
- Release builds and tests made reliable on GitHub's machines.

## 0.1.0 (2026-11-17): first public release

The first public build of Swirvo for Windows, and the `swirvo` command-line tool for Windows and Linux.

- **The wheel:** start dragging any file, tap **Shift** (formats), **Ctrl** (tools) or **Alt** (actions), and drop it on a wedge. The result is saved next to the original; originals are never changed.
- **The Board:** convert whole folders at once, each file to its own format, in parallel. Search formats and tools with Ctrl+F.
- **About 96 formats (138 file extensions)** across images, documents, PDF, Office files, spreadsheets and data, fonts, audio, video, subtitles and archives. PDF → Word rebuilds the layout; Word, Excel and PowerPoint → PDF without Office installed.
- **59 tools**, including 35 for PDF: merge, split (your own page ranges too), compress, watermark, crop, page numbers, Bates, protect/unlock, OCR, redaction, forms, compare, digital signatures and PDF/A, many with a live preview.
- **Private by design:** everything runs on your PC, with no account, no upload and no telemetry. Each conversion runs in a separate sandboxed process.
- **Settings window** and a per-user installer (Start menu, Settings → Apps, start with Windows).
- **For AI assistants:** `swirvo mcp` runs a local MCP server with `convert_files`, `run_tool`, `extract_text`, `list_tools`, `list_formats` and `detect_format`, for Claude Desktop, Claude Code, Cursor, VS Code and other MCP clients ([docs/19](docs/19-ai-assistants-mcp.md)).

## Development history (before 0.1.0)

### Added
- **Settings window** (tray menu → Settings…, or the gear in the Board): theme (System, Light, Dark), start with Windows, where new files go, default quality, show the new file in its folder, pause the wheel (also in the tray menu), which installed programs to use (Microsoft Office, LibreOffice, FFmpeg, with what was found), how many files at once, offline mode, clear the recent-format history, and About with the licences. Changes apply at once to every Swirvo window.
- **Install for this user.** The first start of a downloaded `Swirvo.exe` offers to install it (no administrator rights): it moves into your user folder, appears in the Start menu and Start search and in Settings → Apps with an uninstaller, and starts with Windows. Uninstall asks whether to keep your settings. Under the Microsoft Store package, Windows handles install and startup.
- **The Swirvo icon.** A swirl of three white blades on a warm orange-to-coral square, with a simpler variant for 16–24 px. It's the tray and title-bar icon, the icon and version info of `Swirvo.exe` and `swirvo-cli.exe`, and the Store tile set; all drawn from one geometry.
- **Microsoft Store package.** `scripts/build-msix.sh` builds `Swirvo-<version>-x64.msix` (start with Windows as an opt-in startup task, and `swirvo` in any terminal). `scripts/sign-msix.ps1` signs it for a local test install; `scripts/sign-exe.ps1` code-signs the exes for direct download.
- **Pictures in HTML and EPUB.** Word, ODT, RTF, EPUB and PDF → HTML now keep their pictures, embedded in the page so it stays one file; → EPUB stores them inside the book. EMF/WMF become SVG, and other bitmaps become PNG. Pictures that only link to files elsewhere on the PC are never loaded.
- **Pictures into Word, ODT and PDF.** The other way round too: HTML (embedded pictures), EPUB, RTF, ODT, Word and PDF → DOCX, ODT and the typeset PDF keep their pictures, each stored once, sized as in the source and fitted to the page. Plain text still leaves them out.
- **Pictures in table cells and in Markdown.** Pictures inside table cells now survive Word, ODT, HTML, EPUB and Markdown → HTML, EPUB, DOCX, ODT, PDF and Markdown, fitted to the cell. Markdown files keep pictures of up to 256 KB inside the file, and reading Markdown picks up embedded pictures and ones in the `<name> files` folder next to it (never anything elsewhere on the PC or on the web).
- **Live previews for PDF tools** (docs/12 §3.5). While a PDF tool's options are open, the Board shows the page as the tool will leave it, with After/Before and page arrows. Covers watermark, page numbers, header and footer, Bates, crop, trim, resize, N-up, booklet, grayscale, redact and compress. Previews run on a copy of just that page in a temporary folder (page numbers included, so even a 500-page file previews quickly), a moment after the options stop changing. You can also point at the page:
  - **Crop:** drag the box's edges, corners or middle, or draw a new box. Apply it to every page or just this one. *Fit to content* snaps it to the printed area.
  - **Page numbers, Bates, header and footer:** click where they go.
  - **Sign:** drag the signature box to any spot on any page.
- **Conversions run in a separate worker process.** A file that crashes a converter now fails on its own ("the converter stopped unexpectedly on this file") while the rest of the batch carries on in a fresh worker, and the app stays up. On Windows the worker runs in a Job Object (it ends with the app, its memory is capped, no crash dialogs) with process mitigations against DLL injection, at below-normal priority.
- **Search in the Board.** A search box under the Convert and Tools tabs (Ctrl+F) finds files by name, formats to convert to, and tools by name or what they do, including tools for other kinds of files.
- **Faster batches.** A batch with several targets (some files to PDF, others to JPG) now runs in one pool across all cores instead of one target after another, and tools work on several files at once (images, PDFs and Word files; fewer at a time for heavy ones like redaction and video). Office conversions stay one at a time.
- **Errors you can act on.** Tool options are checked as you type, with the message under the field in plain words. Page numbers are checked against the PDF's real page count, which is shown by the field. Run stays disabled, with the reason above it, until everything is filled in correctly.
- **Starts on Windows "N" editions.** The Media Foundation DLLs are loaded only when needed, so the app no longer fails to start where they're missing; video conversion then uses FFmpeg if installed.
- **Watermark placement.** Put the watermark anywhere by dragging it on the preview, resize it with the mouse wheel, and choose its colour (red, grey, blue, black) and angle (diagonal, flat, upright, falling). Pictures (logos) can be placed and sized too.
- **Sweep to select pages.** In the page picker, press on a page and drag across others to select (or deselect) them all at once; works in Groups mode too.
- **Split into your own groups.** The page picker's Split has a Groups mode: one PDF per group. Type `1-15, 16, 18-20; 25-40, 42` or click pages into PDF 1, PDF 2…, each in its own colour. Also the **Split ranges** tool in the Board, with a row per PDF, *Add another PDF*, and *Pick visually* to open the page picker on those groups (`pdf-split-ranges` on the CLI).
- **Per-page crops.** Crop takes different margins for chosen pages (`3: 0, 0, 20, 20; 5-7: 5, 5, 5, 5`) as well as the shared ones.
- **EMF and WMF pictures** (docs/16 §12). Windows metafiles in Word, PowerPoint, Excel, ODT and RTF files (logos, pasted charts, diagrams) are drawn as vectors with our own interpreter, so lines stay sharp and text stays text. EMF/WMF files also convert to PDF, SVG and PNG on their own.
- **PDF/A-3 and PDF/UA** (docs/16 §13). The PDF/A tool has a *Standard* choice: PDF/A-2, PDF/A-3 with the original Word, ODT, RTF, PowerPoint or Excel file attached inside the PDF, or PDF/UA-1 for accessibility (title bar title, outline, descriptions on every picture). Also `--pdfa3` and `--pdfua` on the CLI.
- **PDF/A and tagged PDF** (docs/16 §13). Word, ODT, RTF, PowerPoint and Excel → PDF can write archival PDF/A-2 (2a when tagged) and accessible tagged PDF: headings, lists, tables, figures with alt text, reading order and language. Available as the PDF/A tool and as `--pdfa` / `--tagged` on the CLI.
- **Certificate signatures** (Windows, docs/17). Sign a PDF with a certificate from your Windows certificate store, optionally with a visible signature box. Verify signatures: who signed, when, whether the file changed afterwards, and whether the certificate is trusted.
- **Video tools:**
  - save a frame (automatically a good one near the start, or at a time you choose), or one every N seconds;
  - compress a video to fit a size, such as an e-mail limit.

  They use Media Foundation on Windows, or FFmpeg when it's installed.
- **Visual compare.** Compare two PDFs by appearance as well as by words: changed areas are tinted and boxed. *Automatic* picks appearance for scans and drawings.
- **Page previews for Office files in the Board.** Word, ODT, RTF, PowerPoint and Excel files show their first page, laid out by our own engines.
- **Organise pages by hand** (docs/12 §4.3). The page picker's new Organise mode:
  - drag pages into a new order, turn, duplicate or delete them, and add blank pages or pages from other PDFs, with undo;
  - a page editor to crop, black out (true redaction), place a signature picture, and add highlights, notes, text, drawings and boxes as real PDF comments;
  - the result is saved as a new PDF.
- **Charts in Word, PowerPoint and Excel → PDF** (docs/16 §11). Bar and column (clustered, stacked, 100 %), line, area, pie, doughnut and scatter charts are drawn from the values Office stores with each chart. Titles, legends, tidy axis scales, gridlines, data labels, number formats and theme colours all carry over. Not yet: 3-D, secondary axes, trendlines and the 2016 chart types.
- **ODT and RTF → PDF with our layout engine** (docs/16 §10). Covers styles, master pages and title pages, lists, tables with spans, frames, text boxes and custom shapes, sections with columns, headers, footers and notes. Linked files outside the document are never loaded.
- **MS Office as an engine.** When Word, Excel or PowerPoint is installed, Office → PDF and old `.doc`/`.xls`/`.ppt` → modern formats can use it. It runs hidden, with macros disabled and no password prompts, and a hung Word or Excel is stopped after a deadline. DOCX → PDF still prefers our own engine; Office is the fallback. Turn it off in Settings → Engines.
- **Windows Media Foundation as an engine.** Video (MP4, MOV, MKV, WebM, AVI, WMV, 3GP, TS) → MP4 or animated GIF, soundtracks and audio → MP3/M4A/AAC/WAV, without FFmpeg, through codecs Windows already has. FFmpeg, when installed, covers the other formats.
- **PDF tools** (docs/17 §2):
  - make searchable: scans get an invisible OCR text layer, and the page looks the same;
  - true redaction of words, phrases, emails and phone numbers. Redacted pages are rebuilt from pixels and metadata is scrubbed. The written file is checked again, and nothing is saved if a match is still readable;
  - grayscale that keeps text and vector art;
  - compare two PDFs (word diff with a side-by-side report);
  - form fields → spreadsheet, fill a form once per spreadsheet row, flatten forms and comments.
- **PDF → Excel.** Tables become real cells, with merged cells, bold headers and numbers kept as numbers in their original format. Tables that span pages are joined on one sheet.
- **Word tools:**
  - compress pictures;
  - remove personal data and comments, optionally accepting tracked changes;
  - find and replace, including inside headers, footers and notes;
  - save pictures;
  - word count;
  - mail merge from CSV or a spreadsheet, optionally with a PDF each.

### Fixed
- **ODT list spacing.** List indents, contextual spacing and header room now follow the file, and all six ODT test files match LibreOffice's page counts.
- **Page layout could loop forever.** A line taller than the page followed by more text made widow/orphan control push the paragraph to the next page endlessly. This affected DOCX, ODT and RTF → PDF.

- **Excel → PDF without LibreOffice** (docs/16 §9). Sheets print like Excel prints them: print area, page setup and fit to pages, repeated titles, manual breaks, styles, Excel number formats (dates, currency, percent, fractions, colours), merged cells, overflowing text, `####` for numbers too wide, pictures, links, headers and footers. Charts are drawn (see below).
- **PowerPoint → PDF without LibreOffice** (docs/16 §8). Placeholders inherit through layout and master, and theme styles, gradients, transparency, about 60 preset shapes, custom shapes, groups, rotation and flips, pictures, styled tables, SmartArt, hyperlinks and bullets are all drawn. Slide titles become PDF bookmarks. Charts are drawn (see below).
- **PDF tools** (docs/17), in the Board's Tools tab, on the wheel and in the CLI:
  - compress (by displayed DPI, or to fit under N MB);
  - page numbers, header/footer, Bates numbers, text or picture watermark;
  - protect and unlock (AES-256; restrictions only come off with the owner password);
  - trim margins, crop, resize to a paper size, N-up, booklet;
  - split every N pages, at bookmarks or under a size;
  - remove hidden data, properties, repair, bookmarks from headings, save pictures and attachments, attach files.
- **Tool options.** Tools that need input show a form in the Board (secrets drawn as dots, a file picker for pictures). The CLI takes `--set key=value`. From the wheel such tools open the form, and **More…** opens the full tool list when they don't all fit.
- **PDF permissions are honoured.** Copy-protected PDFs aren't converted to Word or text and their pictures aren't extracted. Restricted files keep their protection when edited, and tools that would drop it refuse with a clear message. Rotating a protected PDF used to remove its protection silently.
- **Only sensible conversions are offered.** The wheel, the Board and the CLI no longer show routes like video → Word (first frame, then OCR), JSON → PDF, subtitles → EPUB or PowerPoint → Word. The rule is per source category (docs/10).
- **Fallback engines.** When an engine fails on a file, the job plans again without it and tries up to two other routes. A Word file the layout engine can't read still converts, with LibreOffice if installed or with the built-in typesetter. The result says a fallback was used.
- **Notes reach the user.** The tray notification and the Board card now show a job's note: a fallback engine was used, or the file was saved to Downloads because its own folder wasn't writable. Before, both were dropped silently.
- **DOCX → PDF that lays the pages out like Word** (docs/16), with no LibreOffice. Built on libraries already in the app.
  - Covers styles and themes, fonts with Word's line metrics, lists, tabs with leaders, and tables with table styles, merges, repeated headers and split rows.
  - Also pictures (inline, floating, wrapped), text boxes, sections, columns (balanced at continuous breaks), headers and footers with page numbers, footnotes, links, bookmarks and a heading outline.
  - Typical documents convert in 5–100 ms.
  - On ten test documents it gives the same page count as LibreOffice and near-identical pages.
  - A crafted file nested thousands of levels deep is refused with an error. It used to overflow the stack and abort the whole app, which `catch_unwind` can't prevent.
- **Terminal choices.** The Terminal wedge offers every shell found on the PC: Windows Terminal, PowerShell, Cmd, each WSL distro and Git Bash. Dropping on the wedge opens the first; resting on it shows the rest. Before, it tried Windows Terminal and silently fell back to PowerShell.
- **Double-click the tray icon** to open the Board. A single click still shows the menu, where *Open Board…* is now the bold default.
- `scripts/compact-wsl-disk.ps1`: gives the space freed inside WSL back to Windows (docs/09 §2).
- **PDF → DOCX that keeps the layout** (docs/13). Each page becomes a section of the same size and margins, and the following survive:
  - fonts, sizes, bold/italic, colours, underline and highlight;
  - columns, which become an invisible table so every editor shows them the same way;
  - ruled tables with merged cells, borders and shading;
  - pictures, and charts or vector art as pictures;
  - shaded boxes and rules as shapes;
  - real bullet lists, tab stops and dot leaders;
  - repeated headers and footers, with live page numbers;
  - links.

  It is our own code on hayro, with no new dependencies.
- PDF → Markdown/HTML/TXT/ODT/EPUB read through the same layout analysis: headings, lists and tables in reading order.
- `examples/pdf_layout`: dumps the analysed layout and compares PDFs side by side for visual round-trip checks.
- `THIRD-PARTY-NOTICES.txt`, generated by `scripts/third-party-notices.py`. It covers every bundled component, including fonts and CMaps inside crates. It ships next to the exe, and **Open-source licences…** in the tray menu opens it.
- Docs: [13 · PDF reconstruction](docs/13-pdf-reconstruction.md) ; draft `PRIVACY.md` and `EULA.md`.

### Changed
- `.gitignore` now covers build output, packaging artefacts, signing keys and certificates, env files, profiler and test scratch, and editor and OS files.
- The docs no longer name other products in comparisons.
- The dev build profile keeps line tables only and no debug info for dependencies. The build cache dropped from 44 GB to about 6 GB.

### Removed
- The `pdf-extract` dependency; PDF text now comes from our layout engine.

### Initial development (2026-11)

The first layered commits, which build in order:
- core types;
- format detection;
- conversion planner;
- wheel geometry and gesture state machine;
- built-in engines (images, documents, data, archives, audio, fonts; OS engines for OCR, HEIC/AVIF/RAW and 7z/RAR);
- jobs and tools;
- wheel renderer;
- CLI;
- UI toolkit;
- Board and page picker;
- the Windows agent with tray, overlay and app windows.
