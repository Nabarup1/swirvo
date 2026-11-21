// Release facts fetched once at build time, so visitors' browsers never call GitHub.
// When the API can't be reached (offline builds), the version from Cargo.toml is used
// and sizes are left out.
import { VERSION, assets, assetUrl } from './site';

export interface Asset {
  name: string;
  url: string;
  size?: number;
  sha256?: string;
}
export interface Release {
  version: string;
  date?: string;
  assets: Asset[];
  stars?: number;
  live: boolean;
}

let cached: Promise<Release> | null = null;

async function get(url: string) {
  const res = await fetch(url, {
    headers: {
      Accept: 'application/vnd.github+json',
      'User-Agent': 'swirvo-website-build',
      ...(process.env.GITHUB_TOKEN ? { Authorization: `Bearer ${process.env.GITHUB_TOKEN}` } : {}),
    },
    signal: AbortSignal.timeout(8000),
  });
  if (!res.ok) throw new Error(`${res.status} ${url}`);
  return res.json();
}

function fallback(): Release {
  const a = assets(VERSION);
  return {
    version: VERSION,
    live: false,
    assets: Object.values(a).map((name) => ({ name, url: assetUrl(name) })),
  };
}

export function release(): Promise<Release> {
  cached ??= (async () => {
    try {
      const [rel, repo] = await Promise.all([
        get('https://api.github.com/repos/Nabarup1/swirvo/releases/latest'),
        get('https://api.github.com/repos/Nabarup1/swirvo').catch(() => null),
      ]);
      const version = String(rel.tag_name ?? '').replace(/^v/, '') || VERSION;
      const list: Asset[] = (rel.assets ?? [])
        .filter((x: { name: string }) => !x.name.endsWith('.sha256'))
        .map((x: { name: string; browser_download_url: string; size: number; digest?: string }) => ({
          name: x.name,
          url: x.browser_download_url,
          size: x.size,
          sha256: x.digest?.startsWith('sha256:') ? x.digest.slice(7) : undefined,
        }));
      return {
        version,
        date: rel.published_at,
        assets: list.length ? list : fallback().assets,
        stars: repo?.stargazers_count,
        live: true,
      };
    } catch (e) {
      console.warn(`github: using offline release data (${(e as Error).message})`);
      return fallback();
    }
  })();
  return cached;
}

export const mb = (n?: number) => (n ? `${(n / 1_000_000).toFixed(1)} MB` : '');

export function find(r: Release, test: (name: string) => boolean) {
  return r.assets.find((a) => test(a.name));
}
