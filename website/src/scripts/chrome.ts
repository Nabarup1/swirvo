// Header behaviour: theme switch, mobile menu, and the nav pill that springs between links.
import { Spring, onTick, reduced, finePointer } from './motion';

const root = document.documentElement;

/* ── Theme: dark → light → system ─────────────────────── */
const ORDER = ['dark', 'light', 'system'] as const;
type Pref = (typeof ORDER)[number];
function applyTheme(pref: Pref) {
  const light = pref === 'light' || (pref === 'system' && matchMedia('(prefers-color-scheme: light)').matches);
  root.dataset.theme = light ? 'light' : 'dark';
  root.dataset.themePref = pref;
  document.querySelector('meta[data-theme-color]')?.setAttribute('content', light ? '#f7f1e9' : '#0c0a0e');
  document.querySelectorAll('[data-theme-toggle]').forEach((b) =>
    b.setAttribute('aria-label', `Theme: ${pref}. Switch theme`)
  );
  try {
    localStorage.setItem('swirvo-theme', pref);
  } catch {}
  document.dispatchEvent(new CustomEvent('themechange', { detail: { light } }));
}
document.querySelectorAll<HTMLElement>('[data-theme-toggle]').forEach((btn) =>
  btn.addEventListener('click', (e) => {
    const cur = (root.dataset.themePref as Pref) || 'dark';
    const next = ORDER[(ORDER.indexOf(cur) + 1) % ORDER.length];
    const swap = () => applyTheme(next);
    // Ink spreads from the button when the browser supports same-page view transitions.
    const start = (document as Document & { startViewTransition?: (cb: () => void) => { ready: Promise<void> } }).startViewTransition;
    if (!start || reduced) return swap();
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const x = r.left + r.width / 2;
    const y = r.top + r.height / 2;
    const rad = Math.hypot(Math.max(x, innerWidth - x), Math.max(y, innerHeight - y));
    root.classList.add('theme-vt');
    const t = start.call(document, swap);
    t.ready
      .then(() =>
        root.animate(
          { clipPath: [`circle(0px at ${x}px ${y}px)`, `circle(${rad}px at ${x}px ${y}px)`] },
          { duration: 650, easing: 'cubic-bezier(0.16, 1, 0.3, 1)', pseudoElement: '::view-transition-new(root)' }
        ).finished
      )
      .finally(() => root.classList.remove('theme-vt'));
  })
);
matchMedia('(prefers-color-scheme: light)').addEventListener('change', () => {
  if (root.dataset.themePref === 'system') applyTheme('system');
});

/* ── Mobile menu ──────────────────────────────────────── */
const menuBtn = document.querySelector<HTMLButtonElement>('[data-menu-open]');
const menu = document.querySelector<HTMLElement>('[data-menu]');
function setMenu(open: boolean) {
  root.classList.toggle('menu-open', open);
  menuBtn?.setAttribute('aria-expanded', String(open));
  menuBtn?.setAttribute('aria-label', open ? 'Close menu' : 'Menu');
  document.querySelector('.site-header')?.classList.remove('tucked');
  document.body.style.overflow = open ? 'hidden' : '';
  if (open) menu?.querySelector<HTMLElement>('a')?.focus({ preventScroll: true });
}
menuBtn?.addEventListener('click', () => setMenu(!root.classList.contains('menu-open')));
menu?.addEventListener('click', (e) => {
  if ((e.target as HTMLElement).closest('a, [data-search-open]')) setMenu(false);
});
addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && root.classList.contains('menu-open')) {
    setMenu(false);
    menuBtn?.focus();
  }
});
matchMedia('(min-width: 961px)').addEventListener('change', (m) => m.matches && setMenu(false));

/* ── Nav pill ─────────────────────────────────────────── */
const nav = document.querySelector<HTMLElement>('[data-nav]');
const pill = document.querySelector<HTMLElement>('.nav-pill');
if (nav && pill) {
  const x = new Spring(0, 320, 28);
  const w = new Spring(0, 320, 28);
  const links = [...nav.querySelectorAll<HTMLAnchorElement>('a')];
  const active = links.find((a) => a.classList.contains('active'));
  let stop: (() => void) | null = null;
  const draw = () => {
    pill.style.transform = `translateX(${x.value.toFixed(2)}px)`;
    pill.style.width = `${Math.max(0, w.value).toFixed(2)}px`;
  };
  const go = (a: HTMLElement | undefined, instant = false) => {
    if (!a) {
      pill.classList.remove('on');
      return;
    }
    const base = nav.getBoundingClientRect().left;
    const r = a.getBoundingClientRect();
    x.target = r.left - base;
    w.target = r.width;
    pill.classList.add('on');
    if (instant || reduced || !pill.style.width) {
      x.value = x.target;
      w.value = w.target;
      x.velocity = w.velocity = 0;
      draw();
      return;
    }
    if (stop) return;
    stop = onTick((dt) => {
      x.step(dt);
      w.step(dt);
      draw();
      if (x.settled && w.settled) {
        stop = null;
        return false;
      }
    });
  };
  requestAnimationFrame(() => go(active, true));
  if (finePointer) links.forEach((a) => a.addEventListener('pointerenter', () => go(a)));
  links.forEach((a) => a.addEventListener('focus', () => go(a)));
  nav.addEventListener('pointerleave', () => go(active));
  addEventListener('resize', () => go(active, true));
}
