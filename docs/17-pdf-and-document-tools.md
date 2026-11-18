# 17 · PDF and document tools

*Started 2026-11-09. Code: `crates/engines/src/pdfedit/` (the operations), `crates/engines/src/docxtools.rs` (Word tools), `crates/jobs/src/pdf_ops.rs` and `doc_ops.rs` (running them on files), `crates/jobs/src/args.rs` (tool options), the Board's Tools tab (`crates/board/src/board.rs`).*

The paid PDF apps sell five things: edit, convert, compress, sign and protect. Converting already works ([13](13-pdf-reconstruction.md), [16](16-docx-to-pdf.md)). This document covers the rest, built on libraries already in the app: lopdf (objects), krilla (writing), hayro (rendering), our layout analysis and the OS's OCR. The only new crate is `getrandom` for encryption keys, and lopdf already depended on it.

## 1. Two ways to edit a PDF

| Technique | Keeps | Used for |
|---|---|---|
| **In place** (lopdf): edit the page objects, keep everything else | Links, form fields, bookmarks, metadata, protection | Stamps, crop, resize, compress, protect/unlock, clean-up, properties, bookmarks, attachments, rotate |
| **Re-embedded** (krilla): draw pages into a new document | Page content and annotations | N-up, booklet, splitting, assembling pages from several files, repair as a last resort |

**Stamps** (page numbers, headers and footers, Bates numbers, watermarks, placed pictures) get the best of both.
1. krilla draws each page's stamp as a small PDF page, with real fonts (subset, full Unicode, a fallback font per script), images and opacity.
2. That page is grafted onto the original page as a Form XObject.
3. The original content is wrapped in `q … Q` and the stamp is drawn after it.

The page's links and fields stay where they were. Rotated and cropped pages are handled by mapping *display* coordinates (what the reader sees, upright) to the page's user space (`pdfedit::geometry`). The tests stamp the top-right corner of pages rotated 0°, 90°, 180° and 270°.

## 2. The tools

**Organize pages**
| Tool | How |
|---|---|
| Merge, split, rotate, extract, delete (page picker) | krilla re-embed; rotate edits `/Rotate` in place |
| Assemble: reorder, duplicate, insert another PDF after page N, blank pages | `assemble::assemble` with a page plan; runs of pages from one file are embedded together so fonts and images are written once |
| Split into your own groups (`1-15, 16, 18-20; 25-40, 42`: one PDF per group) | `assemble::split_groups`; also the page picker's Split → Groups (docs/12 §4.2) |
| Split every N pages · at top-level bookmarks · into parts under N MB | Size split: doubling, then binary search on the real written size (a few dozen trial writes for 500 pages, not hundreds) |
| N-up (2, 4, 6, 8, 9 per sheet), booklet | Pages drawn as XObjects into cells. Booklet order for saddle stitching: sheet *s* front = (last − 2s, 2s), back = (2s + 1, last − 1 − 2s), padded to a multiple of 4 |
| Resize to A4/Letter/A3/A5/Legal | In place: new MediaBox, a `cm` that scales and centres the content, annotation rectangles and quad points moved with it |
| Crop by margins, trim white margins | CropBox only (lossless). Crop takes shared margins for the selected pages plus per-page boxes (`custom`: `3: 0, 0, 20, 20; 5-7: 5, 5, 5, 5`, mm left, right, top, bottom; later entries win). In the Board the box is dragged on a preview of the page (docs/12 §3.5). Trim renders the page at 48 dpi and takes the bounding box of non-white pixels |

