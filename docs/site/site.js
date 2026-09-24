// vendiOS site — small, dependency-free motion + docs navigation.
document.documentElement.classList.add('js');
const calm = matchMedia('(prefers-reduced-motion: reduce)').matches;

// Nav turns to glass once the page moves.
const nav = document.querySelector('.nav');
const onScrollNav = () => nav && nav.classList.toggle('is-scrolled', scrollY > 8);
addEventListener('scroll', onScrollNav, { passive: true }); onScrollNav();

// Reveal blocks once as they enter the viewport.
const io = new IntersectionObserver((entries) => {
  for (const e of entries) if (e.isIntersecting) { e.target.classList.add('is-in'); io.unobserve(e.target); }
}, { rootMargin: '0px 0px -8% 0px', threshold: 0.08 });
document.querySelectorAll('.reveal').forEach((el) => io.observe(el));

const clamp01 = (v) => Math.min(Math.max(v, 0), 1);
const easeOut = (t) => 1 - Math.pow(1 - t, 3);
const onFrame = (fn) => {
  let ticking = false;
  const run = () => { ticking = false; fn(); };
  addEventListener('scroll', () => { if (!ticking) { ticking = true; requestAnimationFrame(run); } }, { passive: true });
  addEventListener('resize', run); run();
};

// Hero stage: the screenshot starts small in a pool of mauve light and grows to
// full size while the section is pinned.
const stage = document.querySelector('.stage');
if (stage && !calm) {
  const shot = stage.querySelector('.shot');
  const pin = stage.querySelector('.stage-pin');
  onFrame(() => {
    if (innerWidth <= 700) { shot.style.transform = ''; pin.style.removeProperty('--glow'); return; }
    const r = stage.getBoundingClientRect();
    const start = innerHeight * 0.65, span = start + (r.height - innerHeight) * 0.75;
    const e = easeOut(clamp01((start - r.top) / span));
    shot.style.transform = `translateY(${(1 - e) * 8}vh) scale(${0.74 + e * 0.26})`;
    pin.style.setProperty('--glow', (1 - e * 0.4).toFixed(3));
  });
}

// Statement: words light up in reading order as it scrolls through.
document.querySelectorAll('[data-words]').forEach((el) => {
  const words = [];
  const wrap = (node, hl) => {
    for (const n of [...node.childNodes]) {
      if (n.nodeType === 3) {
        const frag = document.createDocumentFragment();
        n.textContent.split(/(\s+)/).forEach((t) => {
          if (!t) return;
          if (/^\s+$/.test(t)) { frag.append(t); return; }
          const w = document.createElement('span');
          w.className = 'w' + (hl ? ' hl' : ''); w.textContent = t;
          frag.append(w); words.push(w);
        });
        n.replaceWith(frag);
      } else if (n.nodeType === 1) {
        wrap(n, hl || n.classList.contains('hl'));
        if (n.classList.contains('hl')) n.replaceWith(...n.childNodes);
      }
    }
  };
  wrap(el, false);
  if (calm) { words.forEach((w) => w.classList.add('on')); return; }
  onFrame(() => {
    const r = el.getBoundingClientRect();
    const p = clamp01((innerHeight * 0.85 - r.top) / (r.height + innerHeight * 0.35));
    const n = Math.round(p * words.length);
    words.forEach((w, i) => w.classList.toggle('on', i < n));
  });
});

// Feature screenshots settle into place as they arrive.
document.querySelectorAll('.feature .shot').forEach((el) => io.observe(el));

// Big numbers count up once when they come into view.
const counter = new IntersectionObserver((entries) => {
  for (const e of entries) {
    if (!e.isIntersecting) continue;
    counter.unobserve(e.target);
    const el = e.target, to = parseFloat(el.dataset.count), dec = +(el.dataset.dec || 0);
    const unit = el.dataset.unit ? `<small>${el.dataset.unit}</small>` : '';
    if (calm || to === 0) continue;
    const t0 = performance.now(), dur = 1400;
    const step = (now) => {
      const k = Math.min((now - t0) / dur, 1), v = to * (1 - Math.pow(1 - k, 4));
      el.innerHTML = v.toFixed(dec) + unit;
      if (k < 1) requestAnimationFrame(step);
    };
    requestAnimationFrame(step);
  }
}, { threshold: 0.6 });
document.querySelectorAll('[data-count]').forEach((el) => counter.observe(el));
document.querySelectorAll('.get .mark-big').forEach((el) => io.observe(el));

