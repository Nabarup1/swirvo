# 16 · DOCX → PDF with layout (`docxlayout`)

*Built 2026-11-09. Code: `crates/engines/src/docxlayout/`. It replaces LibreOffice as the default Word → PDF engine; LibreOffice stays optional.*

## 1. Why our own

Before this engine, Word → PDF had two choices, both poor:
- **LibreOffice:** exact layout, but 2–3 s per file, a 350 MB separate install, and its own interpretation of Word's layout.
- **The built-in typesetter:** about 40 ms, but it reflowed the text onto A4 and dropped pictures, fonts and page geometry.

Exact layout without another program is what people pay for, and it's the most common document conversion there is.

`docxlayout` reads the document the way Word does: styles, theme, numbering, sections. It lays it out with Word's rules and metrics and writes the PDF directly, in milliseconds. It adds **no new dependencies**: font discovery (fontdb), shaping (rustybuzz) and PDF writing (krilla) were already compiled in for other engines.

## 2. Pipeline

```
.docx ──read──▶ model::Document ──layout──▶ pages of Ops ──render──▶ PDF
          xml · styles · theme          text (atoms, line breaking)   krilla: glyphs, paths,
          numbering · props             fonts (metrics, shaping)      images, links, outline
```

| Module | Role |
|---|---|
| `xml` | Element tree with local names; `mc:AlternateContent` resolved to the DrawingML branch (VML fallback only for groups, canvases and chartex). |
| `props` | Run, paragraph, table, cell and row properties as written (all optional), with parsers, units (twips, half-points, EMU, eighths) and the merge rule. |
| `theme` | Color scheme and major/minor fonts; DrawingML color modifiers (lumMod/lumOff, tint, shade). |
| `styles` | Doc defaults, `basedOn` chains, table styles with conditional regions (header row, last row, first/last column, banding, corners). |
| `numbering` | Abstract and concrete lists, overrides, `numStyleLink`; counters shared per abstract list as in Word; decimal, roman, letters, ordinal, bullets, legal numbering. |
| `read` | The package (rels, parts, media) → resolved model. Precedence: doc defaults → table style → numbering → paragraph style → character style → direct. Word's twist: with the default paragraph style, the table style wins over it. |
| `fonts` | System fonts (once per process); metric-compatible substitutes; GDI line height (win ascent + descent + external leading); `fsType` restricted faces are never embedded; per-character fallback; shaping without ligatures, with kerning only on request. |
| `text` | Atoms: words with their trailing spaces, CJK characters, soft-hyphen points, tabs, breaks and inline objects. Greedy line breaking with Word's tab rules: custom stops, the implicit stop at a hanging indent, default stops from the margin, right/center/decimal alignment and leaders. Line height follows the spacing rule, with extra space above the text. |
| `layout` | Sections (next page, continuous, even/odd, next column); columns; headers and footers (first, even, default, inherited); pagination (space before dropped after overflow, keep-with-next chains, keep lines, widow/orphan control); paragraph shading and borders (merged across identical neighbours); tables (grid widths, merges, borders, shading, vertical alignment, repeated header rows, row splitting); floating pictures and shapes with square and top-and-bottom wrap; text boxes; footnotes at the page bottom; endnotes; bookmarks; heading outline. |
| `render` | krilla. Glyphs at their exact positions (synthetic bold/italic when a face is missing), lines with dash styles, shapes, pictures with crop (PNG, JPEG and GIF embedded as-is, others re-encoded), URL links, jumps to bookmarks, outline, metadata. |

## 3. What is supported

