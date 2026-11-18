---
name: swirvo
description: Convert, merge, split, compress, watermark, redact, OCR or read documents and images on the user's own computer with Swirvo's MCP tools. Use when the user asks to change a file's format (PDF to Word, Word/Excel/PowerPoint to PDF, HEIC to JPG, CSV to Excel...), edit PDFs, or read the text of a PDF, Office file or scanned image.
---

# Working with files through Swirvo

Swirvo runs on the user's computer. Its tools read and write local files; nothing is uploaded.

## Choose the tool

- **Change format:** `convert_files` with absolute `paths` and `to` (a format key such as `pdf`, `docx`, `jpg`, `xlsx`, `md`). Use `list_formats` with `from` when you aren't sure a conversion exists.
- **Edit or process:** call `list_tools` with the files first. It returns each applicable tool's `id` and option keys. Then call `run_tool` with that `tool` id and `options`.
  - Merge: `pdf-merge` (the order of `paths` is the page order).
  - Split into chosen groups: `pdf-split-ranges` with `{"ranges": "1-3, 5; 6-9"}`.
  - Watermark: `pdf-watermark` with `{"text": "DRAFT"}`.
  - Searchable scans: `pdf-searchable`. Redaction: `pdf-redact` with `{"find": "..."}`.
  - Single pages of one PDF: a page tool with `pages`, e.g. `"1-3, 7"`, and optionally `page_action`.
- **Read a document:** `extract_text`. It reads PDFs, Word, slides, sheets, Markdown, HTML and images; scans and photos use Windows OCR.
- **Check a file's real type:** `detect_format`.

## Rules

- Always pass **absolute paths**. Ask the user for the folder if you don't know it.
- Results are **new files** next to the originals (or in `out_dir`). Originals are never changed, so retrying is safe. Tell the user the output paths from the result.
- `pdf-redact` removes only the text it finds. Ask the user to check the result before sharing it.
- Passwords for `pdf-protect` / `pdf-unlock` go in `options`. Don't repeat them back in your reply.

## If the tools are missing

The tools come from the Swirvo app, which must be installed on the user's PC so that the `swirvo` command exists: from the Microsoft Store, or from https://github.com/Nabarup1/swirvo/releases (run `Swirvo.exe` once and accept the install). Then restart the session.
