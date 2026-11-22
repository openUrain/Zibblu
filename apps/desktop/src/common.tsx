import { Check, ChevronLeft, Circle, CircleCheck, UsersRound, X } from "lucide-react";
import { createContext, useContext, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { ComponentPropsWithoutRef, KeyboardEventHandler, ReactNode } from "react";
import { api, errorMessage } from "./api";
import type { Conversation, Directory, Message, Page, User } from "./api";
import { ios, useIosBack } from "./ios";
import { actions, t } from "./locale";
import { useWorkspaceImage, WorkspaceImages } from "./workspace-image";

export const People = createContext<Pick<Directory, "remarks" | "avatars"> & { online?: boolean; open?: (user: User) => void }>({ remarks: {}, avatars: {} });

export function displayName(user: User, remarks: Directory["remarks"]) {
  return remarks[user.id] || user.handle;
}

export function PersonAvatar({ user, large, version }: { user: User; large?: boolean; version?: string | null }) {
  const { remarks, avatars, online } = useContext(People);
  const resources = useContext(WorkspaceImages);
  const name = displayName(user, remarks);
  const image = version === undefined ? avatars[user.id] : version;
  const cached = resources && image && image === avatars[user.id] ? { id: user.id, version: image } : undefined;
  return <Avatar name={name} large={large} online={online}
    cached={cached} src={!cached && image ? api.profileImageUrl(user.id, image) : undefined} />;
}

export function Brand() {
  return <img className="brand" src={new URL("../src-tauri/icons/icon.png", import.meta.url).href}
    width={42} height={42} alt="Zibblu" />;
}

export function Avatar({ name, group = false, large = false, src, online, cached }: {
  name: string; group?: boolean; large?: boolean; src?: string; online?: boolean; cached?: { id: string; version: string };
}) {
  const node = useRef<HTMLSpanElement>(null);
  const [visible, setVisible] = useState(false);
  const key = cached && `avatar/${cached.id}/${cached.version}`;
  useEffect(() => {
    if (!key || !node.current) return;
    const observer = new IntersectionObserver(([entry]) => setVisible(entry.isIntersecting));
    observer.observe(node.current);
    return () => observer.disconnect();
  }, [key]);
  const { url } = useWorkspaceImage(key, cached?.id ?? "", cached?.version, Infinity, Number(online), visible);
  src = url ?? src;
  const tones = ["violet", "mint", "peach", "blue", "rose"];
  const tone = Array.from(name).reduce((sum, character) => sum + (character.codePointAt(0) ?? 0), 0) % tones.length;
  const initial = Array.from(name)[0]?.toUpperCase() ?? "?";
  const [failed, setFailed] = useState<{ src: string; online?: boolean }>();
  return <span ref={node} aria-hidden="true" className={"avatar " + tones[tone] + (large ? " avatar-large" : "")}>
    {group ? <UsersRound size={large ? 23 : 18} /> : src && (failed?.src !== src || failed.online !== online)
      ? <img src={src} alt="" loading="lazy" onError={() => setFailed({ src, online })} /> : initial}
  </span>;
}

export function chatName(chat: Conversation, ownId: string, remarks: Directory["remarks"]): string {
  const peer = chat.members.find(member => member.id !== ownId);
  return chat.title ?? (peer ? displayName(peer, remarks) : "Direct chat");
}

export { dateTimeLabel, messageTimeLabel, startsTimeGroup } from "./message-time";

export function bytes(value: number): string {
  if (value < 1024) return value + " B";
  if (value < 1024 * 1024) return (value / 1024).toFixed(1) + " KiB";
  return (value / (1024 * 1024)).toFixed(1) + " MiB";
}

export function Blank({ icon, title, children }: { icon: ReactNode; title: string; children?: ReactNode }) {
  return <div className="blank"><span className="blank-icon" aria-hidden="true">{icon}</span>
    <h2>{title}</h2>{children}
  </div>;
}

export function Choice({ children, className = "", type = "checkbox", ...input }: {
  children: ReactNode; className?: string; type?: "checkbox" | "radio";
} & Pick<ComponentPropsWithoutRef<"input">, "checked" | "disabled" | "name" | "onChange">) {
  const Indicator = input.checked ? CircleCheck : Circle;
  return <label className={`choice ${className}`}>
    <input type={type} {...input} />
    {children}
    {ios ? input.checked && <Check className="choice-indicator" size={22} aria-hidden="true" />
      : <Indicator className="choice-indicator" size={16} aria-hidden="true" />}
  </label>;
}

export function placeDialog(dialog: HTMLDialogElement, x: number, y: number, flip = false) {
  const bounds = document.querySelector(".shell")?.getBoundingClientRect()
    ?? new DOMRect(0, 0, innerWidth, innerHeight);
  const { width, height } = dialog.getBoundingClientRect();
  const left = flip && x + width > bounds.right ? x - width : x;
  const top = flip && y + height > bounds.bottom ? y - height : y;
  dialog.style.margin = "0";
  dialog.style.inset = "auto";
  dialog.style.left = Math.max(bounds.left, Math.min(left, bounds.right - width)) + "px";
  dialog.style.top = Math.max(bounds.top, Math.min(top, bounds.bottom - height)) + "px";
}

export function Dialog({ title, close, children, className, onKeyDown, closeDisabled, mobilePage, visible = true, modal = true }: {
  title: string; close: () => void; children: ReactNode; className?: string;
  onKeyDown?: KeyboardEventHandler<HTMLDialogElement>; closeDisabled?: boolean; mobilePage?: boolean;
  visible?: boolean; modal?: boolean;
}) {
  const element = useRef<HTMLDialogElement>(null);
  const drag = useRef<{ x: number; y: number }>(undefined);
  const movable = modal && !(ios && mobilePage);
  useLayoutEffect(() => {
    const dialog = element.current;
    if (!movable || !dialog || !visible) return;
    const clamp = () => {
      if (!dialog.style.left) return;
      const rect = dialog.getBoundingClientRect();
      placeDialog(dialog, rect.left, rect.top);
    };
    window.addEventListener("resize", clamp);
    return () => window.removeEventListener("resize", clamp);
  }, [movable, visible]);
  useLayoutEffect(() => {
    const dialog = element.current;
    if (!dialog || !visible) return;
    if (modal) dialog.showModal();
    else dialog.show();
    return () => dialog?.close();
  }, [visible, modal]);
  useIosBack(visible ? () => { if (!closeDisabled) close(); } : undefined, 20);
  const page = ios && mobilePage;
  return <dialog ref={element} inert={!visible} className={(className ?? "") + (movable ? " formal-dialog" : "") + (page ? " dialog-page" : "")}
    onCancel={event => { event.preventDefault(); if (!closeDisabled) close(); }}
    onKeyDown={onKeyDown} aria-label={title}>
    <div className="dialog-head" onPointerDown={event => {
      if (!movable || event.button !== 0 || !element.current || (event.target as Element).closest("button")) return;
      const bounds = element.current.getBoundingClientRect();
      drag.current = { x: event.clientX - bounds.left, y: event.clientY - bounds.top };
      event.currentTarget.setPointerCapture(event.pointerId);
    }} onPointerMove={event => {
      if (drag.current && element.current) placeDialog(element.current, event.clientX - drag.current.x, event.clientY - drag.current.y);
    }} onLostPointerCapture={() => { drag.current = undefined; }}>
      {!page && <h2>{title}</h2>}
      <button className="icon-button" type="button" aria-label={page ? actions.back : t("Close dialog", "关闭")}
        onClick={close} disabled={closeDisabled}>{page ? <ChevronLeft size={25} /> : <X size={19} />}</button>
      {page && <h2>{title}</h2>}
    </div>
    {page ? <div className="dialog-page-body">{children}</div> : children}
  </dialog>;
}

export function usePage(read: (before?: number) => Promise<Page>, revision: number,
  recalled: ReadonlySet<number>, showRecalled = false, latestAiSeq?: number,
  hidden?: ReadonlySet<number>, oldestFirst = false, seed?: Page, save?: (page: Page) => void) {
  const [page, setPage] = useState(seed);
  const [failure, setFailure] = useState("");
  const [loading, setLoading] = useState(false);
  const expanding = useRef(false);
  const newest = useRef(seed ? Math.max(0, ...seed.items.map(item => item.seq)) : undefined);
  const [oldestExpanded, setOldestExpanded] = useState<Message>();
  const reviewedAiSeq = useRef(latestAiSeq);
  const [loadedRevision, setLoadedRevision] = useState<number>();

  useEffect(() => {
    let current = true;
    const aiChanged = reviewedAiSeq.current !== latestAiSeq;
    const oldest = oldestExpanded;
    async function refresh() {
      const next = await read();
      if ((!aiChanged && !oldestFirst) || !oldest) return next;
      // A shared AI reply can appear anywhere in the already expanded history.
      let cursor = next.items.at(-1);
      while (current && next.has_more && cursor
        && (oldestFirst ? cursor.created_at < oldest.created_at || (cursor.created_at === oldest.created_at && cursor.seq < oldest.seq)
          : cursor.created_at > oldest.created_at || (cursor.created_at === oldest.created_at && cursor.seq > oldest.seq))) {
        const older = await read(cursor.seq);
        next.items.push(...older.items);
        next.has_more = older.has_more;
        cursor = older.items.at(-1);
      }
      return next;
    }
    refresh().then(next => {
      if (!current) return;
      save?.(next);
      setFailure("");
      setLoadedRevision(revision);
      reviewedAiSeq.current = latestAiSeq;
      if (newest.current === undefined) newest.current = Math.max(0, ...next.items.map(item => item.seq));
      if (!oldest || aiChanged || oldestFirst) {
        setPage(next);
        return;
      }
      setPage(previous => {
        if (!previous || next.items.length === 0) return next;
        const last = next.items.at(-1)!.seq;
        const lastIndex = previous.items.findIndex(item => item.seq === last);
        return lastIndex < 0 ? next : {
          ...previous,
          items: [...next.items, ...previous.items.slice(lastIndex + 1).filter(item => !item.pending && !hidden?.has(item.seq) && (showRecalled || !recalled.has(item.seq)))],
        };
      });
    }).catch(error => { if (current) setFailure(errorMessage(error)); });
    return () => { current = false; };
  }, [read, revision, recalled, showRecalled, latestAiSeq, oldestExpanded, hidden, oldestFirst, save]);

  async function older() {
    const last = page?.items.at(-1);
    if (!last || expanding.current) return;
    const cursor = last.seq;
    expanding.current = true;
    setLoading(true);
    try {
      const next = await read(cursor);
      setOldestExpanded(next.items.at(-1) ?? last);
      setPage(previous => previous?.items.at(-1)?.seq === cursor
        ? { items: [...previous.items, ...next.items], has_more: next.has_more } : previous);
    } catch (error) { setFailure(errorMessage(error)); }
    finally { expanding.current = false; setLoading(false); }
  }
  const visible = (recalled.size || hidden?.size) && page ? {
    ...page,
    items: showRecalled
      ? page.items.filter(item => !hidden?.has(item.seq)).map(item => !item.recalled && recalled.has(item.seq)
        ? { ...item, recalled: true, text: null, attachment: null } : item)
      : page.items.filter(item => !recalled.has(item.seq) && !hidden?.has(item.seq)),
  } : page;
  return { page: visible, failure, loading, older, newestAtOpen: newest.current, loadedRevision };
}
