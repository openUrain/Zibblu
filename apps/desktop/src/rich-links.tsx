import { invoke } from "@tauri-apps/api/core";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { CSSProperties, ReactNode } from "react";
import { createPortal } from "react-dom";
import { t } from "./locale";
import { safeWebUrl } from "./rich-markdown";

const hoverDelay = 500;
const leaveDelay = 160;
let dismissActivePreview: (() => void) | undefined;

type PreviewPosition = {
  style: CSSProperties & { "--preview-width": string; "--preview-height": string };
  keyboard: boolean;
};

export function RichLink({ source, children, image, privateRoom }: {
  source: string; children: ReactNode; image?: boolean; privateRoom: boolean;
}) {
  const href = safeWebUrl(source);
  if (!href) return <>{children}</>;
  return <ExternalLink key={href} {...{ href, image, privateRoom }}>{children}</ExternalLink>;
}

function ExternalLink({ href, children, image, privateRoom }: {
  href: string; children: ReactNode; image?: boolean; privateRoom: boolean;
}) {
  const anchor = useRef<HTMLAnchorElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const hoverTimer = useRef<number>(undefined);
  const leaveTimer = useRef<number>(undefined);
  const [position, setPosition] = useState<PreviewPosition>();
  const [openFailed, setOpenFailed] = useState(false);
  const canPreview = image && href.startsWith("https:");
  const disclosure = t("Previewing contacts a third party and shares your IP address.", "预览会访问第三方，并向其披露你的 IP 地址。");
  const hint = canPreview ? t("Hover or press Shift+Enter to preview", "悬停或按 Shift+Enter 预览图片") : undefined;

  function stopHover() {
    window.clearTimeout(hoverTimer.current);
    hoverTimer.current = undefined;
  }

  function show(keyboard: boolean, point?: { x: number; y: number }) {
    stopHover();
    if (!anchor.current) return;
    const rects = Array.from(anchor.current.getClientRects());
    const rect = (point && rects.find(rect => rect.left <= point.x && rect.right >= point.x && rect.top <= point.y && rect.bottom >= point.y))
      || rects.find(rect => rect.top >= 0 && rect.bottom <= window.innerHeight);
    if (!rect) return;
    const gap = 8;
    const width = Math.min(320, window.innerWidth - 2 * gap);
    const below = window.innerHeight - rect.bottom - gap;
    const above = rect.top - gap;
    const up = below < 256 && above > below;
    const height = Math.min(256, Math.max(above, below));
    const left = Math.max(gap, rect.left);
    const right = Math.max(gap, window.innerWidth - rect.right);
    setPosition({ keyboard, style: {
      "--preview-width": `${width}px`, "--preview-height": `${height}px`,
      ...(left + width <= window.innerWidth - gap ? { left } : { right }),
      ...(up ? { bottom: window.innerHeight - rect.top } : { top: rect.bottom }),
    } });
  }

  function queueClose(next: EventTarget | null) {
    stopHover();
    if (next instanceof Node && (panel.current?.contains(next) || anchor.current?.contains(next))) return;
    window.clearTimeout(leaveTimer.current);
    leaveTimer.current = window.setTimeout(() => setPosition(undefined), leaveDelay);
  }

  useEffect(() => {
    if (!canPreview) return;
    return () => {
      stopHover();
      window.clearTimeout(leaveTimer.current);
    };
  }, [canPreview]);

  useLayoutEffect(() => {
    if (!position) return;
    const close = () => {
      window.clearTimeout(leaveTimer.current);
      setPosition(undefined);
    };
    dismissActivePreview?.();
    dismissActivePreview = close;
    function outside(event: PointerEvent) {
      const target = event.target;
      if (target instanceof Node && !panel.current?.contains(target) && !anchor.current?.contains(target)) close();
    }
    function onScroll(event: Event) {
      if (!(event.target instanceof Node && panel.current?.contains(event.target))) close();
    }
    function onKey(event: KeyboardEvent) {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      event.preventDefault();
      if (panel.current?.contains(document.activeElement)) anchor.current?.focus({ preventScroll: true });
      close();
    }
    if (position.keyboard) panel.current?.focus({ preventScroll: true });
    window.addEventListener("pointerdown", outside);
    window.addEventListener("scroll", onScroll, true);
    window.addEventListener("resize", close);
    window.addEventListener("blur", close);
    window.addEventListener("keydown", onKey);
    return () => {
      if (dismissActivePreview === close) dismissActivePreview = undefined;
      window.removeEventListener("pointerdown", outside);
      window.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("resize", close);
      window.removeEventListener("blur", close);
      window.removeEventListener("keydown", onKey);
    };
  }, [position]);

  return <>
    <a className="rich-link" href={href} rel="noreferrer" ref={anchor}
      title={hint ? `${hint}. ${disclosure}` : undefined}
      aria-description={canPreview ? disclosure : undefined}
      aria-keyshortcuts={canPreview ? "Shift+Enter" : undefined}
      onPointerMove={canPreview ? event => {
        if (event.pointerType !== "mouse" || !event.isTrusted || position || hoverTimer.current !== undefined) return;
        if (!event.movementX && !event.movementY) return;
        const { clientX, clientY } = event;
        hoverTimer.current = window.setTimeout(() => {
          if (anchor.current?.contains(document.elementFromPoint(clientX, clientY))) show(false, { x: clientX, y: clientY });
          else stopHover();
        }, hoverDelay);
      } : undefined}
      onPointerEnter={canPreview ? () => window.clearTimeout(leaveTimer.current) : undefined}
      onPointerLeave={canPreview ? event => queueClose(event.relatedTarget) : undefined}
      onBlur={canPreview ? event => queueClose(event.relatedTarget) : undefined}
      onKeyDown={canPreview ? event => {
        if (event.key !== "Enter" || !event.shiftKey) return;
        event.preventDefault();
        show(true);
      } : undefined}
      onAuxClick={event => event.preventDefault()}
      onClick={event => {
        event.preventDefault();
        stopHover();
        setPosition(undefined);
        setOpenFailed(false);
        void invoke("open_message_link", { url: href }).catch(() => setOpenFailed(true));
      }}>{children}</a>
    {openFailed && <span className="rich-link-error" role="status">{t("Could not open this link.", "无法打开链接。")}</span>}
    {position && createPortal(<div ref={panel} className={privateRoom ? "rich-preview rich-preview-private" : "rich-preview"}
      style={position.style} role="group" tabIndex={-1} aria-label={t("Image preview", "图片预览")}
      onPointerEnter={() => window.clearTimeout(leaveTimer.current)}
      onPointerLeave={event => queueClose(event.relatedTarget)}
      onBlur={event => queueClose(event.relatedTarget)}
      onContextMenu={event => event.stopPropagation()}>
      <ImagePreview href={href} alt={typeof children === "string" ? children : ""} />
    </div>, document.body)}
  </>;
}

function ImagePreview({ href, alt }: { href: string; alt: string }) {
  const [source, setSource] = useState<string>();
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    const id = crypto.randomUUID();
    let current = true;
    let pending = true;
    let objectUrl: string | undefined;
    void invoke<ArrayBuffer | number[]>("preview_message_image", { id, url: href }).then(bytes => {
      if (!current) return;
      objectUrl = URL.createObjectURL(new Blob([Array.isArray(bytes) ? Uint8Array.from(bytes) : bytes], { type: "image/png" }));
      setSource(objectUrl);
    }).catch(() => {
      if (current) setFailed(true);
    }).finally(() => { pending = false; });
    return () => {
      current = false;
      if (pending) void invoke("cancel_message_image", { id }).catch(() => {});
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [href]);

  return <div className="rich-preview-result" aria-busy={!source && !failed}>
    {failed ? <span role="status">{t("Image preview unavailable.", "无法预览图片。")}</span>
      : source ? <img src={source} alt={alt} onError={() => setFailed(true)} />
        : <span role="status">{t("Loading preview…", "正在加载预览…")}</span>}
  </div>;
}
