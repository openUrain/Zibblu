import {
  ArrowDown, ArrowLeft, FilePlus2, FolderOpen, Globe2, MessageCircle, Paperclip, Plus, Search, Send,
} from "lucide-react";
import { Fragment, useCallback, useLayoutEffect, useRef, useState } from "react";
import type { FormEvent } from "react";
import { api, errorMessage } from "./api";
import type { Conversation, ShareSource, Transfer, User } from "./api";
import { Avatar, Blank, chatName, dateLabel, timeLabel, usePage } from "./common";
import { FilesView, SharedFile, TransferList } from "./files";

export function ChatList({ chats, own, selected, open, newChat }: {
  chats: Conversation[]; own: User; selected?: string; open: (id: string) => void; newChat: () => void;
}) {
  const [search, setSearch] = useState("");
  const query = search.trim().toLocaleLowerCase();
  const filtered = chats.filter(chat => chatName(chat, own.id).toLocaleLowerCase().includes(query));
  return <aside className="chat-list">
    <div className="chat-list-head"><div><h1>Chats</h1></div>
      <button className="square-primary" aria-label="New conversation" title="New conversation" onClick={newChat}><Plus size={22} /></button>
    </div>
    <label className="search"><Search size={17} /><input aria-label="Search conversations"
      placeholder="Search conversations" value={search} onChange={event => setSearch(event.target.value)} /></label>
    <div className="chat-rows">
      {filtered.map(chat => {
        const name = chatName(chat, own.id);
        return <button key={chat.id} className={"chat-row" + (chat.id === selected ? " selected" : "")}
          aria-current={chat.id === selected ? "true" : undefined} onClick={() => open(chat.id)}>
          <Avatar name={name} group={!!chat.title} />
          <span className="chat-row-text"><strong>{name}</strong><small>{chat.preview || "New conversation"}</small></span>
          <time title={new Date(chat.activity_at).toLocaleString()}>{dateLabel(chat.activity_at) === "Today"
            ? timeLabel(chat.activity_at) : new Date(chat.activity_at).toLocaleDateString(undefined, { month: "short", day: "numeric" })}</time>
        </button>;
      })}
      {filtered.length === 0 && <p className="list-empty muted">{search ? "No matching chats." : "Start a chat with a contact."}</p>}
    </div>
  </aside>;
}

export function ChatView({ chat, own, revision, transfers, changed, refresh, back, share }: {
  chat: Conversation; own: User; revision: number; transfers: Transfer[]; changed: () => void;
  refresh: () => Promise<void>; back: () => void; share: (source: ShareSource) => Promise<void>;
}) {
  const [showFiles, setShowFiles] = useState(false);
  const name = chatName(chat, own.id);
  return <section className="chat-view">
    <header className="chat-header">
      <button className="icon-button mobile-back" onClick={back} aria-label="Back to chats"><ArrowLeft size={20} /></button>
      <Avatar name={name} group={!!chat.title} />
      <div className="chat-heading"><h2>{name}</h2>
        <span>{chat.title ? chat.members.length + " members" : "@" + name}</span>
      </div>
      <span className="online-mode" title="Messages and attachments are delivered through your Zibblu server"><Globe2 size={15} /> Normal chat</span>
      <button className={"button subtle header-files" + (showFiles ? " toggled" : "")} aria-pressed={showFiles}
        onClick={() => setShowFiles(!showFiles)}><FolderOpen size={17} /> <span>{showFiles ? "Back to messages" : "Shared files"}</span></button>
    </header>
    {showFiles
      ? <FilesView conversation={chat.id} people={chat.members} transfers={transfers} revision={revision} onChanged={changed} />
      : <>
        <Timeline chat={chat} own={own} revision={revision} transfers={transfers} changed={changed} />
        <Composer conversation={chat.id} refresh={refresh} share={share} />
      </>}
  </section>;
}