// ── Bento tiles: cursor spotlight + tilt, and a live detail in each ──
const tiles = [...document.querySelectorAll('.b')];
const fine = matchMedia('(hover: hover) and (pointer: fine)').matches;
tiles.forEach((t) => {
  t.addEventListener('pointermove', (e) => {
    const r = t.getBoundingClientRect();
    const x = (e.clientX - r.left) / r.width, y = (e.clientY - r.top) / r.height;
    t.style.setProperty('--mx', `${x * 100}%`);
    t.style.setProperty('--my', `${y * 100}%`);
    if (fine && !calm) {
      t.style.setProperty('--rx', `${(0.5 - y) * 5}deg`);
      t.style.setProperty('--ry', `${(x - 0.5) * 5}deg`);
    }
    t.classList.add('is-hot');
  });
  t.addEventListener('pointerleave', () => {
    t.classList.remove('is-hot');
    t.style.setProperty('--rx', '0deg'); t.style.setProperty('--ry', '0deg');
  });
});

// Tiles only animate while they're on screen.
const live = new IntersectionObserver((entries) => {
  for (const e of entries) e.target.classList.toggle('is-live', e.isIntersecting);
}, { threshold: 0.2 });
tiles.forEach((t) => live.observe(t));
const isLive = (el) => el.closest('.b')?.classList.contains('is-live');
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const whenLive = async (el) => { while (!isLive(el)) await sleep(300); };

// Typed terminal lines that cycle through their commands.
document.querySelectorAll('[data-type]').forEach(async (el) => {
  const lines = JSON.parse(el.dataset.type);
  const out = el.parentElement.querySelector('.out');
  if (calm) { el.textContent = lines[0]; if (out) out.textContent = el.dataset.after || ''; return; }
  for (let i = 0; ; i = (i + 1) % lines.length) {
    await whenLive(el);
    for (const ch of lines[i]) { el.textContent += ch; await sleep(55 + Math.random() * 60); }
    if (out && el.dataset.after) { await sleep(350); out.textContent = el.dataset.after; }
    await sleep(2200);
    while (el.textContent) { el.textContent = el.textContent.slice(0, -1); await sleep(22); }
    if (out) out.textContent = '';
    await sleep(400);
  }
});

// 85 shortcuts: super + <key> presses, with what it does.
document.querySelectorAll('.keys-demo').forEach(async (box) => {
  const key = box.querySelector('.k'), act = box.querySelector('.act'), sup = box.querySelector('kbd');
  const binds = [['D', 'dashboard'], ['space', 'launcher'], ['E', 'files'], ['K', 'all shortcuts'], ['Q', 'close'], ['F', 'fullscreen'], ['G', 'tabs']];
  if (calm) return;
  for (let i = 0; ; i = (i + 1) % binds.length) {
    await whenLive(box);
    act.style.opacity = 0; await sleep(250);
    key.textContent = binds[i][0]; act.textContent = binds[i][1];
    sup.classList.add('down'); await sleep(160); key.classList.add('down'); act.style.opacity = 1;
    await sleep(380); key.classList.remove('down'); sup.classList.remove('down');
    await sleep(1600);
  }
});

