// The site's motion layer. One rAF loop, springs with real physics, and attribute-driven
// behaviours so pages stay plain HTML:
//   data-reveal, data-split, data-stagger, data-magnetic, data-tilt, data-count,
//   data-parallax, data-marquee, data-copy.
// Everything respects prefers-reduced-motion.

export const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;
export const finePointer = matchMedia('(pointer: fine)').matches;
const root = document.documentElement;
root.classList.remove('no-js');
if (reduced) root.classList.add('reduce');

/* ── A tiny spring integrator ─────────────────────────── */
export class Spring {
  value: number;
  target: number;
  velocity = 0;
  constructor(v = 0, public stiffness = 170, public damping = 20, public mass = 1) {
    this.value = v;
    this.target = v;
  }
  step(dt: number) {
    const f = -this.stiffness * (this.value - this.target) - this.damping * this.velocity;
    this.velocity += (f / this.mass) * dt;
    this.value += this.velocity * dt;
    return this;
  }
  get settled() {
    return Math.abs(this.value - this.target) < 0.001 && Math.abs(this.velocity) < 0.001;
  }
}

type Tick = (dt: number, t: number) => boolean | void;
const ticks = new Set<Tick>();
let last = 0;
let running = false;
function frame(t: number) {
  const dt = Math.min(0.05, (t - (last || t)) / 1000) || 1 / 60;
  last = t;
  for (const fn of [...ticks]) if (fn(dt, t) === false) ticks.delete(fn);
  if (ticks.size) requestAnimationFrame(frame);
  else {
    running = false;
    last = 0;
  }
}
/** Run `fn` every frame until it returns false. */
export function onTick(fn: Tick) {
  ticks.add(fn);
  if (!running) {
    running = true;
    requestAnimationFrame(frame);
  }
  return () => ticks.delete(fn);
}

/* ── Toasts ───────────────────────────────────────────── */
export function toast(text: string, icon: 'check' | 'info' = 'check') {
  let host = document.querySelector<HTMLElement>('.toasts');
  if (!host) {
    host = document.createElement('div');
    host.className = 'toasts';
    host.setAttribute('role', 'status');
    host.setAttribute('aria-live', 'polite');
    document.body.append(host);
  }
  const el = document.createElement('div');
  el.className = 'toast';
  el.innerHTML = `<span class="toast-ic ${icon}"></span><span></span>`;
  (el.lastElementChild as HTMLElement).textContent = text;
  host.append(el);
  setTimeout(() => {
    el.classList.add('out');
    el.addEventListener('animationend', () => el.remove(), { once: true });
  }, 2200);
}

/* ── Split headings into words ────────────────────────── */
function split(el: HTMLElement) {
  if (el.dataset.splitDone) return;
  el.dataset.splitDone = '1';
  let i = 0;
  // `inside` is the emphasis around a word: each word gets its own copy, so a phrase like
  // <span class="serif">same engines.</span> can still wrap on a narrow screen.
  const wrap = (node: Node, inside: HTMLElement[] = []): Node[] => {
    if (node.nodeType === Node.TEXT_NODE) {
      const parts = (node.textContent ?? '').split(/(\s+)/);
      return parts.filter(Boolean).map((p) => {
        if (/^\s+$/.test(p)) return document.createTextNode(' ');
        const w = document.createElement('span');
        w.className = 'w';
        const inner = document.createElement('span');
        inner.style.setProperty('--i', String(i++));
        let content: Node = document.createTextNode(p);
        for (let k = inside.length - 1; k >= 0; k--) {
          const copy = inside[k].cloneNode(false);
          copy.appendChild(content);
          content = copy;
        }
        inner.append(content);
        w.append(inner);
        return w;
      });
    }
    if (node.nodeType === Node.ELEMENT_NODE) {
      const e = node as HTMLElement;
      if (e.tagName === 'BR') return [e];
      if (!e.querySelector('svg, img') && !(e instanceof SVGElement)) {
        return [...e.childNodes].flatMap((c) => wrap(c, [...inside, e]));
      }
      const w = document.createElement('span');
      w.className = 'w';
      const inner = document.createElement('span');
      inner.style.setProperty('--i', String(i++));
      inner.append(e.cloneNode(true));
      w.append(inner);
      return [w];
    }
    return [];
  };
  const kids = [...el.childNodes];
  el.replaceChildren(...kids.flatMap((k) => wrap(k)));
  el.classList.add('split');
}

