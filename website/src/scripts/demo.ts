// The homepage demo: the real Swirvo gesture, in the browser.
//   Mouse: drag a file, tap Shift / Ctrl / Alt, drop on a wedge.
//   Touch: hold a file, drag; the Convert wheel opens when you pause (or tap a key below).
//   Keyboard: focus a file, tap Shift / Ctrl / Alt; arrows move, Tab switches, Enter applies.
// While nobody touches it, a ghost hand plays the gesture on a loop.
import { FILES, actions, type DemoFile } from '../data/demo';
import { drawWheel, drawSub, G, angleOf, slotAt, centre, span, pt, type WItem, type WheelKind, type DrawnWheel } from './wheel-draw';
import { Spring, onTick, reduced, toast } from './motion';

const demo = document.querySelector<HTMLElement>('[data-demo]');
if (demo) init(demo);

type St = 'idle' | 'press' | 'drag' | 'open' | 'commit';
const KEY_KIND: Record<string, WheelKind> = { Shift: 'convert', Control: 'tools', Alt: 'actions' };
const KIND_ORDER: WheelKind[] = ['convert', 'tools', 'actions'];
const NS = 'http://www.w3.org/2000/svg';
const clamp = (v: number, a: number, b: number) => Math.max(a, Math.min(b, v));

