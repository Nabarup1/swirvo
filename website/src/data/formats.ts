// The formats Swirvo knows (crates/formats) and what each can become (docs/10). `via`
// says what does the work: built in, something Windows already ships, or an optional
// program the user installed. The wheel only ever shows targets that work on that PC.

export type Via = 'built-in' | 'windows' | 'ocr' | 'office' | 'ffmpeg';
export type Category =
  | 'image' | 'pdf' | 'document' | 'slides' | 'spreadsheet' | 'data'
  | 'audio' | 'video' | 'subtitle' | 'ebook' | 'archive' | 'font';

export interface Format {
  key: string;
  name: string;
  cat: Category;
  /** How Swirvo reads it. 'planned' = recognised (detected), no engine yet. */
  read: Via | 'planned';
  to: [string, Via][];
  note?: string;
}

export const VIA: Record<Via, { label: string; text: string }> = {
  'built-in': { label: 'Built in', text: 'Works with nothing else installed.' },
  windows: { label: 'Windows', text: 'Uses an engine Windows already has (image codecs, Media Foundation, tar), adding 0 bytes.' },
  ocr: { label: 'OCR', text: 'Text recognised with Windows OCR (tesseract on Linux, if installed).' },
  office: { label: 'Office', text: 'With Microsoft Office or LibreOffice installed, for an exact Office layout.' },
  ffmpeg: { label: 'FFmpeg', text: 'Lights up automatically when FFmpeg is installed.' },
};

export const CATEGORIES: Record<Category, { title: string; hue: number }> = {
  image: { title: 'Images', hue: 28 },
  pdf: { title: 'PDF', hue: 8 },
  document: { title: 'Documents', hue: 215 },
  slides: { title: 'Slides', hue: 18 },
  spreadsheet: { title: 'Spreadsheets', hue: 145 },
  data: { title: 'Data', hue: 170 },
  audio: { title: 'Audio', hue: 300 },
  video: { title: 'Video', hue: 265 },
  subtitle: { title: 'Subtitles', hue: 50 },
  ebook: { title: 'E-books', hue: 190 },
  archive: { title: 'Archives', hue: 38 },
  font: { title: 'Fonts', hue: 330 },
};

const B = 'built-in' as const;
const each = (keys: string[], via: Via): [string, Via][] => keys.map((k) => [k, via]);
const without = (list: [string, Via][], key: string) => list.filter(([k]) => k !== key);

const RASTER = ['jpg', 'png', 'webp', 'gif', 'bmp', 'tiff', 'ico', 'tga', 'qoi', 'pnm'];
const rasterTo = [...each(RASTER, B), ['pdf', B] as [string, Via]];
const ocrTo = each(['txt', 'docx', 'odt', 'md', 'html'], 'ocr');
const PHOTO = ['jpg', 'png', 'webp', 'bmp', 'tiff', 'gif'];

const DOCS = ['pdf', 'docx', 'odt', 'rtf', 'txt', 'md', 'html', 'epub'];
// RTF is written only from Word and OpenDocument text (docs/10).
const docTo = (self: string, extra: [string, Via][] = []) =>
  [...without(each(DOCS, B), self).filter(([k]) => k !== 'rtf' || ['docx', 'odt'].includes(self)), ...extra];

const SHEET_OUT = ['csv', 'tsv', 'json', 'html', 'md', 'docx', 'odt', 'pdf'];
const DATA = ['json', 'ndjson', 'yaml', 'toml', 'csv', 'tsv', 'xlsx'];

const AUDIO_WIN = ['mp3', 'm4a', 'aac', 'wav'];
const audioTo = (self: string, nativeIn: boolean, winIn: boolean): [string, Via][] => {
  const out: [string, Via][] = [];
  if (nativeIn) out.push(['wav', B], ['flac', B]);
  if (winIn) for (const k of AUDIO_WIN) if (!out.some(([o]) => o === k)) out.push([k, 'windows']);
  for (const k of ['mp3', 'm4a', 'aac', 'ogg', 'opus', 'wav', 'flac', 'aiff'])
    if (!out.some(([o]) => o === k)) out.push([k, 'ffmpeg']);
  return without(out, self);
};