/* ── Reveal on scroll ─────────────────────────────────── */
// Masked reveals start fully clipped, which an observer can't see, so their parent is
// watched instead and passes the reveal down.
const proxies = new WeakMap<Element, HTMLElement[]>();
const reveal = (el: HTMLElement) => {
  if (el.classList.contains('is-in')) return;
  el.classList.add('is-in');
  if (el.dataset.splitDone) {
    // Unclip once the words have landed, so italics and descenders aren't shaved (Safari).
    const last = el.querySelectorAll('.w').length * 55 + (parseInt(el.style.getPropertyValue('--delay')) || 0);
    setTimeout(() => el.classList.add('settled'), last + 1000);
  }
  if (el.dataset.count) countUp(el);
};
const io = new IntersectionObserver(
  (entries) => {
    for (const e of entries) {
      if (!e.isIntersecting) continue;
      const kids = proxies.get(e.target);
      if (kids) kids.forEach(reveal);
      else reveal(e.target as HTMLElement);
      io.unobserve(e.target);
    }
  },
  { rootMargin: '0px 0px -6% 0px', threshold: 0.08 }
);
function watch(el: HTMLElement) {
  if (el.dataset.reveal === 'mask' && el.parentElement) {
    const p = el.parentElement;
    const list = proxies.get(p) ?? [];
    list.push(el);
    proxies.set(p, list);
    io.observe(p);
  } else io.observe(el);
}

function setupReveals(scope: ParentNode = document) {
  scope.querySelectorAll<HTMLElement>('[data-stagger]').forEach((g) => {
    const step = Number(g.dataset.stagger) || 70;
    [...g.children].forEach((c, i) => {
      const ch = c as HTMLElement;
      if (!ch.hasAttribute('data-reveal')) ch.setAttribute('data-reveal', g.dataset.staggerKind ?? '');
      ch.style.setProperty('--delay', `${i * step}ms`);
    });
  });
  scope.querySelectorAll<HTMLElement>('[data-split]').forEach((el) => {
    split(el);
    watch(el);
  });
  const all = [...scope.querySelectorAll<HTMLElement>('[data-reveal], [data-count]')];
  all.forEach(watch);
  // Anything already on screen when the page opens animates in straight away.
  const vh = innerHeight;
  [...scope.querySelectorAll<HTMLElement>('[data-split]'), ...all].forEach((el) => {
    const r = (el.dataset.reveal === 'mask' ? el.parentElement! : el).getBoundingClientRect();
    if (r.top < vh && r.bottom > 0) reveal(el);
  });
}

/* ── Count up ─────────────────────────────────────────── */
function countUp(el: HTMLElement) {
  const to = Number(el.dataset.count);
  const dec = Number(el.dataset.decimals ?? 0);
  const suffix = el.dataset.suffix ?? '';
  if (reduced) {
    el.textContent = to.toFixed(dec) + suffix;
    return;
  }
  const dur = 1600;
  const t0 = performance.now();
  onTick((_, t) => {
    const p = Math.min(1, (t - t0) / dur);
    const e = 1 - Math.pow(2, -10 * p);
    el.textContent = (to * (p === 1 ? 1 : e)).toFixed(dec) + suffix;
    return p < 1;
  });
}

/* ── Buttons: ripple and magnetism ───────────────────── */
function ripple(e: PointerEvent) {
  const btn = (e.target as HTMLElement).closest<HTMLElement>('.btn, [data-ripple]');
  if (!btn || reduced) return;
  const r = btn.getBoundingClientRect();
  const size = Math.max(r.width, r.height) * 2.4;
  const s = document.createElement('span');
  s.className = 'ripple';
  s.style.cssText = `left:${e.clientX - r.left}px;top:${e.clientY - r.top}px;width:${size}px;height:${size}px`;
  btn.append(s);
  s.addEventListener('animationend', () => s.remove(), { once: true });
}