| Area | Supported | Not yet |
|---|---|---|
| Page | Size, orientation, margins, gutter, header/footer distance, columns (equal and custom, separator line), section types, page-number start and format | Page borders, background, vertical page alignment, line numbers, balancing columns at a continuous break |
| Text | Fonts per script slot (ASCII, Latin, East Asian, complex), theme fonts, size, bold/italic, colors (theme, tint/shade, auto on dark fill), underline (single, double, thick, dotted, dashed, words), strike, double strike, caps, small caps, super/subscript, raised/lowered, character spacing, horizontal scale, highlight, run shading, hidden text, symbol fonts | Text effects (outline, shadow, glow), emphasis marks, vertical text, full bidirectional reordering |
| Paragraphs | Alignment (incl. justify and distribute), indents, hanging indents, spacing before/after (auto 14 pt), line spacing (multiple, exact, at least), contextual spacing, keep with next, keep lines, page break before, widow/orphan control, tabs with leaders, shading, borders, lists | Drop caps, frames, hyphenation zone, East Asian line-breaking rules beyond basic kinsoku |
| Tables | Grid widths (fixed and percent), indent, alignment, borders (table and cell), shading, cell margins, vertical alignment, horizontal and vertical merges, header rows repeated, rows split across pages, nested tables, table styles with conditional formatting | Floating tables, cell text direction, autofit recomputation, diagonal borders |
| Drawings | Inline and floating pictures (crop, all common formats); shapes (rect, rounded rect, ellipse, line) with theme fills and outlines; text boxes; charts (§11); EMF/WMF pictures as vectors (§12); wrap square/tight/through (as square) and top-and-bottom; behind/in front; legacy VML pictures | SmartArt, groups, rotation, wrap polygons |
| Fields | PAGE, NUMPAGES, SECTIONPAGES (with format switches), HYPERLINK, and every other field's cached result (TOC, REF, DATE…) | Recomputing TOC page numbers |
| Notes | Footnotes at the bottom of the page with a separator, endnotes at the end, numbered in order | Notes continuing onto the next page, custom marks |
| PDF | Embedded subset fonts with real text (copy, search), links, internal jumps, outline from headings, title/author/subject/keywords/language | Tagged PDF (PDF/UA), PDF/A |

Unsupported objects keep their space, so the text around them lands where Word puts it.

## 4. Fonts and fidelity

Fidelity depends on the fonts:
- **On Windows** the document's fonts are normally installed (Calibri, Cambria, Times New Roman, Arial, Aptos when Office is present), so line breaks and page breaks match Word.
- **On Linux** the metric-compatible families stand in: Carlito for Calibri, Caladea for Cambria, Liberation for Arial/Times/Courier. They have the same widths, so the same breaks.
- **Under WSL** the Windows fonts are used directly.

Fonts whose licence forbids embedding are skipped for a substitute. Microsoft fonts are only *used* from the user's system and embedded as subsets in the user's own PDF, as Word does. They are never shipped (PROJECT_GUIDE.md rule 5).

## 5. Planner cost

The capability is `docx → pdf`, loss 1, 60 ms + 40 ms per MB: the same fidelity class as LibreOffice and about 40× faster. With LibreOffice installed, the planner still prefers this engine because time breaks the tie; LibreOffice remains available for the legacy `.doc` format and for spreadsheets and slides.

## 6. Quality loop

1. `cargo run --release -p swirvo-engines --example docx_pdf -- in.docx out.pdf` converts and prints the page count and time.
2. LibreOffice makes the reference: `soffice --headless --convert-to pdf in.docx`.
3. `cargo run --release -p swirvo-engines --example pdf_layout -- compare ref.pdf out.pdf side.png 70` renders both side by side.

The test corpus mixes documents from different producers:
- LibreOffice-authored DOCX (from the HTML samples);
- our own PDF → DOCX output (floating shapes, layout tables);
- a hand-built Word-style document with theme fonts, numbering, table styles, a footer with PAGE/NUMPAGES, a footnote and a link (the unit test).

The unit test also reads the PDF back through `pdflayout` and checks the page count, page size and text.

## 6a. Results (2026-11-09)

Eleven documents, each compared page by page with LibreOffice's PDF of the same file:

| Document | Producer | Pages (ours = LibreOffice) | Time, warm | Notes |
|---|---|---|---|---|
| report, report-pl | LibreOffice / our PDF → DOCX | 4 = 4, 2 = 2 | 12 ms, 36 ms | Identical apart from a character-border padding |
| resume, resume-pl | LibreOffice / ours | 1 = 1 | 12 ms | Full-page sidebar text box matches |
| invoice, invoice-pl | LibreOffice / ours | 1 = 1 | 5 ms | Identical |
| columns, columns-pl | LibreOffice / ours | 1 = 1 | 26 ms | Balanced columns; LibreOffice spaces 1.4× lines tighter than Word's rule, so one paragraph splits differently |
| long, long-pl | LibreOffice / ours | 22 = 22, 15 = 15 | 64 ms, 55 ms | Same page breaks throughout |
| wordlike | Word-style XML (theme, Grid Table 4, TOC, text box, wrap, columns, landscape) | 5 = 5 | 72 ms | Table rows differ by one per page (LibreOffice rounds row heights up slightly) |

