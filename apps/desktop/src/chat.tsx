import {
  ArrowDown, ArrowLeft, ArrowRight, AtSign, Bot, FilePlus2, FolderOpen, Ghost, Globe2, MessageCircle, Paperclip, Plus, QrCode, Search, Send,
} from "lucide-react";
import { Fragment, useCallback, useId, useLayoutEffect, useRef, useState } from "react";
import type { FormEvent } from "react";
import { ai } from "./ai-api";
import type { AiReply, Assistant } from "./ai-api";
import { AiRequest, AssistantMenu, AssistantReply } from "./ai";
import { api, errorMessage } from "./api";
import type { Conversation, Page, PrivateSummary, ShareSource, Transfer, User } from "./api";
import { Avatar, Blank, chatName, dateLabel, timeLabel, usePage } from "./common";
import { FilesView, SharedFile, TransferList } from "./files";
import { privatePhaseLabel, privateTitle } from "./private-chat";

export function ChatList({ chats, own, selected, open, newChat, rooms, activePrivate, openPrivate, createQr }: {
  chats: Conversation[]; own: User; selected?: string; open: (id: string) => void; newChat: () => void;
  rooms: PrivateSummary[]; activePrivate?: string; openPrivate: (id: string) => void; createQr: () => void;
}) {
  const [search, setSearch] = useState("");
  const query = search.trim().toLocaleLowerCase();
  const filtered = chats.filter(chat => chatName(chat, own.id).toLocaleLowerCase().includes(query));
  return <aside className="chat-list">
    <div className="chat-list-head"><div><h1>Chats</h1></div>
      <div className="chat-actions">
        <button className="icon-button" aria-label="Start private QR chat" title="Start private QR chat"
          onClick={createQr}><QrCode size={20} /></button>
        <button className="square-primary" aria-label="New conversation" title="New conversation" onClick={newChat}><Plus size={22} /></button>
      </div>
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
      {rooms.length > 0 && <div className="private-rows" aria-label="Private chats">
        <h2>Private chats</h2>
        {rooms.map(room => <button key={room.id} className={"chat-row" + (room.id === activePrivate ? " selected" : "")}
          aria-current={room.id === activePrivate ? "true" : undefined} onClick={() => openPrivate(room.id)}>
          <Ghost size={20} className="private-ghost" />
          <span className="chat-row-text"><strong>{privateTitle(room, own.id)}</strong><small>
            {room.phase === "waiting" && room.open ? "Waiting for a QR scan" : privatePhaseLabel[room.phase]}
          </small></span>
        </button>)}
      </div>}
    </div>
  </aside>;
}

type ChatAssistants = {
  assistants: Assistant[]; settings: () => void; reply?: AiReply;
  answered: (reply: AiReply) => void; dismiss: (id: string) => void;
};

