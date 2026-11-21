// FLIP: when a filter shows and hides items, the ones that stay glide to their new
// places on a spring, new ones pop in, and the grid never jumps.
import { reduced } from './motion';

const SPRING = 'linear(0, 0.025, 0.0895, 0.1798, 0.2846, 0.3951, 0.5045, 0.608, 0.7021, 0.7849, 0.8556, 0.9141, 0.9609, 0.9969, 1.0234, 1.0416, 1.0529, 1.0587, 1.0601, 1.0583, 1.0542, 1.0486, 1.0423, 1.0356, 1.0291, 1.0229, 1.0173, 1.0124, 1.0082, 1.0048, 1.0021, 1, 0.9984, 0.9974, 0.9968, 0.9965, 0.9964, 0.9965, 0.9968, 0.9971, 0.9975, 0.9979, 0.9983, 0.9987, 1)';

export function flip(items: HTMLElement[], change: () => void) {
  if (reduced) return change();
  const before = new Map<HTMLElement, DOMRect>();
  const wasHidden = new Set<HTMLElement>();
  for (const el of items) {
    if (el.hidden) wasHidden.add(el);
    else before.set(el, el.getBoundingClientRect());
  }
  change();
  let shown = 0;
  for (const el of items) {
    if (el.hidden) continue;
    const old = before.get(el);
    if (!old) {
      el.animate(
        [{ opacity: 0, transform: 'scale(0.85) translateY(12px)' }, { opacity: 1, transform: 'none' }],
        { duration: 520, easing: SPRING, delay: Math.min(shown++ * 12, 240), fill: 'backwards' }
      );
      continue;
    }
    const now = el.getBoundingClientRect();
    const dx = old.left - now.left;
    const dy = old.top - now.top;
    if (Math.abs(dx) < 1 && Math.abs(dy) < 1) continue;
    el.animate([{ transform: `translate(${dx}px, ${dy}px)` }, { transform: 'none' }], { duration: 620, easing: SPRING });
  }
  void wasHidden;
}