function magnetic() {
  if (!finePointer || reduced) return;
  document.querySelectorAll<HTMLElement>('[data-magnetic]').forEach((el) => {
    const strength = Number(el.dataset.magnetic) || 0.28;
    el.addEventListener('pointermove', (e) => {
      const r = el.getBoundingClientRect();
      const x = (e.clientX - (r.left + r.width / 2)) * strength;
      const y = (e.clientY - (r.top + r.height / 2)) * strength;
      el.style.setProperty('--bx', `${x.toFixed(1)}px`);
      el.style.setProperty('--by', `${y.toFixed(1)}px`);
    });
    el.addEventListener('pointerleave', () => {
      el.style.setProperty('--bx', '0px');
      el.style.setProperty('--by', '0px');
    });
  });
}

/* ── Tilt cards with a spotlight ─────────────────────── */
function tilt() {
  if (!finePointer) return;
  document.querySelectorAll<HTMLElement>('[data-tilt]').forEach((el) => {
    const max = Number(el.dataset.tilt) || 6;
    const rx = new Spring(0, 220, 22);
    const ry = new Spring(0, 220, 22);
    let stop: (() => void) | null = null;
    const run = () => {
      if (stop) return;
      stop = onTick((dt) => {
        rx.step(dt);
        ry.step(dt);
        el.style.transform = `perspective(900px) rotateX(${rx.value.toFixed(2)}deg) rotateY(${ry.value.toFixed(2)}deg)`;
        if (rx.settled && ry.settled) {
          stop = null;
          return false;
        }
      });
    };
    el.addEventListener('pointermove', (e) => {
      const r = el.getBoundingClientRect();
      const px = (e.clientX - r.left) / r.width;
      const py = (e.clientY - r.top) / r.height;
      el.style.setProperty('--mx', `${(px * 100).toFixed(1)}%`);
      el.style.setProperty('--my', `${(py * 100).toFixed(1)}%`);
      if (reduced) return;
      rx.target = (0.5 - py) * max;
      ry.target = (px - 0.5) * max;
      run();
    });
    el.addEventListener('pointerleave', () => {
      rx.target = 0;
      ry.target = 0;
      run();
    });
  });
}

/* ── Parallax ─────────────────────────────────────────── */
function parallax() {
  const els = [...document.querySelectorAll<HTMLElement>('[data-parallax]')];
  if (!els.length || reduced) return;
  let queued = false;
  const update = () => {
    queued = false;
    const vh = innerHeight;
    for (const el of els) {
      const r = el.getBoundingClientRect();
      if (r.bottom < -200 || r.top > vh + 200) continue;
      const f = Number(el.dataset.parallax) || 0.15;
      const mid = r.top + r.height / 2 - vh / 2;
      el.style.translate = `0 ${(-mid * f).toFixed(1)}px`;
    }
  };
  addEventListener('scroll', () => {
    if (!queued) {
      queued = true;
      requestAnimationFrame(update);
    }
  }, { passive: true });
  update();
}

/* ── Marquee that reacts to scroll speed ─────────────── */
let scrollVel = 0;
{
  let lastY = scrollY;
  let lastT = performance.now();
  addEventListener('scroll', () => {
    const t = performance.now();
    const v = (scrollY - lastY) / Math.max(1, t - lastT);
    scrollVel = scrollVel * 0.6 + v * 0.4;
    lastY = scrollY;
    lastT = t;
  }, { passive: true });
}
export const getScrollVelocity = () => scrollVel;

