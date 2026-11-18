# 13 · PDF → DOCX: rebuilding the page, not just the text

Until now, PDF → DOCX read the PDF's text with `pdf-extract`, guessed paragraphs from line breaks and poured the result into a default A4 template. The words survived, but the look did not: fonts, sizes, colors, columns, tables, images, boxes and margins were all lost. This plan replaces that path with a layout reconstruction engine written in-house. It refines [04](04-algorithms.md) §15; where the two disagree, this document wins.

## 1. Goals

The DOCX should **look like the PDF** when opened in Word or LibreOffice, and **stay editable**: text flows in real paragraphs, tables are real tables, and lists continue when you press Enter.

The following survive the conversion:

| PDF | DOCX |
|---|---|
| Page size, per-page margins | One section per page (`w:pgSz`, `w:pgMar`) |
| Font family, size, bold, italic, color | Run properties. The family is mapped from the PostScript name and substituted when it isn't installed (§5) |
| Underline, strike-through, highlight | Run properties, detected from the rules and boxes drawn under or behind the text |
| Superscript, subscript | `w:vertAlign` |
| Paragraph position, indents, alignment, line pitch, spacing | `w:ind`, `w:jc`, exact line spacing, space before |
| Multi-column text | Continuous sections with `w:cols` (equal or unequal widths) |
| Gapped lines (forms, borderless tables, "Label ..... Value") | Tab stops at the original x positions (left or right tabs) |
| Bulleted lists | Real Word lists (`numbering.xml`) with the original bullet and indents |
| Ruled tables | `w:tbl` with column widths, row heights, merged cells, per-edge borders and cell shading |
| Raster images | PNG or JPEG, inline in the flow, or anchored behind or beside text at their exact page position |
| Boxes and lines (shaded panels, sidebars, rules) | DrawingML shapes anchored behind text at exact positions |
| Charts, logos and other vector art | Rendered at 200 dpi and placed as an image. Labels are baked in; backgrounds are rendered without text |
| Repeated headers and footers | Header/footer parts; page numbers become `PAGE` fields |
| Links | `w:hyperlink` |
| Invisible OCR text over a scan | Visible, editable text; the page-sized scan underneath is dropped |

**Non-goals for now:** rotated text other than 0°, editable charts, form fields, footnote linking, and embedding fonts that aren't installed.

## 2. Dependencies

There are **no new crates**. We use:

- `hayro` / `hayro-interpret`, already used for rendering, for glyphs, images and paths through a custom `Device`;
- `lopdf`, already used for page tools, to make a text-free copy of a page for rendering backgrounds (§4.4);
- `image` for PNG encoding.

The DOCX writer is our own code. `pdf-extract` becomes unused and is removed: every PDF → text path now goes through the same extraction, so PDF → Markdown, HTML, TXT, ODT and EPUB also gain headings, lists and tables.

## 3. Pipeline

```
PDF bytes
  │ extract.rs   hayro Device → per page: glyphs (unicode, font, size, color, origin, advance),
  │              images (pixels + placement + clip), paths (fills/strokes), link annotations
  ▼
  │ fonts.rs     font identity per glyph run: embedded program metadata, or the page's font
  │              dicts matched by width fingerprint → family · bold · italic · serif · mono
  ▼
  │ graphics.rs  paths → rules (h/v segments) · boxes (filled rects) · art (everything else)
  │ tables.rs    rules (+ box edges) → lattice tables: grid, merges, borders, shading
  │ figures      art clusters → figure (labels baked in) or background (text-free render)
  │ decorate     rules under text → underline/strike · boxes behind runs → highlight
  ▼
  │ text.rs      glyphs → words → segments (word runs without big gaps) → lines
  │ pages        headers/footers: top/bottom segments repeated across pages
  │ order.rs     XY-cut over atoms (segments, tables, inline images, figures) → bands of columns
  │ paras.rs     lines → paragraphs: indents, alignment, pitch, spacing, lists, tab stops
  ▼
Layout (portable model, unit-tested without any PDF)
  │ docx.rs      sections · paragraphs · runs · tables · drawings · headers/footers · fonts
  │ to_doc.rs    Layout → Doc (headings by size, lists, tables) for md/html/txt/odt/epub
  ▼
DOCX
```

Coordinates are PDF points with y pointing down from the top of the crop box (`page.initial_transform(true)`). Each module is a pure function of the previous stage's output, so it can be tested in isolation.

## 4. Analysis details

### 4.1 Text

