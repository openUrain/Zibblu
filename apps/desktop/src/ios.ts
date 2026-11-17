export const ios = /iPhone|iPod|iPad/.test(navigator.userAgent)
  || (navigator.platform === "MacIntel" && navigator.maxTouchPoints > 1);

export function setupIosLayout() {
  if (!ios) return;
  const root = document.documentElement;
  root.classList.add("ios");
  const viewport = window.visualViewport;
  if (!viewport) return;
  function resize() {
    if (!viewport || viewport.scale !== 1) return;
    root.style.setProperty("--ios-height", `${viewport.height}px`);
    root.classList.toggle("ios-keyboard", window.innerHeight - viewport.height > 100);
  }
  viewport.addEventListener("resize", resize);
  window.addEventListener("resize", resize);
  resize();
}
