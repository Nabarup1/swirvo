---
title: "PDF tools"
description: "Merge, split, compress, stamp, protect, redact, sign, compare, fill forms and make archival PDFs, all on your PC."
section: Using Swirvo
order: 13
---

Most of Swirvo's tools are for PDF. Tap <kbd>Ctrl</kbd> while dragging a PDF for the common ones, or open the [Board](/docs/board) for all of them with their options and a live preview. Every tool writes a new file; the original is never changed.

## Pages

| Tool | What it does |
|---|---|
| **Merge** | Several PDFs become one. From the wheel, in name order; from the command line, in the order you give. |
| **Split** | One PDF per page, in a new folder. |
| **Split ranges** | Several PDFs from groups you type or click: `1-15, 16; 25-40, 42` (a `;` starts the next PDF). Groups may share pages. |
| **Split by** | A new PDF every N pages. |
| **Chapters** | A PDF per top-level bookmark. |
| **Split size** | Parts that each stay under a size, for email. |
| **Rotate** | 90° clockwise, losslessly. |
| **Pages JPG / PNG** | One image per page, 150 or 300 dpi. |
| **Pages…** | The [page picker](/docs/page-picker): choose pages, then extract, split, rotate, delete or reorder. |

For a PDF with more than one page, Split, Rotate and the page-image tools open the [page picker](/docs/page-picker) first, with every page selected, so the plain result is one <kbd>Enter</kbd> away.

## Edit

| Tool | What it does |
|---|---|
| **Compress** | Screen (150 dpi), Print (220), Email (100), Smallest (72), or "fit under N MB". Pictures are reduced only to the size they're displayed at, photos become JPEG, diagrams and screenshots stay lossless, and duplicates are merged. A change is kept only when it saves at least 10 %. |
| **Numbers** | Page numbers as real text on the page. |
| **Header** | A header and footer on every page. |
| **Bates** | Bates numbering for legal and discovery work. |
| **Watermark** | Diagonal text or a picture, light to strong, in any colour and angle. |
| **Trim** | Cuts the white margins off every page. |
| **Crop** | By margins, for every page or per page. |
| **Resize** | To A4, Letter, A3, A5 or Legal; content and links scale together. |
| **N-up** | 2, 4, 6, 8 or 9 pages per sheet. |
| **Booklet** | Pages in saddle-stitch order: print double-sided, fold, staple. |
| **Grayscale** | Colours to gray, while text stays text and drawings stay sharp. |

Stamps (numbers, headers, Bates, watermarks) are real text and keep links and form fields working. Crop, trim, resize and the stamps edit the PDF **in place**, so links, form fields, bookmarks and protection survive. N-up, booklets and splitting draw the pages into a new document.

All of these show a **live preview** in the Board: the page as the tool will leave it. Drag the watermark where you want it, drag the crop box, click where the page numbers go.

## Protect

| Tool | What it does |
|---|---|
| **Protect** | An AES-256 password, and permissions for what others may do with the file. |
| **Unlock** | Removes the password from a PDF you can open. A file restricted by an owner password needs that password. |
| **Redact** | Removes words, phrases, email addresses or phone numbers for good. See below. |
| **Clean** | Removes hidden data: document info, scripts, attachments, automatic actions and editor leftovers. Pages look the same. |
| **Digital signature** | Signs with your own certificate from the Windows certificate store, as a visible box or invisibly. Earlier signatures stay valid. |
| **Signatures** | Checks every signature: intact or changed afterwards, who signed and when, and whether Windows trusts it (or why not). |

### How redaction works

Covering text with a black box isn't redaction: the words are still in the file. Swirvo finds the matches (and, on scanned pages, reads them with OCR), redraws each affected page as a picture with the boxes painted in, and replaces the page with that picture, so nothing under a box survives in any form. Matching titles, bookmarks, comments and form values are scrubbed too. Finally the saved file is read again: if any match is still readable, nothing is saved.

> **Note:** Redaction removes the text it can **find**. Handwriting, or text drawn as shapes, can be missed. Always check every page before you share a redacted file.

### Signing

Signing uses a certificate with a private key from your personal Windows certificate store; a smart card asks for its PIN in Windows' own dialog, and keys never leave the store. Whether a signature is legally valid depends on your certificate and the law where it's used. A picture of your signature isn't a digital signature. Signing and checking are Windows-only.

## Content

| Tool | What it does |
|---|---|
| **Searchable** | Scans get an invisible text layer from OCR: find, select and copy work, and the page looks exactly the same. |
| **Properties** | Title, author, subject and keywords. |
| **Repair** | Rebuilds a damaged PDF that other apps refuse to open. |
| **Bookmarks** | Adds bookmarks that jump to each heading. |
| **Pictures** | Saves every picture inside the PDF. |
| **Attachments / Attach** | Saves the files inside a PDF, or puts files into one. |
| **Form fields** | Every field and its value, out to a spreadsheet. |
| **Fill form** | Fills a form from a spreadsheet: one filled PDF per row. |
| **Flatten** | Draws fields and comments into the page so they can't be changed. |
| **Compare** | Two versions side by side: removed in red, added in green, with a summary page. Words are compared where there's text, appearance where there isn't (scans, drawings). |

## PDF/A, archives and accessibility

From a Word, ODT, RTF, PowerPoint or Excel file, the **PDF/A** tool makes an archival PDF:

- **PDF/A-2a**, tagged for screen readers, by default (PDF/A-2b when the document doesn't say its language).
- **PDF/A-3** with the original document attached inside it.
- **PDF/UA** for universal accessibility: tagged, titled, with an outline and descriptions on every picture.

The same options exist on the command line: `--pdfa`, `--pdfa3`, `--pdfua` and `--tagged`. Files are written to the standard's rules and checked while they're made, but this isn't a certification. Existing PDFs aren't converted to PDF/A; convert the original document instead.

## From the command line

Every tool has an id:

```bash
swirvo tools "contract.pdf"                       # what fits, with each option
swirvo tool pdf-compress "scan.pdf"
swirvo tool pdf-split-ranges "book.pdf" --set "ranges=1-3, 5; 6-9"
swirvo tool pdf-watermark "draft.pdf" --set text=DRAFT
swirvo tool pdf-pages "report.pdf" --pages "1-3, 7" --op extract
```

The full list is on the [tools page](/tools).
