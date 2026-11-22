// @ts-check
import { defineConfig } from 'astro/config';
import sitemap from '@astrojs/sitemap';

// The public address, used for canonical links, the sitemap and social cards.
// Set SITE_URL in Vercel once the domain is final (Project → Settings → Environment Variables).
const site = process.env.SITE_URL || 'https://swirvo.vercel.app';

export default defineConfig({
  site,
  // Astro 7's default ('jsx') drops a line break between text and a tag, gluing words
  // to the links and keycaps after them ("formats.Ctrl").
  compressHTML: true,
  vite: {
    build: {
      // Small font subsets would become data: URIs, which our CSP (font-src 'self') blocks.
      assetsInlineLimit: (file) => (/\.woff2?$/.test(file) ? false : undefined),
    },
  },
  integrations: [sitemap()],
  prefetch: { prefetchAll: true, defaultStrategy: 'hover' },
  markdown: {
    shikiConfig: { theme: 'css-variables', wrap: false },
  },
  server: { port: 4321, host: true },
  devToolbar: { enabled: false },
});
