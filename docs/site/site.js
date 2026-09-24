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

// Highlights gallery: snap scrolling with arrows, dots, and the centred card in focus.
document.querySelectorAll('.gallery').forEach((g) => {
  const track = g.querySelector('.g-track');
  const cards = [...track.querySelectorAll('.g-card')];
  const dotsBox = g.querySelector('.g-dots');
  const [prev, next] = g.querySelectorAll('.g-arrow');
  const dots = cards.map((c, i) => {
    const b = document.createElement('button');
    b.type = 'button'; b.setAttribute('aria-label', `Highlight ${i + 1}`);
    b.addEventListener('click', () => go(i));
    dotsBox.append(b); return b;
  });
  let cur = 0;
  const go = (i) => {
    const c = cards[Math.max(0, Math.min(cards.length - 1, i))];
    track.scrollTo({ left: c.offsetLeft - (track.clientWidth - c.clientWidth) / 2, behavior: calm ? 'auto' : 'smooth' });
  };
  const sync = () => {
    const mid = track.scrollLeft + track.clientWidth / 2;
    let best = 0, bestD = Infinity;
    cards.forEach((c, i) => { const d = Math.abs(c.offsetLeft + c.clientWidth / 2 - mid); if (d < bestD) { bestD = d; best = i; } });
    cur = best;
    cards.forEach((c, i) => c.classList.toggle('is-current', i === cur));
    dots.forEach((d, i) => d.setAttribute('aria-current', String(i === cur)));
    prev.disabled = cur === 0; next.disabled = cur === cards.length - 1;
  };
  prev.addEventListener('click', () => go(cur - 1));
  next.addEventListener('click', () => go(cur + 1));
  track.addEventListener('keydown', (e) => {
    if (e.key === 'ArrowRight') { e.preventDefault(); go(cur + 1); }
    if (e.key === 'ArrowLeft') { e.preventDefault(); go(cur - 1); }
  });
  let t; track.addEventListener('scroll', () => { cancelAnimationFrame(t); t = requestAnimationFrame(sync); }, { passive: true });
  addEventListener('resize', sync); sync();
});

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