function Timeline({ chat, own, revision, transfers, changed }: {
  chat: Conversation; own: User; revision: number; transfers: Transfer[]; changed: () => void;
}) {
  const read = useCallback((before?: number) => api.messages(chat.id, before), [chat.id]);
  const { page, failure, loading, older } = usePage(read, revision);
  const viewport = useRef<HTMLDivElement>(null);
  const nearBottom = useRef(true);
  const [away, setAway] = useState(false);
  const lastSeq = page?.items[0]?.seq;
  const pending = transfers.filter(job => job.direction === "upload" && job.conversation_id === chat.id && job.status !== "complete");
  const downloads = new Map(transfers.filter(job => job.direction === "download").map(job => [job.attachment.id, job]));
  const history = page?.items.slice().reverse() ?? [];

  useLayoutEffect(() => {
    if (nearBottom.current && viewport.current) viewport.current.scrollTop = viewport.current.scrollHeight;
  }, [lastSeq, pending.length]);

  function checkScroll() {
    const element = viewport.current;
    if (!element) return;
    nearBottom.current = element.scrollHeight - element.scrollTop - element.clientHeight < 100;
    setAway(!nearBottom.current);
  }

  return <div className="timeline-wrap">
    <div className="timeline" ref={viewport} onScroll={checkScroll}>
      {failure && <p className="inline-error" role="alert">{failure}</p>}
      {!page && !failure && <p className="muted loading" role="status">Loading messages…</p>}
      {page?.has_more && <button className="button subtle more" disabled={loading} onClick={() => void older()}>Load older messages</button>}
      {page && history.length === 0 && pending.length === 0 && <Blank icon={<MessageCircle size={27} />} title="Start the conversation">
        <p>Say hello or share a file.</p>
      </Blank>}
      <div className="messages" role="log" aria-label="Messages">
        {history.map((message, index) => {
          const mine = message.sender.id === own.id;
          const previous = history[index - 1];
          const newDay = !previous || new Date(message.created_at).toDateString() !== new Date(previous.created_at).toDateString();
          const sameSender = !newDay && previous?.sender.id === message.sender.id
            && message.created_at - previous.created_at < 5 * 60_000;
          return <Fragment key={message.seq}>
            {newDay && <div className="day-divider"><span>{dateLabel(message.created_at)}</span></div>}
            <div className={"message-row" + (mine ? " mine" : "") + (sameSender ? " continuation" : "")}>
              {!mine && (sameSender ? <span className="avatar-spacer" /> : <Avatar name={message.sender.handle} />)}
              <div className="message-body">
                {!mine && !sameSender && <span className="message-author">{message.sender.handle}</span>}
                {message.attachment
                  ? <SharedFile message={message} transfer={downloads.get(message.attachment.id)} onChanged={changed} compact />
                  : <div className="bubble">{message.text}</div>}
                <time dateTime={new Date(message.created_at).toISOString()}>{timeLabel(message.created_at)}</time>
              </div>
            </div>
          </Fragment>;
        })}
      </div>
      {pending.length > 0 && <div className="pending-chat"><TransferList transfers={pending} onChanged={changed} /></div>}
    </div>
    {away && <button className="button subtle jump-bottom" onClick={() => {
      if (viewport.current) viewport.current.scrollTop = viewport.current.scrollHeight;
    }}><ArrowDown size={15} /> Latest messages</button>}
  </div>;
}

function Composer({ conversation, refresh, share }: {
  conversation: string; refresh: () => Promise<void>; share: (source: ShareSource) => Promise<void>;
}) {
  const [draft, setDraft] = useState("");
  const [sending, setSending] = useState(false);
  const [preparing, setPreparing] = useState(false);
  const [failure, setFailure] = useState("");
  const pending = useRef<{ text: string; nonce: string } | undefined>(undefined);
  const oversized = new TextEncoder().encode(draft.trim()).length > 8192;

  async function send(event: FormEvent) {
    event.preventDefault();
    const text = draft.trim();
    if (!text || oversized || sending) return;
    if (pending.current?.text !== text) pending.current = { text, nonce: crypto.randomUUID() };
    setSending(true);
    setFailure("");
    try {
      await api.send(conversation, text, pending.current.nonce);
      pending.current = undefined;
      setDraft(current => current.trim() === text ? "" : current);
      await refresh();
    } catch (error) { setFailure(errorMessage(error)); }
    finally { setSending(false); }
  }

  async function pick(kind: "file" | "folder") {
    setPreparing(true);
    try { await share({ kind }); }
    finally { setPreparing(false); }
  }

  return <form className="composer" onSubmit={send}>
    {(failure || oversized) && <div className="inline-error" role="alert">{oversized ? "Messages can be up to 8 KiB." : failure}</div>}
    <div className="composer-box">
      <textarea aria-label="Message" placeholder="Write a message…" rows={2} value={draft} onChange={event => setDraft(event.target.value)}
        onKeyDown={event => {
          if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
            event.preventDefault();
            event.currentTarget.form?.requestSubmit();
          }
        }} />
      <div className="composer-bar">
        <div className="composer-attachments">
          <button className="icon-button" type="button" aria-label="Attach a file" title="Attach a file"
            onClick={() => void pick("file")} disabled={preparing}><Paperclip size={20} /></button>
          <button className="icon-button" type="button" aria-label="Attach a folder" title="Attach a folder"
            onClick={() => void pick("folder")} disabled={preparing}><FilePlus2 size={20} /></button>
          <span className="drop-hint">{preparing ? "Preparing attachment…" : "Drop a file or folder to share"}</span>
        </div>
        <button className="button primary send-button" type="submit" disabled={!draft.trim() || oversized || sending}
          aria-label={sending ? "Sending message" : "Send message"}><span>{sending ? "Sending…" : "Send"}</span><Send size={16} /></button>
      </div>
    </div>
  </form>;
}
