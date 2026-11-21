import versionFile from '../generated/version.json';

export const VERSION: string = versionFile.version;

export const LINKS = {
  store: 'https://apps.microsoft.com/detail/9NPGWFBVTDJQ',
  storeId: '9NPGWFBVTDJQ',
  github: 'https://github.com/Nabarup1/swirvo',
  releases: 'https://github.com/Nabarup1/swirvo/releases',
  latest: 'https://github.com/Nabarup1/swirvo/releases/latest',
  issues: 'https://github.com/Nabarup1/swirvo/issues',
  newIssue: 'https://github.com/Nabarup1/swirvo/issues/new',
  contributing: 'https://github.com/Nabarup1/swirvo/blob/main/CONTRIBUTING.md',
  security: 'https://github.com/Nabarup1/swirvo/blob/main/SECURITY.md',
  mcpDocs: 'https://github.com/Nabarup1/swirvo/blob/main/docs/19-ai-assistants-mcp.md',
  registry: 'https://registry.modelcontextprotocol.io',
  author: 'https://github.com/Nabarup1',
  installSh: 'https://raw.githubusercontent.com/Nabarup1/swirvo/main/install.sh',
  editDocs: 'https://github.com/Nabarup1/swirvo/edit/main/website/src/content/docs/',
} as const;

export const INSTALL_LINUX = `curl -fsSL ${LINKS.installSh} | sh`;

/** Release file names for a version. */
export function assets(v: string = VERSION) {
  return {
    windowsZip: `Swirvo-${v}-windows-x64.zip`,
    mcpb: `Swirvo-${v}.mcpb`,
    linuxX64: `swirvo-cli-${v}-linux-x64.tar.gz`,
    linuxArm64: `swirvo-cli-${v}-linux-arm64.tar.gz`,
    debAmd64: `swirvo_${v}_amd64.deb`,
    debArm64: `swirvo_${v}_arm64.deb`,
  };
}

export const assetUrl = (name: string, v: string = VERSION) =>
  `${LINKS.releases}/download/v${v}/${name}`;

export const NAV = [
  { href: '/#features', label: 'Features' },
  { href: '/formats', label: 'Formats' },
  { href: '/tools', label: 'Tools' },
  { href: '/ai', label: 'AI' },
  { href: '/docs/getting-started', label: 'Docs', match: '/docs' },
  { href: '/changelog', label: 'Changelog' },
];

export const FOOTER = [
  {
    title: 'Product',
    links: [
      { href: '/download', label: 'Download' },
      { href: '/formats', label: 'Formats' },
      { href: '/tools', label: 'Tools' },
      { href: '/ai', label: 'For AI assistants' },
      { href: '/changelog', label: 'Changelog' },
    ],
  },
  {
    title: 'Docs',
    links: [
      { href: '/docs/getting-started', label: 'Getting started' },
      { href: '/docs/wheel', label: 'The wheel' },
      { href: '/docs/board', label: 'The Board' },
      { href: '/docs/command-line', label: 'Command line' },
      { href: '/docs/troubleshooting', label: 'Troubleshooting' },
    ],
  },
  {
    title: 'Project',
    links: [
      { href: LINKS.github, label: 'GitHub', external: true },
      { href: LINKS.issues, label: 'Issues & ideas', external: true },
      { href: LINKS.releases, label: 'All releases', external: true },
      { href: LINKS.contributing, label: 'Contributing', external: true },
      { href: LINKS.security, label: 'Security', external: true },
    ],
  },
  {
    title: 'Legal',
    links: [
      { href: '/privacy', label: 'Privacy' },
      { href: '/terms', label: 'App licence (EULA)' },
      { href: '/license', label: 'Source licence' },
      { href: '/trademarks', label: 'Trademarks' },
      { href: '/brand', label: 'Brand' },
      { href: '/credits.txt', label: 'Open-source credits' },
    ],
  },
];