On Windows, a cold command-line conversion takes 0.38–0.52 s for the whole process: start-up, conversion graph, font scan and layout. LibreOffice needs about 2.8 s for the same files. Inside the running app, conversions after the first take 5–100 ms. The first conversion in a process also pays a one-time font scan. It takes 1–2 s under WSL, where Windows fonts are read across a network share; a native Windows run is faster.

**Decisions made along the way:**
- **Linked pictures are not loaded.** A picture that points to a file on disk (`r:link`, `file:///…`) is left blank, with its space kept. LibreOffice loads such files. We don't: a document must not be able to pull arbitrary local files into a PDF the user may share. Pictures embedded in the document are always drawn.
- **List indents win over the paragraph style**, as in Word (the "List Paragraph" indent gives way to the list level's).
- **Character borders** (`w:bdr`) draw a box around each run of bordered text on a line, with a small fixed padding. Word ignores the border's `w:space` for runs.
- **Font lists** in HTML-imported documents (`Calibri;sans-serif`) resolve to the first installed family; CSS generic names map to our generic lists.
- **Nesting is capped.** A part nested more than 256 elements deep is refused, and tables or text boxes more than 16 levels inside each other are left out. Without the first cap, a crafted file 100 000 levels deep overflows the stack and aborts the process; `catch_unwind` can't catch that. Real documents stay far below both limits. The tests run 80-level nested tables on a 1 MB stack, the size of a Windows main thread.
- **Fallback fonts are chosen by script**, so a stray symbol never opens a 20 MB CJK font. This took one document from 852 ms to 84 ms.

## 7. Next

1. ~~EMF/WMF pictures~~: done, our own renderer (§12).
1. A lazy font index (the Windows registry font list, or a cached index on Linux) so the first conversion opens only the fonts it needs.
3. Tagged PDF (accessibility) and PDF/A for archiving.
4. Balanced columns at continuous section breaks; floating tables.

## 8. PowerPoint → PDF (`pptxlayout`)

*Built 2026-11-09. Code: `crates/engines/src/pptxlayout/`.* Slides are fixed layouts, so there's no pagination to do. The work is in resolving what each shape inherits, then drawing it. The engine reuses this one's parts: the text-box layout (fonts, line breaking, bullets, tabs, links), tables, and the PDF renderer.

| Module | Role |
|---|---|
| `read` | Slides in order (hidden ones skipped, as PowerPoint does). Each is resolved against its layout and master. **Placeholders** inherit position, body settings (insets, anchoring, autofit) and list styles: master text style for the family (title, body, other), then master placeholder, layout placeholder, the shape. **Masters and layouts** contribute their non-placeholder shapes (unless `showMasterSp="0"`) and the background. Groups map child coordinates (`chOff`/`chExt`) and pass on rotation and flips. Also pictures (with crop), tables with table styles (whole table, banding, first/last row and column), hyperlinks, slide-number fields, and SmartArt through the drawing PowerPoint saves with it. Slide titles become PDF bookmarks |
| `style` | Colours through the slide's colour map (`tx1` → `dk1`, overrides on layouts and slides), placeholder colour (`phClr`), `alpha`, `lumMod`/`lumOff`, `satMod`, and `tint`/`shade` in linear light as PowerPoint does. Fills: solid, linear and radial gradients, picture, pattern (foreground colour), group. Lines: width, dash presets, caps, arrowheads. Theme style references (`fillRef`, `lnRef`, `fontRef`, `bgRef`) through the theme's format scheme. Text: nine-level paragraph and run properties with inheritance |
| `geom` | About 60 preset geometries with their adjust handles (rectangles, rounded and snipped corners, ellipses, polygons, arrows, chevrons, stars, callouts, flowchart shapes, brackets, braces, connectors), and custom geometry paths including arcs |
| `mod` | Drawing: rotation and flips as transforms (text turns with the shape but is never mirrored; a vertical flip turns it upside down), vertical text, insets and anchoring, unwrapped text, arrowheads on open paths |

The Word renderer gained three operations for this: `Figure` (any path with a solid or gradient fill, transparency and a dashed or round-capped stroke), and `Push`/`Pop` of a transform.

**Results.** Checked on a PowerPoint-structured deck: theme, master with text styles, layout, inherited placeholders, a styled and rotated shape, a group, a connector with an arrowhead, a picture, a styled table and a hidden slide. Side by side with LibreOffice's PDF of the same file, the slides match, including table colours. The differences: LibreOffice shrinks table text that the file sets at 18 pt (we follow the file), and line spacing differs by a point or two.

**Planner.** Loss 2 against 1 for LibreOffice and Microsoft Office (it approximates PowerPoint's own rendering), so an installed office suite still wins for PPTX. Without one, PowerPoint files convert instead of not appearing at all.

**Not yet.**
- ~~EMF/WMF pictures~~: drawn as vectors since 2026-11-10 (§12).
- Text effects, shadows and 3-D, picture clipping to non-rectangular shapes, pattern fills (drawn in the foreground colour), animations (irrelevant for PDF).

## 9. Excel → PDF (`xlsxlayout`)

*Built 2026-11-09. Code: `crates/engines/src/xlsxlayout/`.* Every visible sheet, printed the way Excel prints it, through the same text layout and renderer.

| Module | Role |
|---|---|
| `read` | **Workbook:** styles (number formats, fonts, fills including pattern and gradient, borders, alignment, wrapping, indent), theme, indexed and ARGB colours with Excel's luminance tint, and the Office palette when a file has no theme. Also shared strings with rich runs, the 1900/1904 date systems, and defined names (print area and print titles per sheet). Hidden sheets are skipped. **Per sheet:** cells (shared, inline and formula strings, numbers, booleans, errors; formulas show their cached result), column widths converted the way Excel does (stored widths include padding: 9.14 → 64 px → 48 pt), row heights and hidden rows and columns, merges, page setup, margins, print options, manual page breaks, header and footer, hyperlinks, and pictures anchored to cells. At most 2 million cells per workbook |
| `numfmt` | Excel number formats: sections (positive; negative; zero; text) and conditions, colours (`[Red]`), currency and locale tags, thousands separators and scaling commas, percent, scientific, fractions, `General` (11 characters), dates and times (1900 phantom leap day included), elapsed `[h]:mm`, AM/PM, text sections with `@`. Halves round away from zero, as in Excel |
| `mod` | **Pagination:** print area or used range (cells with values, fills or borders, merges, pictures), scale or fit to N × M pages, pages filled in Excel's order (down, then over, unless set otherwise), repeated title rows and columns, centring. **Drawing:** fills, optional gridlines, borders (hair to thick, dashed, dotted, double), and cell text clipped to its cell. Unwrapped text spills into empty neighbours as in Excel; numbers too wide show `####`; wrapped text is aligned within the cell; vertical alignment is honoured. Rows without a stored height grow for wrapped text and larger fonts. Headers and footers handle `&L`/`&C`/`&R`, `&P`, `&N`, `&A`, `&F`, `&D` and bold. Each sheet name becomes a PDF bookmark |

**Results.** On a two-page A4 sales sheet (merged title, styled header, dates, currency, percentages, a wrapped note, an overflowing label, a too-wide number, a repeated title row, a footer with page codes, a hidden sheet), the pages match LibreOffice's print of the same file. The differences:
- LibreOffice recalculates heights for rows that store none, and fits about 52 rows a page where Excel's 15 pt rows give 45, as ours do.
- LibreOffice drops theme colours when the file has no theme part, where Excel uses the Office defaults, as we do.

**Planner.** Loss 2, so LibreOffice or Microsoft Office wins when installed. Without it, XLSX → PDF now prints the sheets instead of typesetting the first one as a plain table.

**Not yet.** Conditional formatting, text rotation, shrink-to-fit, rich text in headers and footers, and shapes and text boxes on sheets.

## 10. ODT and RTF → PDF

*Built 2026-11-10. Code: `docxlayout/odt.rs`, `docxlayout/rtf.rs`.* Both are read into the same document model as DOCX, so they get the same layout and renderer.

**ODT.**
- Styles with their parent chains, automatic styles, and the default style per family.
- Master pages and page layouts become sections, split where a paragraph names another master page.
- Headers and footers with page numbers and counts.
- Lists with their labels, and tables with spans and covered cells.
- Frames with pictures, text boxes, sections and indexes.

**RTF.**
- A tokenizer with a group state stack.
- Font, colour and stylesheet tables; character and paragraph formatting; tabs with leaders.
- Tables (`\trowd`, `\cellx`, vertical merges; horizontal merges not yet).
- Unicode escapes with their fallbacks; 8-bit text as Windows-1252 (other code pages not yet).
- Pictures (PNG, JPEG), `PAGE`/`NUMPAGES` fields, and page setup.

**Security.** Linked files are never loaded: an ODT picture whose `xlink:href` points outside the package keeps its space, empty, and RTF fields show only their stored result.

**Results.** Six ODT files were exported by LibreOffice from the DOCX test set and compared with LibreOffice's own PDFs. All six give the same page count, and `wordlike`'s pages now match line for line. It used to come out a page longer, for these reasons, all fixed:
- **List indents.** A list level in label-alignment mode *replaces* the paragraph's indent; it doesn't add to it. LibreOffice's rule decides which wins: hard formatting first, then a list applied directly, then the nearest named style.
- **Contextual spacing** (`style:contextual-spacing`) wasn't read. Paragraphs of the same named style (automatic styles don't count) get no space between them.
- **Header room.** A header's `fo:min-height` already includes its spacing. With dynamic spacing the body starts at margin + min-height, and taller header text pushes it down.
- **Frames** with `page-content` positions are placed from the margins, not the page edge.
- **Paragraph borders.** A left border was drawn as a bottom border.
- **Fixed by this check:**
  - Word text boxes that LibreOffice saves as custom shapes (`draw:custom-shape`) are drawn with their fill, outline and text, so the résumé sidebar appears;
  - a master page with `style:next-style-name` (a title page handing over to the running pages) becomes a first-page header and footer;
  - text sections with columns become continuous column sections.
- **Bug found by this check: wrong column for covered cells.** Covered cells after a column-spanning cell were counted twice, which put the next cell outside the grid. Now every cell element is one grid column.
- **Bug found by this check: an endless loop in the shared layout core.** It affected DOCX too. A line taller than the page (a large picture) followed by more text made widow/orphan control move the paragraph to the next page forever. It's fixed, and a regression test covers it.

## 11. Charts (`chart`)

*Built 2026-11-10. Code: `crates/engines/src/chart/`.* Charts are drawn from the values Office caches in every chart part (`c:chartSpace`), so no spreadsheet has to be evaluated. One module serves all three engines: PowerPoint graphic frames, Excel drawing anchors, and Word inline and floating drawings.

- **Types:** bar and column (clustered, stacked, 100 %), line (markers, gaps at missing values unless `dispBlanksAs` is `span`), area, pie, doughnut, scatter. Radar is drawn as lines.
- **Parts:**
  - titles, including the automatic title of a single-series pie;
  - legends at any side, wrapping into rows at the top or bottom;
  - value axes with tidy scales: the smallest 1, 2 or 5 × 10ⁿ step that needs at most ten intervals, from zero unless the data crosses it, 5 % headroom, fixed min, max and step honoured;
  - category labels thinned so they don't overlap; gridlines; axis titles; data labels (values, percentages).
- **Colours and text:**
  - Series and point colours from the file, else the theme's accents in turn and then darker and lighter rounds of them, as Office does.
  - Text in the theme's body font and the chart's text properties.
  - Number formats through the Excel format engine (`xlsxlayout::numfmt`).
- **Hosts:**
  - Excel and Word give a chart without its own area fill a white background (Excel adds its light gray border), as those apps do.
  - PowerPoint charts stay transparent.
  - Text goes through the host's own layouter (the `LayText` trait), so fonts, fallbacks and shaping match the document.
- **Not yet:** 3-D rendering (3-D charts are drawn flat), secondary axes, trendlines, error bars, combo charts with two value axes, chartex (the 2016 types: waterfall, treemap, sunburst, funnel, box and whisker), and rotated axis labels.


## 12. EMF and WMF pictures (`emf`)

*Built 2026-11-10. Code: `crates/engines/src/emf/`.* Office files embed Windows metafiles for logos, pasted charts and diagrams, equation previews and the pictures of OLE objects. They are played back as GDI would, then drawn as **vectors** through the shared PDF writer, so lines stay sharp and text stays text. All three engines use it (Word, ODT and RTF pictures; PowerPoint pictures; Excel drawings), and EMF/WMF files convert on their own to PDF, SVG and PNG.

| Part | What it covers |
|---|---|
| `emr` | EMF records. The header's frame and device size set the picture's size. Covers: polygons, polylines and Béziers (32 and 16-bit, `…To` forms, poly-poly, PolyDraw); rectangles, rounded rectangles, ellipses, arcs, chords, pies, AngleArc and ArcTo; path brackets (fill, stroke, both, clip); pens (styles, widths, caps, joins, user dashes), solid, hatched and pattern brushes, fonts, stock objects; ExtTextOut (UTF-16 and 8-bit) with per-character advances, alignment, opaque and clip boxes, escapement; SmallTextOut; StretchDIBits, BitBlt, StretchBlt, AlphaBlend, TransparentBlt, SetDIBitsToDevice; GradientFill; IntersectClipRect, ExcludeClipRect, region clips, FillRgn and PaintRgn. EMF+ comments are skipped; dual files carry the same drawing as EMF records |
| `wmr` | WMF records (16-bit, parameters stored last-first), with or without the placeable header. Without one, the first window sets the picture's box. Objects take the lowest free slot, as GDI assigns them |
| `gdi` | The device context shared by both: mapping modes (text, metric, English, twips, isotropic, anisotropic), window and viewport origins and extents (with scaling), world transforms, SaveDC/RestoreDC, current position, arc direction, fill mode, text colour and background. Coordinates are converted when recorded, like GDI paths. Pens never get thinner than one device pixel. Hatches are real lines clipped to the shape. Text stays upright when a mapping mode flips y |
| `dib` | Device-independent bitmaps: 1, 4, 8, 16, 24 and 32-bit, palettes, bit-field masks, top-down and bottom-up, premultiplied alpha for AlphaBlend only (plain blits often leave the fourth byte at 0). RLE goes through the BMP decoder; JPEG and PNG bitmaps decode directly |
| `ops`, `svg` | The played-back picture as layout ops (text shaped with the document's fonts and fallbacks, placed by the metafile's own advances) or as SVG (PNG is rendered from it with resvg) |

**Safety.** Every record's size is checked against the file, and every count against its record. Playback stops at fixed limits: 5 million records, 4 million points, 400,000 drawn items, 40 megapixels per bitmap, 256 MB of bitmaps in all, 256 saved states, 65,536 objects. A fuzz test feeds thousands of truncated and corrupted files through playback and both writers.

**Not yet.** EMF+ (its fallback records are drawn instead), raster operations other than copying or masking, glyph-index text (`ETO_GLYPH_INDEX`, rare in Office files), Gouraud-shaded triangles (filled with their average colour), StarView `.svm` pictures in ODT files.

## 13. Archival and accessible output

*Built 2026-11-10. Code: `docxlayout/tags.rs`, `docxlayout/render.rs`.* One renderer writes every engine's pages, so DOCX, ODT, RTF, PPTX and XLSX all get the same two options (`PdfOptions`, passed through `StepRequest::pdf`):

- **Archival:** PDF/A-2 through krilla's validator, which refuses to write a file that breaks the standard. Accessible PDF/A-2a when the PDF is tagged and the document has a language; PDF/A-2b otherwise (tried in that order). Pictures lose the "interpolate" hint PDF/A forbids; the creation date PDF/A needs is the time of conversion.
- **Tagged:** layout emits `Op::Mark { node, y }` before content that belongs to another structure node, and keeps the nodes (role and parent) in a small arena. The mark carries the content's top so splitting a cell or text box across pages keeps each mark with its line. Nodes made during attempts layout backs out of (keep-with-next, balanced columns) are simply never drawn and leave nothing behind. The renderer opens marked content per run of operations with the same node, closes it around every save/restore of the graphics state so marked content and `q`/`Q` nest properly, and builds the tree in reading order: a node joins its parent when its first content is drawn. Untagged slides or sheets content becomes plain paragraphs and figures, never untagged content.

- **PDF/A-3** (`attach_source`, *built 2026-11-11*): the same as PDF/A-2, plus the source document embedded byte for byte as the PDF's associated file (`/AF`, relationship *Source*, its MIME type, a description and a date, as PDF/A-3 requires), named after the PDF (`Report.docx`). 3a when tagged with a language, 3b otherwise. The original stays recoverable from the archive copy.
- **PDF/UA-1** (`accessible`, *built 2026-11-11*): always tagged, through krilla's UA-1 validator. The title shown in the viewer's title bar (`DisplayDocTitle`) is the document's own, else the file name. The outline comes from the headings; a document without any (most sheets) gets one entry for the whole document, because UA-1 requires an outline. Every figure has alternative text: Word's or ODT's description, the chart's title, else "Picture". An empty heading paragraph is tagged as a paragraph, since a heading must have text. PDF/UA and PDF/A are separate choices (krilla validates one standard per file), so `accessible` takes the place of `archival`. A file that still breaks UA-1 isn't written; the error names the problem in words ("a link has no description").

Not yet: a language check for PDF/UA (the standard expects one; we don't refuse documents without it), figure bounding boxes, header scopes per cell beyond the header row, and the list label as its own `Lbl` (it's inside the item body).
