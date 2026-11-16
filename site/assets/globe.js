// Interactive wireframe globe for the hero. Pure canvas, no libraries.
(() => {
  const canvas = document.getElementById("globe");
  if (!canvas) return;
  const ctx = canvas.getContext("2d");
  const reduce = matchMedia("(prefers-reduced-motion: reduce)").matches;
  const ACCENT = [63, 208, 192];
  let W = 0, H = 0, dpr = 1, R = 0;
  const N = 900;
  const pts = [];
  for (let i = 0; i < N; i++) {           // fibonacci sphere
    const y = 1 - (i / (N - 1)) * 2, r = Math.sqrt(1 - y * y), t = i * 2.399963;
    pts.push([Math.cos(t) * r, y, Math.sin(t) * r]);
  }
  const rings = [];
  for (let k = 0; k < 6; k++) {           // meridians
    const a = (k / 6) * Math.PI, ring = [];
    for (let j = 0; j <= 96; j++) { const t = (j / 96) * Math.PI * 2; ring.push([Math.cos(t) * Math.cos(a), Math.sin(t), Math.cos(t) * Math.sin(a)]); }
    rings.push(ring);
  }
  for (const lat of [-0.5, 0, 0.5]) {     // parallels
    const ring = [], r = Math.sqrt(1 - lat * lat);
    for (let j = 0; j <= 96; j++) { const t = (j / 96) * Math.PI * 2; ring.push([Math.cos(t) * r, lat, Math.sin(t) * r]); }
    rings.push(ring);
  }
  let rotY = 0, tiltX = -0.35, tiltY = 0, targetX = -0.35, targetY = 0, spin = 0.0025;
  let visible = true, last = performance.now();

  function resize() {
    dpr = Math.min(devicePixelRatio || 1, 2);
    const b = canvas.getBoundingClientRect();
    W = b.width; H = b.height;
    canvas.width = W * dpr; canvas.height = H * dpr;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    R = Math.min(W, H) * 0.36;
  }
  function project(p) {
    let [x, y, z] = p;
    const cy = Math.cos(rotY + tiltY), sy = Math.sin(rotY + tiltY);
    [x, z] = [x * cy - z * sy, x * sy + z * cy];
    const cx = Math.cos(tiltX), sx = Math.sin(tiltX);
    [y, z] = [y * cx - z * sx, y * sx + z * cx];
    const s = 2.6 / (2.6 + z);
    return [W / 2 + x * R * s, H / 2 + y * R * s, z, s];
  }
  function frame(now) {
    const dt = Math.min(50, now - last); last = now;
    if (visible) {
      rotY += spin * (dt / 16);
      tiltX += (targetX - tiltX) * 0.06; tiltY += (targetY - tiltY) * 0.06;
      ctx.clearRect(0, 0, W, H);
      // glow
      const g = ctx.createRadialGradient(W / 2, H / 2, R * 0.2, W / 2, H / 2, R * 1.6);
      g.addColorStop(0, `rgba(${ACCENT},0.16)`); g.addColorStop(1, "rgba(0,0,0,0)");
      ctx.fillStyle = g; ctx.fillRect(0, 0, W, H);
      // rings
      ctx.lineWidth = 1;
      for (const ring of rings) {
        let prev = null;
        for (const p of ring) {
          const q = project(p);
          if (prev) {
            const a = 0.05 + 0.35 * (1 - (q[2] + 1) / 2);
            ctx.strokeStyle = `rgba(${ACCENT},${a.toFixed(3)})`;
            ctx.beginPath(); ctx.moveTo(prev[0], prev[1]); ctx.lineTo(q[0], q[1]); ctx.stroke();
          }
          prev = q;
        }
      }
      // dots
      for (const p of pts) {
        const q = project(p), front = 1 - (q[2] + 1) / 2;
        ctx.fillStyle = `rgba(${ACCENT},${(0.08 + front * 0.8).toFixed(3)})`;
        const s = 0.6 + front * 1.4 * q[3];
        ctx.fillRect(q[0] - s / 2, q[1] - s / 2, s, s);
      }
      // orbiting moon
      const t = now / 1600;
      const m = project([Math.cos(t) * 1.35, Math.sin(t * 0.7) * 0.35 - 0.2, Math.sin(t) * 1.35]);
      const mg = ctx.createRadialGradient(m[0], m[1], 0, m[0], m[1], 14 * m[3]);
      mg.addColorStop(0, `rgba(${ACCENT},1)`); mg.addColorStop(1, `rgba(${ACCENT},0)`);
      ctx.fillStyle = mg; ctx.beginPath(); ctx.arc(m[0], m[1], 14 * m[3], 0, Math.PI * 2); ctx.fill();
      ctx.fillStyle = "#e6fffb"; ctx.beginPath(); ctx.arc(m[0], m[1], 3.2 * m[3], 0, Math.PI * 2); ctx.fill();
    }
    if (!reduce) requestAnimationFrame(frame);
  }
  addEventListener("resize", resize);
  addEventListener("pointermove", (e) => {
    targetY = (e.clientX / innerWidth - 0.5) * 0.9;
    targetX = -0.35 + (e.clientY / innerHeight - 0.5) * 0.5;
  });
  // drag to spin
  let dragging = false, lx = 0;
  canvas.addEventListener("pointerdown", (e) => { dragging = true; lx = e.clientX; canvas.setPointerCapture(e.pointerId); });
  canvas.addEventListener("pointerup", () => { dragging = false; spin = 0.0025; });
  canvas.addEventListener("pointermove", (e) => { if (dragging) { rotY += (e.clientX - lx) * 0.008; lx = e.clientX; spin = 0; } });
  new IntersectionObserver(([e]) => (visible = e.isIntersecting)).observe(canvas);
  resize();
  requestAnimationFrame(frame);
})();

