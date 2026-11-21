// Copies the pieces of the Swirvo repository the website shows (changelog, legal texts,
// the Linux installer, llms.txt, screenshots and the demo video) into the website, so
// there is one source of truth. When the website is built on its own (no parent
// repository), the copies already committed here are used as they are.
import { copyFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const site = join(dirname(fileURLToPath(import.meta.url)), '..');
const repo = join(site, '..');

const copies = [
  ['CHANGELOG.md', 'src/generated/CHANGELOG.md'],
  ['PRIVACY.md', 'src/generated/PRIVACY.md'],
  ['EULA.md', 'src/generated/EULA.md'],
  ['LICENSE.md', 'src/generated/LICENSE.md'],
  ['TRADEMARKS.md', 'src/generated/TRADEMARKS.md'],
  ['install.sh', 'public/install.sh'],
  ['llms.txt', 'public/llms.txt'],
  ['media/swirvo-demo.mp4', 'public/media/swirvo-demo.mp4'],
  ['media/wheel.png', 'public/media/wheel.png'],
  ['media/board.png', 'public/media/board.png'],
  ['media/preview.png', 'public/media/preview.png'],
  ['packaging/store/screenshots/01.png', 'public/media/screen-wheel.png'],
  ['packaging/store/screenshots/02.png', 'public/media/screen-board.png'],
  ['packaging/store/screenshots/03.png', 'public/media/screen-pages.png'],
  ['packaging/store/screenshots/04.png', 'public/media/screen-preview.png'],
  ['packaging/store/screenshots/05.png', 'public/media/screen-redact.png'],
  ['packaging/icon/png/swirvo-512.png', 'public/icons/swirvo-512.png'],
  ['packaging/icon/png/swirvo-256.png', 'public/icons/swirvo-256.png'],
];

if (!existsSync(join(repo, 'Cargo.toml'))) {
  console.log('sync: no parent repository; using the committed copies');
  process.exit(0);
}

let n = 0;
for (const [from, to] of copies) {
  const src = join(repo, from);
  if (!existsSync(src)) continue;
  const dst = join(site, to);
  mkdirSync(dirname(dst), { recursive: true });
  copyFileSync(src, dst);
  n++;
}

const cargo = readFileSync(join(repo, 'Cargo.toml'), 'utf8');
const version = cargo.match(/^version\s*=\s*"([^"]+)"/m)?.[1] ?? '0.0.0';
mkdirSync(join(site, 'src/generated'), { recursive: true });
writeFileSync(join(site, 'src/generated/version.json'), JSON.stringify({ version }, null, 2) + '\n');
console.log(`sync: ${n} files from the repository, version ${version}`);
