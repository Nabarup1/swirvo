// Every tool in the app, in the order of `Tool::ALL` (crates/jobs/src/tools.rs). The label
// and caption are exactly what the Tools wheel shows; `id` is what the command line and
// the MCP server take (`swirvo tool <id> FILE`).

export type Group = 'pdf-pages' | 'pdf-edit' | 'pdf-secure' | 'pdf-content' | 'images' | 'office' | 'video' | 'files';

export interface Tool {
  id: string;
  label: string;
  caption: string;
  group: Group;
  text: string;
  /** Needs something only Windows has (OCR, the certificate store, Media Foundation). */
  windows?: string;
  /** Shows a live preview in the Board before saving. */
  preview?: boolean;
  /** Lives on the Actions (Alt) wheel rather than Tools (Ctrl). */
  action?: boolean;
}

export const GROUPS: Record<Group, { title: string; blurb: string }> = {
  'pdf-pages': { title: 'PDF · pages', blurb: 'Merge, split and pick pages, without touching the original.' },
  'pdf-edit': { title: 'PDF · edit', blurb: 'Stamp, crop, resize and lay out pages, with a live preview.' },
  'pdf-secure': { title: 'PDF · protect', blurb: 'Passwords, redaction, clean-up and digital signatures.' },
  'pdf-content': { title: 'PDF · content', blurb: 'OCR, forms, bookmarks, attachments and comparing versions.' },
  images: { title: 'Images', blurb: 'Resize, rotate, compress and strip location before sharing.' },
  office: { title: 'Word & Office', blurb: 'Clean, compress, find and replace, mail merge and PDF/A.' },
  video: { title: 'Video', blurb: 'A frame as a picture, or a clip squeezed under a size.' },
  files: { title: 'Any file', blurb: 'Archives, checksums, ZIP and copying text out of anything.' },
};

