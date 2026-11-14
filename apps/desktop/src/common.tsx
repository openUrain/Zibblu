import { Circle, CircleCheck, MessageCircleMore, UsersRound, X } from "lucide-react";
import { createContext, useContext, useEffect, useRef, useState } from "react";
import type { ComponentPropsWithoutRef, KeyboardEventHandler, ReactNode } from "react";
import { api, errorMessage } from "./api";
import type { Conversation, Directory, Message, Page, User } from "./api";

export const People = createContext<Pick<Directory, "remarks" | "avatars"> & { open?: (user: User) => void }>({ remarks: {}, avatars: {} });

export function displayName(user: User, remarks: Directory["remarks"]) {
  return remarks[user.id] || user.handle;
}

export function PersonAvatar({ user, large, version }: { user: User; large?: boolean; version?: string | null }) {
  const { remarks, avatars } = useContext(People);
  const name = displayName(user, remarks);
  const image = version === undefined ? avatars[user.id] : version;
  return <Avatar name={name} large={large} src={image ? api.profileImageUrl(user.id, image) : undefined} />;
}

export function Brand() {
  return <span className="brand" aria-label="Zibblu"><MessageCircleMore size={26} strokeWidth={2.3} /></span>;
}

export function Avatar({ name, group = false, large = false, src }: { name: string; group?: boolean; large?: boolean; src?: string }) {
  const tones = ["violet", "mint", "peach", "blue", "rose"];
  const tone = Array.from(name).reduce((sum, character) => sum + (character.codePointAt(0) ?? 0), 0) % tones.length;
  const initial = Array.from(name)[0]?.toUpperCase() ?? "?";
  const [failed, setFailed] = useState<string>();
  return <span aria-hidden="true" className={"avatar " + tones[tone] + (large ? " avatar-large" : "")}>
    {group ? <UsersRound size={large ? 23 : 18} /> : src && failed !== src
      ? <img src={src} alt="" loading="lazy" onError={() => setFailed(src)} /> : initial}
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
    <Indicator className="choice-indicator" size={16} aria-hidden="true" />
  </label>;
}

export function Dialog({ title, close, children, className, onKeyDown, closeDisabled }: {
  title: string; close: () => void; children: ReactNode; className?: string;
  onKeyDown?: KeyboardEventHandler<HTMLDialogElement>; closeDisabled?: boolean;
}) {
  const element = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const dialog = element.current;
    dialog?.showModal();
    return () => dialog?.close();
  }, []);
  return <dialog ref={element} className={className} onCancel={event => { event.preventDefault(); if (!closeDisabled) close(); }}
    onKeyDown={onKeyDown} aria-label={title}>
    <div className="dialog-head"><h2>{title}</h2>
      <button className="icon-button" aria-label="Close dialog" onClick={close} disabled={closeDisabled}><X size={19} /></button>
    </div>
    {children}
  </dialog>;
}

export function usePage(read: (before?: number) => Promise<Page>, revision: number,
  recalled: ReadonlySet<number>, showRecalled = false, latestAiSeq?: number,
  hidden?: ReadonlySet<number>, oldestFirst = false) {
  const [page, setPage] = useState<Page>();
  const [failure, setFailure] = useState("");
  const [loading, setLoading] = useState(false);
  const expanding = useRef(false);
  const newest = useRef<number | undefined>(undefined);
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
          items: [...next.items, ...previous.items.slice(lastIndex + 1).filter(item => !hidden?.has(item.seq) && (showRecalled || !recalled.has(item.seq)))],
        };
      });
    }).catch(error => { if (current) setFailure(errorMessage(error)); });
    return () => { current = false; };
  }, [read, revision, recalled, showRecalled, latestAiSeq, oldestExpanded, hidden, oldestFirst]);

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
