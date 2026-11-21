// The Swirvo mark, ported 1:1 from crates/render/src/icon.rs so the website draws exactly
// the same swirl as the app, the Store tile and the tray: a rounded square in the accent
// gradient with three white blades curling clockwise out of a centre dot.
// Design space: 256 × 256, y down.

export const GRADIENT = { top: '#F79A45', bottom: '#D9482B' } as const;

interface Spec {
  inset: number;
  radius: number;
  rIn: number;
  rOut: number;
  sweep: number;
  width: number;
  taper: number;
  dot: number;
}

const LARGE: Spec = { inset: 14, radius: 56, rIn: 26, rOut: 92, sweep: 128, width: 66, taper: 0.85, dot: 15 };
const SMALL: Spec = { inset: 6, radius: 44, rIn: 14, rOut: 100, sweep: 112, width: 84, taper: 0.6, dot: 0 };

export type Pt = [number, number];

const rad = (d: number) => (d * Math.PI) / 180;

/** One blade as a closed polygon (leading edge, tip arc, trailing edge). */
export function blade(k: number, small = false, steps = 36): Pt[] {
  const s = small ? SMALL : LARGE;
  const at = (a: number, r: number): Pt => [128 + r * Math.cos(rad(a)), 128 + r * Math.sin(rad(a))];
  const radius = (t: number) => s.rIn + (s.rOut - s.rIn) * Math.pow(t, 0.85);
  const width = (t: number) => s.width * Math.pow(t, s.taper);
  const base = -90 + 120 * k;
  const pts: Pt[] = [];
  for (let i = 0; i <= steps; i++) {
    const t = i / steps;
    pts.push(at(base + s.sweep * t, radius(t)));
  }
  const a1 = base + s.sweep;
  const a0 = a1 - width(1);
  const arcSteps = 14;
  for (let i = 1; i < arcSteps; i++) {
    const a = a1 + ((a0 - a1) * i) / arcSteps;
    pts.push(at(a, s.rOut));
  }
  for (let i = steps; i >= 0; i--) {
    const t = i / steps;
    pts.push(at(base + s.sweep * t - width(t), radius(t)));
  }
  return pts;
}

export function dotRadius(small = false) {
  return (small ? SMALL : LARGE).dot;
}

export function tile(small = false) {
  const s = small ? SMALL : LARGE;
  return { x: s.inset, y: s.inset, size: 256 - 2 * s.inset, radius: s.radius };
}

const f = (n: number) => n.toFixed(2);

export function bladePath(k: number, small = false): string {
  const p = blade(k, small);
  return 'M' + p.map(([x, y]) => `${f(x)} ${f(y)}`).join('L') + 'Z';
}

export function tilePath(small = false): string {
  const { x, y, size, radius: r } = tile(small);
  const x1 = x + size;
  const y1 = y + size;
  return `M${x + r} ${y}H${x1 - r}A${r} ${r} 0 0 1 ${x1} ${y + r}V${y1 - r}A${r} ${r} 0 0 1 ${x1 - r} ${y1}H${x + r}A${r} ${r} 0 0 1 ${x} ${y1 - r}V${y + r}A${r} ${r} 0 0 1 ${x + r} ${y}Z`;
}

/** The whole icon as an SVG string (for favicons and social cards). */
export function iconSvg(small = false): string {
  const blades = [0, 1, 2].map((k) => `<path d="${bladePath(k, small)}" fill="#fff"/>`).join('');
  const dot = dotRadius(small) ? `<circle cx="128" cy="128" r="${dotRadius(small)}" fill="#fff"/>` : '';
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="${GRADIENT.top}"/><stop offset="1" stop-color="${GRADIENT.bottom}"/></linearGradient></defs><path d="${tilePath(small)}" fill="url(#g)"/>${blades}${dot}</svg>`;
}