export const TOOLS: Tool[] = [
  { id: 'resize-half', label: 'Resize', caption: '50 %', group: 'images', text: 'Half the width and height, with a high-quality Lanczos filter.' },
  { id: 'fit-1920', label: 'Fit 1920', caption: 'Longest side', group: 'images', text: 'The longest side at most 1920 px. Smaller images are left as they are.' },
  { id: 'rotate-right', label: 'Rotate', caption: '90° right', group: 'images', text: 'Turns the image 90° clockwise.' },
  { id: 'flip', label: 'Flip', caption: 'Mirror', group: 'images', text: 'Mirrors the image horizontally.' },
  { id: 'compress', label: 'Compress', caption: 'Smaller file', group: 'images', text: 'JPEG at quality 72, PNG at maximum compression, and keeps whichever file is smaller.' },
  { id: 'strip-metadata', label: 'Strip info', caption: 'GPS, camera', group: 'images', text: 'Removes EXIF (GPS location, camera), XMP and IPTC. For JPEG it is lossless, segment by segment.' },
  { id: 'images-to-pdf', label: 'To one PDF', caption: 'Page each', group: 'images', text: 'Several images become one PDF, a page per image, in name order.' },
  { id: 'pdf-merge', label: 'Merge', caption: 'One PDF', group: 'pdf-pages', text: 'Several PDFs become one: in name order from the wheel, or in your order from the command line.' },
  { id: 'pdf-split', label: 'Split', caption: 'Page each', group: 'pdf-pages', text: 'One PDF per page, in a new folder. For a long PDF, the page picker opens first.' },
  { id: 'pdf-rotate', label: 'Rotate', caption: '90° right', group: 'pdf-pages', text: 'Turns pages 90° clockwise, losslessly (only the page attribute changes).' },
  { id: 'pdf-to-png', label: 'Pages PNG', caption: 'Image each', group: 'pdf-pages', text: 'One PNG per page (150 or 300 dpi), in a new folder.' },
  { id: 'pdf-to-jpg', label: 'Pages JPG', caption: 'Image each', group: 'pdf-pages', text: 'One JPG per page (150 or 300 dpi), in a new folder.' },
  { id: 'pdf-pages', label: 'Pages…', caption: 'Pick pages', group: 'pdf-pages', text: 'The page picker: click pages or type 1-3, 7, 10-, then save them as a new PDF, split, rotate, delete or export images. Organise mode reorders, duplicates and annotates pages.' },
  { id: 'extract', label: 'Extract', caption: 'New folder', group: 'files', text: 'Unpacks an archive into a new folder named after it, never merging into an existing one. 7Z, RAR, ISO and CAB through Windows’ own libarchive.' },
  { id: 'test-archive', label: 'Test', caption: 'Integrity', group: 'files', text: 'Reads every entry of an archive and reports files, size and errors.' },
  { id: 'checksum', label: 'Checksum', caption: 'SHA-256', group: 'files', text: 'The SHA-256 of each file, copied to the clipboard.' },
  { id: 'zip', label: 'To ZIP', caption: 'Compress', group: 'files', action: true, text: 'All the dragged files into one ZIP next to them.' },
  { id: 'copy-text', label: 'Copy text', caption: 'Clipboard', group: 'files', action: true, windows: 'OCR for photos and scans', text: 'The text of a document, slide deck, sheet or text file onto the clipboard. Photos and screenshots are read with Windows OCR.' },
  { id: 'pdf-compress', label: 'Compress', caption: 'Smaller PDF', group: 'pdf-edit', preview: true, text: 'Screen, print, email or smallest, or “fit under N MB”. Images are downsampled to their displayed size, photos become JPEG, diagrams stay lossless and duplicates merge.' },
  { id: 'pdf-numbers', label: 'Numbers', caption: 'Page 1 of 9', group: 'pdf-edit', preview: true, text: 'Page numbers as real text on the pages. Links and form fields keep working.' },
  { id: 'pdf-header-footer', label: 'Header', caption: 'Text on pages', group: 'pdf-edit', preview: true, text: 'A header and footer stamped on every page, aligned where you click.' },
  { id: 'pdf-bates', label: 'Bates', caption: 'Legal numbering', group: 'pdf-edit', preview: true, text: 'Bates numbers stamped on every page, for legal and discovery work.' },
  { id: 'pdf-watermark', label: 'Watermark', caption: 'Text or logo', group: 'pdf-edit', preview: true, text: 'Diagonal text or a picture, light to strong. Drag it into place on the preview and scroll to resize.' },
  { id: 'pdf-protect', label: 'Protect', caption: 'Password', group: 'pdf-secure', text: 'An AES-256 password, plus permissions for what others may do with the file.' },
  { id: 'pdf-unlock', label: 'Unlock', caption: 'Remove password', group: 'pdf-secure', text: 'Removes the password from a PDF you can open. A restricted file needs its owner password.' },
  { id: 'pdf-trim', label: 'Trim', caption: 'White margins', group: 'pdf-edit', preview: true, text: 'Cuts the white margins off every page, losslessly.' },
  { id: 'pdf-crop', label: 'Crop', caption: 'By margins', group: 'pdf-edit', preview: true, text: 'Draw the crop box on the page: every page at once, or just this one.' },
  { id: 'pdf-resize', label: 'Resize', caption: 'A4, Letter…', group: 'pdf-edit', preview: true, text: 'Pages to A4, Letter or another size; content and links scale together.' },
  { id: 'pdf-nup', label: 'N-up', caption: 'Pages per sheet', group: 'pdf-edit', preview: true, text: '2 to 9 pages per sheet.' },
  { id: 'pdf-booklet', label: 'Booklet', caption: 'Fold & staple', group: 'pdf-edit', preview: true, text: 'Saddle-stitch booklet order: print, fold, staple.' },
  { id: 'pdf-split-every', label: 'Split by', caption: 'Every N pages', group: 'pdf-pages', text: 'A new PDF every N pages, in a new folder.' },
  { id: 'pdf-split-ranges', label: 'Split ranges', caption: 'Your own groups', group: 'pdf-pages', text: 'Several PDFs from page groups you type or click: 1-15, 16; 25-40, 42. Groups may share pages.' },
  { id: 'pdf-split-chapters', label: 'Chapters', caption: 'Split at bookmarks', group: 'pdf-pages', text: 'A PDF per chapter, split at the bookmarks.' },
  { id: 'pdf-split-size', label: 'Split size', caption: 'Email-sized parts', group: 'pdf-pages', text: 'Parts that each stay under a size you choose.' },
  { id: 'pdf-clean', label: 'Clean', caption: 'Hidden data', group: 'pdf-secure', text: 'Removes author information, scripts, attachments and other hidden data.' },
  { id: 'pdf-info', label: 'Properties', caption: 'Title, author', group: 'pdf-content', text: 'Sets the title, author, subject and keywords; the viewer then shows the title.' },
  { id: 'pdf-repair', label: 'Repair', caption: 'Damaged PDF', group: 'pdf-content', text: 'Rebuilds a damaged PDF that other apps refuse to open.' },
  { id: 'pdf-bookmarks', label: 'Bookmarks', caption: 'From headings', group: 'pdf-content', text: 'Adds bookmarks that jump to each heading.' },
  { id: 'pdf-images', label: 'Pictures', caption: 'Save images', group: 'pdf-content', text: 'Saves every picture inside the PDF into a folder.' },
  { id: 'pdf-attachments', label: 'Attachments', caption: 'Save files', group: 'pdf-content', text: 'Saves the files attached inside a PDF.' },
  { id: 'pdf-attach', label: 'Attach', caption: 'Files into PDF', group: 'pdf-content', text: 'Puts other files inside the PDF as attachments.' },
  { id: 'pdf-searchable', label: 'Searchable', caption: 'OCR text layer', group: 'pdf-content', windows: 'Windows OCR', text: 'Scans get an invisible text layer: find, select and copy work, and the page looks exactly the same.' },
  { id: 'pdf-redact', label: 'Redact', caption: 'Remove found text', group: 'pdf-secure', preview: true, text: 'Words, phrases, emails or phone numbers removed for good: matching pages are flattened with black boxes, metadata and comments scrubbed, and the result re-read to check.' },
  { id: 'pdf-gray', label: 'Grayscale', caption: 'Black & white', group: 'pdf-edit', preview: true, text: 'Colours to gray; text stays text.' },
  { id: 'pdf-form-fields', label: 'Form fields', caption: 'To a spreadsheet', group: 'pdf-content', text: 'Every form field and its value, out to a spreadsheet.' },
  { id: 'pdf-fill', label: 'Fill form', caption: 'A PDF per row', group: 'pdf-content', text: 'Fills a PDF form from a spreadsheet: one filled PDF per row.' },
  { id: 'pdf-flatten', label: 'Flatten', caption: 'Fields into page', group: 'pdf-content', text: 'Draws form fields and comments into the page so they can’t be changed.' },
  { id: 'pdf-compare', label: 'Compare', caption: 'What changed', group: 'pdf-content', text: 'Two PDFs side by side, removed in red and added in green, word by word or by appearance for scans, with a summary page.' },
  { id: 'pdf-sign', label: 'Digital signature', caption: 'Your certificate', group: 'pdf-secure', windows: 'Windows certificate store', text: 'Signs with your certificate from the Windows certificate store, visibly or invisibly. Earlier signatures stay valid.' },
  { id: 'pdf-verify', label: 'Signatures', caption: 'Check them', group: 'pdf-secure', windows: 'the cryptographic check', text: 'Checks every signature: intact or changed afterwards, who signed and when, trusted or why not.' },
  { id: 'docx-compress', label: 'Compress', caption: 'Smaller file', group: 'office', text: 'Pictures in a Word document shrunk to their displayed size.' },
  { id: 'docx-clean', label: 'Clean', caption: 'Author, changes', group: 'office', text: 'Removes author information and comments, and accepts tracked changes.' },
  { id: 'docx-replace', label: 'Replace', caption: 'Find & replace', group: 'office', text: 'Find and replace, also across Word’s split runs, in headers, footers and notes.' },
  { id: 'docx-pictures', label: 'Pictures', caption: 'Save images', group: 'office', text: 'Saves every picture in the document into a folder.' },
  { id: 'docx-stats', label: 'Word count', caption: 'Clipboard', group: 'office', text: 'Words, characters, paragraphs and pages, copied to the clipboard.' },
  { id: 'docx-mail-merge', label: 'Mail merge', caption: 'Letter per row', group: 'office', text: 'A letter plus a spreadsheet: a DOCX (and PDF) for every row.' },
  { id: 'video-frame', label: 'Frame', caption: 'Save a picture', group: 'video', windows: 'Media Foundation (FFmpeg elsewhere)', text: 'A picture of one moment: type a time (1:23, 83, 40 %), or let it pick a good frame near the start. Or one every N seconds.' },
  { id: 'video-compress', label: 'Compress', caption: 'To a size', group: 'video', windows: 'Media Foundation (FFmpeg elsewhere)', text: 'Email 25 MB, chat 10 or 8 MB, upload 50 or 100 MB, or any size. H.264 + AAC, and a size limit is never crossed.' },
  { id: 'pdfa', label: 'PDF/A', caption: 'Archival PDF', group: 'office', text: 'Word, ODT, RTF, PowerPoint or Excel to archival PDF/A-2, tagged for screen readers.' },
];

export const ACTIONS = [
  { label: 'Show', caption: 'In folder', text: 'Opens File Explorer with the file selected.' },
  { label: 'Copy path', caption: 'Plain, quoted, /', text: 'Copies the path: plain, quoted, with forward slashes, or as a WSL /mnt/c/… path.' },
  { label: 'Terminal', caption: 'Here', text: 'Windows Terminal (or PowerShell) in the file’s folder.' },
  { label: 'To ZIP', caption: 'Compress', text: 'All dragged files into one ZIP next to them.' },
  { label: 'Board', caption: 'Open', text: 'Opens the dragged files in the Board.' },
  { label: 'Copy text', caption: 'Clipboard', text: 'The text of a file, or the text in an image (OCR), onto the clipboard.' },
];