**Organise by hand** (the page picker's Organise mode, docs/12 §4.3; `organise::organise`)
| Step | How |
|---|---|
| Pages | The plan (order, duplicates, blank pages, pages of inserted PDFs) is assembled with krilla. When it is the file's own pages in their own order, the file is edited in place instead, so links, forms, bookmarks, metadata and protection survive |
| Turn and crop | In place: `/Rotate` and a CropBox. Crops are drawn on the turned page, so they are applied after turning; marks are moved by the crop's corner |
| Black out | `redact` with the boxes as areas: each page is rebuilt from pixels |
| Signatures | Pictures stamped into the page content (aspect ratio kept), after redaction so they aren't flattened |
| Comments | Highlight (`Highlight` with QuadPoints, multiplied so text stays readable), note (`Text`, NoZoom and NoRotate), ink, box (`Square`) and text box (`FreeText`). Each has its own appearance stream, turned against the page's rotation so it reads upright everywhere. Text outside Helvetica's character set is drawn into the page in a system font instead |

**Stamps:** page numbers (four styles, six positions, start number, page selection), header and footer (tokens `{n}`, `{total}`, `{file}`, `{date}`), Bates numbering (prefix, zero-padded), text watermark (diagonal by default, sized to the page; or placed, sized, turned and coloured), picture watermark (placed and sized), picture placement (signatures).

**Size and clean-up**
| Tool | How |
|---|---|
| Compress (Screen 150 dpi · Print 220 · Email 100 · Smallest 72; or "fit under N MB") | Each image's *displayed* size comes from the content streams (the CTM at every `Do`, through nested forms), so an image is only downsampled when it has more pixels than its displayed size needs. Photos (many colours) become JPEG; screenshots and diagrams stay lossless. A re-encoding is kept only when it's at least 10 % smaller. Then identical streams are merged (fonts and logos embedded twice by merging), thumbnails and `PieceInfo` dropped, streams deflated, and objects packed into object streams. "Fit under" tries stronger levels in turn and keeps the smallest result |
| Remove hidden data | Document info and XMP, JavaScript (document, page, link and form-field actions), embedded files and attachment annotations, automatic actions, thumbnails, editor data. Pages look the same |
| Properties | Title, author, subject, keywords (the stale XMP copy is removed); the viewer shows the title |
| Repair | lopdf rebuilds a broken cross-reference table and keeps everything; failing that, hayro's tolerant parser recovers the pages |
| Bookmarks from headings | Our layout analysis ([13](13-pdf-reconstruction.md)) finds headings and their levels; each bookmark jumps to the heading's position, not just its page |
| Save pictures | JPEG and JPEG 2000 copied byte for byte; other images decoded (1/2/4/8-bit gray, RGB, CMYK, indexed, soft masks as alpha) and saved as PNG |
| Attachments: save them, or attach files to a PDF | Saved names are made safe: no folders, no reserved names, never outside the new folder |
| Grayscale | Colours are rewritten where they're set, so text stays text and art stays vector: `rg`/`k` → `g`, RGB/CMYK colour spaces (device, calibrated, ICC) → DeviceGray with their `sc` values converted, pictures re-encoded as gray (JPEG stays JPEG), gradients built from exponential and stitching functions converted exactly (the gray of a blend is the blend of the grays), comment colours too. Spot colours (Separation, DeviceN) and mesh gradients keep theirs, and the result says how many |

**Text, redaction and comparison**
| Tool | How |
|---|---|
| Make searchable (scans) | Each page without text is rendered at 300 dpi and read by the OS OCR (`ocr::recognize_lines`: Windows OCR word boxes, or tesseract's TSV on Linux). Every word is stamped as invisible text (`3 Tr`) stretched to its box, with a trailing space up to the next word so copied text keeps its spaces. The page looks exactly as before. krilla has no invisible mode, so the words are drawn as stroked text and the overlay's `1 Tr` is switched to `3 Tr` before grafting |
| Redact (words, phrases, emails, phone numbers) | **True redaction.** Matches are found in the text (a space in a phrase matches any space or line break) and, on pages with pictures or no text, by OCR. Each page with a match is rendered at 200 dpi, the boxes are painted black into the pixels, and the page's content, resources, links and comments are replaced by that one picture, so nothing under a box survives in any form. Optionally the redacted picture is read back with OCR so the page stays searchable without the removed words. Metadata entries, XMP, bookmark titles, comment text and form values that match are scrubbed. Text the extractor can't place (rotated, clipped, never drawn) but that lopdf's text extraction still finds makes its page flattened too, and the result names that page so the user checks it. Finally the written file is read again; if any match is still readable, nothing is saved. Pages without matches keep their vector content |
| Compare two PDFs (mode: automatic, words or appearance) | **Words:** words with their positions from both files are diffed as one sequence each (Myers, linear space), so text that moved to the next page isn't a change. The diff is checked against a brute-force LCS on random inputs. **Appearance** (`pdfedit::visual`): pages are rendered at about 100 dpi, the old page is scaled to the new page's size, and a shift of up to 8 px is found on half-size copies, refined at full size and undone. A pixel counts as changed only when nothing within one pixel on the other side has a similar colour, so anti-aliasing and resampling don't count. Isolated specks (scanner dust) are dropped, and the rest is grouped into clusters (about 1 % of the page width apart) with an orange box around each. **Automatic** compares words where both pages have text and appearance where either side has none (scans, drawings). Words on pages compared by appearance stay out of the word diff, so a page scanned on one side doesn't count all its words as added. **The report** opens with a summary page (changed pages, word counts, changed areas), then sets each old page beside the new one: removed in red on the left, added in green on the right. The older file (by date) is the original. No changes → no report |
| PDF → Excel (a conversion, pdf → xlsx) | The layout analysis's tables ([13](13-pdf-reconstruction.md)) become cells: merged cells merged, header rows bold, numbers as numbers with a format that prints them the same (`1,234.50`, `(12)`, `-3.5 %`, `$ 1,200`, `1.234,56`). Leading zeros, long ids, dates and phone numbers stay text. One sheet per page with the other text in reading order; when tables span pages, an "All tables" sheet first joins them and drops repeated header rows. Scans are made searchable first |

**Forms**
| Tool | How |
|---|---|
| Form fields → spreadsheet | The fields (full names, `parent.child`) as a CSV header with the current values as a row: add a row per copy |
| Fill form (PDF + CSV or spreadsheet) | One filled PDF per row, named from a column; optionally flattened. Columns match field names case-insensitively, by full name or last part. Text fields get a new appearance in Helvetica when the value fits its character set (alignment, auto size, multi-line); otherwise the old appearance is removed and viewers are asked to draw it. Check boxes take yes/no, radio groups the option's name. Read-only fields are skipped and XFA data is dropped so every viewer shows the filled values |
| Flatten | Each field's (and with *comments*, each comment's) current appearance is drawn into the page where it was, with the PDF matrix rule (BBox × Matrix fitted to Rect); hidden ones are dropped, links kept. With no fields left, the form is removed |

**Archiving and accessibility** (for documents our layout engines write)
| Tool | How |
|---|---|
| PDF/A (Word, ODT, RTF, PowerPoint, Excel → archival PDF) | Written through krilla with its PDF/A-2 validator, so the file is checked against the standard as it's made: every font embedded with a Unicode mapping for every glyph, colours device-independent (sRGB output intent), XMP metadata with the identification and a creation date, no smoothing hints on pictures. With **Tagged for screen readers** (the default) it's PDF/A-2a; a document that doesn't say its language (Word's default `w:lang`, or `dc:language`) can't be 2a and comes out PDF/A-2b, still tagged, and the result says why. A character no installed font has fails with that character named, because PDF/A forbids drawing it as a blank box. Also `swirvo convert --to pdf --pdfa [--tagged]` |
| PDF/A-3, original attached | The *Standard* option of the PDF/A tool, or `--pdfa3` on the CLI: PDF/A-3a or 3b with the Word, ODT, RTF, PowerPoint or Excel file embedded in the PDF as its source (relationship *Source*), so the archive copy carries the document it came from. Nothing else is attached |
| PDF/UA (accessible) | The *Standard* option *PDF/UA*, or `--pdfua`: PDF/UA-1 checked by krilla while writing. Always tagged, the title shown in the viewer's title bar, an outline (one entry for the whole document when it has no headings), alternative text on every picture and chart ("Picture" when the document gives none), links described. Not combined with PDF/A in one file |
| Tagged PDF | Layout records the structure as it places content (`docxlayout::tags`): headings with their levels and text, paragraphs, lists (nested by indent, numbering from the label: decimal, roman, letters, bullets), tables with header cells (repeated header rows on later pages are marked as decoration), pictures and charts as figures with their alternative text (Word's description or title, ODT's `svg:desc`; charts use their title), footnotes and endnotes as notes next to where they're referenced, links tied to the paragraph they're in. Headers, footers, page numbers, rules, borders and shading are marked as artifacts. The structure tree follows reading order (a node takes its place when its first content is drawn). Slides: the title placeholder is the slide's heading, pictures and charts are figures, backgrounds are artifacts. Sheets: cell text as paragraphs, charts as figures, the printed header and footer as artifacts. Also `swirvo convert --to pdf --tagged` |
| Existing PDFs | Not converted to PDF/A. krilla can only draw an existing page into a new file as an embedded page, which a validator can't vouch for, and checking an arbitrary file in place against every PDF/A rule (font programs, colour spaces, transparency groups, annotation appearances) without a full validator would mean claiming compliance we can't prove. Convert the original document instead |

