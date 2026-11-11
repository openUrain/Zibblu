import { MessageCircleMore, UsersRound, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { errorMessage } from "./api";
import type { Conversation, Page } from "./api";

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

export function Dialog({ title, close, children }: { title: string; close: () => void; children: ReactNode }) {
  const element = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const dialog = element.current;
    dialog?.showModal();
    return () => dialog?.close();
  }, []);
  return <dialog ref={element} onCancel={event => { event.preventDefault(); close(); }} aria-label={title}>
    <div className="dialog-head"><h2>{title}</h2>
      <button className="icon-button" aria-label="Close dialog" onClick={close}><X size={19} /></button>
    </div>
    {children}
  </dialog>;
}

export function usePage(read: (before?: number) => Promise<Page>, revision: number) {
  const [page, setPage] = useState<Page & { expanded?: boolean }>();
  const [failure, setFailure] = useState("");
  const [loading, setLoading] = useState(false);
  const expanding = useRef(false);

  useEffect(() => {
    let current = true;
    read().then(next => {
      if (!current) return;
      setFailure("");
      setPage(previous => {
        if (!previous?.expanded || next.items.length === 0 || previous.items.length === 0
          || next.items.at(-1)!.seq > previous.items[0].seq) return next;
        const last = next.items.at(-1)!.seq;
        return { ...previous, items: [...next.items, ...previous.items.filter(item => item.seq < last)] };
      });
    }).catch(error => { if (current) setFailure(errorMessage(error)); });
    return () => { current = false; };
  }, [read, revision]);

  async function older() {
    const cursor = page?.items.at(-1)?.seq;
    if (cursor === undefined || expanding.current) return;
    expanding.current = true;
    setLoading(true);
    try {
      const next = await read(cursor);
      setPage(previous => previous?.items.at(-1)?.seq === cursor
        ? { items: [...previous.items, ...next.items], has_more: next.has_more, expanded: true } : previous);
    } catch (error) { setFailure(errorMessage(error)); }
    finally { expanding.current = false; setLoading(false); }
  }
  return { page, failure, loading, older };
}
