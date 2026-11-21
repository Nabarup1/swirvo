// The search index for the Ctrl+K palette: pages, every docs section, every tool and
// every format. Built once at build time; fetched only when the palette opens.
import type { APIRoute } from 'astro';
import { getCollection, render } from 'astro:content';
import { TOOLS, GROUPS } from '../data/tools';
import { FORMATS, CATEGORIES } from '../data/formats';

export const GET: APIRoute = async () => {
  const items: { t: string; s?: string; u: string; k: string; x?: string }[] = [
    { t: 'Home', s: 'Drag. Tap. Convert.', u: '/', k: 'Page' },
    { t: 'Download', s: 'Microsoft Store, ZIP, Linux, checksums', u: '/download', k: 'Page', x: 'install windows linux deb store zip mcpb' },
    { t: 'Every format', s: 'What each format can become', u: '/formats', k: 'Page', x: 'formats conversions' },
    { t: 'All 59 tools', s: 'With command-line ids', u: '/tools', k: 'Page', x: 'tools' },
    { t: 'For AI assistants', s: 'MCP server for Claude, Cursor, VS Code', u: '/ai', k: 'Page', x: 'mcp claude cursor vscode ai agent' },
    { t: 'Changelog', s: 'What’s new in each version', u: '/changelog', k: 'Page', x: 'release notes version' },
    { t: 'Privacy policy', u: '/privacy', k: 'Page', x: 'privacy data tracking' },
    { t: 'App licence (EULA)', u: '/terms', k: 'Page', x: 'eula terms licence license free commercial' },
    { t: 'Source licence', u: '/license', k: 'Page', x: 'polyform noncommercial open source code' },
    { t: 'Trademarks', u: '/trademarks', k: 'Page', x: 'logo name brand' },
    { t: 'Brand', u: '/brand', k: 'Page', x: 'logo colours colors fonts' },
  ];
  const docs = await getCollection('docs');
  for (const d of docs) {
    items.push({ t: d.data.title, s: d.data.description, u: `/docs/${d.id}`, k: 'Docs', x: d.body?.slice(0, 600).replace(/[#*`|>\-\[\]()]/g, ' ') });
    const { headings } = await render(d);
    for (const h of headings.filter((h) => h.depth === 2)) {
      items.push({ t: h.text, s: d.data.title, u: `/docs/${d.id}#${h.slug}`, k: 'Section' });
    }
  }
  for (const t of TOOLS) {
    items.push({ t: `${t.label} · ${t.caption}`, s: t.text, u: `/tools#${t.id}`, k: 'Tool', x: `${t.id} ${GROUPS[t.group].title}` });
  }
  for (const f of FORMATS) {
    items.push({
      t: f.key.toUpperCase(),
      s: `${f.name} · ${f.read === 'planned' ? 'planned' : `to ${f.to.slice(0, 8).map(([k]) => k.toUpperCase()).join(', ')}`}`,
      u: `/formats?from=${encodeURIComponent(f.key)}`,
      k: 'Format',
      x: `${f.name} ${CATEGORIES[f.cat].title}`,
    });
  }
  return new Response(JSON.stringify(items), { headers: { 'Content-Type': 'application/json' } });
};