// 9 themes: hover or tap a swatch to recolour the tile.
document.querySelectorAll('.themes-tile').forEach((tile) => {
  const btns = [...tile.querySelectorAll('.swatches button')], name = tile.querySelector('.tname');
  const pick = (b) => {
    btns.forEach((x) => x.setAttribute('aria-pressed', String(x === b)));
    tile.style.setProperty('--tile-accent', b.classList.contains('dyn') ? 'var(--theme-nord)' : getComputedStyle(b).getPropertyValue('--c'));
    name.textContent = b.getAttribute('aria-label').split(',')[0];
  };
  btns.forEach((b) => { b.addEventListener('pointerenter', () => pick(b)); b.addEventListener('focus', () => pick(b)); b.addEventListener('click', () => pick(b)); });
  pick(btns[0]);
});

// Encrypted: the passphrase types in, the lock opens.
document.querySelectorAll('.unlock').forEach(async (box) => {
  const dots = box.querySelector('.dots');
  if (calm) { dots.textContent = '••••••••••'; return; }
  for (;;) {
    await whenLive(box);
    dots.textContent = ''; box.classList.remove('open'); await sleep(700);
    for (let i = 0; i < 10; i++) { dots.textContent += '•'; await sleep(90 + Math.random() * 80); }
    await sleep(400); box.classList.add('open'); await sleep(2600);
  }
});

// Games: a frame counter that wobbles like a real one.
document.querySelectorAll('[data-fps]').forEach(async (el) => {
  if (calm) return;
  for (;;) { await whenLive(el); el.textContent = 138 + Math.round(Math.random() * 9); await sleep(450); }
});

// Docs: sliding marker in the index + highlight the section you're reading.
const rail = document.querySelector('.rail');
if (rail) {
  const marker = rail.querySelector('.marker');
  const links = [...rail.querySelectorAll('a[href^="#"]')];
  const byId = new Map(links.map((a) => [a.hash.slice(1), a]));
  const heads = [...byId.keys()].map((id) => document.getElementById(id)).filter(Boolean);
  let current = null;

  const moveMarker = (a) => {
    if (!a || !marker) return;
    marker.style.opacity = 1;
    marker.style.height = a.offsetHeight + 'px';
    marker.style.transform = `translateY(${a.offsetTop}px)`;
    // keep the active link visible inside a scrolling rail
    const box = rail.closest('.rail-box');
    if (box && box.scrollHeight > box.clientHeight) {
      const t = a.offsetTop - box.clientHeight / 2;
      box.scrollTo({ top: t, behavior: calm ? 'auto' : 'smooth' });
    }
  };
  const setActive = (a) => {
    if (a === current) return;
    current?.classList.remove('is-active');
    a?.classList.add('is-active');
    current = a; moveMarker(a);
  };
  let lock = 0; // while a click-scroll is animating, don't let the spy fight it
  const spy = () => {
    if (performance.now() < lock) return;
    const line = parseFloat(getComputedStyle(document.documentElement).scrollPaddingTop) + 8;
    let cur = heads[0];
    for (const h of heads) if (h.getBoundingClientRect().top <= line) cur = h;
    if (innerHeight + scrollY >= document.body.scrollHeight - 4) cur = heads[heads.length - 1];
    setActive(byId.get(cur.id));
  };
  addEventListener('scroll', spy, { passive: true });
  addEventListener('resize', () => moveMarker(current));

  const toggle = document.querySelector('.rail-toggle');
  const box = document.querySelector('.rail-box');
  const closeSheet = () => { box?.classList.remove('is-open'); toggle?.setAttribute('aria-expanded', 'false'); };
  toggle?.addEventListener('click', () => {
    const open = !box.classList.contains('is-open');
    box.classList.toggle('is-open', open);
    toggle.setAttribute('aria-expanded', String(open));
    if (open) requestAnimationFrame(() => moveMarker(current));
  });
  addEventListener('keydown', (e) => { if (e.key === 'Escape') closeSheet(); });

  links.forEach((a) => a.addEventListener('click', (e) => {
    const target = document.getElementById(a.hash.slice(1));
    if (!target) return;
    e.preventDefault();
    setActive(a);
    lock = performance.now() + 900;
    target.scrollIntoView({ behavior: calm ? 'auto' : 'smooth', block: 'start' });
    history.replaceState(null, '', a.hash);
    closeSheet();
  }));

  spy();
}
