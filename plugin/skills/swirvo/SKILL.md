---
name: swirvo
description: Convert, merge, split, compress, watermark, redact, OCR or read documents and images on the user's own Windows PC with the Swirvo command line. Use when the user asks to change a file's format (PDF to Word, Word/Excel/PowerPoint to PDF, HEIC to JPG, CSV to Excel...), edit PDFs, or read the text of a PDF, Office file or scanned image.
---

# Working with files through Swirvo

Swirvo is a free file converter and PDF toolkit that runs on the user's computer. Run its command line, `swirvo`, in the terminal. Nothing is uploaded; results are new files next to the originals, and originals are never changed, so retrying is safe.

First check it's installed: `swirvo --version`. If the command isn't found, tell the user to install Swirvo from the Microsoft Store or from https://github.com/Nabarup1/swirvo/releases (run `Swirvo.exe` once and accept the install), then open a new terminal.

Always quote paths, and prefer absolute paths. Add `--json` to get machine-readable output (JSON Lines).

## Convert

```
swirvo convert "C:\path\report.pdf" --to docx --json
swirvo convert "C:\path\a.heic" "C:\path\b.heic" --to jpg --json
swirvo convert "C:\path\report.docx" --to pdf --pdfa --json
```

- `--to` takes a format key or extension: `pdf`, `docx`, `jpg`, `png`, `webp`, `xlsx`, `csv`, `md`, `html`, `txt`, `epub`...
- `--out "C:\folder"` writes the results into another folder. `--preset best|balanced|smallest` trades quality for size.
- PDF output options: `--tagged` (accessible), `--pdfa`, `--pdfa3` (attaches the original), `--pdfua`.
- Not sure a conversion exists? `swirvo formats --from heic` lists every target.
- Each `done` line has the `output` path; tell the user where the new files are.

## Tools (PDF, Word, images, archives)

1. List the tools for the files, with their option keys: `swirvo tools "C:\path\contract.pdf"`
2. Run one: `swirvo tool <tool-id> <files...> --set key=value --json`

Common ones:

```
swirvo tool pdf-merge "a.pdf" "b.pdf" "c.pdf" --json
swirvo tool pdf-split-ranges "book.pdf" --set "ranges=1-3, 5; 6-9" --json
swirvo tool pdf-compress "scan.pdf" --json
swirvo tool pdf-watermark "draft.pdf" --set text=DRAFT --json
swirvo tool pdf-numbers "report.pdf" --json
swirvo tool pdf-protect "report.pdf" --set password=... --json
swirvo tool pdf-searchable "scan.pdf" --json
swirvo tool pdf-redact "contract.pdf" --set "find=jane@example.com, 555-0100" --json
swirvo tool docx-replace "letter.docx" --set find=Acme --set with=Initech --json
swirvo tool resize-half "photo.jpg" --json
```

- The order of files is the page order for `pdf-merge`.
- Pages of one PDF: `swirvo tool pdf-pages "file.pdf" --pages "1-3, 7" --op extract` (`--op` is `extract`, `split`, `rotate`, `delete`, `images-png` or `images-jpg`).
- `pdf-redact` removes only the text it finds. Ask the user to check the result before sharing it.
- Don't repeat passwords back in your reply.

## Read text

```
swirvo tool copy-text "C:\path\invoice.pdf" --json
```

The text is in `result.clipboard`. It works for PDFs, Word, ODT, RTF, slides, sheets, Markdown, HTML and images; scanned pages and photos are read with Windows OCR.

## Check a file's real type

```
swirvo detect "C:\path\file.bin" --json
```