- **Glyph geometry.** The device transform is `props.transform × glyph.transform()`, in 1000 units per em. The origin is the baseline start. The em size is the length of the transformed vertical unit, and the advance is `advance_width` along the transformed x axis. Glyphs whose baseline isn't horizontal (beyond ±2°) are dropped from the flow; this is a non-goal for now.
- **Duplicates.** The same character drawn at the same spot within 0.1 em (fake bold, or a fill followed by a stroke) is kept once. A stroked or offset duplicate marks the character bold.
- **Words, segments and lines.**
  - Glyphs on one baseline (within 0.2 em) join a word while the gap is under 0.15 em.
  - Words join a segment while the gap is under 1.2 em and the size is compatible.
  - Spaces are inserted from geometry rather than trusted from the PDF.
  - Segments on one baseline in one region make a line. A gap between segments becomes a tab.
- **Super- and subscript.** A smaller glyph whose baseline is shifted by more than 0.15 em from its line's baseline.
- **Hyphenation.** A line ending in `-` followed by a lowercase start is joined when converting to `Doc`. DOCX keeps the hyphen, because it keeps the line structure through flow.

### 4.2 Reading order: XY-cut with vetoes

Recursive XY-cut runs over **atoms** (text segments, tables, inline images, figures):

- **Vertical cuts (columns).** A vertical cut needs a whitespace river spanning the region's full height, at least 9 pt wide. Each side must hold at least 15% of the width and 50 pt, and at least two lines. It is **vetoed when the split looks like a table**: at least 70% of the smaller side's lines share a baseline with the other side, and its lines are short (median below 55% of its width). Labels and values then stay on the same line, as tab stops, instead of becoming columns.
- **Horizontal cuts.** These need a full-width gap larger than `max(4 pt, 0.6 × median font size)`. Inter-line gaps therefore never split a two-column page into interleaved strips.
- **Order.** Vertical cuts are tried first, then horizontal. A region with no accepted cut is a leaf.

The tree is flattened into **bands** (stacked top to bottom). Each band holds one or more **columns**, and each column holds a list of leaf blocks. Columns nested inside columns are flattened into tab-stop lines; Word can't nest columns.

### 4.3 Paragraphs

Within a leaf, consecutive lines join one paragraph when all of the following hold:

- the line pitch is within 25% of the paragraph's pitch;
- the dominant font size and weight match;
- the new line doesn't start with a list marker;
- the previous line wasn't visibly short. A line that ends before 80% of the leaf width, while the next line starts at the left edge, closes the paragraph.

Within each paragraph:

- **Alignment** comes from the line edges.
  - Justified: the right edges align (within 2 pt) on every line but the last.
  - Centered: the centers align and the edges don't.
  - Right: the right edges align and the left edges don't.
  - Otherwise left.
- **Indent and first-line indent** come from the first line versus the rest.
- **Line pitch** is the median baseline distance. A single line uses 1.2 × size.
- **Space before** is computed from the previous block's bottom baseline.

**List items** start with a bullet glyph (`•◦▪‣●○■–-*` and Private Use Area symbol bullets) followed by a gap, or with `1.`/`1)`/`a)`/`(i)`. Bullets become Word lists with the same glyph and hanging indent. Numbers stay literal text with a hanging indent, so `5.` stays 5.

### 4.4 Graphics

- **Rules.** A rule is a horizontal or vertical stroked line, or a filled rectangle thinner than 2 pt. Stroked rectangles add their four edges.
- **Boxes.** A box is a filled axis-aligned rectangle. White boxes covering more than 90% of the page are dropped as page backgrounds.
- **Tables (lattice).**
  - Collinear rules are merged into segments, and connected components are formed from those that intersect (2 pt tolerance). The edges of adjacent filled boxes also count as segments, so shaded tables with no lines still work.
  - Each component with at least 2 × 2 cells and text in at least two cells becomes a table.
  - Grid lines are the clustered x and y positions; missing outer edges are added from the segment extents.
  - Missing interior boundaries make merged cells (`gridSpan`, `vMerge`). Each cell edge gets the border of the segment that covers it, and a box covering a cell becomes its shading.
  - Cell text runs through §4.3 with the cell as its region.
- **Underline and strike.** A rule within 0.3 em below a run's baseline that spans at least 80% of the run's width is an underline; one at 0.2–0.45 em above the baseline is a strike. A box hugging a run is a highlight (`w:shd`). Consumed rules and boxes aren't drawn again.
- **Other boxes and rules** become anchored shapes behind the text.
- **Art.**
  - Other paths (curves, gradients, polygons) and stencil masks are clustered with a 6 pt proximity.
  - A cluster with sparse text (under 0.004 characters per pt²) and covering under 70% of the page is a **figure**. It is rendered with its text at 200 dpi, and that text leaves the flow.
  - Otherwise it's a **background**. It is rendered from a copy of the page whose text-showing operators (`Tj TJ ' "`) were removed with lopdf, and anchored behind the text, which stays editable.