function init(demo: HTMLElement) {
  const list = demo.querySelector<HTMLUListElement>('[data-files]')!;
  const svg = demo.querySelector<SVGSVGElement>('[data-wheel-layer]')!;
  const ghost = demo.querySelector<HTMLElement>('[data-ghost]')!;
  const hint = demo.querySelector<HTMLElement>('[data-hint]')!;
  const keys = demo.querySelector<HTMLElement>('[data-keys]')!;
  const hand = demo.querySelector<HTMLElement>('[data-hand]')!;
  const receipt = demo.querySelector<HTMLElement>('[data-receipt]')!;
  const stepEls = [...demo.querySelectorAll<HTMLElement>('[data-step]')];
  const status = demo.querySelector<HTMLElement>('[data-status]')!;
  const live = demo.querySelector<HTMLElement>('[data-live]')!;

  let st: St = 'idle';
  let src: { li: HTMLLIElement; file: DemoFile } | null = null;
  let px = 0;
  let py = 0;
  let ptype = 'mouse';
  let pressX = 0;
  let pressY = 0;
  let pressTimer = 0;
  let stillTimer = 0;
  let inView = false;
  let pendingTap: WheelKind | null = null;
  let byRobot = false;
  let pid = -1; // the pointer that started the drag; a second finger only presses keys

  interface Wheel {
    kind: WheelKind;
    items: WItem[];
    drawn: DrawnWheel;
    pos: SVGGElement;
    cx: number;
    cy: number;
    s: number;
    hover: number;
    preset: number;
    subFor: number;
    sub: { a0: number; step: number; count: number } | null;
    dwell: number;
    keyboard: boolean;
  }
  let wheel: Wheel | null = null;

  /* ── Springs: the dragged file and the wheel ─────────── */
  const gx = new Spring(0, 520, 34);
  const gy = new Spring(0, 520, 34);
  const gr = new Spring(0, 260, 18);
  const gs = new Spring(1, 300, 20);
  const wk = new Spring(1, 420, 24);
  const wr = new Spring(0, 260, 22);
  let anim: (() => void) | null = null;
  const animate = () => {
    if (anim) return;
    anim = onTick((dt) => {
      const lx = gx.value;
      gx.target = st === 'drag' || st === 'open' ? px : gx.target;
      gy.target = st === 'drag' || st === 'open' ? py : gy.target;
      gx.step(dt);
      gy.step(dt);
      gr.target = clamp((gx.value - lx) * 2.2, -16, 16);
      gr.step(dt);
      gs.step(dt);
      ghost.style.transform = `translate3d(${gx.value.toFixed(1)}px, ${gy.value.toFixed(1)}px, 0) rotate(${gr.value.toFixed(2)}deg) scale(${gs.value.toFixed(3)})`;
      wk.step(dt);
      wr.step(dt);
      if (wheel) wheel.pos.setAttribute('transform', `translate(${wheel.cx.toFixed(1)} ${wheel.cy.toFixed(1)}) scale(${(wheel.s * wk.value).toFixed(4)}) rotate(${wr.value.toFixed(2)})`);
      const quiet = gx.settled && gy.settled && gr.settled && gs.settled && wk.settled && wr.settled;
      if (quiet && st === 'idle') {
        anim = null;
        return false;
      }
    });
  };

  /* ── Helpers ──────────────────────────────────────────── */
  const size = () => ({ W: demo.clientWidth, H: demo.clientHeight });
  const resize = () => {
    const { W, H } = size();
    svg.setAttribute('viewBox', `0 0 ${W} ${H}`);
  };
  resize();
  new ResizeObserver(resize).observe(demo);
  new IntersectionObserver(([e]) => {
    inView = e.isIntersecting;
    if (inView) kickRobot();
  }, { threshold: 0.35 }).observe(demo);

  const local = (e: { clientX: number; clientY: number }) => {
    const r = demo.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  };
  const setSteps = (n: number) =>
    stepEls.forEach((s, i) => {
      s.classList.toggle('done', i < n);
      s.classList.toggle('now', i === n);
    });
  const say = (t: string) => (live.textContent = t);
  const fileOf = (li: HTMLLIElement) => FILES[Number(li.dataset.file)];
  const itemsFor = (kind: WheelKind, f: DemoFile) => (kind === 'convert' ? f.convert : kind === 'tools' ? f.tools : actions);
  const presetsFor = (kind: WheelKind, item: WItem, f: DemoFile) =>
    item.more ? null : kind === 'convert' ? ['Best', 'Balanced', 'Smallest'] : f.presets?.[item.label] ?? null;

  function showHint(html: string) {
    hint.innerHTML = html;
    hint.classList.add('on');
  }
  const hideHint = () => hint.classList.remove('on');
  function flashKey(kind: WheelKind) {
    const k = keys.querySelector<HTMLElement>(`[data-key="${kind}"] kbd`);
    if (!k) return;
    k.classList.add('is-down');
    setTimeout(() => k.classList.remove('is-down'), 160);
  }

  /* ── Press and drag ───────────────────────────────────── */
  function press(li: HTMLLIElement, x: number, y: number, type: string) {
    if (st !== 'idle' || li.classList.contains('result')) return;
    st = 'press';
    src = { li, file: fileOf(li) };
    ptype = type;
    pressX = px = x;
    pressY = py = y;
    if (type === 'touch') pressTimer = window.setTimeout(beginDrag, 260);
  }

  function beginDrag() {
    if (st !== 'press' || !src) return;
    st = 'drag';
    const r = src.li.getBoundingClientRect();
    const d = demo.getBoundingClientRect();
    gx.value = r.left - d.left + 40;
    gy.value = r.top - d.top + r.height / 2;
    gx.velocity = gy.velocity = 0;
    gs.value = 0.7;
    gs.target = 1;
    ghost.querySelector('.g-name')!.textContent = src.file.name;
    ghost.dataset.kind = src.file.kind;
    ghost.classList.add('on');
    src.li.classList.add('dragging');
    demo.classList.add('dragging');
    document.documentElement.classList.add('demo-dragging');
    if (ptype === 'touch') {
      keys.classList.add('on');
      showHint('Pause to open the wheel, or tap a key');
      armStill();
    } else showHint('Now tap <kbd>Shift</kbd> <span>· or <kbd>Ctrl</kbd> <kbd>Alt</kbd></span>');
    setSteps(1);
    say(`Dragging ${src.file.name}. Tap Shift for formats, Control for tools, Alt for actions.`);
    if (navigator.vibrate && ptype === 'touch') navigator.vibrate(8);
    animate();
  }

  function armStill() {
    clearTimeout(stillTimer);
    if (ptype !== 'touch' || st !== 'drag') return;
    stillTimer = window.setTimeout(() => st === 'drag' && openWheel('convert'), 420);
  }

  /* ── The wheel ────────────────────────────────────────── */
  function openWheel(kind: WheelKind, at?: { x: number; y: number }, keyboard = false) {
    if (!src) return;
    if (st === 'open' && wheel) {
      if (wheel.kind !== kind) switchWheel(kind);
      return;
    }
    if (st !== 'drag' && !keyboard) return;
    st = 'open';
    clearTimeout(stillTimer);
    const { W, H } = size();
    const s = Math.min(1, (W - 20) / (2 * G.subOut), (H - 20) / (2 * G.R + 120));
    const ax = at?.x ?? px;
    const ay = at?.y ?? py;
    const cx = clamp(ax, G.R * s + 10, W - G.R * s - 10);
    const cy = clamp(ay, (G.R + 50) * s + 6, H - G.R * s - 10);
    const pos = document.createElementNS(NS, 'g') as SVGGElement;
    pos.setAttribute('class', 'wh-pos');
    svg.append(pos);
    const items = itemsFor(kind, src.file);
    const drawn = drawWheel(pos, items, { name: src.file.name, meta: src.file.size }, kind);
    wheel = { kind, items, drawn, pos, cx, cy, s, hover: -1, preset: -1, subFor: -1, sub: null, dwell: 0, keyboard };
    wk.value = 0.82;
    wk.velocity = 0;
    wk.target = 1;
    wr.value = -10;
    wr.target = 0;
    requestAnimationFrame(() => drawn.root.classList.add('open'));
    ghost.classList.add('tucked');
    keys.classList.add('on');
    flashKey(kind);
    setSteps(2);
    const noun = kind === 'convert' ? 'a format' : kind === 'tools' ? 'a tool' : 'an action';
    showHint(keyboard ? 'Arrows to choose, <kbd>Enter</kbd> to apply' : `Drop it on ${noun}`);
    say(`${kind === 'convert' ? 'Convert' : kind === 'tools' ? 'Tools' : 'Actions'} wheel: ${items.map((i) => i.label).join(', ')}.`);
    if (keyboard) setHover(0);
    else hoverAt();
    animate();
  }

  function switchWheel(kind: WheelKind) {
    if (!wheel || !src) return;
    const old = wheel.drawn.root;
    old.classList.add('leaving');
    setTimeout(() => old.remove(), 200);
    const items = itemsFor(kind, src.file);
    const drawn = drawWheel(wheel.pos, items, { name: src.file.name, meta: src.file.size }, kind);
    Object.assign(wheel, { kind, items, drawn, hover: -1, preset: -1, subFor: -1, sub: null });
    wr.value = -8;
    wr.target = 0;
    requestAnimationFrame(() => drawn.root.classList.add('open'));
    flashKey(kind);
    const noun = kind === 'convert' ? 'a format' : kind === 'tools' ? 'a tool' : 'an action';
    if (!wheel.keyboard) showHint(`Drop it on ${noun}`);
    say(`${kind} wheel: ${items.map((i) => i.label).join(', ')}.`);
    if (wheel.keyboard) setHover(0);
    else hoverAt();
    animate();
  }

  function setHover(i: number) {
    if (!wheel || !src) return;
    if (i === wheel.hover) return;
    wheel.hover = i;
    wheel.drawn.wedges.forEach((w, k) => w.classList.toggle('hov', k === i));
    clearTimeout(wheel.dwell);
    if (wheel.subFor !== i) hideSub();
    if (i >= 0) {
      const it = wheel.items[i];
      say(`${it.label}, ${it.caption}. ${i + 1} of ${wheel.items.length}.`);
      if (presetsFor(wheel.kind, it, src.file)) wheel.dwell = window.setTimeout(() => showSub(i), 450);
    }
  }

  function showSub(i: number) {
    if (!wheel || !src || wheel.subFor === i) return;
    const labels = presetsFor(wheel.kind, wheel.items[i], src.file);
    if (!labels) return;
    wheel.sub = drawSub(wheel.drawn, i, wheel.items.length, labels);
    wheel.subFor = i;
    wheel.preset = -1;
    wheel.drawn.root.classList.add('sub-open');
    requestAnimationFrame(() => wheel?.drawn.sub.classList.add('open'));
  }
  function hideSub() {
    if (!wheel) return;
    wheel.drawn.sub.classList.remove('open');
    wheel.drawn.root.classList.remove('sub-open');
    wheel.drawn.sub.replaceChildren();
    wheel.sub = null;
    wheel.subFor = -1;
    wheel.preset = -1;
  }
  function setPreset(k: number) {
    if (!wheel || wheel.preset === k) return;
    wheel.preset = k;
    wheel.drawn.sub.querySelectorAll('.wh-p').forEach((p, j) => p.classList.toggle('hov', j === k));
  }

  function hoverAt() {
    if (!wheel || wheel.keyboard) return;
    const dx = (px - wheel.cx) / wheel.s;
    const dy = (py - wheel.cy) / wheel.s;
    const d = Math.hypot(dx, dy);
    const a = angleOf(dx, dy);
    const n = wheel.items.length;
    if (d < G.r0) {
      setHover(-1);
      return;
    }
    if (d <= G.R + 4) {
      setHover(slotAt(a, n));
      setPreset(-1);
      return;
    }
    // Past the rim: open the preset ring for the wedge we left through.
    const slot = slotAt(a, n);
    if (wheel.sub && d >= G.subIn - 8 && d <= G.subOut + 40) {
      const rel = (((a - wheel.sub.a0) % 360) + 360) % 360;
      const k = Math.floor(rel / wheel.sub.step);
      setPreset(k >= 0 && k < wheel.sub.count ? k : -1);
      return;
    }
    if (d < G.subIn && wheel.hover >= 0 && slot === wheel.hover) {
      showSub(wheel.hover);
      return;
    }
    if (d > G.subOut + 60) {
      setHover(-1);
      hideSub();
    }
  }

  /* ── Finish: commit or cancel ─────────────────────────── */
  const stem = (n: string) => n.replace(/\.[^.]+$/, '');
  const ext = (n: string) => n.slice(n.lastIndexOf('.'));
  function uniqueName(name: string) {
    const names = new Set([...list.querySelectorAll('.f-name')].map((e) => e.textContent));
    if (!names.has(name)) return name;
    for (let i = 2; ; i++) {
      const c = `${stem(name)} (${i})${ext(name)}`;
      if (!names.has(c)) return c;
    }
  }

  function outcome(kind: WheelKind, item: WItem, f: DemoFile, preset: string | null): { name?: string; note: string; folder?: boolean } {
    const s = stem(f.name);
    const size = f.out?.[item.label] ?? '';
    if (kind === 'convert') {
      const e = item.label === 'JPG' ? 'jpg' : item.label.toLowerCase();
      const q = preset && preset !== 'Balanced' ? ` · ${preset}` : '';
      return { name: `${s}.${e}`, note: `${size}${q} · next to the original` };
    }
    if (kind === 'actions') {
      switch (item.label) {
        case 'Show': return { note: `Opens File Explorer with ${f.name} selected` };
        case 'Copy path': return { note: `Copied C:\\Users\\you\\Downloads\\${f.name}` };
        case 'Terminal': return { note: 'Opens Windows Terminal in Downloads' };
        case 'To ZIP': return { name: `${s}.zip`, note: 'One ZIP next to the original' };
        case 'Board': return { note: `Opens ${f.name} in the Board` };
        default: return { note: f.kind === 'image' ? 'Copied the text in the photo (OCR)' : 'Copied the text to the clipboard' };
      }
    }
    const L = item.label;
    if (f.kind === 'video' && L === 'Compress') return { name: `${s} (${preset ?? '25 MB'}).mp4`, note: `${preset ?? '25 MB'} or less · H.264 + AAC` };
    if (L === 'Frame') return { name: `${s}.png`, note: 'A good frame near the start' };
    if (L === 'To one PDF') return { name: `${s}.pdf`, note: 'A page per image' };
    if (L === 'Split ranges') return { name: `${s} (parts)`, folder: true, note: 'A PDF per group of pages' };
    if (L === 'Pages…') return { note: 'Opens the page picker: click pages or type 1-3, 7' };
    if (L === 'Pictures') return { name: `${s} (pictures)`, folder: true, note: 'Every picture, saved' };
    if (L === 'Word count') return { note: 'Copied: 2,184 words' };
    if (L === 'Checksum') return { note: 'SHA-256 copied to the clipboard' };
    if (L === 'Mail merge') return { note: 'Pick a spreadsheet: a letter per row' };
    if (L === 'PDF/A') return { name: `${s}.pdf`, note: 'PDF/A-2a, tagged for screen readers' };
    const suffix: Record<string, string> = {
      Resize: '50 %', 'Fit 1920': '1920', Rotate: 'rotated', Flip: 'flipped', Compress: 'compressed', 'Strip info': 'clean',
      Watermark: 'watermark', Numbers: 'numbered', Protect: 'protected', Redact: 'redacted', Clean: 'clean', Replace: 'edited',
    };
    const e = f.kind === 'image' ? '.jpg' : ext(f.name);
    return { name: `${s} (${suffix[L] ?? L.toLowerCase()})${e}`, note: size ? `${size} · original untouched` : 'Original untouched' };
  }

  function addRow(name: string, folder: boolean, after: HTMLLIElement, meta: string, kind: string) {
    const li = document.createElement('li');
    li.className = 'f result born';
    li.innerHTML = `<span class="f-ic" data-kind="${folder ? 'folder' : kind}"></span><span class="f-name"></span><span class="f-meta"></span><span class="f-new">New</span>`;
    li.querySelector('.f-name')!.textContent = name;
    li.querySelector('.f-meta')!.textContent = meta;
    after.after(li);
    setTimeout(() => li.classList.remove('born'), 1400);
    return li;
  }

  function kindOfName(n: string) {
    const e = n.split('.').pop()!.toLowerCase();
    if (['jpg', 'png', 'webp', 'gif', 'tiff'].includes(e)) return 'image';
    if (e === 'pdf') return 'pdf';
    if (['mp4', 'mov'].includes(e)) return 'video';
    if (['mp3', 'm4a', 'wav', 'aac'].includes(e)) return 'audio';
    if (['xlsx', 'csv', 'tsv'].includes(e)) return 'sheet';
    if (['zip'].includes(e)) return 'archive';
    if (['json', 'yaml', 'toml', 'ndjson'].includes(e)) return 'data';
    return 'doc';
  }

  function showReceipt(text: string, sub: string, li?: HTMLElement) {
    receipt.innerHTML = '<span class="r-ok"></span><span class="r-t"></span><span class="r-s"></span>';
    receipt.querySelector('.r-t')!.textContent = text;
    receipt.querySelector('.r-s')!.textContent = sub;
    const d = demo.getBoundingClientRect();
    if (li) {
      const r = li.getBoundingClientRect();
      receipt.style.left = `${Math.min(r.left - d.left + 40, d.width - 300)}px`;
      receipt.style.top = `${r.bottom - d.top + 6}px`;
    } else {
      receipt.style.left = `${clamp((wheel?.cx ?? d.width / 2) - 140, 12, d.width - 300)}px`;
      receipt.style.top = `${clamp((wheel?.cy ?? d.height / 2) + 30, 12, d.height - 70)}px`;
    }
    receipt.classList.remove('on');
    void receipt.offsetWidth;
    receipt.classList.add('on');
    clearTimeout(Number(receipt.dataset.t));
    receipt.dataset.t = String(setTimeout(() => receipt.classList.remove('on'), 2600));
  }

  async function commit() {
    if (!wheel || !src || wheel.hover < 0) return cancel();
    const w = wheel;
    const s = src;
    const item = w.items[w.hover];
    if (item.more) {
      toast('More… opens the full list, with search');
      return cancel();
    }
    st = 'commit';
    const labels = presetsFor(w.kind, item, s.file);
    const preset = labels ? labels[w.preset >= 0 ? w.preset : Math.min(1, labels.length - 1)] : null;
    const res = outcome(w.kind, item, s.file, preset);
    hideHint();
    setSteps(3);
    w.drawn.wedges[w.hover].classList.add('hit');
    // Collapse into a progress ring at the drop point.
    w.drawn.root.classList.add('collapsing');
    const ring = document.createElementNS(NS, 'g');
    ring.setAttribute('class', 'wh-progress');
    ring.innerHTML = '<circle r="14" class="pr-bg"/><circle r="14" class="pr-fg" pathLength="100" transform="rotate(-90)"/><path d="M-6 0.5l4 4 8-9" class="pr-ok" pathLength="20"/>';
    w.pos.append(ring);
    wk.target = 1;
    await wait(reduced ? 50 : 160);
    ring.classList.add('run');
    await wait(reduced ? 80 : 720);
    ring.classList.add('ok');
    ghost.classList.remove('on', 'tucked');
    s.li.classList.remove('dragging');
    demo.classList.remove('dragging');
    document.documentElement.classList.remove('demo-dragging');
    let row: HTMLLIElement | undefined;
    if (res.name) {
      const name = res.folder ? res.name : uniqueName(res.name);
      row = addRow(name, !!res.folder, s.li, res.folder ? 'Folder' : (s.file.out?.[item.label] ?? ''), kindOfName(name));
      if (!reduced) fly(w.cx, w.cy, row);
      showReceipt(name, res.note, row);
      say(`Saved ${name}, ${res.note}.`);
      status.textContent = `${list.children.length} items · ${name} saved`;
    } else {
      showReceipt(item.label, res.note);
      say(res.note);
    }
    await wait(reduced ? 50 : 420);
    w.pos.classList.add('gone');
    setTimeout(() => w.pos.remove(), 300);
    wheel = null;
    keys.classList.remove('on');
    st = 'idle';
    src = null;
    setTimeout(() => setSteps(byRobot ? 0 : 3), 1200);
    return row;
  }

  function fly(x: number, y: number, row: HTMLElement) {
    const d = demo.getBoundingClientRect();
    const r = row.getBoundingClientRect();
    const tx = r.left - d.left + 24;
    const ty = r.top - d.top + r.height / 2;
    const chip = document.createElement('span');
    chip.className = 'fly-chip';
    demo.append(chip);
    const mx = (x + tx) / 2 + (ty < y ? -60 : 60);
    const my = Math.min(y, ty) - 90;
    chip.style.offsetPath = `path('M ${x.toFixed(0)} ${y.toFixed(0)} Q ${mx.toFixed(0)} ${my.toFixed(0)} ${tx.toFixed(0)} ${ty.toFixed(0)}')`;
    chip.animate(
      [
        { offsetDistance: '0%', transform: 'scale(1.3)', opacity: 1 },
        { offsetDistance: '100%', transform: 'scale(0.5)', opacity: 0.2 },
      ],
      { duration: 620, easing: 'cubic-bezier(0.65, 0, 0.35, 1)', fill: 'forwards' }
    ).finished.then(() => chip.remove());
  }

  function cancel(quiet = false) {
    clearTimeout(pressTimer);
    clearTimeout(stillTimer);
    if (wheel) {
      const w = wheel;
      w.drawn.root.classList.add('leaving');
      wk.target = 0.9;
      setTimeout(() => w.pos.remove(), 220);
      wheel = null;
    }
    if (src) {
      const r = src.li.getBoundingClientRect();
      const d = demo.getBoundingClientRect();
      gx.target = r.left - d.left + 40;
      gy.target = r.top - d.top + r.height / 2;
      gs.target = 0.6;
      ghost.classList.remove('tucked');
      ghost.classList.add('returning');
      const li = src.li;
      setTimeout(() => {
        ghost.classList.remove('on', 'returning');
        gs.value = 1;
        gs.target = 1;
        li.classList.remove('dragging');
      }, 380);
    }
    demo.classList.remove('dragging');
    document.documentElement.classList.remove('demo-dragging');
    keys.classList.remove('on');
    hideHint();
    if (!quiet && st !== 'press' && st !== 'idle') say('Cancelled. Nothing was changed.');
    st = 'idle';
    src = null;
    setSteps(0);
    animate();
  }

  /* ── Pointer input ────────────────────────────────────── */
  list.addEventListener('pointerdown', (e) => {
    const li = (e.target as HTMLElement).closest<HTMLLIElement>('li.f');
    if (!li || e.button !== 0) return;
    if (li.classList.contains('result')) {
      toast('A new file, next to the original. In the app you could drag it again.', 'info');
      return;
    }
    takeOver();
    if (e.pointerType === 'mouse') e.preventDefault();
    const p = local(e);
    pid = e.pointerId;
    press(li, p.x, p.y, e.pointerType);
    addEventListener('pointermove', onMove);
    addEventListener('pointerup', onUp);
    addEventListener('pointercancel', onCancel);
  });
  function onMove(e: PointerEvent) {
    if (e.pointerId !== pid) return;
    const p = local(e);
    px = p.x;
    py = p.y;
    if (st === 'press') {
      const moved = Math.hypot(px - pressX, py - pressY);
      if (ptype !== 'touch' && moved > 5) beginDrag();
      else if (ptype === 'touch' && moved > 10) {
        clearTimeout(pressTimer);
        st = 'idle';
        src = null;
        detach();
      }
      return;
    }
    if (st === 'drag') armStill();
    if (st === 'open') hoverAt();
    animate();
  }
  function onUp(e: PointerEvent) {
    if (e.pointerId !== pid) return;
    detach();
    if (st === 'open') commit();
    else if (st === 'drag') {
      cancel();
      toast('Tap Shift while you drag to open the wheel', 'info');
    } else if (st === 'press') {
      clearTimeout(pressTimer);
      st = 'idle';
      const name = src?.file.name;
      src = null;
      if (ptype === 'touch') toast(`Hold ${name} for a moment, then drag`, 'info');
      else showHint('Drag it a little, then tap <kbd>Shift</kbd>');
      setTimeout(hideHint, 2200);
    }
  }
  function onCancel(e: PointerEvent) {
    if (e.pointerId !== pid) return;
    detach();
    if (st !== 'idle' && st !== 'commit') cancel(true);
  }
  function detach() {
    removeEventListener('pointermove', onMove);
    removeEventListener('pointerup', onUp);
    removeEventListener('pointercancel', onCancel);
  }
  // While dragging on a touch screen, the page must not scroll.
  demo.addEventListener('touchmove', (e) => {
    if (st === 'drag' || st === 'open') e.preventDefault();
  }, { passive: false });
  list.addEventListener('contextmenu', (e) => e.preventDefault());
  list.addEventListener('dragstart', (e) => e.preventDefault());

  // On-screen keys (touch, or a second finger)
  keys.addEventListener('pointerdown', (e) => {
    const b = (e.target as HTMLElement).closest<HTMLElement>('[data-key]');
    if (!b) return;
    e.preventDefault();
    e.stopPropagation();
    const kind = b.dataset.key as WheelKind;
    if (st === 'drag' || st === 'open') openWheel(kind);
    else toast('Start dragging a file first, then tap a key', 'info');
  });

  /* ── Keyboard ─────────────────────────────────────────── */
  addEventListener('keydown', (e) => {
    if (!inView) return;
    const kind = KEY_KIND[e.key];
    if (kind && (st === 'drag' || st === 'open') && !wheel?.keyboard) {
      e.preventDefault();
      takeOver();
      openWheel(kind);
      return;
    }
    if (e.key === 'Escape' && st !== 'idle' && st !== 'commit') {
      e.preventDefault();
      cancel();
      return;
    }
    if (wheel?.keyboard && st === 'open') {
      const n = wheel.items.length;
      if (e.key === 'ArrowRight' || e.key === 'ArrowDown' && !wheel.sub) {
        e.preventDefault();
        if (wheel.sub) setPreset((Math.max(0, wheel.preset) + 1) % wheel.sub.count);
        else setHover((wheel.hover + 1) % n);
      } else if (e.key === 'ArrowLeft') {
        e.preventDefault();
        if (wheel.sub) setPreset((Math.max(0, wheel.preset) - 1 + wheel.sub.count) % wheel.sub.count);
        else setHover((wheel.hover - 1 + n) % n);
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        if (wheel.hover >= 0) {
          showSub(wheel.hover);
          setPreset(1);
        }
      } else if (e.key === 'ArrowDown') {
        e.preventDefault();
        hideSub();
      } else if (e.key === 'Tab') {
        e.preventDefault();
        switchWheel(KIND_ORDER[(KIND_ORDER.indexOf(wheel.kind) + (e.shiftKey ? 2 : 1)) % 3]);
      } else if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        commit();
      }
      return;
    }
    // A lone tap of Shift / Ctrl / Alt on a focused file opens that wheel (keyboard mode).
    const li = (document.activeElement as HTMLElement | null)?.closest?.<HTMLLIElement>('li.f');
    if (kind && st === 'idle' && li && list.contains(li) && !li.classList.contains('result')) {
      pendingTap = kind;
      if (e.key === 'Alt') e.preventDefault();
    } else pendingTap = null;
    if (st === 'idle' && li && list.contains(li) && (e.key === 'Enter' || e.key === ' ')) {
      e.preventDefault();
      pendingTap = null;
      keyOpen(li, 'convert');
    }
  });
  addEventListener('keyup', (e) => {
    if (e.key === 'Alt') e.preventDefault();
    const kind = KEY_KIND[e.key];
    if (kind && pendingTap === kind) {
      const li = (document.activeElement as HTMLElement | null)?.closest?.<HTMLLIElement>('li.f');
      pendingTap = null;
      if (li && st === 'idle') keyOpen(li, kind);
    }
  });
  function keyOpen(li: HTMLLIElement, kind: WheelKind) {
    takeOver();
    const r = li.getBoundingClientRect();
    const d = demo.getBoundingClientRect();
    src = { li, file: fileOf(li) };
    st = 'drag';
    li.classList.add('dragging');
    openWheel(kind, { x: Math.min(r.left - d.left + 300, d.width - 200), y: r.top - d.top + r.height / 2 }, true);
  }

  /* ── The ghost hand (autoplay) ────────────────────────── */
  let robot: Promise<void> | null = null;
  let stopped = reduced;
  let abort = 0;
  const wait = (ms: number) => new Promise<void>((res) => setTimeout(res, ms));
  function takeOver() {
    if (stopped) return;
    stopped = true;
    abort++;
    hand.classList.remove('on');
    demo.classList.add('user');
    if (byRobot) {
      byRobot = false;
      if (st !== 'idle') cancel(true);
    }
  }
  function kickRobot() {
    if (stopped || robot) return;
    robot = runRobot().finally(() => (robot = null));
  }
  async function runRobot() {
    const token = abort;
    const live = () => token === abort && !stopped;
    const pause = async (ms: number) => {
      await wait(ms);
      while (live() && (!inView || document.hidden)) await wait(300);
      if (!live()) throw new Error('stop');
    };
    const moveTo = async (x: number, y: number, ms: number) => {
      const x0 = hx;
      const y0 = hy;
      const t0 = performance.now();
      const bend = (Math.random() - 0.5) * 80;
      await new Promise<void>((res) => {
        const step = () => {
          if (!live()) return res();
          const t = Math.min(1, (performance.now() - t0) / ms);
          const e = t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2;
          hx = x0 + (x - x0) * e + Math.sin(Math.PI * e) * bend * 0.3;
          hy = y0 + (y - y0) * e - Math.sin(Math.PI * e) * bend * 0.5;
          hand.style.transform = `translate3d(${hx}px, ${hy}px, 0)`;
          if (st === 'drag' || st === 'open' || st === 'press') {
            px = hx;
            py = hy;
            if (st === 'open') hoverAt();
            animate();
          }
          if (t < 1) requestAnimationFrame(step);
          else res();
        };
        requestAnimationFrame(step);
      });
      if (!live()) throw new Error('stop');
    };
    let hx = size().W * 0.8;
    let hy = size().H * 0.9;
    const scripts: { file: number; kind: WheelKind; label: string; preset?: number }[] = [
      { file: 0, kind: 'convert', label: 'JPG', preset: 1 },
      { file: 1, kind: 'convert', label: 'DOCX' },
      { file: 3, kind: 'tools', label: 'Compress', preset: 1 },
      { file: 4, kind: 'convert', label: 'XLSX' },
    ];
    try {
      await pause(900);
      hand.classList.add('on');
      for (let loop = 0; ; loop++) {
        for (const sc of scripts) {
          const li = list.querySelector<HTMLLIElement>(`li.f[data-file="${sc.file}"]`)!;
          const d = demo.getBoundingClientRect();
          const r = li.getBoundingClientRect();
          await moveTo(r.left - d.left + 120, r.top - d.top + r.height / 2, 900);
          hand.classList.add('down');
          byRobot = true;
          press(li, hx, hy, 'mouse');
          await pause(120);
          beginDrag();
          const { W, H } = size();
          await moveTo(clamp(W * 0.62, 280, W - 200), clamp(H * 0.52, 240, H - 180), 800);
          await pause(260);
          openWheel(sc.kind);
          await pause(500);
          if (!wheel) throw new Error('stop');
          const i = wheel.items.findIndex((it) => it.label === sc.label);
          const c = centre(i, wheel.items.length);
          const [ox, oy] = pt(c, G.R * 0.72 * wheel.s);
          await moveTo(wheel.cx + ox, wheel.cy + oy, 520);
          if (sc.preset !== undefined) {
            await pause(520);
            if (wheel?.sub) {
              const a = wheel.sub.a0 + wheel.sub.step * (sc.preset + 0.5);
              const [qx, qy] = pt(a, ((G.subIn + G.subOut) / 2) * wheel.s);
              await moveTo(wheel.cx + qx, wheel.cy + qy, 420);
            }
          }
          await pause(380);
          hand.classList.remove('down');
          const row = await commit();
          byRobot = false;
          await pause(2100);
          if (row) {
            row.classList.add('dying');
            setTimeout(() => row.remove(), 450);
          }
          await pause(500);
        }
        if (loop >= 2) break;
      }
      hand.classList.remove('on');
    } catch {
      hand.classList.remove('on', 'down');
      byRobot = false;
    }
  }
  if (reduced) demo.classList.add('user');
}
