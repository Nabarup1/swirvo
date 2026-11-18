# 11 · Tools, actions and features

What the **Tools** (Ctrl) and **Actions** (Alt) wheels offer, and the app features around them. The wheels show only what applies to the dragged files and actually works; nothing is shown as "coming soon".

**Status:** ✅ built · 🧩 needs an engine pack · 🔜 planned (phase from docs/08: P1 MVP, P2 v1.0, P3 v1.5)

Every tool follows the conversion rules: outputs go next to the originals with a fresh name (`photo (2).jpg`), are written to a temp file and committed atomically, and originals are never modified.

## Tools wheel (Ctrl)

### Images
| Tool | What it does | Status |
|---|---|---|
| Resize 50 % | Half width and height, high-quality Lanczos filter | ✅ |
| Fit 1920 px | Longest side at most 1920 px (smaller images unchanged) | ✅ |
| Rotate right | 90° clockwise | ✅ |
| Flip | Mirror horizontally | ✅ |
| Compress | JPEG at quality 72, PNG at maximum compression; keeps the smaller file | ✅ |
| Strip metadata | Removes EXIF (GPS, camera), XMP and IPTC. JPEG: lossless, segment by segment | ✅ |
| Combine into PDF | Several images become one PDF, a page per image, in name order | ✅ |
| Copy text (OCR) | Reads the text in a photo or screenshot onto the clipboard (Actions wheel) | ✅ 🪟 OCR |
| Compress to a target size, crop, annotate, redact | | 🔜 P2 |
| Remove background, upscale | Local AI | 🔜 P3 |

### PDF
| Tool | What it does | Status |
|---|---|---|
| Merge | Several PDFs become one, in name order | ✅ |
| Split pages | One PDF per page, in a new folder | ✅ |
| Rotate right | 90° clockwise (lossless, page attribute only) | ✅ |
| Pages to PNG / Pages to JPG | One image per page (150 or 300 dpi), in a new folder | ✅ |
| **Page picker** | For a multi-page PDF, the four tools above and **Pages…** open a window: click pages (Shift for a range) or type `1-3, 7, 10-`; then export images, save a new PDF, split, rotate or delete the selected pages | ✅ |
| Text from scanned PDFs | Convert to TXT/DOCX: OCR runs when a PDF has no text layer | ✅ 🪟 OCR |
| Compress | Screen, print, email or smallest; or "fit under N MB". Images are downsampled by their displayed size, photos become JPEG, diagrams stay lossless, duplicates merge | ✅ |
| Page numbers, header/footer, Bates numbers | Real text stamped on the pages; links and form fields keep working | ✅ |
| Watermark | Diagonal text or a picture, light to strong | ✅ |
| Protect / Unlock | AES-256 password and permissions. Unlocking a restricted file needs its owner password | ✅ |
| Trim margins, crop, resize to A4/Letter… | Lossless crop box; resize scales content and links together | ✅ |
| N-up, booklet | 2–9 pages per sheet; saddle-stitch booklet order | ✅ |
| Split every N pages, at bookmarks, under N MB | Into a new folder | ✅ |
| Remove hidden data, properties, repair | Author info, scripts, attachments; title/author; damaged files | ✅ |
| Bookmarks from headings, save pictures, attachments | Bookmarks jump to each heading | ✅ |
| Make searchable | Scans get an invisible OCR text layer: find, select and copy work, the page looks the same | ✅ 🪟 OCR |
| Redact | Words, phrases, emails or phone numbers removed for good: matching pages are flattened with black boxes, metadata and comments scrubbed, the result re-read to check | ✅ |
| Grayscale | Colours to gray, text stays text | ✅ |
| Compare | Two PDFs side by side, removed red, added green: word by word, or by appearance for scans and drawings (changed areas boxed), with a summary page | ✅ |
| Form fields, fill form, flatten | Fields to a spreadsheet; one filled PDF per row; fields and comments drawn into the page | ✅ |
| Sign | With your certificate from the Windows certificate store; visible box or invisible; earlier signatures stay valid | ✅ 🪟 |
| Signatures | Checks every signature: intact, changed afterwards, who signed and when, trusted or why not | ✅ (the cryptographic check 🪟) |
| Reorder by drag, redaction boxes, comments by hand | Plan and order in [17](17-pdf-and-document-tools.md) §5 | 🔜 |

### Word documents
| Tool | What it does | Status |
|---|---|---|
| Compress | Pictures shrunk to their displayed size | ✅ |
| Clean | Author info, comments; accept tracked changes | ✅ |
| Find and replace | Also across Word's split runs, in headers, footers and notes | ✅ |
| Pictures, word count | Save the pictures; counts to the clipboard | ✅ |
| Mail merge | Letter + spreadsheet → a DOCX (and PDF) per row | ✅ |
| PDF/A | Word, ODT, RTF, PowerPoint or Excel → archival PDF/A-2, tagged for screen readers (2a) by default | ✅ |

