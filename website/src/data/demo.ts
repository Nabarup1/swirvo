// Files in the homepage demo, with the wedges the real app shows for them. Labels and
// captions come from crates/jobs (wheel_items.rs `purpose`, tools.rs `label`).
import type { WItem } from '../scripts/wheel-draw';

export type Kind = 'image' | 'pdf' | 'doc' | 'video' | 'sheet' | 'archive';

export interface DemoFile {
  name: string;
  kind: Kind;
  meta: string;
  size: string;
  convert: WItem[];
  tools: WItem[];
  /** What each preset sub-ring offers for a wedge label (default: Best · Balanced · Smallest). */
  presets?: Record<string, string[]>;
  /** Sizes for results, by wedge label. */
  out?: Record<string, string>;
}

const ACTIONS: WItem[] = [
  { label: 'Show', caption: 'In folder' },
  { label: 'Copy path', caption: 'Clipboard' },
  { label: 'Terminal', caption: 'Here' },
  { label: 'To ZIP', caption: 'Compress' },
  { label: 'Board', caption: 'Open' },
  { label: 'Copy text', caption: 'Clipboard' },
];
export const actions = ACTIONS;

export const FILES: DemoFile[] = [
  {
    name: 'Holiday.heic',
    kind: 'image',
    meta: 'HEIC image · 4032 × 3024',
    size: '3.1 MB',
    convert: [
      { label: 'JPG', caption: 'Universal' },
      { label: 'PNG', caption: 'Lossless' },
      { label: 'WEBP', caption: 'Web' },
      { label: 'PDF', caption: 'Document' },
      { label: 'GIF', caption: '256 colors' },
      { label: 'TIFF', caption: 'Archival' },
      { label: 'TXT', caption: 'Plain text' },
    ],
    tools: [
      { label: 'Resize', caption: '50 %' },
      { label: 'Fit 1920', caption: 'Longest side' },
      { label: 'Rotate', caption: '90° right' },
      { label: 'Flip', caption: 'Mirror' },
      { label: 'Compress', caption: 'Smaller file' },
      { label: 'Strip info', caption: 'GPS, camera' },
      { label: 'To one PDF', caption: 'Page each' },
    ],
    out: { JPG: '1.4 MB', PNG: '9.8 MB', WEBP: '0.9 MB', PDF: '1.4 MB', GIF: '2.2 MB', TIFF: '36 MB', TXT: '1 KB', Resize: '0.8 MB', 'Fit 1920': '0.7 MB', Rotate: '3.1 MB', Flip: '3.1 MB', Compress: '1.1 MB', 'Strip info': '3.1 MB', 'To one PDF': '1.4 MB' },
  },
  {
    name: 'Team handbook.pdf',
    kind: 'pdf',
    meta: 'PDF · 13 pages',
    size: '2.4 MB',
    convert: [
      { label: 'DOCX', caption: 'Lossy' },
      { label: 'TXT', caption: 'Plain text' },
      { label: 'HTML', caption: 'Web page' },
      { label: 'MD', caption: 'Lossy' },
      { label: 'EPUB', caption: 'Lossy' },
      { label: 'XLSX', caption: 'Excel workbook' },
      { label: 'ODT', caption: 'Lossy' },
      { label: 'More…', caption: '2 more', more: true },
    ],
    tools: [
      { label: 'Compress', caption: 'Smaller PDF' },
      { label: 'Split ranges', caption: 'Your own groups' },
      { label: 'Watermark', caption: 'Text or logo' },
      { label: 'Numbers', caption: 'Page 1 of 9' },
      { label: 'Protect', caption: 'Password' },
      { label: 'Redact', caption: 'Remove found text' },
      { label: 'Pages…', caption: 'Pick pages' },
      { label: 'More…', caption: '31 more', more: true },
    ],
    out: { DOCX: '410 KB', TXT: '38 KB', HTML: '520 KB', MD: '41 KB', EPUB: '460 KB', XLSX: '22 KB', ODT: '380 KB', Compress: '0.9 MB', Watermark: '2.4 MB', Numbers: '2.4 MB', Protect: '2.4 MB', Redact: '2.3 MB' },
  },
  {
    name: 'Quarterly report.docx',
    kind: 'doc',
    meta: 'Word document · 9 pages',
    size: '46 KB',
    convert: [
      { label: 'PDF', caption: 'Document' },
      { label: 'HTML', caption: 'Web page' },
      { label: 'TXT', caption: 'Plain text' },
      { label: 'ODT', caption: 'Lossy' },
      { label: 'MD', caption: 'Lossy' },
      { label: 'EPUB', caption: 'Lossy' },
      { label: 'RTF', caption: 'Lossy' },
    ],
    tools: [
      { label: 'Compress', caption: 'Smaller file' },
      { label: 'Clean', caption: 'Author, changes' },
      { label: 'Replace', caption: 'Find & replace' },
      { label: 'Pictures', caption: 'Save images' },
      { label: 'Word count', caption: 'Clipboard' },
      { label: 'Mail merge', caption: 'Letter per row' },
      { label: 'PDF/A', caption: 'Archival PDF' },
    ],
    out: { PDF: '182 KB', HTML: '64 KB', TXT: '12 KB', ODT: '31 KB', MD: '11 KB', EPUB: '40 KB', RTF: '88 KB', Compress: '30 KB', Clean: '45 KB', 'PDF/A': '240 KB' },
  },
  {
    name: 'Interview.mov',
    kind: 'video',
    meta: 'QuickTime · 2:41',
    size: '148 MB',
    convert: [
      { label: 'MP4', caption: 'Lossy' },
      { label: 'GIF', caption: '256 colors' },
      { label: 'MP3', caption: 'Lossy' },
      { label: 'M4A', caption: 'Lossy' },
      { label: 'WAV', caption: 'Lossless' },
      { label: 'AAC', caption: 'Lossy' },
    ],
    tools: [
      { label: 'Compress', caption: 'To a size' },
      { label: 'Frame', caption: 'Save a picture' },
    ],
    presets: { Compress: ['10 MB', '25 MB', '50 MB'] },
    out: { MP4: '61 MB', GIF: '24 MB', MP3: '3.8 MB', M4A: '3.1 MB', WAV: '28 MB', AAC: '3.0 MB', Frame: '1.2 MB', Compress: '24.6 MB' },
  },
  {
    name: 'Budget.csv',
    kind: 'sheet',
    meta: 'CSV · 1,204 rows',
    size: '84 KB',
    convert: [
      { label: 'XLSX', caption: 'Excel workbook' },
      { label: 'JSON', caption: 'Data' },
      { label: 'YAML', caption: 'Readable' },
      { label: 'TSV', caption: 'Tabs' },
      { label: 'TOML', caption: 'Config' },
      { label: 'NDJSON', caption: 'Streaming' },
      { label: 'HTML', caption: 'Web page' },
      { label: 'More…', caption: '4 more', more: true },
    ],
    tools: [{ label: 'Checksum', caption: 'SHA-256' }],
    out: { XLSX: '61 KB', JSON: '190 KB', YAML: '170 KB', TSV: '84 KB', TOML: '176 KB', NDJSON: '188 KB', HTML: '240 KB' },
  },
];

export const KIND_LABEL: Record<Kind, string> = {
  image: 'IMG',
  pdf: 'PDF',
  doc: 'DOC',
  video: 'VID',
  sheet: 'CSV',
  archive: 'ZIP',
};