export function ChatView({ chat, own, revision, transfers, changed, refresh, back, share, openPrivate, groupCode, aiChat }: {
  chat: Conversation; own: User; revision: number; transfers: Transfer[]; changed: () => void;
  refresh: () => Promise<void>; back: () => void; share: (source: ShareSource) => Promise<void>; openPrivate: () => void;
  groupCode: () => void; aiChat: ChatAssistants;
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
      <button className="icon-button" aria-label="Open private chat" title="Open private chat"
        onClick={openPrivate}><Ghost size={20} /></button>
      {chat.title && <button className="icon-button" aria-label="Group invite QR code" title="Group invite QR code"
        onClick={groupCode}><QrCode size={20} /></button>}
      <button className={"button subtle header-files" + (showFiles ? " toggled" : "")} aria-pressed={showFiles}
        onClick={() => setShowFiles(!showFiles)}><FolderOpen size={17} /> <span>{showFiles ? "Back to messages" : "Shared files"}</span></button>
    </header>
    {showFiles
      ? <FilesView conversation={chat.id} people={chat.members} transfers={transfers} revision={revision} onChanged={changed} />
      : <ChatMessages chat={chat} name={name} own={own} revision={revision} transfers={transfers}
        changed={changed} refresh={refresh} share={share} aiChat={aiChat} />}
  </section>;
}

function ChatMessages({ chat, name, own, revision, transfers, changed, refresh, share, aiChat }: {
  chat: Conversation; name: string; own: User; revision: number; transfers: Transfer[]; changed: () => void;
  refresh: () => Promise<void>; share: (source: ShareSource) => Promise<void>; aiChat: ChatAssistants;
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
  }, [lastSeq, pending.length, aiChat.reply]);

  function checkScroll() {
    const element = viewport.current;
    if (!element) return;
    nearBottom.current = element.scrollHeight - element.scrollTop - element.clientHeight < 100;
    setAway(!nearBottom.current);
  }

  return <><div className="timeline-wrap">
    <div className="timeline" ref={viewport} onScroll={checkScroll}>
      {failure && <p className="inline-error" role="alert">{failure}</p>}
      {!page && !failure && <p className="muted loading" role="status">Loading messages…</p>}
      {page?.has_more && <button className="button subtle more" disabled={loading} onClick={() => void older()}>Load older messages</button>}
      {page && history.length === 0 && pending.length === 0 && !aiChat.reply && <Blank icon={<MessageCircle size={27} />} title="Start the conversation">
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
        {aiChat.reply && <AssistantReply key={aiChat.reply.id} reply={aiChat.reply} chat={name}
          conversation={chat.id} refresh={refresh} dismiss={() => aiChat.dismiss(aiChat.reply!.id)} />}
      </div>
      {pending.length > 0 && <div className="pending-chat"><TransferList transfers={pending} onChanged={changed} /></div>}
    </div>
    {away && <button className="button subtle jump-bottom" onClick={() => {
      if (viewport.current) viewport.current.scrollTop = viewport.current.scrollHeight;
    }}><ArrowDown size={15} /> Latest messages</button>}
  </div>
    <Composer conversation={chat.id} chat={name} refresh={refresh} share={share} aiChat={aiChat}
      page={page} loading={loading} older={older} />
  </>;
}

function Composer({ conversation, chat, refresh, share, aiChat, page, loading, older }: {
  conversation: string; chat: string; refresh: () => Promise<void>; share: (source: ShareSource) => Promise<void>;
  aiChat: ChatAssistants; page?: Page; loading: boolean; older: () => Promise<void>;
}) {
  const [draft, setDraft] = useState("");
  const [view, setView] = useState<string>();
  const [menu, setMenu] = useState(false);
  const [menuIndex, setMenuIndex] = useState(0);
  const [request, setRequest] = useState<{ id: string; assistant: Assistant; prompt: string; draft: string }>();
  const textarea = useRef<HTMLTextAreaElement>(null);
  const menuId = useId();
  const mention = /^\s*@([\p{L}\p{N}_-]*)(?=\s|$)/u.exec(draft);
  const prefix = mention?.[1].toLowerCase() ?? "";
  const prompt = mention ? draft.slice(mention[0].length).trim() : draft.trim();
  const assistant = prefix ? aiChat.assistants.find(entry => entry.name === prefix) : undefined;
  const suggestions = aiChat.assistants.filter(entry => entry.name.startsWith(prefix));
  const activeOption = suggestions[menuIndex];

  useLayoutEffect(() => {
    const id = crypto.randomUUID();
    let current = true;
    void ai.openView(id, conversation).then(() => { if (current) setView(id); })
      .catch(error => { if (current) setFailure(errorMessage(error)); });
    return () => {
      current = false;
      void ai.closeView(id).catch(() => {});
    };
  }, [conversation]);

  function choose(entry: Assistant) {
    setDraft("@" + entry.name + " " + prompt);
    setMenu(false);
    textarea.current?.focus();
  }

  function closeRequest() {
    if (view && request) void ai.cancel(view, request.id).catch(() => {});
    setRequest(undefined);
  }
  const [sending, setSending] = useState(false);
  const [preparing, setPreparing] = useState(false);
  const [failure, setFailure] = useState("");
  const pending = useRef<{ text: string; nonce: string } | undefined>(undefined);
  const oversized = new TextEncoder().encode(draft.trim()).length > 8192;

  async function send(event: FormEvent) {
    event.preventDefault();
    const text = draft.trim();
    if (!text || oversized || sending || request) return;
    if (assistant) {
      if (!prompt || !view) return;
      const id = crypto.randomUUID();
      setSending(true);
      setMenu(false);
      setFailure("");
      try {
        await ai.begin(view, id);
        setRequest({ id, assistant, prompt, draft: text });
      } catch (error) { setFailure(errorMessage(error)); }
      finally { setSending(false); }
      return;
    }
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

  return <>
    <form className="composer" onSubmit={send} onBlur={event => {
      if (!event.currentTarget.contains(event.relatedTarget)) setMenu(false);
    }}>
      {(failure || oversized) && <div className="inline-error" role="alert">{oversized ? "Messages can be up to 8 KiB." : failure}</div>}
      {menu && <AssistantMenu id={menuId} assistants={suggestions} active={menuIndex}
        choose={choose} settings={aiChat.settings} />}
      <div className={"composer-box" + (assistant ? " ai-composer" : "")}>
        <textarea ref={textarea} aria-label={assistant ? "AI prompt" : "Message"}
          aria-autocomplete="list" aria-controls={menu ? menuId : undefined}
          aria-activedescendant={menu && activeOption ? menuId + "-" + menuIndex : undefined}
          placeholder="Write a message…" rows={2} value={draft} onChange={event => {
            const value = event.target.value;
            setDraft(value);
            setMenu(/^\s*@[\p{L}\p{N}_-]*\s*$/u.test(value));
            setMenuIndex(0);
          }}
          onKeyDown={event => {
            if (event.nativeEvent.isComposing) return;
            if (menu && event.key === "Escape") {
              event.preventDefault();
              setMenu(false);
            }
            if (menu && suggestions.length > 0 && (event.key === "ArrowDown" || event.key === "ArrowUp")) {
              event.preventDefault();
              const step = event.key === "ArrowDown" ? 1 : -1;
              setMenuIndex(index => (index + step + suggestions.length) % suggestions.length);
            }
            if (event.key === "Enter" && !event.shiftKey) {
              event.preventDefault();
              if (menu && activeOption) choose(activeOption);
              else event.currentTarget.form?.requestSubmit();
            }
          }} />
        <div className="composer-bar">
          <div className="composer-attachments">
            {assistant ? <span className="ai-composer-target"><Bot size={16} /> @{assistant.name}</span> : <>
              <button className="icon-button" type="button" aria-label="Attach a file" title="Attach a file"
                onClick={() => void pick("file")} disabled={preparing}><Paperclip size={20} /></button>
              <button className="icon-button" type="button" aria-label="Attach a folder" title="Attach a folder"
                onClick={() => void pick("folder")} disabled={preparing}><FilePlus2 size={20} /></button>
            </>}
            <button className="icon-button ai-mention-button" type="button" aria-label="Mention an AI assistant"
              aria-controls={menu ? menuId : undefined} aria-expanded={menu} title="Mention an AI assistant" onClick={() => {
                setMenu(!menu);
                setMenuIndex(0);
                textarea.current?.focus();
              }}><AtSign size={19} /></button>
            {!assistant && <span className="drop-hint">{preparing ? "Preparing attachment…" : "Drop a file or folder to share"}</span>}
          </div>
          <button className="button primary send-button" type="submit"
            disabled={!draft.trim() || oversized || sending || (!!assistant && (!view || !prompt))}
            aria-label={assistant ? "Choose AI context" : sending ? "Sending message" : "Send message"}>
            {assistant ? <>Choose context <ArrowRight size={16} /></>
              : <><span>{sending ? "Sending…" : "Send"}</span><Send size={16} /></>}
          </button>
        </div>
      </div>
    </form>
    {view && request && <AiRequest view={view} draft={request.id} assistant={request.assistant} prompt={request.prompt}
      messages={page?.items ?? []} hasMore={page?.has_more ?? false} loading={loading} older={older} chat={chat}
      close={closeRequest} answered={reply => {
        aiChat.answered(reply);
        setDraft(current => current.trim() === request.draft ? "" : current);
        closeRequest();
      }} />}
  </>;
}
