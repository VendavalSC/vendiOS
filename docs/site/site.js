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

// Hero screenshot: starts slightly tilted back and small, settles flat as you scroll to it.
const hero = document.querySelector('.showcase .shot');
if (hero && !calm) {
  let ticking = false;
  const update = () => {
    ticking = false;
    const r = hero.getBoundingClientRect();
    const p = Math.min(Math.max(1 - (r.top - innerHeight * 0.15) / (innerHeight * 0.75), 0), 1); // 0 → 1
    const e = 1 - Math.pow(1 - p, 3);
    hero.style.transform = `perspective(1600px) rotateX(${(1 - e) * 14}deg) scale(${0.9 + e * 0.1})`;
  };
  addEventListener('scroll', () => { if (!ticking) { ticking = true; requestAnimationFrame(update); } }, { passive: true });
  addEventListener('resize', update); update();
}

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