**Signatures** (`pdfedit::sign` for the PDF side, `os::certsign` for the cryptography)
| Tool | How |
|---|---|
| Sign with a certificate | The user's own certificates with a private key from the Windows certificate store (Personal), chosen by part of the name or the thumbprint, or the only current one. The PDF gets an incremental update: a signature field (a visible box with the signer, the date, reason and location, bottom right of the first or last page or wherever it was dragged on the Board's preview (`place`), or invisible), the form's `SigFlags`, and a signature dictionary (`adbe.pkcs7.detached`) whose `/ByteRange` covers the whole file except a 16 KB `/Contents` placeholder. Everything before the update stays byte for byte, so earlier signatures stay valid. The ranges are signed by Windows (`CryptSignMessage`: SHA-256, detached CMS, the certificate chain and a signed signing time) and the result is written into the placeholder as hex. Keys never leave the store: a smart card asks for its PIN in Windows' own dialog. The update uses the same kind of cross-reference section as the file (table or stream). Windows only |
| Check signatures | Every signature field: its byte range must start at 0 and skip exactly `/Contents`; whether it covers the whole file (anything appended later shows as "changed after this signature"); the CMS verified against the signed bytes (`CryptVerifyDetachedMessageSignature`); the signer from the certificate; the signing time from the signed attribute; trust from Windows' chain engine at the signing time, with the reason in words when it isn't trusted (self-signed, unknown issuer, revoked, expired). One line per signature, on the clipboard. Other systems list the signatures and say the check needs Windows |