Tools with options show a form in the Board (Tools tab). From the wheel, the ones that need input (a password, watermark text) open that form, and **More…** opens the full list. Details, the protection rules and how each tool works: [docs/17](17-pdf-and-document-tools.md).

### Archives
| Tool | What it does | Status |
|---|---|---|
| Extract here | Into a new folder named after the archive; never merges into an existing one. 7Z, RAR, ISO and CAB through the system's libarchive | ✅ |
| Test | Reads every entry and reports files, size and errors | ✅ |

### Video and audio
| Tool | What it does | Status |
|---|---|---|
| Frame | A picture of one moment. You can type a time (`1:23`, `83`, `40 %`); by default it takes a good frame near the start: 10 % in, skipping black, white and single-colour frames. Or one picture every N seconds into a new folder (`clip 00m05s.png`, at most 1000). PNG or JPG | ✅ Windows; FFmpeg elsewhere |
| Compress to a size | Email 25 MB, chat 10 or 8 MB, upload 50 or 100 MB, or any size (1 MB = 10⁶ bytes, so a limit is never crossed). Gives `clip (25 MB).mp4`, H.264 + AAC; a video already under the size is left alone | ✅ Windows; FFmpeg elsewhere |
| Extract audio, GIF from clip | As conversions (video → mp3, m4a, wav, gif) | ✅ |
| Mute, trim, normalize loudness, transcribe | | 🔜 P2–P3 |

How **Compress to a size** decides:
1. **Budget.** It aims at 94 % of the target, leaving room for the container and rate control. The bitrate is that budget over the video's length.
2. **Sound** gets the best rate that fits in about 10 % of the bitrate, and the picture gets the rest.
3. **Picture size.** When the picture would get fewer than 0.05 bits per pixel per frame (where H.264 turns to mush), it's scaled down (never up, never below 144 lines) until it gets that much. A target too small even then is refused, with the smallest size that works ("try at least 6.2 MB").
4. **Second try.** If the file comes out over the target anyway, the picture's bitrate is cut in proportion and it's encoded once more. If it's still over, the result says so.
5. **Encoder.** FFmpeg, when installed, encodes in two passes with libx264, or in one pass with another H.264 encoder it has. Otherwise Windows Media Foundation encodes (constant bitrate, hardware when the GPU has it). A backend that can't read the file hands over to the next. Code: `engines/src/video.rs`, tools in `jobs/src/media_ops.rs`.

### Any file
| Tool | What it does | Status |
|---|---|---|
| Checksum | SHA-256 of each file, copied to the clipboard | ✅ |
| Batch rename, Base64, split/join | | 🔜 P2 |

## Actions wheel (Alt)

| Action | What it does | Status |
|---|---|---|
| Show in folder | Opens Explorer with the file selected | ✅ |
| Copy path | Sub-ring: plain, quoted, forward slashes, WSL `/mnt/c/…` | ✅ |
| Terminal here | Windows Terminal (or PowerShell) in the file's folder | ✅ |
| Compress to ZIP | All dragged files into one ZIP next to them | ✅ |
| Board | Opens the dragged files in the Board (docs/12 §3) | ✅ |
| Copy text | Contents of a text file (TXT, MD, CSV, JSON, code…), or the text in an image (OCR), to the clipboard | ✅ |
| Copy as image, Share, Send to device, Shelf, Copy/Move to…, Open with… | | 🔜 P1–P3 |

## App features

| Feature | Status |
|---|---|
| Drag + Shift / Ctrl / Alt wheel, preset sub-ring (Best · Balanced · Smallest) | ✅ |
| Tray icon, notifications for results, single instance | ✅ |
| Standalone `.exe` (no installer, no runtime) | ✅ |
| Command line: `swirvo-cli convert / tool / tools / extract / plan / detect / formats` | ✅ |
| Engine packs detected automatically (FFmpeg, LibreOffice) | ✅ |
| **Board**: drop many files, pick a target per source type, quality and destination, convert all; run tools on a selection; thumbnails, progress, results with Show in folder (tray → Open Board, Actions → Board, Convert → More…) | ✅ |
| Glass UI: Acrylic backdrop, custom title bar with Snap Layouts, light/dark following Windows, per-monitor DPI | ✅ |
| Start with Windows, settings, history with undo | 🔜 P1–P2 |
| Selection shortcut (no drag), Explorer context menu, hot edge | 🔜 P1 |
| Engine-pack downloader, auto-update | 🔜 P2 |
| Linux overlay (X11/Wayland) | 🔜 P2 |
| Recipes, watch folders | 🔜 P2–P3 |