// Cursor spotlight on cards, magnetic buttons, 3D tilt on the screenshot.
(() => {
  const reduce = matchMedia("(prefers-reduced-motion: reduce)").matches;
  document.querySelectorAll(".spot").forEach((el) => el.addEventListener("pointermove", (e) => {
    const r = el.getBoundingClientRect();
    el.style.setProperty("--x", `${e.clientX - r.left}px`);
    el.style.setProperty("--y", `${e.clientY - r.top}px`);
  }));
  if (reduce) return;
  document.querySelectorAll(".magnetic").forEach((el) => {
    el.addEventListener("pointermove", (e) => {
      const r = el.getBoundingClientRect();
      el.style.transform = `translate(${(e.clientX - r.left - r.width / 2) * 0.25}px, ${(e.clientY - r.top - r.height / 2) * 0.35}px)`;
    });
    el.addEventListener("pointerleave", () => (el.style.transform = ""));
  });
  const stage = document.querySelector(".stage"), frame = document.querySelector(".stage .frame");
  if (stage && frame) {
    let mx = 0, my = 0, scrollTilt = 18;
    const apply = () => (frame.style.transform = `rotateX(${scrollTilt + my}deg) rotateY(${mx}deg) scale(${1 - scrollTilt / 200})`);
    addEventListener("scroll", () => {
      const r = stage.getBoundingClientRect();
      const p = Math.min(1, Math.max(0, 1 - (r.top - innerHeight * 0.15) / (innerHeight * 0.7)));
      scrollTilt = 18 * (1 - p); apply();
    }, { passive: true });
    stage.addEventListener("pointermove", (e) => {
      const r = stage.getBoundingClientRect();
      mx = ((e.clientX - r.left) / r.width - 0.5) * 8; my = -((e.clientY - r.top) / r.height - 0.5) * 6; apply();
    });
    stage.addEventListener("pointerleave", () => { mx = my = 0; apply(); });
    dispatchEvent(new Event("scroll"));
  }
})();
