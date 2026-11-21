// Repository Markdown (changelog, legal texts) links to files relative to the repo root.
// On the website those become site pages where one exists, or GitHub links.
const REPO = 'https://github.com/Nabarup1/swirvo/blob/main/';
const PAGES: Record<string, string> = {
  'LICENSE.md': '/license',
  'EULA.md': '/terms',
  'PRIVACY.md': '/privacy',
  'TRADEMARKS.md': '/trademarks',
  'CHANGELOG.md': '/changelog',
  'docs/19-ai-assistants-mcp.md': '/ai',
};

export function fixLinks(html: string): string {
  return html
    .replace(/href="(?!https?:|#|mailto:|\/)([^"]+)"/g, (_m, href: string) => {
      const [path, hash] = href.split('#');
      const clean = path.replace(/^\.\//, '');
      const page = PAGES[clean];
      const url = page ?? REPO + clean;
      return `href="${url}${hash ? '#' + hash : ''}"`;
    })
    .replace(/<a href="https?:\/\/(?!swirvo)/g, '<a target="_blank" rel="noopener" href="https://')
    .replace(/<table>/g, '<div class="table-wrap"><table>')
    .replace(/<\/table>/g, '</table></div>');
}

/** Headings (h2/h3) with ids, for a table of contents. */
export function headings(html: string) {
  const out: { depth: number; slug: string; text: string }[] = [];
  for (const m of html.matchAll(/<h([23]) id="([^"]+)">(.*?)<\/h\1>/g)) {
    out.push({ depth: Number(m[1]), slug: m[2], text: m[3].replace(/<[^>]+>/g, '') });
  }
  return out;
}