const VIDEO_WIN_OUT = ['mp4', 'gif', 'mp3', 'm4a', 'aac', 'wav'];
const videoTo = (self: string, win: boolean): [string, Via][] => {
  const out: [string, Via][] = win ? each(VIDEO_WIN_OUT, 'windows') : [];
  for (const k of ['mp4', 'mkv', 'webm', 'mov', 'avi', 'gif', 'mp3', 'm4a', 'wav', 'flac', 'ogg', 'opus'])
    if (!out.some(([o]) => o === k)) out.push([k, 'ffmpeg']);
  return without(out, self);
};

const ARCH = ['zip', 'tar', 'tar.gz', 'tar.bz2', 'tar.xz'];

export const FORMATS: Format[] = [
  // Images
  ...RASTER.map((k): Format => ({
    key: k,
    name: { jpg: 'JPEG', png: 'PNG', webp: 'WebP', gif: 'GIF', bmp: 'Bitmap', tiff: 'TIFF', ico: 'Windows icon', tga: 'Targa', qoi: 'QOI', pnm: 'Netpbm' }[k]!,
    cat: 'image',
    read: B,
    to: [...without(rasterTo, k), ...(PHOTO.includes(k) ? ocrTo : [])],
  })),
  { key: 'heic', name: 'HEIC / HEIF (iPhone photos)', cat: 'image', read: 'windows', to: [...each([...RASTER, 'pdf'], 'windows'), ...ocrTo], note: 'Decoded by Windows’ HEIF Image Extension, common on Windows 11.' },
  { key: 'avif', name: 'AVIF', cat: 'image', read: 'windows', to: [...each([...RASTER, 'pdf'], 'windows'), ...ocrTo] },
  { key: 'jxl', name: 'JPEG XL', cat: 'image', read: B, to: [...rasterTo, ...ocrTo] },
  { key: 'psd', name: 'Photoshop (flattened)', cat: 'image', read: B, to: rasterTo },
  { key: 'svg', name: 'SVG', cat: 'image', read: B, to: rasterTo },
  { key: 'emf', name: 'Windows metafile (EMF)', cat: 'image', read: B, to: [['svg', B], ...rasterTo] },
  { key: 'wmf', name: 'Windows metafile (WMF)', cat: 'image', read: B, to: [['svg', B], ...rasterTo] },
  ...['dng', 'cr2', 'cr3', 'nef', 'arw'].map((k): Format => ({
    key: k,
    name: `Camera RAW (${k.toUpperCase()})`,
    cat: 'image',
    read: B,
    to: rasterTo,
    note: 'Built in: the camera’s full-size JPEG preview. A full develop to PNG uses the Windows Raw Image Extension.',
  })),

  // PDF
  { key: 'pdf', name: 'PDF', cat: 'pdf', read: B, to: [...each(['docx', 'odt', 'txt', 'md', 'html', 'epub', 'xlsx', 'csv', 'tsv'], B)], note: 'PDF to Word rebuilds the layout. Scans without a text layer are read with OCR first. Page images are a tool (Pages JPG / PNG).' },

  // Documents
  { key: 'docx', name: 'Word document', cat: 'document', read: B, to: docTo('docx', [['doc', 'office']]) },
  { key: 'doc', name: 'Word 97–2003', cat: 'document', read: 'office', to: each(['pdf', 'docx', 'odt', 'rtf'], 'office') },
  { key: 'odt', name: 'OpenDocument text', cat: 'document', read: B, to: docTo('odt') },
  { key: 'rtf', name: 'Rich Text', cat: 'document', read: B, to: docTo('rtf') },
  { key: 'md', name: 'Markdown', cat: 'document', read: B, to: docTo('md') },
  { key: 'html', name: 'HTML', cat: 'document', read: B, to: docTo('html') },
  { key: 'txt', name: 'Plain text', cat: 'document', read: B, to: docTo('txt') },
  { key: 'pages', name: 'Apple Pages', cat: 'document', read: 'planned', to: [] },

  // Slides
  { key: 'pptx', name: 'PowerPoint', cat: 'slides', read: B, to: [['pdf', B], ['odp', 'office'], ['ppt', 'office']] },
  { key: 'ppt', name: 'PowerPoint 97–2003', cat: 'slides', read: 'office', to: each(['pdf', 'pptx', 'odp'], 'office') },
  { key: 'odp', name: 'OpenDocument slides', cat: 'slides', read: 'office', to: each(['pdf', 'pptx'], 'office') },
  { key: 'key', name: 'Apple Keynote', cat: 'slides', read: 'planned', to: [] },

  // Spreadsheets
  { key: 'xlsx', name: 'Excel workbook', cat: 'spreadsheet', read: B, to: [...each(SHEET_OUT, B), ['ods', 'office'], ['xls', 'office']], note: 'PDF printed like Excel: print area, page setup, styles, charts.' },
  { key: 'xls', name: 'Excel 97–2003', cat: 'spreadsheet', read: B, to: [...each(SHEET_OUT, B), ['xlsx', 'office'], ['ods', 'office']], note: 'PDF as a table built in; the printed sheet layout needs Office.' },
  { key: 'ods', name: 'OpenDocument sheet', cat: 'spreadsheet', read: B, to: [...each(SHEET_OUT, B), ['xlsx', 'office'], ['xls', 'office']], note: 'PDF as a table built in; the printed sheet layout needs Office.' },
  { key: 'csv', name: 'CSV', cat: 'spreadsheet', read: B, to: [...each(['xlsx', 'tsv', 'json', 'ndjson', 'yaml', 'toml', 'html', 'md', 'docx', 'odt', 'pdf'], B)] },
  { key: 'tsv', name: 'Tab-separated', cat: 'spreadsheet', read: B, to: [...each(['xlsx', 'csv', 'json', 'ndjson', 'yaml', 'toml', 'html', 'md', 'docx', 'odt', 'pdf'], B)] },
  { key: 'numbers', name: 'Apple Numbers', cat: 'spreadsheet', read: 'planned', to: [] },

  // Data
  ...['json', 'ndjson', 'yaml', 'toml'].map((k): Format => ({
    key: k,
    name: { json: 'JSON', ndjson: 'JSON Lines', yaml: 'YAML', toml: 'TOML' }[k]!,
    cat: 'data',
    read: B,
    to: without(each(DATA, B), k),
  })),
  { key: 'xml', name: 'XML', cat: 'data', read: B, to: each(['json', 'yaml', 'toml', 'csv', 'xlsx'], B) },
  { key: 'parquet', name: 'Parquet', cat: 'data', read: 'planned', to: [] },

  // Audio
  ...['mp3', 'm4a', 'aac', 'flac', 'ogg', 'wav', 'aiff', 'caf'].map((k): Format => ({
    key: k,
    name: { mp3: 'MP3', m4a: 'M4A (AAC)', aac: 'AAC', flac: 'FLAC', ogg: 'Ogg Vorbis', wav: 'WAV', aiff: 'AIFF', caf: 'Core Audio' }[k]!,
    cat: 'audio',
    read: B,
    to: audioTo(k, true, ['mp3', 'm4a', 'aac', 'flac', 'wav'].includes(k)),
  })),
  ...['wma', 'amr', 'ac3'].map((k): Format => ({
    key: k,
    name: { wma: 'Windows Media Audio', amr: 'AMR (voice)', ac3: 'Dolby AC-3' }[k]!,
    cat: 'audio',
    read: 'windows',
    to: audioTo(k, false, true),
  })),
  { key: 'opus', name: 'Opus', cat: 'audio', read: 'ffmpeg', to: audioTo('opus', false, false) },

  // Video
  ...['mp4', 'mov', 'mkv', 'webm', 'avi', 'wmv', '3gp', 'ts'].map((k): Format => ({
    key: k,
    name: { mp4: 'MP4', mov: 'QuickTime', mkv: 'Matroska', webm: 'WebM', avi: 'AVI', wmv: 'Windows Media Video', '3gp': '3GP', ts: 'MPEG transport stream' }[k]!,
    cat: 'video',
    read: 'windows',
    to: videoTo(k, true),
  })),
  ...['flv', 'ogv'].map((k): Format => ({
    key: k,
    name: { flv: 'Flash video', ogv: 'Ogg video' }[k]!,
    cat: 'video',
    read: 'ffmpeg',
    to: videoTo(k, false),
  })),

  // Subtitles
  ...['srt', 'vtt', 'ass', 'sbv'].map((k): Format => ({
    key: k,
    name: { srt: 'SubRip', vtt: 'WebVTT', ass: 'Advanced SubStation', sbv: 'SubViewer' }[k]!,
    cat: 'subtitle',
    read: B,
    to: [...without(each(['srt', 'vtt', 'ass', 'sbv'], B), k), ['txt', B]],
  })),
  { key: 'ttml', name: 'TTML / DFXP', cat: 'subtitle', read: 'planned', to: [] },

  // E-books
  { key: 'epub', name: 'EPUB', cat: 'ebook', read: B, to: each(['txt', 'md', 'html', 'docx', 'odt', 'pdf'], B) },
  ...['mobi', 'azw3', 'fb2'].map((k): Format => ({
    key: k,
    name: { mobi: 'Mobipocket', azw3: 'Kindle (AZW3)', fb2: 'FictionBook' }[k]!,
    cat: 'ebook',
    read: 'planned',
    to: [],
  })),

  // Archives
  ...ARCH.map((k): Format => ({
    key: k,
    name: { zip: 'ZIP', tar: 'Tar', 'tar.gz': 'Tar + gzip', 'tar.bz2': 'Tar + bzip2', 'tar.xz': 'Tar + xz' }[k]!,
    cat: 'archive',
    read: B,
    to: without(each(ARCH, B), k),
  })),
  { key: 'tar.zst', name: 'Tar + Zstandard', cat: 'archive', read: B, to: each(ARCH, B), note: 'Linux command line today; the Windows app is next.' },
  ...['7z', 'rar', 'iso', 'cab'].map((k): Format => ({
    key: k,
    name: { '7z': '7-Zip', rar: 'RAR', iso: 'Disc image (ISO)', cab: 'Cabinet' }[k]!,
    cat: 'archive',
    read: 'windows',
    to: each(['zip', 'tar', 'tar.gz', 'tar.bz2', 'tar.xz'], 'windows'),
  })),

  // Fonts
  { key: 'ttf', name: 'TrueType', cat: 'font', read: B, to: each(['woff', 'woff2'], B) },
  { key: 'otf', name: 'OpenType', cat: 'font', read: B, to: each(['woff', 'woff2'], B) },
  { key: 'woff', name: 'WOFF', cat: 'font', read: B, to: each(['woff2', 'ttf'], B), note: 'Unpacks to the font it holds (TrueType or OpenType).' },
  { key: 'woff2', name: 'WOFF2', cat: 'font', read: B, to: each(['woff', 'ttf'], B), note: 'Unpacks to the font it holds (TrueType or OpenType).' },
];

export const formatCount = FORMATS.length;
export const workingCount = FORMATS.filter((f) => f.read !== 'planned').length;
export const pairCount = FORMATS.reduce((n, f) => n + f.to.length, 0);
