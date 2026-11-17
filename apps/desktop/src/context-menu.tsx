import { createContext, useContext, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { MouseEvent, ReactNode } from "react";
import { createPortal } from "react-dom";
import { ClipboardPaste, Copy, TextCursorInput } from "lucide-react";
import { api, errorMessage } from "./api";
import { ios, useIosBack } from "./ios";
import { t } from "./locale";

const windows = navigator.userAgent.includes("Windows");

export function copyText(text: string) {
  return ios || windows ? api.copyText(text) : navigator.clipboard.writeText(text);
}

export type MenuItem = { label: string; action: () => void; disabled?: boolean; icon?: ReactNode };
type Menu = { x: number; y: number; anchor: HTMLElement; items: MenuItem[]; restoreFocus: HTMLElement | null };
const OpenMenu = createContext<(anchor: HTMLElement, items: MenuItem[], point?: { x: number; y: number }) => void>(() => {});
const MenuAnchor = createContext<HTMLElement | undefined>(undefined);
export const useMenu = () => useContext(OpenMenu);
export const useMenuAnchor = () => useContext(MenuAnchor);

export function useRowMenu() {
  const open = useMenu();
  const press = useRef<{ target: HTMLElement; x: number; y: number; timer: number; opened: boolean }>(undefined);
  const release = () => { window.clearTimeout(press.current?.timer); };
  useEffect(() => release, []);

  return (items: MenuItem[]): React.HTMLAttributes<HTMLButtonElement> => ({
    onContextMenu(event) {
      event.preventDefault();
      event.stopPropagation();
      if (press.current?.opened && press.current.target === event.currentTarget) return;
      open(event.currentTarget, items, event.clientX || event.clientY ? { x: event.clientX, y: event.clientY } : undefined);
    },
    onPointerDown(event) {
      release();
      press.current = undefined;
      if (event.pointerType !== "touch") return;
      const target = event.currentTarget;
      const x = event.clientX;
      const y = event.clientY;
      const timer = window.setTimeout(() => {
        if (!press.current || !target.isConnected) return;
        press.current.opened = true;
        open(target, items, { x, y });
      }, 480);
      press.current = { target, x, y, timer, opened: false };
    },
    onPointerMove(event) {
      const active = press.current;
      if (active && Math.hypot(active.x - event.clientX, active.y - event.clientY) > 10) release();
    },
    onPointerUp: release,
    onPointerCancel: release,
    onPointerLeave: release,
    onClickCapture(event) {
      if (event.detail && press.current?.opened && press.current.target === event.currentTarget) {
        event.preventDefault();
        event.stopPropagation();
        press.current = undefined;
      }
    },
  });
}

export function DesktopMenus({ children }: { children: ReactNode }) {
  const [menu, setMenu] = useState<Menu>();
  const [failure, setFailure] = useState("");
  const panel = useRef<HTMLDivElement>(null);

  function open(anchor: HTMLElement, items: MenuItem[], point?: { x: number; y: number }) {
    setFailure("");
    if (menu?.anchor === anchor && !point) { close(true); return; }
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
  useIosBack(menu ? () => close(true) : undefined, 30);

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
      if (!(event.target instanceof Node) || (!panel.current?.contains(event.target) && !menu.anchor.contains(event.target))) {
        if (panel.current?.contains(document.activeElement)) menu.restoreFocus?.focus({ preventScroll: true });
        setMenu(undefined);
      }
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
    const target = event.target instanceof HTMLElement ? event.target : event.target.parentElement ?? event.currentTarget;
    const privateChat = !!target.closest(".private-chat");
    const field = target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement ? target : undefined;
    if (!windows && field && !privateChat) return;
    event.preventDefault();
    const editable = field && !field.readOnly && !field.disabled
      && (field instanceof HTMLTextAreaElement || /^(text|search|url|tel|email|password)$/.test(field.type));
    const password = field instanceof HTMLInputElement && (field.type === "password" || field.hasAttribute("data-password"));
    const start = field?.selectionStart ?? null;
    const end = field?.selectionEnd ?? null;
    const items: MenuItem[] = [];
    if (!privateChat && !password) {
      const text = field ? start !== null && end !== null ? field.value.slice(start, end) : "" : window.getSelection()?.toString();
      if (text) items.push({
        label: t("Copy", "复制"), icon: <Copy size={15} />,
        action: () => { void copyText(text).catch(error => setFailure(errorMessage(error))); },
      });
    }
    if (editable && windows) items.push({
      label: t("Paste", "粘贴"), icon: <ClipboardPaste size={15} />, action: () => {
        field.focus({ preventScroll: true });
        if (start !== null && end !== null) field.setSelectionRange(start, end);
        void api.pasteText().then(value => {
          if (!field.isConnected || document.activeElement !== field) return;
          if (value === null) {
            setFailure(t("Use Ctrl+V to paste an image.", "按 Ctrl+V 粘贴图片。"));
            return;
          }
          document.execCommand("insertText", false, value);
        }).catch(error => setFailure(errorMessage(error)));
      },
    });
    if (editable || (field && start !== null && !privateChat)) {
      items.push({ label: t("Select all", "全选"), icon: <TextCursorInput size={15} />, action: () => field.select() });
    }
    open(target, items,
      event.clientX || event.clientY ? { x: event.clientX, y: event.clientY } : undefined);
  }

  return <OpenMenu.Provider value={open}><MenuAnchor.Provider value={menu?.anchor}><div onContextMenu={context}>
    {children}
    {failure && createPortal(<div className="clipboard-error inline-error" role="alert">
      {failure}<button type="button" className="text-button" onClick={() => setFailure("")}>{t("Dismiss", "关闭")}</button>
    </div>, document.querySelector("dialog[open], .private-chat") ?? document.body)}
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
  </div></MenuAnchor.Provider></OpenMenu.Provider>;
}
