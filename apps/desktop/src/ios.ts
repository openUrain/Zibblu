import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useLayoutEffect, useRef, useSyncExternalStore } from "react";

const ipad = /iPad/.test(navigator.userAgent)
  || (navigator.platform === "MacIntel" && navigator.maxTouchPoints > 1);
export const ios = ipad || /iPhone|iPod/.test(navigator.userAgent);
export const phone = ios && !ipad;
const regularWidth = window.matchMedia("(min-width: 700px)");
const backActions = new Map<() => void, number>();

function subscribeWidth(update: () => void) {
  if (!ipad) return () => {};
  regularWidth.addEventListener("change", update);
  return () => regularWidth.removeEventListener("change", update);
}

export function useIpadSplit() {
  return useSyncExternalStore(subscribeWidth, () => ipad && regularWidth.matches);
}

function requestLayout(backEnabled?: boolean) {
  void invoke("ios_layout", { backEnabled }).catch(() => {});
}

export function useIosBack(action: (() => void) | undefined, priority = 0) {
  const current = useRef(action);
  current.current = action;
  const enabled = !!action;
  useLayoutEffect(() => {
    if (!ios || !enabled) return;
    const back = () => current.current?.();
    backActions.set(back, priority);
    if (backActions.size === 1) requestLayout(true);
    return () => {
      backActions.delete(back);
      if (backActions.size === 0) requestLayout(false);
    };
  }, [enabled, priority]);
}

export function setupIosLayout() {
  if (!ios) return;
  const root = document.documentElement;
  root.classList.add("ios");
  void listen<[number, number]>("ios-input-layout", ({ payload: [inset, textScale] }) => {
    root.style.setProperty("--keyboard-inset", inset + "px");
    root.style.setProperty("--ios-text-scale", String(textScale));
    root.classList.toggle("ios-keyboard", inset > 0);
    if (inset > 0 && document.activeElement instanceof HTMLElement && document.activeElement.matches("input, textarea, select")) {
      const field = document.activeElement;
      const scroll = field.closest(".profile-content, .dialog-page-body, .settings-detail, .signin");
      if (scroll) requestAnimationFrame(() => {
        if (document.activeElement !== field) return;
        const bounds = scroll.getBoundingClientRect();
        const rect = field.getBoundingClientRect();
        if (rect.bottom > bounds.bottom - 12) scroll.scrollTop += rect.bottom - bounds.bottom + 12;
      });
    }
  }).then(() => requestLayout(backActions.size > 0));
  void listen("ios-back", () => {
    let best: (() => void) | undefined;
    let priority = -Infinity;
    for (const [handler, rank] of backActions) {
      if (rank >= priority) {
        best = handler;
        priority = rank;
      }
    }
    best?.();
  });
  window.addEventListener("resize", () => requestLayout());
}
