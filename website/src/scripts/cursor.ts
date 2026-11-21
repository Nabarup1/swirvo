// A soft cursor companion for mouse users: a ring that trails the pointer on a spring,
// swells over things you can press, and shows a word over things you can drag or copy.
// The system cursor stays visible; this only adds weight to it.
import { Spring, onTick, reduced, finePointer } from './motion';

const el = document.querySelector<HTMLElement>('.cursor');
if (el && finePointer && !reduced) {
  const ring = el.querySelector<HTMLElement>('.cursor-ring')!;
  const dot = el.querySelector<HTMLElement>('.cursor-dot')!;
  const label = el.querySelector<HTMLElement>('.cursor-label')!;
  const x = new Spring(-100, 260, 26);
  const y = new Spring(-100, 260, 26);
  const s = new Spring(1, 300, 22);
  let px = -100;
  let py = -100;
  let stop: (() => void) | null = null;
  let shown = false;
  const run = () => {
    if (stop) return;
    stop = onTick((dt) => {
      x.step(dt);
      y.step(dt);
      s.step(dt);
      const vx = x.velocity;
      const vy = y.velocity;
      const speed = Math.min(1, Math.hypot(vx, vy) / 3000);
      const ang = Math.atan2(vy, vx);
      ring.style.transform = `translate3d(${x.value}px, ${y.value}px, 0) rotate(${ang}rad) scale(${(s.value * (1 + speed * 0.35)).toFixed(3)}, ${(s.value * (1 - speed * 0.25)).toFixed(3)})`;
      label.style.transform = `rotate(${-ang}rad)`;
      dot.style.transform = `translate3d(${px}px, ${py}px, 0)`;
      if (x.settled && y.settled && s.settled) {
        stop = null;
        return false;
      }
    });
  };
  addEventListener('pointermove', (e) => {
    if (e.pointerType !== 'mouse') return;
    px = e.clientX;
    py = e.clientY;
    x.target = px;
    y.target = py;
    if (!shown) {
      shown = true;
      x.value = px;
      y.value = py;
      el.classList.add('on');
    }
    run();
  }, { passive: true });
  document.addEventListener('pointerleave', () => el.classList.remove('on'));
  document.addEventListener('pointerenter', () => shown && el.classList.add('on'));

  const HOT = 'a, button, [data-cursor], input, textarea, select, summary, label[for]';
  document.addEventListener('pointerover', (e) => {
    const t = (e.target as HTMLElement).closest<HTMLElement>(HOT);
    const word = t?.dataset.cursor ?? '';
    const text = t?.matches('input, textarea') ?? false;
    el.classList.toggle('hot', !!t && !text);
    el.classList.toggle('worded', !!word);
    el.classList.toggle('texty', text);
    label.textContent = word;
    s.target = word ? 1.9 : t && !text ? 1.5 : 1;
    run();
  });
  addEventListener('pointerdown', () => {
    s.velocity -= 9;
    run();
  });
}
