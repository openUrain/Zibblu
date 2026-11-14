import { createContext, useContext, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { MouseEvent, ReactNode } from "react";
import { createPortal } from "react-dom";
import { ClipboardPaste, Copy, TextCursorInput } from "lucide-react";

export type MenuItem = { label: string; action: () => void; disabled?: boolean; icon?: ReactNode };
type Menu = { x: number; y: number; anchor: HTMLElement; items: MenuItem[]; restoreFocus: HTMLElement | null };
const OpenMenu = createContext<(anchor: HTMLElement, items: MenuItem[], point?: { x: number; y: number }) => void>(() => {});
export const useMenu = () => useContext(OpenMenu);

export function DesktopMenus({ children }: { children: ReactNode }) {
  const [menu, setMenu] = useState<Menu>();
  const panel = useRef<HTMLDivElement>(null);

  function open(anchor: HTMLElement, items: MenuItem[], point?: { x: number; y: number }) {
    if (!items.some(item => !item.disabled)) { setMenu(undefined); return; }
    const rect = anchor.getBoundingClientRect();
    setMenu({
      x: point?.x ?? rect.left, y: point?.y ?? rect.bottom + 4, anchor, items,
      restoreFocus: anchor.matches("button") ? anchor
        : document.activeElement instanceof HTMLElement ? document.activeElement : null,
    });
  }

  function close(restore = false) {
    setMenu(undefined);
    if (restore) menu?.restoreFocus?.focus({ preventScroll: true });
  }

  useLayoutEffect(() => {
    const element = panel.current;
    if (!menu || !element) return;
    element.style.left = Math.max(8, Math.min(menu.x, window.innerWidth - element.offsetWidth - 8)) + "px";
    element.style.top = Math.max(8, Math.min(menu.y, window.innerHeight - element.offsetHeight - 8)) + "px";
    (element.querySelector<HTMLElement>("button:not(:disabled)") ?? element).focus({ preventScroll: true });
  }, [menu]);

  useEffect(() => {
    if (!menu) return;
    const outside = (event: PointerEvent) => {
      if (!(event.target instanceof Node) || !panel.current?.contains(event.target)) setMenu(undefined);
    };
    const reposition = () => {
      const anchor = menu.anchor.getBoundingClientRect();
      if (!menu.anchor.isConnected || anchor.bottom < 0 || anchor.top > window.innerHeight) setMenu(undefined);
    };
    const dismiss = () => setMenu(undefined);
    window.addEventListener("pointerdown", outside);
    window.addEventListener("resize", dismiss);
    window.addEventListener("scroll", reposition, true);
    return () => {
      window.removeEventListener("pointerdown", outside);
      window.removeEventListener("resize", dismiss);
      window.removeEventListener("scroll", reposition, true);
    };
  }, [menu]);

  function context(event: MouseEvent<HTMLDivElement>) {
    if (event.defaultPrevented || !(event.target instanceof Element)) return;
    event.preventDefault();
    const target = event.target instanceof HTMLElement ? event.target : event.target.parentElement ?? event.currentTarget;
    const privateChat = !!target.closest(".private-chat");
    const field = target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement ? target : undefined;
    const editable = field && !field.readOnly && !field.disabled
      && (field instanceof HTMLTextAreaElement || /^(text|search|url|tel|email|password)$/.test(field.type));
    const password = field instanceof HTMLInputElement && field.type === "password";
    const start = field?.selectionStart ?? null;
    const end = field?.selectionEnd ?? null;
    const items: MenuItem[] = [];
    if (!privateChat && !password) {
      const text = field ? start !== null && end !== null ? field.value.slice(start, end) : "" : window.getSelection()?.toString();
      if (text) items.push({
        label: "Copy", icon: <Copy size={15} />, action: () => { void navigator.clipboard.writeText(text).catch(() => {}); },
      });
    }
    if (editable) items.push({
      label: "Paste", icon: <ClipboardPaste size={15} />, action: () => {
        field.focus({ preventScroll: true });
        if (start !== null && end !== null) field.setSelectionRange(start, end);
        void navigator.clipboard.readText().then(value => {
          if (field.isConnected && document.activeElement === field) document.execCommand("insertText", false, value);
        }).catch(() => {});
      },
    });
    if (editable || (field && start !== null && !privateChat)) {
      items.push({ label: "Select all", icon: <TextCursorInput size={15} />, action: () => field.select() });
    }
    open(target, items,
      event.clientX || event.clientY ? { x: event.clientX, y: event.clientY } : undefined);
  }

  return <OpenMenu.Provider value={open}><div onContextMenu={context}>
    {children}
    {menu && createPortal(<div className="app-menu" ref={panel} role="menu" aria-label="Actions" tabIndex={-1}
      onContextMenu={event => { event.preventDefault(); event.stopPropagation(); }}
      onKeyDown={event => {
        if (event.key === "Escape" || event.key === "Tab") {
          if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); }
          close(true);
        }
        if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
          event.preventDefault();
          const items = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>("button:not(:disabled)"));
          if (!items.length) return;
          const index = items.indexOf(document.activeElement as HTMLButtonElement);
          const next = event.key === "Home" ? 0 : event.key === "End" ? items.length - 1
            : (index + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
          items[next]?.focus();
        }
      }}>
      {menu.items.map(item => <button type="button" role="menuitem" key={item.label} disabled={item.disabled}
        onClick={() => { close(true); item.action(); }}>{item.icon}{item.label}</button>)}
    </div>, menu.anchor.closest("dialog, .private-chat") ?? document.body)}
  </div></OpenMenu.Provider>;
}
