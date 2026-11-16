// Site configuration — fill in DISCORD_URL when the invite exists.
const ATLAS = {
  DISCORD_URL: "",
  REPO: "sayovc/Atlas-Launcher",
};

document.addEventListener("DOMContentLoaded", () => {
  // Discord buttons stay hidden until a URL is configured.
  document.querySelectorAll("[data-discord]").forEach((el) => {
    if (ATLAS.DISCORD_URL) { el.href = ATLAS.DISCORD_URL; el.hidden = false; }
  });
  // Mobile menu
  const btn = document.querySelector(".menu-btn");
  const list = document.querySelector(".nav ul");
  if (btn && list) btn.addEventListener("click", () => {
    const open = list.classList.toggle("open");
    btn.setAttribute("aria-expanded", String(open));
  });
  // Footer year
  document.querySelectorAll("[data-year]").forEach((el) => (el.textContent = new Date().getFullYear()));
  // Gentle reveal on scroll
  const io = "IntersectionObserver" in window ? new IntersectionObserver((es) => {
    es.forEach((e) => { if (e.isIntersecting) { e.target.classList.add("in"); io.unobserve(e.target); } });
  }, { threshold: 0.12 }) : null;
  document.querySelectorAll(".reveal").forEach((el) => (io ? io.observe(el) : el.classList.add("in")));
});