- **Images.**
  - Decoded through hayro, which converts color spaces and applies masks, and cropped to the clip.
  - Re-encoded as PNG, or as JPEG at quality 90 when the image is large and opaque, and de-duplicated.
  - An image covered by text (over 30% of its area overlapping text) is anchored behind; an image of at least 90% of the page under invisible text is the OCR scan and is dropped; any other image is inline.

### 4.5 Fonts

- **Name.** Use the embedded program's PostScript name and weight. When there is none (Type 1, standard 14, Type 3), match the run's (code, width) pairs against the `Widths` of the page's font dictionaries, including those in forms, and read the `BaseFont` and descriptor flags.
- **Family.** Drop the subset tag (`ABCDEF+`). Split the style off at `-` or `,`, and remove `MT`/`PS`/`PSMT`. Split CamelCase, keeping acronyms such as `UI`. Then apply a small alias table: Helvetica → Arial, Times → Times New Roman, Courier → Courier New, Computer Modern → Cambria (serif) or Consolas (mono).
- **Style.** Bold when the weight is 600 or more, the name contains Bold/Black/Heavy/Semibold/Demi, or the text is fake-bolded. Italic when the font is flagged italic, the name contains Italic/Oblique or an `It` suffix, or the glyph transform is sheared.
- **Installed?** On Windows, a family that isn't installed (checked against the system and per-user font folders) is substituted: Arial for sans, Times New Roman for serif, Courier New for mono. Widths then stay close. `fontTable.xml` records the family class and pitch either way.

### 4.6 Headers and footers

- **Detection.** Look at segments in the top and bottom 12% of each page. A segment is a header or footer when its text, with digits normalized, appears at the same position (within 3 pt) on at least 60% of the pages (minimum 2).
- **Output.** The header and footer parts are taken from the first page that has them. A number that equals the page number becomes a `PAGE` field, and "of N" becomes `NUMPAGES`.

## 5. DOCX mapping

- **Sections.** Each page is its own section (`nextPage`), sized to the PDF page.
  - The left, right and top margins come from the page's body content. The bottom margin is `min(actual, 36 pt)`, which leaves room for small metric differences without spilling a line onto a new page.
  - Multi-column bands add continuous sections with `w:cols`, separated by explicit column breaks.
- **Paragraphs.**
  - Exact line spacing at the measured pitch, so vertical positions don't drift with font substitution.
  - Space before = gap − pitch × 0.8 (Word puts about 80% of an exact line above the baseline).
  - The right indent leaves 1.5% slack so a slightly wider substitute font doesn't wrap early.
- **Styles.** `Normal` is the dominant body font and size. Headings (sizes in the top clusters, short, not ending in a period) get `Heading 1–3` for the navigation pane, with direct formatting so they still look identical.
- **Drawings.** Anchored images and shapes use `wp:anchor` positioned relative to the page, `behindDoc` for backgrounds, and `wrapNone`. Shapes use `wps:wsp` inside `mc:AlternateContent`.

## 6. Verification

- **Unit tests** cover each stage on synthetic inputs, with no PDF needed: lines, XY-cut vetoes, paragraph splitting, alignment, lists, tables (with merges), header/footer detection and font naming.
- **Integration tests** build PDFs with krilla (text in known fonts and positions, rules, boxes, images), convert them, and assert on the DOCX XML: page size, margins, fonts, a table with N×M cells, an image, a list.
- **Visual round trip (manual, on Windows).** Realistic PDFs are made from HTML with Edge (`--print-to-pdf`) and from LibreOffice: an article in two columns, an invoice, a resume with a sidebar and a report with a chart. Each goes PDF → DOCX (ours) → PDF (LibreOffice) → PNG, and we compare it side by side with the original page.

## 7. Phases

1. **Now:** everything in §1, flow mode.
2. **Next:**
   - stream-mode (borderless) tables as real tables;
   - OCR word boxes (`Windows.Media.Ocr` lines and words) feeding §4.1, so scans become positioned, editable DOCX;
   - rotated text boxes;
   - letter-spacing (`w:spacing`) and horizontal scale (`w:w`);
   - PDF → XLSX from detected tables;
   - PDF → PPTX with one slide per page;
   - embedding missing fonts.