**Word documents** (`docxtools`, edited in place through a lossless XML tree, `xmldom`)
| Tool | How |
|---|---|
| Compress | Pictures shrunk to what their displayed size needs (Screen 150 dpi · Print 220 · Email 100); photos become JPEG, screenshots stay lossless; repacked at maximum compression |
| Clean | Author, company and last editor, comments, editing-session ids; optionally accepts all tracked changes (insertions kept, deletions removed) |
| Find and replace | Body, headers, footers and notes, also where Word split a word across runs; match case, whole words |
| Pictures, word count | Pictures saved to a folder; words, characters, paragraphs and pages (pages from our layout engine) to the clipboard |
| Mail merge (letter + CSV or spreadsheet) | `{{Name}}`, `«Name»` and MERGEFIELD fields filled per row; one DOCX per row, optionally a PDF each, named from a column |

## 3. Options

Tools describe their options as data (`args::Field`: text, secret, number with a range, choice, flag, file).
- **The Board** draws a form from it (Tools tab → a tool with options → its form → Run). The file field uses the system picker; secrets are drawn as dots. Tools that change how pages look also show a live preview of the page beside the form, and crop boxes and number positions are set on it directly (docs/12 §3.5).
- **The CLI** takes `--set key=value`; `swirvo tools FILE` lists every tool's options.
- **The wheel** runs a tool straight away when its defaults make sense (Compress, Numbers, Trim…). Tools that need something from the user (Protect, Unlock, Watermark, Header, Properties, Crop, Split by, Split ranges, Bates, Redact, Find and replace, Mail merge) open the Board on their form. When a file has more tools than fit, the last wedge is **More…** and opens the Board's tool list.

Unknown option names are errors, so a typo on the command line never passes silently.

