---
title: "Command line"
description: "The swirvo command: convert, run tools, detect types and plan routes from any terminal or script, with JSON output."
section: Power users
order: 20
---

The `swirvo` command uses the same engines as the app. It's on your PATH after installing on Windows (Store or the installer), and installs with one line on [Linux](/docs/linux). From an unzipped, uninstalled copy, it's `swirvo-cli.exe` in that folder.

```bash
swirvo --help
swirvo --version
```

## Convert

```bash
swirvo convert report.pdf --to docx
swirvo convert *.heic --to jpg --preset smallest
swirvo convert slides.pptx --to pdf --pdfa --tagged --out "D:\Archive"
```

| Option | What it does |
|---|---|
| `-t, --to <FORMAT>` | The target: a format key or an extension (`jpg`, `webp`, `pdf`, `yaml`, `tar.zst`…). |
| `-p, --preset <PRESET>` | `best`, `balanced` (the default) or `smallest`. |
| `-o, --out <FOLDER>` | Write results into this folder instead of next to each file. |
| `-j, --jobs <N>` | How many files at once (default: one per CPU thread). |
| `--json` | JSON Lines events on stdout, for scripts. |
| `--pdfa` | PDFs written as PDF/A-2 (2a with `--tagged`, otherwise 2b). |
| `--pdfa3` | PDF/A-3 with the original document attached inside. |
| `--tagged` | Tagged, accessible PDFs: headings, lists, tables, alt text. |
| `--pdfua` | PDF/UA-1 (universal accessibility). Not combined with `--pdfa`. |

Output looks like this:

```
✓ logo.png → logo.webp  27 KB (+63 %) · 8 ms · lossless
✓ hero.png → hero.webp  27 KB (+64 %) · 10 ms · lossless
2 converted, 0 failed, 0 skipped in 0.0 s · 33 KB → 53 KB
```

## Tools

List the tools that fit some files, with every option:

```bash
swirvo tools contract.pdf
```

Then run one by its id:

```bash
swirvo tool pdf-merge a.pdf b.pdf c.pdf
swirvo tool pdf-watermark draft.pdf --set text=DRAFT
swirvo tool pdf-protect report.pdf --set password=…
swirvo tool docx-replace letter.docx --set find=Acme --set with=Initech
swirvo tool video-compress talk.mov --set size=25
```

| Option | What it does |
|---|---|
| `--set KEY=VALUE` | A tool option; repeat for more. `swirvo tools FILE` lists each tool's keys. |
| `--pages <PAGES>` | PDF page tools: which pages, such as `1-3, 7, 10-`, `odd` or `even`. |
| `--op <OP>` | With `--pages`: `extract`, `split`, `rotate`, `delete`, `images-jpg` or `images-png`. |
| `--dpi <DPI>` | With an images operation: resolution (default 150). |
| `--json` | JSON output. |

Every tool id is on the [tools page](/tools). For `pdf-merge`, the order you give the files is the page order.

## Read and inspect

```bash
swirvo tool copy-text invoice.pdf --json     # the text, OCR for photos and scans (Windows)
swirvo detect mystery.bin                    # the real type, from the content
swirvo formats                               # every format, by category
swirvo formats --from pdf                    # what one format can become
swirvo formats --supported                   # only what this PC's engines can do
swirvo plan photo.heic --to webp             # the route Swirvo would take
swirvo extract archive.7z                    # into a new folder next to it
```

## Other commands

| Command | What it does |
|---|---|
| `swirvo mcp` | Runs Swirvo as a local MCP server on stdin/stdout for AI assistants. See [AI assistants](/docs/ai-assistants). |
| `swirvo wheel FILE…` | Shows the Convert wheel Swirvo would open for these files, and can render it. |

## In scripts

- Quote paths and prefer absolute ones.
- Use `--json` for machine-readable JSON Lines; each finished file reports its output path.
- Results are new files, and originals are never changed, so a script can always safely retry.
