// @ts-check
import { defineConfig } from 'astro/config';
import sitemap from '@astrojs/sitemap';

// The public address, used for canonical links, the sitemap and social cards.
// Set SITE_URL in Vercel once the domain is final (Project → Settings → Environment Variables).
const site = process.env.SITE_URL || 'https://swirvo.vercel.app';

export default defineConfig({
  site,
  integrations: [sitemap()],
  prefetch: { prefetchAll: true, defaultStrategy: 'hover' },
  markdown: {
    shikiConfig: { theme: 'css-variables', wrap: false },
  },
  server: { port: 4321, host: true },
  devToolbar: { enabled: false },
});
