// Writes public/credits.txt: the licences of the open-source code and fonts this website
// ships to visitors (Three.js, Astro's runtime, and four OFL font families).
import { readFileSync, writeFileSync, existsSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const site = join(dirname(fileURLToPath(import.meta.url)), '..');
const parts = [
  ['Three.js (3D logo on the homepage)', 'three'],
  ['Astro (site framework and its small runtime)', 'astro'],
  ['Bricolage Grotesque (font)', '@fontsource-variable/bricolage-grotesque'],
  ['Instrument Sans (font)', '@fontsource-variable/instrument-sans'],
  ['Instrument Serif (font)', '@fontsource/instrument-serif'],
  ['JetBrains Mono (font)', '@fontsource-variable/jetbrains-mono'],
];

let out = `Open-source software and fonts used by the Swirvo website
=========================================================

The Swirvo app's own open-source notices are in THIRD-PARTY-NOTICES.txt, which comes with
every download. This file covers only what this website sends to your browser.
The "Get it from Microsoft" badge is Microsoft's official artwork, used as Microsoft's
Store badge guidelines allow.

`;
for (const [title, pkg] of parts) {
  const dir = join(site, 'node_modules', pkg);
  const lic = ['LICENSE', 'LICENSE.md', 'LICENSE.txt'].map((f) => join(dir, f)).find(existsSync);
  const version = JSON.parse(readFileSync(join(dir, 'package.json'), 'utf8')).version;
  out += `\n${'-'.repeat(78)}\n${title}\n${pkg} ${version}\n${'-'.repeat(78)}\n\n`;
  out += lic ? readFileSync(lic, 'utf8').trim() + '\n' : '(licence file not found)\n';
}
writeFileSync(join(site, 'public', 'credits.txt'), out);
console.log(`credits: ${parts.length} entries`);
