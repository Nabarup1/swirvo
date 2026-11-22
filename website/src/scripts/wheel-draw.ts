// Draws the Swirvo wheel as SVG, using the app's own geometry (docs/02 §3.1): outer
// radius 156, hub 50, wedge band 56 → 156, 3 px gaps, preset sub-ring 164 → 220, slot 0
// at 12 o'clock running clockwise, labels at 0.64 R.

export interface WItem {
  label: string;
  caption: string;
  more?: boolean;
}
export type WheelKind = 'convert' | 'tools' | 'actions';

export const G = { R: 156, r0: 50, rIn: 56, subIn: 164, subOut: 220, gap: 3, label: 0.64 };
const NS = 'http://www.w3.org/2000/svg';

/** Point at `deg` clockwise from 12 o'clock. */
export const pt = (deg: number, r: number): [number, number] => {
  const a = (deg * Math.PI) / 180;
  return [r * Math.sin(a), -r * Math.cos(a)];
};
const f = (n: number) => n.toFixed(2);

/** An annular sector between two angles, with a constant-width gap on each side. */
export function sector(a0: number, a1: number, rIn: number, rOut: number, gap = G.gap): string {
  const dOut = ((gap / 2 / rOut) * 180) / Math.PI;
  const dIn = ((gap / 2 / rIn) * 180) / Math.PI;
  const [x0, y0] = pt(a0 + dOut, rOut);
  const [x1, y1] = pt(a1 - dOut, rOut);
  const [x2, y2] = pt(a1 - dIn, rIn);
  const [x3, y3] = pt(a0 + dIn, rIn);
  const large = a1 - a0 > 180 ? 1 : 0;
  return `M${f(x0)} ${f(y0)}A${rOut} ${rOut} 0 ${large} 1 ${f(x1)} ${f(y1)}L${f(x2)} ${f(y2)}A${rIn} ${rIn} 0 ${large} 0 ${f(x3)} ${f(y3)}Z`;
}

export const span = (n: number) => 360 / n;
export const centre = (i: number, n: number) => i * span(n);

/** Which wedge an angle (clockwise from 12) falls in. */
export function slotAt(deg: number, n: number) {
  const s = span(n);
  return Math.floor((((deg + s / 2) % 360) + 360) % 360 / s) % n;
}

/** Angle clockwise from 12 o'clock for a vector. */
export const angleOf = (dx: number, dy: number) => ((Math.atan2(dx, -dy) * 180) / Math.PI + 360) % 360;

function el<K extends keyof SVGElementTagNameMap>(tag: K, attrs: Record<string, string | number> = {}, parent?: Element) {
  const e = document.createElementNS(NS, tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, String(v));
  parent?.append(e);
  return e;
}

const TABS: [WheelKind, string, string][] = [
  ['convert', '⇧', 'Convert'],
  ['tools', 'Ctrl', 'Tools'],
  ['actions', 'Alt', 'Actions'],
];

export interface DrawnWheel {
  root: SVGGElement;
  wedges: SVGGElement[];
  sub: SVGGElement;
  tabs: SVGGElement;
}

/** Builds a wheel group (centred at 0,0) into `parent`. */
export function drawWheel(
  parent: SVGElement,
  items: WItem[],
  hub: { name: string; meta: string },
  kind: WheelKind
): DrawnWheel {
  const root = el('g', { class: `wh wh-${kind}` }, parent);
  el('circle', { class: 'wh-shadow', r: G.R + 10 }, root);
  el('circle', { class: 'wh-disc', r: G.R + 2 }, root);
  const n = items.length;
  const wedges = items.map((it, i) => {
    const g = el('g', { class: `wh-w${it.more ? ' more' : ''}`, 'data-i': i }, root);
    const c = centre(i, n);
    const [dx, dy] = pt(c, 4);
    g.style.setProperty('--dx', `${f(dx)}px`);
    g.style.setProperty('--dy', `${f(dy)}px`);
    g.style.setProperty('--i', String(i));
    el('path', { class: 'wh-fill', d: sector(c - span(n) / 2, c + span(n) / 2, G.rIn, G.R) }, g);
    const [lx, ly] = pt(c, G.R * G.label);
    const t = el('text', { class: 'wh-label', x: f(lx), y: f(ly - 3) }, g);
    t.textContent = it.label;
    const cap = el('text', { class: 'wh-cap', x: f(lx), y: f(ly + 13) }, g);
    cap.textContent = it.caption;
    return g;
  });
  // Hub
  const hubG = el('g', { class: 'wh-hub' }, root);
  el('circle', { r: G.r0 }, hubG);
  el('circle', { class: 'wh-hub-ring', r: G.r0 - 6 }, hubG);
  const name = hub.name.length > 12 ? hub.name.slice(0, 10) + '…' : hub.name;
  const hn = el('text', { class: 'wh-hub-name', y: -2 }, hubG);
  hn.textContent = name;
  const hm = el('text', { class: 'wh-hub-meta', y: 14 }, hubG);
  hm.textContent = hub.meta;
  // Preset sub-ring (filled in when a wedge is hovered)
  const sub = el('g', { class: 'wh-sub' }, root);
  // Tab pill
  const tabs = el('g', { class: 'wh-tabs', transform: `translate(0 ${-(G.R + 30)})` }, root);
  el('rect', { class: 'wh-tabs-bg', x: -132, y: -15, width: 264, height: 30, rx: 15 }, tabs);
  TABS.forEach(([k, key, label], i) => {
    const x = -88 + i * 88;
    const tg = el('g', { class: `wh-tab${k === kind ? ' on' : ''}`, transform: `translate(${x} 0)` }, tabs);
    el('rect', { x: -42, y: -11, width: 84, height: 22, rx: 11 }, tg);
    const tt = el('text', { y: 4 }, tg);
    tt.textContent = `${key} ${label}`;
  });
  return { root, wedges, sub, tabs };
}

/** The slimmer preset ring for narrow screens; its words run along the arc to fit. */
export const COMPACT_SUB_OUT = 198;

/** Draws the Best · Balanced · Smallest ring outside wedge `i`. */
export function drawSub(w: DrawnWheel, i: number, n: number, labels: string[], subOut = G.subOut) {
  w.sub.replaceChildren();
  const c = centre(i, n);
  const total = Math.max(span(n), 96);
  const step = total / labels.length;
  labels.forEach((lab, k) => {
    const a0 = c - total / 2 + k * step;
    const g = el('g', { class: 'wh-p', 'data-k': k }, w.sub);
    g.style.setProperty('--k', String(k));
    el('path', { d: sector(a0, a0 + step, G.subIn, subOut) }, g);
    const mid = a0 + step / 2;
    const [x, y] = pt(mid, (G.subIn + subOut) / 2);
    let at: Record<string, string> = { x: f(x), y: f(y + 4) };
    if (subOut < G.subOut) {
      const a = ((mid % 360) + 360) % 360;
      const upright = a > 90 && a < 270 ? a + 180 : a;
      at = { y: '4', transform: `translate(${f(x)} ${f(y)}) rotate(${f(upright)})` };
    }
    const t = el('text', at, g);
    t.textContent = lab;
  });
  return { a0: c - total / 2, step, count: labels.length };
}
