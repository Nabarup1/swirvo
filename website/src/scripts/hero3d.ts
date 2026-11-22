// The hero: the Swirvo tile as a real 3D object, lit like a studio render. Built from the
// same geometry as the app icon (lib/logo.ts). Tilt follows the pointer; drag or flick to
// spin the swirl, which settles into 120° detents like a wheel clicking into place.
// Format chips orbit it. Loads after first paint; the SVG tile stays if WebGL can't run.
import * as THREE from 'three';
import { RoomEnvironment } from 'three/addons/environments/RoomEnvironment.js';
import { blade, tile, GRADIENT } from '../lib/logo';
import { Spring } from './motion';

const U = 1 / 64; // design units → world units
const TAU = Math.PI * 2;
const DETENT = TAU / 3;

export function startHero(host: HTMLElement, canvas: HTMLCanvasElement) {
  let renderer: THREE.WebGLRenderer;
  try {
    // Without a real GPU the browser says so, and the SVG tile stays: software WebGL runs
    // this scene at a frame every few seconds and freezes the page with it.
    renderer = new THREE.WebGLRenderer({
      canvas,
      antialias: true,
      alpha: true,
      powerPreference: 'high-performance',
      failIfMajorPerformanceCaveat: true,
    });
  } catch {
    return null;
  }
  {
    // Not every browser reports that caveat, so also ask who is doing the drawing.
    const gl = renderer.getContext();
    const info = gl.getExtension('WEBGL_debug_renderer_info');
    const who = String(gl.getParameter(info ? info.UNMASKED_RENDERER_WEBGL : gl.RENDERER));
    if (/swiftshader|llvmpipe|softpipe|software|basic render/i.test(who)) {
      renderer.dispose();
      canvas.style.pointerEvents = 'none';
      return null;
    }
  }
  const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;
  let dpr = Math.min(devicePixelRatio || 1, matchMedia('(pointer: coarse)').matches ? 1.5 : 1.75);
  renderer.setPixelRatio(dpr);
  renderer.outputColorSpace = THREE.SRGBColorSpace;
  // Neutral keeps the brand's orange; ACES bleached it to peach.
  renderer.toneMapping = THREE.NeutralToneMapping;
  renderer.toneMappingExposure = 1;

  const scene = new THREE.Scene();
  const camera = new THREE.PerspectiveCamera(26, 1, 0.1, 100);
  camera.position.set(0, 0.1, 15.5);

  const pmrem = new THREE.PMREMGenerator(renderer);
  scene.environment = pmrem.fromScene(new RoomEnvironment(), 0.035).texture;
  scene.environmentIntensity = 0.5;

  const key = new THREE.DirectionalLight(0xfff0e0, 1.5);
  key.position.set(-4, 5, 7);
  const rim = new THREE.DirectionalLight(0xb4a6ff, 1.6);
  rim.position.set(5, -3, -5);
  const warm = new THREE.PointLight(0xffa060, 7, 18, 1.6);
  warm.position.set(2, 1, 5);
  scene.add(key, rim, warm, new THREE.AmbientLight(0xffffff, 0.12));

  /* ── The tile ─────────────────────────────────────────── */
  const t = tile();
  const half = (t.size / 2) * U;
  const r = t.radius * U;
  const tileShape = new THREE.Shape();
  tileShape.moveTo(-half + r, -half);
  tileShape.lineTo(half - r, -half);
  tileShape.absarc(half - r, -half + r, r, -Math.PI / 2, 0, false);
  tileShape.lineTo(half, half - r);
  tileShape.absarc(half - r, half - r, r, 0, Math.PI / 2, false);
  tileShape.lineTo(-half + r, half);
  tileShape.absarc(-half + r, half - r, r, Math.PI / 2, Math.PI, false);
  tileShape.lineTo(-half, -half + r);
  tileShape.absarc(-half + r, -half + r, r, Math.PI, Math.PI * 1.5, false);
  const depth = 0.46;
  const bevel = 0.18;
  const tileGeo = new THREE.ExtrudeGeometry(tileShape, {
    depth,
    bevelEnabled: true,
    bevelThickness: bevel,
    bevelSize: 0.16,
    bevelSegments: 14,
    curveSegments: 40,
  });
  tileGeo.translate(0, 0, -depth / 2);
  // The app's gradient, top-left to bottom-right, as vertex colours.
  {
    const pos = tileGeo.attributes.position;
    const top = new THREE.Color(GRADIENT.top);
    const bottom = new THREE.Color(GRADIENT.bottom);
    const c = new THREE.Color();
    const colors = new Float32Array(pos.count * 3);
    for (let i = 0; i < pos.count; i++) {
      const k = THREE.MathUtils.clamp((pos.getX(i) + half - pos.getY(i) + half) / (4 * half), 0, 1);
      c.copy(top).lerp(bottom, k);
      colors.set([c.r, c.g, c.b], i * 3);
    }
    tileGeo.setAttribute('color', new THREE.BufferAttribute(colors, 3));
  }
  const tileMat = new THREE.MeshPhysicalMaterial({
    vertexColors: true,
    roughness: 0.34,
    metalness: 0,
    clearcoat: 1,
    clearcoatRoughness: 0.14,
    sheen: 0.08,
    sheenColor: new THREE.Color(0xffc08a),
    sheenRoughness: 0.45,
  });
  const tileMesh = new THREE.Mesh(tileGeo, tileMat);
  const face = depth / 2 + bevel;

  /* ── The swirl ────────────────────────────────────────── */
  const bladeMat = new THREE.MeshPhysicalMaterial({
    color: 0xffffff,
    roughness: 0.2,
    metalness: 0,
    clearcoat: 1,
    clearcoatRoughness: 0.05,
    iridescence: 0.55,
    iridescenceIOR: 1.32,
  });
  const swirl = new THREE.Group();
  const blades: THREE.Mesh[] = [];
  for (let k = 0; k < 3; k++) {
    const pts = blade(k, false, 48);
    const shape = new THREE.Shape();
    const seen: [number, number][] = [];
    for (const [x, y] of pts) {
      const p: [number, number] = [(x - 128) * U, -(y - 128) * U];
      const prev = seen[seen.length - 1];
      if (prev && Math.hypot(prev[0] - p[0], prev[1] - p[1]) < 1e-4) continue;
      seen.push(p);
    }
    if (Math.hypot(seen[0][0] - seen.at(-1)![0], seen[0][1] - seen.at(-1)![1]) < 1e-4) seen.pop();
    seen.forEach(([x, y], i) => (i ? shape.lineTo(x, y) : shape.moveTo(x, y)));
    const g = new THREE.ExtrudeGeometry(shape, { depth: 0.2, bevelEnabled: false, curveSegments: 1 });
    const m = new THREE.Mesh(g, bladeMat);
    const pivot = new THREE.Group();
    pivot.add(m);
    swirl.add(pivot);
    blades.push(m);
  }
  const dotGeo = new THREE.CylinderGeometry(15 * U, 15 * U, 0.26, 48);
  dotGeo.rotateX(Math.PI / 2);
  const dot = new THREE.Mesh(dotGeo, bladeMat);
  dot.position.z = 0.13;
  swirl.add(dot);
  swirl.position.z = face - 0.02;

  const token = new THREE.Group();
  token.add(tileMesh, swirl);
  const rig = new THREE.Group();
  rig.add(token);
  scene.add(rig);

  /* ── Soft contact shadow ──────────────────────────────── */
  const shCanvas = document.createElement('canvas');
  shCanvas.width = shCanvas.height = 128;
  {
    const g = shCanvas.getContext('2d')!;
    const grad = g.createRadialGradient(64, 64, 4, 64, 64, 62);
    grad.addColorStop(0, 'rgba(0,0,0,0.55)');
    grad.addColorStop(1, 'rgba(0,0,0,0)');
    g.fillStyle = grad;
    g.fillRect(0, 0, 128, 128);
  }
  const shadow = new THREE.Mesh(
    new THREE.PlaneGeometry(6, 2.2),
    new THREE.MeshBasicMaterial({ map: new THREE.CanvasTexture(shCanvas), transparent: true, depthWrite: false })
  );
  shadow.position.set(0, -3.1, -0.5);
  shadow.rotation.x = -Math.PI / 2.6;
  scene.add(shadow);

  /* ── Orbiting format chips ────────────────────────────── */
  const LABELS: [string, string][] = [
    ['PDF', '#ff7a8a'], ['DOCX', '#78c7ff'], ['JPG', '#6fd69a'], ['HEIC', '#6fd69a'], ['MP4', '#b4a6ff'],
    ['XLSX', '#5fd08c'], ['PNG', '#6fd69a'], ['MP3', '#ff9ad5'], ['EPUB', '#78c7ff'], ['CSV', '#5fd08c'],
    ['WEBP', '#6fd69a'], ['ZIP', '#ffd27a'],
  ];
  const chips: { sprite: THREE.Sprite; phase: number; tex: THREE.CanvasTexture; label: string; dot: string }[] = [];
  const drawChip = (cv: HTMLCanvasElement, label: string, dotColor: string, light: boolean) => {
    const g = cv.getContext('2d')!;
    const w = cv.width;
    const h = cv.height;
    g.clearRect(0, 0, w, h);
    const rr = h / 2 - 4;
    g.beginPath();
    g.roundRect(4, 4, w - 8, h - 8, rr);
    g.fillStyle = light ? 'rgba(255,252,248,0.94)' : 'rgba(30,25,35,0.9)';
    g.fill();
    g.lineWidth = 3;
    g.strokeStyle = light ? 'rgba(60,30,10,0.14)' : 'rgba(255,236,220,0.18)';
    g.stroke();
    g.beginPath();
    g.arc(h / 2 + 6, h / 2, 9, 0, TAU);
    g.fillStyle = dotColor;
    g.fill();
    g.font = `600 ${Math.round(h * 0.42)}px "Instrument Sans Variable", system-ui, sans-serif`;
    g.fillStyle = light ? '#1d1410' : '#f6efe7';
    g.textBaseline = 'middle';
    g.fillText(label, h / 2 + 26, h / 2 + 2);
  };
  const isLight = () => document.documentElement.dataset.theme === 'light';
  LABELS.forEach(([label, color], i) => {
    const cv = document.createElement('canvas');
    cv.width = 256;
    cv.height = 96;
    drawChip(cv, label, color, isLight());
    const tex = new THREE.CanvasTexture(cv);
    tex.colorSpace = THREE.SRGBColorSpace;
    tex.anisotropy = 4;
    const sprite = new THREE.Sprite(new THREE.SpriteMaterial({ map: tex, transparent: true, depthWrite: false }));
    sprite.scale.set(1.28, 0.48, 1);
    scene.add(sprite);
    chips.push({ sprite, phase: (i / LABELS.length) * TAU, tex, label, dot: color });
  });
  const redrawChips = () =>
    chips.forEach((c) => {
      drawChip(c.tex.image as HTMLCanvasElement, c.label, c.dot, isLight());
      c.tex.needsUpdate = true;
    });
  document.fonts?.ready.then(redrawChips);
  document.addEventListener('themechange', () => {
    redrawChips();
    renderer.toneMappingExposure = isLight() ? 0.97 : 1;
    (shadow.material as THREE.MeshBasicMaterial).opacity = isLight() ? 0.6 : 1;
  });

  /* ── Motion state ─────────────────────────────────────── */
  const tiltX = new Spring(0, 60, 11);
  const tiltY = new Spring(0, 60, 11);
  // Starts near full size: it takes over from the SVG tile, so must never be a blank.
  const intro = new Spring(reduced ? 1 : 0.72, 90, 12);
  intro.target = 1;
  const introBlades = [0, 1, 2].map(() => new Spring(reduced ? 1 : 0, 90, 10));
  const orbitR = new Spring(1, 120, 9);
  const detent = new Spring(0, 34, 5.5);
  let angle = 0;
  let omega = 0;
  let dragging = false;
  let settling = false;
  let lastPointerAngle = 0;
  let lastMoveT = 0;
  let downAt = { x: 0, y: 0, t: 0 };
  let scrollP = 0;
  let orbit = 0;
  let nextTick = 2.6;
  const t0 = performance.now();
  if (!reduced) introBlades.forEach((s, i) => setTimeout(() => (s.target = 1), 260 + i * 140));
  else introBlades.forEach((s) => (s.target = 1));

  const centerOnScreen = () => {
    const v = new THREE.Vector3();
    token.getWorldPosition(v);
    v.project(camera);
    const rect = canvas.getBoundingClientRect();
    return { x: rect.left + ((v.x + 1) / 2) * rect.width, y: rect.top + ((1 - v.y) / 2) * rect.height };
  };

  addEventListener('pointermove', (e) => {
    if (e.pointerType !== 'mouse') return; // a finger scrolling the page shouldn't tip the logo
    const rect = host.getBoundingClientRect();
    const nx = ((e.clientX - rect.left) / rect.width) * 2 - 1;
    const ny = ((e.clientY - rect.top) / rect.height) * 2 - 1;
    if (!dragging) {
      tiltY.target = THREE.MathUtils.clamp(nx, -1.2, 1.2) * 0.38;
      tiltX.target = THREE.MathUtils.clamp(ny, -1.2, 1.2) * 0.28;
    }
    warm.position.set(nx * 5, -ny * 3.5, 5);
  }, { passive: true });

  canvas.addEventListener('pointerdown', (e) => {
    if (e.button !== 0) return;
    canvas.setPointerCapture(e.pointerId);
    dragging = true;
    settling = false;
    const c = centerOnScreen();
    lastPointerAngle = Math.atan2(e.clientY - c.y, e.clientX - c.x);
    lastMoveT = performance.now();
    downAt = { x: e.clientX, y: e.clientY, t: lastMoveT };
    host.classList.add('grabbing');
  });
  canvas.addEventListener('pointermove', (e) => {
    if (!dragging) return;
    const c = centerOnScreen();
    const a = Math.atan2(e.clientY - c.y, e.clientX - c.x);
    let d = a - lastPointerAngle;
    if (d > Math.PI) d -= TAU;
    if (d < -Math.PI) d += TAU;
    lastPointerAngle = a;
    const now = performance.now();
    const dt = Math.max(1, now - lastMoveT) / 1000;
    lastMoveT = now;
    angle -= d; // screen y is down; the swirl turns with the finger
    omega = omega * 0.5 + (-d / dt) * 0.5;
  });
  const release = (e: PointerEvent) => {
    if (!dragging) return;
    dragging = false;
    host.classList.remove('grabbing');
    const tap = Math.hypot(e.clientX - downAt.x, e.clientY - downAt.y) < 6 && performance.now() - downAt.t < 350;
    if (tap) {
      omega -= 7.5;
      orbitR.velocity += 3.2;
    }
    if (performance.now() - lastMoveT > 120) omega *= 0.2;
  };
  canvas.addEventListener('pointerup', release);
  canvas.addEventListener('pointercancel', release);

  addEventListener('scroll', () => {
    const rect = host.getBoundingClientRect();
    scrollP = THREE.MathUtils.clamp(-rect.top / Math.max(1, rect.height * 0.9), 0, 1);
  }, { passive: true });

  /* ── Size and loop ────────────────────────────────────── */
  let small = false;
  const resize = () => {
    const w = canvas.clientWidth;
    const h = canvas.clientHeight;
    if (!w || !h) return;
    renderer.setSize(w, h, false);
    camera.aspect = w / h;
    // 26° tall, opened up when the canvas is too narrow to hold the whole orbit.
    small = w < 600; // fewer, larger chips on a closer orbit, so they can still be read
    const reach = small ? 4.5 : 4.9;
    const tan = Math.max(Math.tan(THREE.MathUtils.degToRad(13)), reach / (camera.position.z * camera.aspect));
    camera.fov = THREE.MathUtils.radToDeg(Math.atan(tan)) * 2;
    camera.updateProjectionMatrix();
  };
  new ResizeObserver(resize).observe(canvas);
  resize();

  let visible = true;
  let raf = 0;
  let last = performance.now();
  let slow = 0;
  let stalled = 0;
  let stopped = false;
  let drawn = false;
  // Back to the SVG tile, when the GPU can't keep up or the phone takes the context away.
  const stop = () => {
    if (stopped) return;
    stopped = true;
    cancelAnimationFrame(raf);
    raf = 0;
    host.classList.remove('gl-ready');
    canvas.style.opacity = '';
    canvas.style.pointerEvents = 'none';
  };
  canvas.addEventListener('webglcontextlost', stop);
  new IntersectionObserver(([e]) => {
    visible = e.isIntersecting;
    if (visible && !raf && !stopped) {
      last = performance.now();
      raf = requestAnimationFrame(loop);
    }
  }).observe(host);
  document.addEventListener('visibilitychange', () => {
    if (!document.hidden && visible && !raf && !stopped) {
      last = performance.now();
      raf = requestAnimationFrame(loop);
    }
  });

  function loop(now: number) {
    raf = 0;
    if (!visible || document.hidden || stopped) return;
    const dt = Math.min(0.05, (now - last) / 1000);
    const frameMs = now - last;
    last = now;
    const time = (now - t0) / 1000;

    // Adaptive quality: drop resolution if frames are slow.
    slow = frameMs > 24 ? slow + 1 : Math.max(0, slow - 1);
    if (slow > 50 && dpr > 1) {
      dpr = 1;
      renderer.setPixelRatio(dpr);
      resize();
      slow = 0;
    }
    if (frameMs > 55) stalled += frameMs > 500 ? 15 : 1;
    else stalled = Math.max(0, stalled - 2);
    if (stalled > 45) return stop();

    intro.step(dt);
    introBlades.forEach((s) => s.step(dt));
    tiltX.step(dt);
    tiltY.step(dt);
    orbitR.step(dt);

    // The swirl: free spin with friction, then a springy click into the nearest detent.
    if (!dragging) {
      if (!reduced && time > nextTick && Math.abs(omega) < 0.3 && !settling) {
        omega -= 4.2;
        nextTick = time + 3.6 + Math.random() * 2;
      }
      omega *= Math.exp(-1.6 * dt);
      if (Math.abs(omega) > 0.5) {
        angle += omega * dt;
        settling = false;
      } else {
        if (!settling) {
          settling = true;
          detent.value = angle;
          detent.velocity = omega;
          detent.target = Math.round(angle / DETENT) * DETENT;
        }
        detent.step(dt);
        angle = detent.value;
        omega = 0;
      }
    }
    swirl.rotation.z = angle;
    swirl.children.slice(0, 3).forEach((p, i) => {
      const s = introBlades[i].value;
      p.rotation.z = (1 - s) * -2.4;
      p.scale.setScalar(Math.max(0.001, 0.3 + 0.7 * s));
    });

    const k = Math.max(0.001, intro.value);
    const lift = reduced ? 0 : Math.sin(time * 1.1) * 0.09;
    token.scale.setScalar(k * (1 - scrollP * 0.3));
    token.rotation.set(
      tiltX.value + (reduced ? 0 : Math.sin(time * 0.6) * 0.05) + scrollP * 0.7,
      tiltY.value + (1 - k) * -1.6 + (reduced ? 0 : Math.cos(time * 0.5) * 0.06),
      (1 - k) * 0.4
    );
    token.position.y = lift + scrollP * 1.4;
    shadow.scale.setScalar(1 - lift * 0.8 - scrollP * 0.4);
    (shadow.material as THREE.MeshBasicMaterial).opacity = (isLight() ? 0.6 : 1) * (1 - scrollP) * k;

    // Chips orbit faster when the swirl spins.
    orbit += dt * (0.22 + Math.min(4, Math.abs(omega)) * 0.12) * (reduced ? 0 : 1);
    const R = 3.55 * orbitR.value;
    chips.forEach((c, i) => {
      const a = c.phase + orbit;
      c.sprite.visible = !small || i % 2 === 0;
      // The ring leans toward you, passing under the swirl in front and over it behind.
      const y = -Math.sin(a) * 1.85 + Math.sin(a * 2 + i) * 0.14;
      c.sprite.position.set(Math.cos(a) * R * (small ? 0.9 : 1.12), y + token.position.y * 0.6, Math.sin(a) * R * 0.55);
      const front = (Math.sin(a) + 1) / 2;
      (c.sprite.material as THREE.SpriteMaterial).opacity = (0.35 + 0.65 * front) * k * (1 - scrollP * 0.8);
      const sc = (0.85 + front * 0.3) * k * (small ? 1.6 : 1.1);
      c.sprite.scale.set(1.28 * sc, 0.48 * sc, 1);
    });

    if (drawn) canvas.style.opacity = String(1 - scrollP * 0.9);
    renderer.render(scene, camera);
    if (!drawn) {
      drawn = true;
      host.classList.add('gl-ready');
    }
    raf = requestAnimationFrame(loop);
  }
  raf = requestAnimationFrame(loop);
  return renderer;
}