function marquees() {
  document.querySelectorAll<HTMLElement>('[data-marquee]').forEach((track) => {
    const dir = track.dataset.marquee === 'right' ? 1 : -1;
    const speed = Number(track.dataset.speed) || 40;
    // Duplicate the content once so it can loop seamlessly.
    track.append(...[...track.children].map((c) => {
      const k = c.cloneNode(true) as HTMLElement;
      k.setAttribute('aria-hidden', 'true');
      return k;
    }));
    if (reduced) return;
    let x = 0;
    let visible = false;
    let hover = 0;
    const boost = new Spring(0, 90, 18);
    const skew = new Spring(0, 140, 16);
    new IntersectionObserver(([e]) => {
      visible = e.isIntersecting;
      if (visible) start();
    }).observe(track);
    track.addEventListener('pointerenter', () => (hover = 1));
    track.addEventListener('pointerleave', () => (hover = 0));
    let stop: (() => void) | null = null;
    const start = () => {
      if (stop) return;
      stop = onTick((dt) => {
        if (!visible) {
          stop = null;
          return false;
        }
        boost.target = scrollVel * 900 * (dir === -1 ? 1 : -1);
        skew.target = Math.max(-8, Math.min(8, scrollVel * 6));
        boost.step(dt);
        skew.step(dt);
        scrollVel *= 0.92;
        const half = track.scrollWidth / 2;
        x += (dir * speed * (1 - hover * 0.75) - boost.value) * dt;
        if (x <= -half) x += half;
        if (x > 0) x -= half;
        track.style.transform = `translate3d(${x.toFixed(2)}px,0,0) skewX(${skew.value.toFixed(2)}deg)`;
      });
    };
  });
}

/* ── Header: glass once scrolled, hides going down ───── */
function header() {
  const h = document.querySelector<HTMLElement>('.site-header');
  if (!h) return;
  let lastY = scrollY;
  let ticking = false;
  const update = () => {
    ticking = false;
    const y = scrollY;
    h.classList.toggle('scrolled', y > 8);
    const menuOpen = root.classList.contains('menu-open');
    if (!menuOpen && y > 420 && y > lastY + 4) h.classList.add('tucked');
    else if (y < lastY - 4 || y < 420) h.classList.remove('tucked');
    lastY = y;
  };
  addEventListener('scroll', () => {
    if (!ticking) {
      ticking = true;
      requestAnimationFrame(update);
    }
  }, { passive: true });
  update();
}

/* ── Copy to clipboard ───────────────────────────────── */
function copyButtons() {
  document.addEventListener('click', async (e) => {
    const btn = (e.target as HTMLElement).closest<HTMLElement>('[data-copy]');
    if (!btn) return;
    const text = btn.dataset.copy || btn.closest('[data-copy-scope]')?.querySelector('[data-copy-text]')?.textContent || '';
    try {
      await navigator.clipboard.writeText(text.trim());
      btn.classList.add('copied');
      toast(btn.dataset.copyLabel ?? 'Copied to the clipboard');
      setTimeout(() => btn.classList.remove('copied'), 1600);
    } catch {
      toast('Couldn’t copy. Select the text and press Ctrl+C.', 'info');
    }
  });
}

/* ── Page transitions: remember where the click was ──────
   The new page (inline script in <head>) grows out of this point. */
function transitions() {
  const remember = (x: number, y: number) => {
    try {
      sessionStorage.setItem('vt', JSON.stringify({ x, y, t: Date.now() }));
    } catch {
      /* private mode */
    }
  };
  document.addEventListener('pointerdown', (e) => {
    const a = (e.target as HTMLElement).closest('a');
    if (!a || a.target === '_blank' || a.origin !== location.origin) return;
    remember(e.clientX, e.clientY);
  }, { capture: true });
  document.addEventListener('keydown', (e) => {
    if (e.key !== 'Enter') return;
    const a = (document.activeElement as HTMLElement | null)?.closest?.('a');
    if (!a) return;
    const r = a.getBoundingClientRect();
    remember(r.left + r.width / 2, r.top + r.height / 2);
  });
}

/* ── Pause what nobody can see ────────────────────────── */
function sleepOffscreen() {
  const seen = new IntersectionObserver(
    (entries) => entries.forEach((e) => e.target.classList.toggle('asleep', !e.isIntersecting)),
    { rootMargin: '160px 0px' }
  );
  document.querySelectorAll('main > section, .site-footer > .cta, .site-footer > .giant').forEach((el) => seen.observe(el));
}

/* ── Boot ─────────────────────────────────────────────── */
setupReveals();
document.addEventListener('pointerdown', ripple);
magnetic();
tilt();
parallax();
marquees();
header();
copyButtons();
transitions();
sleepOffscreen();

export { setupReveals };
