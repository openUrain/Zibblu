import { MessageCircleMore, UsersRound, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { errorMessage } from "./api";
import type { Conversation, Message, Page } from "./api";

export function Brand() {
  return <span className="brand" aria-label="Zibblu"><MessageCircleMore size={26} strokeWidth={2.3} /></span>;
}

export function Avatar({ name, group = false, large = false }: { name: string; group?: boolean; large?: boolean }) {
  const tones = ["violet", "mint", "peach", "blue", "rose"];
  const tone = Array.from(name).reduce((sum, character) => sum + (character.codePointAt(0) ?? 0), 0) % tones.length;
  const initial = Array.from(name)[0]?.toUpperCase() ?? "?";
  return <span aria-hidden="true" className={"avatar " + tones[tone] + (large ? " avatar-large" : "")}>
    {group ? <UsersRound size={large ? 23 : 18} /> : initial}
  </span>;
}

export function chatName(chat: Conversation, ownId: string): string {
  return chat.title ?? chat.members.find(member => member.id !== ownId)?.handle ?? "Direct chat";
}

const hour = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });
const date = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", year: "numeric" });

export function timeLabel(value: number) { return hour.format(value); }
export function dateLabel(value: number) {
  return new Date(value).toDateString() === new Date().toDateString() ? "Today" : date.format(value);
}

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

export function Dialog({ title, close, children, className }: {
  title: string; close: () => void; children: ReactNode; className?: string;
}) {
  const element = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const dialog = element.current;
    dialog?.showModal();
    return () => dialog?.close();
  }, []);
  return <dialog ref={element} className={className} onCancel={event => { event.preventDefault(); close(); }} aria-label={title}>
    <div className="dialog-head"><h2>{title}</h2>
      <button className="icon-button" aria-label="Close dialog" onClick={close}><X size={19} /></button>
    </div>
    {children}
  </dialog>;
}

export function usePage(read: (before?: number) => Promise<Page>, revision: number,
  recalled: ReadonlySet<number>, showRecalled = false, latestAiSeq?: number) {
  const [page, setPage] = useState<Page>();
  const [failure, setFailure] = useState("");
  const [loading, setLoading] = useState(false);
  const expanding = useRef(false);
  const newest = useRef<number | undefined>(undefined);
  const [oldestExpanded, setOldestExpanded] = useState<Message>();
  const reviewedAiSeq = useRef(latestAiSeq);

  useEffect(() => {
    let current = true;
    const aiChanged = reviewedAiSeq.current !== latestAiSeq;
    const oldest = oldestExpanded;
    async function refresh() {
      const next = await read();
      if (!aiChanged || !oldest) return next;
      // A shared AI reply can appear anywhere in the already expanded history.
      let cursor = next.items.at(-1);
      while (current && next.has_more && cursor
        && (cursor.created_at > oldest.created_at || (cursor.created_at === oldest.created_at && cursor.seq > oldest.seq))) {
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
      reviewedAiSeq.current = latestAiSeq;
      if (newest.current === undefined) newest.current = Math.max(0, ...next.items.map(item => item.seq));
      if (!oldest || aiChanged) {
        setPage(next);
        return;
      }
      setPage(previous => {
        if (!previous || next.items.length === 0) return next;
        const last = next.items.at(-1)!.seq;
        const lastIndex = previous.items.findIndex(item => item.seq === last);
        return lastIndex < 0 ? next : {
          ...previous,
          items: [...next.items, ...previous.items.slice(lastIndex + 1).filter(item => showRecalled || !recalled.has(item.seq))],
        };
      });
    }).catch(error => { if (current) setFailure(errorMessage(error)); });
    return () => { current = false; };
  }, [read, revision, recalled, showRecalled, latestAiSeq, oldestExpanded]);

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
  const visible = recalled.size && page ? {
    ...page,
    items: showRecalled
      ? page.items.map(item => !item.recalled && recalled.has(item.seq)
        ? { ...item, recalled: true, text: null, attachment: null } : item)
      : page.items.filter(item => !recalled.has(item.seq)),
  } : page;
  return { page: visible, failure, loading, older, newestAtOpen: newest.current };
}