### 3.1 Error messages
One standard for every form, the CLI and tool results:
- **Say what's wrong and what to type instead,** in plain words, starting with the field's name when there is one: "Pages: page 61 doesn't exist (there are 50)", "Left (mm): between 0 and 1000", "PDF 2: “x” isn't a page number". No codes, no jargon, no blame.
- **Check as the user types.** A field with a problem gets a red border and the message under it, in place of its hint. Empty fields aren't flagged until they're needed.
- **Page numbers are checked against the file.** The Board counts the PDF's pages in the background and shows "50 pages" by the field; page selections (`FieldKind::Pages`) and split groups (`FieldKind::PageGroups`, one row per output PDF) are checked against that count. On the CLI the same check happens when the tool runs.
- **Run waits until it can work.** While anything is missing or wrong, Run is disabled and the first problem is shown above it ("Pages for each PDF is needed"), in normal text, not red, because nothing has failed yet.
- **Failures are red, with the file.** When a tool fails on a file, the message names the file and says why, and other files in the batch carry on. Nothing is left half-written: outputs are only renamed into place once complete.

## 4. Protection and permissions

Our rules, for legal safety and so we can sell it:

1. **Protect** uses AES-256, the PDF 2.0 security handler, read by every current viewer. Without a permissions (owner) password a random one is used, so the restrictions can't be lifted later.
2. **Unlock needs the right password.** Removing printing, copying or editing restrictions needs the **owner** password. The open password only removes protection from a file that has no restrictions.
   - A file that opens without a password but restricts copying can't be unlocked without its owner password.
   - The owner password is checked by rebuilding the encryption dictionary that lopdf consumes on load and authenticating against it.
3. **Edits keep protection.** lopdf decrypts on load and doesn't re-encrypt on save, so a stamped or compressed copy of a protected file would come out unprotected. `pdfedit::save` re-applies the original encryption, with the same passwords and restrictions.
4. **Permissions are honoured** (`secure::permits`):

   | Tool needs | Tools |
   |---|---|
   | Editing | In-place tools (they keep the protection), including make searchable, redact, grayscale, fill and flatten |
   | Assembling pages | Rotate |
   | Copying content | Converting a PDF to Word, Excel or text, saving pictures and attachments, page images, compare, form fields → spreadsheet |
   | No restrictions at all | Tools whose output can't carry the protection: merge, split, N-up, booklet, extract, delete pages |

   A file that needs a password to open is refused by everything except Unlock.
5. **Signing never weakens a file.** A protected PDF isn't signed: the signature's update would have to be encrypted with the file's keys, which isn't supported yet, and removing the protection first needs the owner password (rule 2). A file certified to allow no changes (DocMDP, P = 1) is refused. Signing appends and never rewrites, so earlier signatures and their byte ranges stay intact. Checking signatures only reads.

## 5. Done and next

**Done (2026-11-09).**
- Every tool in §2, with tests on rotated pages, protected files and real LibreOffice PDFs.
- The options system and the Board form.
- Wheel overflow into the Board.
- Permission checks on the existing PDF tools and on PDF conversions.

**Done (2026-11-10).**
- Word tools, make searchable, true redaction, grayscale, compare, PDF → Excel, forms (fields → spreadsheet, fill per row, flatten), each with tests: redaction checks the words are gone from the written file, the searchable layer is checked to be invisible (pixel-identical page) and findable, flatten checks the value is real page text.

- Organise by hand in the page picker (reorder, turn, duplicate, delete, blank pages, insert PDFs, crop, black out, sign) and comments by hand (highlight, note, text, ink, box), with tests: the comments are checked to render where they were put on plain and rotated pages, and the organised file's order, page sizes, signature and blacked-out area are checked by rendering.

**Done (2026-11-10, later).**
- PDF/A-2a/2b and tagged PDF for Word, ODT, RTF, PowerPoint and Excel → PDF, as a tool and as conversion options (`--pdfa`, `--tagged`). Tests validate with krilla's PDF/A-2 validator while writing and read the structure tree back (roles in reading order, figure alt text, artifacts, the PDF/A identification).
- Certificate signatures and checking them (Windows). Linux tests cover the update's structure, byte ranges, placeholders, both cross-reference kinds, a second signature over a first, and appended changes; the Windows test signs with a throwaway in-memory certificate (an ephemeral key that's never stored), verifies, and catches a changed byte.

**Next, in order.**
1. Signatures: timestamps from a time-stamping server (an explicit, visible network action), PAdES (`ETSI.CAdES.detached`), and signing protected files.
2. PDF/A-3 with the source document attached, and PDF/UA (krilla's UA-1 validator) once figures carry their bounding boxes.
