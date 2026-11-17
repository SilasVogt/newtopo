// newtopo site: small progressive enhancements. Everything works without this file.
(() => {
  "use strict";

  /* ---------------------------------------------------------- before / after slider */
  for (const box of document.querySelectorAll(".compare")) {
    const range = box.querySelector(".compare__range");
    const set = () => box.style.setProperty("--pos", `${range.value}%`);
    range.addEventListener("input", set);
    set();
  }

  /* ---------------------------------------------------------- core loop: sticky stage */
  const frame = document.querySelector(".loop__frame");
  const steps = [...document.querySelectorAll(".loop__steps .step")];
  if (frame && steps.length) {
    const stageImgs = steps.map((step, i) => {
      const src = step.querySelector(".step__art img");
      const img = document.createElement("img");
      img.src = src.currentSrc || src.src;
      img.width = src.width;
      img.height = src.height;
      img.alt = "";
      img.decoding = "async";
      if (i === 0) img.className = "is-on";
      frame.appendChild(img);
      return img;
    });
    const activate = (i) => {
      steps.forEach((s, k) => s.classList.toggle("is-on", k === i));
      stageImgs.forEach((im, k) => im.classList.toggle("is-on", k === i));
    };
    activate(0);
    const io = new IntersectionObserver((entries) => {
      for (const e of entries) if (e.isIntersecting) activate(steps.indexOf(e.target));
    }, { rootMargin: "-45% 0px -45% 0px" });
    steps.forEach((s) => io.observe(s));
  }

  /* ---------------------------------------------------------- OS detection for downloads */
  // Phones and tablets get no "Your system" badge (there is no build for
  // them); they get a note to download on a computer instead.
  const ua = (navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || navigator.userAgent || "";
  const handheld = (navigator.userAgentData && navigator.userAgentData.mobile) ||
    /android|iphone|ipad|ipod|mobile/i.test(navigator.userAgent) ||
    (/mac/i.test(ua) && navigator.maxTouchPoints > 1);
  const os = handheld ? null : /win/i.test(ua) ? "windows" : /mac/i.test(ua) ? "mac" : /linux|x11/i.test(ua) ? "linux" : null;
  if (handheld) {
    const note = document.querySelector(".os__handheld");
    if (note) note.hidden = false;
    // The download buttons still lead to the download section, which
    // explains that the app needs a computer; say so on the button.
    document.querySelectorAll("[data-download-cta]").forEach((a) => { a.textContent = "Get it for desktop"; });
  }
  if (os) {
    const card = document.querySelector(`.os__card[data-os="${os}"]`);
    if (card) card.classList.add("is-mine");
  }

  /* ---------------------------------------------------------- bridge: one send animation when it scrolls in */
  const bridge = document.querySelector(".bridge");
  if (bridge && "IntersectionObserver" in window) {
    const bio = new IntersectionObserver(([e]) => {
      if (e.isIntersecting) { bridge.classList.add("is-in"); bio.disconnect(); }
    }, { threshold: 0.5 });
    bio.observe(bridge);
  }

  /* ---------------------------------------------------------- interactive torus (Connect A x B) */
  const host = document.getElementById("torus");
  const inA = document.getElementById("torus-a");
  const inB = document.getElementById("torus-b");
  if (!host || !inA || !inB) return;

  const LEVELS = 28;
  const css = [];
  for (let i = 0; i <= LEVELS; i++) {
    css.push(`.torus .l${i}{--f:color-mix(in oklab,var(--hi) ${Math.round((100 * i) / LEVELS)}%,var(--lo))}`);
  }
  const st = document.createElement("style");
  st.textContent = css.join("");
  document.head.appendChild(st);

  // Same orthographic camera as tools/site-art/gen_art.py
  const deg = Math.PI / 180;
  const yaw = -30 * deg, elev = 36 * deg;
  const ca = Math.cos(yaw), sa = Math.sin(yaw), ce = Math.cos(elev), se = Math.sin(elev);
  const Rv = [ca, -sa, 0], Uv = [sa * se, ca * se, ce], Vv = [-sa * ce, -ca * ce, se];
  const dot = (a, b) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
  const nrm = (a) => { const l = Math.hypot(a[0], a[1], a[2]); return [a[0] / l, a[1] / l, a[2] / l]; };
  const L = nrm([Rv[0] * -0.45 + Uv[0] * 0.75 + Vv[0] * 0.55, Rv[1] * -0.45 + Uv[1] * 0.75 + Vv[1] * 0.55, Rv[2] * -0.45 + Uv[2] * 0.75 + Vv[2] * 0.55]);
  const W = 480, H = 360, S = 92, CX = W / 2, CY = H / 2 + 6;
  const R0 = 1.55, r0 = 0.62, TAU = Math.PI * 2;
  const P = (t, p) => { const k = R0 + r0 * Math.cos(p); return [k * Math.cos(t), k * Math.sin(t), r0 * Math.sin(p)]; };
  const N = (t, p) => [Math.cos(p) * Math.cos(t), Math.cos(p) * Math.sin(t), Math.sin(p)];
  const proj = (q) => [CX + dot(q, Rv) * S, CY - dot(q, Uv) * S];
  const f1 = (v) => Math.round(v * 10) / 10;
  const shade = (n) => {
    const d = Math.max(0, dot(n, L));
    const rim = Math.pow(Math.max(0, 1 - dot(n, Vv)), 3) * 0.12;
    return Math.max(0, Math.min(LEVELS, Math.round((0.1 + 0.86 * d + rim) * LEVELS)));
  };

  // Precompute the shaded surface once.
  const NU = 96, NV = 40, surf = [];
  for (let i = 0; i < NU; i++) {
    for (let j = 0; j < NV; j++) {
      const t0 = (TAU * i) / NU, t1 = (TAU * (i + 1)) / NU, p0 = (TAU * j) / NV, p1 = (TAU * (j + 1)) / NV;
      const n = N((t0 + t1) / 2, (p0 + p1) / 2);
      if (dot(n, Vv) <= 0) continue;
      const pts = [P(t0, p0), P(t1, p0), P(t1, p1), P(t0, p1)];
      const depth = pts.reduce((s, q) => s + dot(q, Vv), 0) / 4;
      const d = "M" + pts.map((q) => proj(q).map(f1).join(" ")).join("L") + "Z";
      surf.push({ depth, html: `<path class="l${shade(n)}" d="${d}"/>` });
    }
  }

  const lineSegs = (a, b) => {
    const out = [];
    const seg = (q0, q1, n, cls) => {
      if (dot(n, Vv) <= 0.02) return;
      const depth = (dot(q0, Vv) + dot(q1, Vv)) / 2 + 0.09;
      const [x0, y0] = proj(q0), [x1, y1] = proj(q1);
      out.push({ depth, html: `<path class="${cls}" d="M${f1(x0)} ${f1(y0)}L${f1(x1)} ${f1(y1)}"/>` });
    };
    const steps = 72;
    for (let k = 0; k < a; k++) { // A: around the ring, each line a small circle around the tube
      const t = (TAU * k) / a + 0.13;
      for (let s = 0; s < steps; s++) {
        const p0 = (TAU * s) / steps, p1 = (TAU * (s + 1)) / steps;
        seg(P(t, p0), P(t, p1), N(t, (p0 + p1) / 2), "ln");
      }
    }
    for (let k = 0; k < b; k++) { // B: around the tube, each line a loop around the ring
      const p = (TAU * k) / b + 0.21;
      for (let s = 0; s < steps * 2; s++) {
        const t0 = (TAU * s) / (steps * 2), t1 = (TAU * (s + 1)) / (steps * 2);
        seg(P(t0, p), P(t1, p), N((t0 + t1) / 2, p), "lb");
      }
    }
    return out;
  };

  const outA = document.getElementById("torus-a-out");
  const outB = document.getElementById("torus-b-out");
  const quads = document.getElementById("torus-quads");
  let raf = 0;
  const draw = () => {
    raf = 0;
    const a = +inA.value, b = +inB.value;
    const items = surf.concat(lineSegs(a, b)).sort((x, y) => x.depth - y.depth);
    host.innerHTML = `<svg viewBox="0 0 ${W} ${H}" aria-hidden="true" focusable="false">${items.map((x) => x.html).join("")}</svg>`;
    outA.value = outA.textContent = a;
    outB.value = outB.textContent = b;
    quads.textContent = a * b;
  };
  const queue = () => { if (!raf) raf = requestAnimationFrame(draw); };
  inA.addEventListener("input", queue);
  inB.addEventListener("input", queue);
  draw();
})();
