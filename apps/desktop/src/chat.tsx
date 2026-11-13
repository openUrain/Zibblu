import {
  ArrowDown, ArrowLeft, ArrowRight, AtSign, FilePlus2, Ghost, Globe2, LockKeyhole,
  MessageCircle, MoreHorizontal, Paperclip, Plus, QrCode, Search, Send, UsersRound, X,
} from "lucide-react";
import { Fragment, useCallback, useId, useLayoutEffect, useRef, useState } from "react";
import type { FormEvent } from "react";
import { ai } from "./ai-api";
import type { AiReply, Assistant } from "./ai-api";
import { AiRequest, AssistantMenu, ProviderLogo, ShareAiReply } from "./ai";
import { api, errorMessage } from "./api";
import type { Conversation, Page, PrivateSummary, ShareSource, Transfer, User } from "./api";
import { Avatar, Blank, Dialog, chatName, dateLabel, timeLabel, usePage } from "./common";
import { useMenu } from "./context-menu";
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
  assistants: Assistant[]; settings: () => void; replies: AiReply[];
  answered: (reply: AiReply) => void; dismiss: (id: string) => void;
};

export function ChatView({ chat, own, revision, latestAiSeq, recalled, clockOffset, transfers, changed, refresh, back, share, openPrivate, groupCode, aiChat }: {
  chat: Conversation; own: User; revision: number; latestAiSeq: number; recalled: ReadonlySet<number>; clockOffset: number; transfers: Transfer[]; changed: () => void;
  refresh: () => Promise<void>; back: () => void; share: (source: ShareSource) => Promise<void>; openPrivate: () => void;
  groupCode: () => void; aiChat: ChatAssistants;
}) {
  const [showFiles, setShowFiles] = useState(false);
  const [search, setSearch] = useState(false);
  const openMenu = useMenu();
  const name = chatName(chat, own.id);
  return <section className="chat-view" aria-label={name}>
    <header className="chat-header">
      <button className="icon-button mobile-back" onClick={back} aria-label="Back to chats"><ArrowLeft size={20} /></button>
      <span className="online-mode" title="Messages and attachments are delivered through your Zibblu server"><Globe2 size={15} /> Normal chat</span>
      {showFiles && <button className="button subtle small" onClick={() => setShowFiles(false)}><ArrowLeft size={15} /> Messages</button>}
      <button className="icon-button" aria-label="Open private chat" title="Open private chat"
        onClick={openPrivate}><Ghost size={20} /></button>
      {chat.title && <button className="icon-button" aria-label="Group invite QR code" title="Group invite QR code"
        onClick={groupCode}><QrCode size={20} /></button>}
      <button className="icon-button" aria-label="Conversation actions" title="Conversation actions" aria-haspopup="menu"
        onClick={event => openMenu(event.currentTarget, [
          { label: "Search chat", action: () => setSearch(true) },
          { label: "Shared files", action: () => setShowFiles(true), disabled: showFiles },
        ])}><MoreHorizontal size={22} /></button>
    </header>
    {showFiles
      ? <FilesView conversation={chat.id} people={chat.members} transfers={transfers} revision={revision} recalled={recalled} onChanged={changed} />
      : <ChatMessages chat={chat} own={own} revision={revision} latestAiSeq={latestAiSeq} recalled={recalled} clockOffset={clockOffset} transfers={transfers}
        changed={changed} refresh={refresh} share={share} aiChat={aiChat} />}
    {search && <ChatSearch conversation={chat.id} revision={revision} latestAiSeq={latestAiSeq} recalled={recalled} close={() => setSearch(false)} />}
  </section>;
}

function ChatSearch({ conversation, revision, latestAiSeq, recalled, close }: {
  conversation: string; revision: number; latestAiSeq: number; recalled: ReadonlySet<number>; close: () => void;
}) {
  const [query, setQuery] = useState("");
  const [search, setSearch] = useState("");
  return <Dialog title="Search chat" close={close}>
    <form className="chat-search" onSubmit={event => { event.preventDefault(); setSearch(query.trim()); }}>
      <label className="search"><Search size={16} /><input aria-label="Search message text" value={query}
        onChange={event => setQuery(event.target.value)} placeholder="Search message text" autoFocus /></label>
      <button className="button primary" type="submit" disabled={!query.trim()}>Search</button>
    </form>
    {search && <SearchResults key={search} {...{ conversation, search, revision, latestAiSeq, recalled }} />}
  </Dialog>;
}

function SearchResults({ conversation, search, revision, latestAiSeq, recalled }: {
  conversation: string; search: string; revision: number; latestAiSeq: number; recalled: ReadonlySet<number>;
}) {
  const read = useCallback((before?: number) => api.messages(conversation, before, search), [conversation, search]);
  const { page, failure, loading, older } = usePage(read, revision, recalled, false, latestAiSeq);
  return <div className="chat-search-results">
    {failure && <p className="inline-error" role="alert">{failure}</p>}
    {!page && !failure && <p className="muted" role="status">Searching…</p>}
    {page?.items.length === 0 && <p className="muted">No matching messages.</p>}
    {page?.items.map(message => <article key={message.seq}>
      <small>{message.ai_provider ? <ProviderLogo provider={message.ai_provider} /> : message.sender.handle}
        {" · "}{new Date(message.created_at).toLocaleString()}</small>
      <p>{message.text}</p>
    </article>)}
    {page?.has_more && <button className="button subtle" disabled={loading} onClick={() => void older()}>Load older matches</button>}
  </div>;
}

function ChatMessages({ chat, own, revision, latestAiSeq, recalled, clockOffset, transfers, changed, refresh, share, aiChat }: {
  chat: Conversation; own: User; revision: number; latestAiSeq: number; recalled: ReadonlySet<number>; clockOffset: number; transfers: Transfer[]; changed: () => void;
  refresh: () => Promise<void>; share: (source: ShareSource) => Promise<void>; aiChat: ChatAssistants;
}) {
  const read = useCallback((before?: number) => api.messages(chat.id, before), [chat.id]);
  const { page, failure, loading, older, newestAtOpen } = usePage(read, revision, recalled, true, latestAiSeq);
  const viewport = useRef<HTMLDivElement>(null);
  const nearBottom = useRef(true);
  const [away, setAway] = useState(false);
  const [recallError, setRecallError] = useState("");
  const [recalling, setRecalling] = useState<number>();
  const [sharingReply, setSharingReply] = useState<AiReply>();
  const openMenu = useMenu();
  const lastSeq = page?.items[0]?.seq;
  const pending = transfers.filter(job => job.direction === "upload" && job.conversation_id === chat.id && job.status !== "complete");
  const downloads = new Map(transfers.filter(job => job.direction === "download").map(job => [job.attachment.id, job]));
  const history = [
    ...(page?.items.map(message => ({ message, reply: undefined, time: message.created_at, key: String(message.seq) })) ?? []),
    ...aiChat.replies.map(reply => ({ message: undefined, reply, time: reply.createdAt, key: reply.id })),
  ].sort((a, b) => a.time - b.time || (a.message?.seq ?? Infinity) - (b.message?.seq ?? Infinity));

  useLayoutEffect(() => {
    if (nearBottom.current && viewport.current) viewport.current.scrollTop = viewport.current.scrollHeight;
  }, [lastSeq, pending.length, aiChat.replies.length]);

  async function recall(seq: number) {
    if (recalling !== undefined) return;
    setRecalling(seq);
    setRecallError("");
    try { await api.recall(chat.id, seq); await refresh(); }
    catch (error) { setRecallError(errorMessage(error)); }
    finally { setRecalling(undefined); }
  }

  function checkScroll() {
    const element = viewport.current;
    if (!element) return;
    nearBottom.current = element.scrollHeight - element.scrollTop - element.clientHeight < 100;
    setAway(!nearBottom.current);
  }

  return <><div className="timeline-wrap">
    <div className="timeline" ref={viewport} onScroll={checkScroll}>
      {failure && <p className="inline-error" role="alert">{failure}</p>}
      {recallError && <p className="inline-error" role="alert">{recallError}</p>}
      {!page && !failure && <p className="muted loading" role="status">Loading messages…</p>}
      {page?.has_more && <button className="button subtle more" disabled={loading} onClick={() => void older()}>Load older messages</button>}
      {page && history.length === 0 && pending.length === 0 && <Blank icon={<MessageCircle size={27} />} title="Start the conversation">
        <p>Say hello or share a file.</p>
      </Blank>}
      <div className="messages" role="log" aria-label="Messages">
        {history.map(({ message, reply, key, time }, index) => {
          const provider = message?.ai_provider ?? reply?.provider;
          const mine = !provider && message?.sender.id === own.id;
          const newDay = !index || new Date(time).toDateString() !== new Date(history[index - 1].time).toDateString();
          const animated = !!reply || (!!message && newestAtOpen !== undefined && message.seq > newestAtOpen);
          const canRecall = !!message && mine && !message.recalled && Date.now() + clockOffset - time <= 5 * 60_000;
          const avatar = provider ? <ProviderLogo provider={provider} /> : <Avatar name={message!.sender.handle} />;
          const actions = canRecall ? [{ label: "Recall message", danger: true, disabled: recalling !== undefined,
            action: () => void recall(message.seq) }] : [];
          return <Fragment key={key}>
            {newDay && <div className="day-divider"><span>{dateLabel(time)}</span></div>}
            <div className={"message-row" + (mine ? " mine" : "") + (animated ? " message-arrival" : "")}
              onContextMenu={actions.length ? event => {
                const selection = window.getSelection()?.toString();
                event.preventDefault();
                openMenu(event.currentTarget, selection ? [{
                  label: "Copy", action: () => { void navigator.clipboard.writeText(selection).catch(() => {}); },
                }, ...actions] : actions, { x: event.clientX, y: event.clientY });
              } : undefined}>
              {!mine && avatar}
              <div className="message-body">
                {!provider && !mine && <span className="message-author">{message!.sender.handle}</span>}
                {message?.recalled ? <div className="bubble recalled-message">Message recalled</div>
                  : message?.attachment ? <SharedFile message={message} transfer={downloads.get(message.attachment.id)} onChanged={changed} compact />
                    : <div className="bubble">{message?.text ?? reply?.text}</div>}
                <div className="message-meta">
                  <time dateTime={new Date(time).toISOString()}>{timeLabel(time)}</time>
                  {reply && <>
                    <small><LockKeyhole size={12} /> Only you</small>
                    <button type="button" className="text-button" onClick={() => setSharingReply(reply)}><UsersRound size={13} /> Visible to</button>
                    <button className="icon-button" aria-label="Dismiss AI reply" onClick={() => aiChat.dismiss(reply.id)}><X size={14} /></button>
                  </>}
                  {canRecall && <button className="icon-button message-menu" aria-label="Message actions" aria-haspopup="menu"
                    onClick={event => openMenu(event.currentTarget, actions)}><MoreHorizontal size={16} /></button>}
                </div>
              </div>
              {mine && avatar}
            </div>
          </Fragment>;
        })}
      </div>
      {pending.length > 0 && <div className="pending-chat"><TransferList transfers={pending} onChanged={changed} /></div>}
    </div>
    {away && <button className="button subtle jump-bottom" onClick={() => {
      if (viewport.current) viewport.current.scrollTop = viewport.current.scrollHeight;
    }}><ArrowDown size={15} /> Latest messages</button>}
  </div>
    <Composer conversation={chat.id} refresh={refresh} share={share} changed={changed} aiChat={aiChat}
      page={page} loading={loading} older={older} />
    {sharingReply && <ShareAiReply key={sharingReply.id} reply={sharingReply}
      members={chat.members.filter(user => user.id !== own.id)} close={() => setSharingReply(undefined)} published={async () => {
        await refresh();
        aiChat.dismiss(sharingReply.id);
        setSharingReply(undefined);
      }} />}
  </>;
}

function Composer({ conversation, refresh, share, changed, aiChat, page, loading, older }: {
  conversation: string; refresh: () => Promise<void>; share: (source: ShareSource) => Promise<void>; changed: () => void;
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
  const [pastedImage, setPastedImage] = useState<{ file: File; url: string }>();
  useLayoutEffect(() => {
    const url = pastedImage?.url;
    return () => { if (url) URL.revokeObjectURL(url); };
  }, [pastedImage]);
  const pending = useRef<{ text: string; nonce: string } | undefined>(undefined);
  const oversized = new TextEncoder().encode(draft.trim()).length > 8192;

  async function send(event: FormEvent) {
    event.preventDefault();
    const text = draft.trim();
    if (oversized || sending || request || (!text && !pastedImage)) return;
    if (pastedImage && !assistant) {
      setSending(true);
      setFailure("");
      try {
        await api.pasteImage(conversation, await pastedImage.file.arrayBuffer());
        setPastedImage(undefined);
        changed();
      } catch (error) {
        setFailure(errorMessage(error));
        return;
      } finally { setSending(false); }
      if (!text) return;
    }
    if (assistant) {
      if (pastedImage) {
        setFailure("Send or remove the pasted image before asking the assistant.");
        return;
      }
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
      <div className={"composer-box floating-field" + (assistant ? " ai-composer" : "")}>
        <textarea ref={textarea} id={menuId + "-message"} aria-label={assistant ? "AI prompt" : "Message"}
          aria-autocomplete="list" aria-controls={menu ? menuId : undefined}
          aria-activedescendant={menu && activeOption ? menuId + "-" + menuIndex : undefined}
          placeholder=" " rows={2} value={draft} onChange={event => {
            const value = event.target.value;
            setDraft(value);
            setMenu(/^\s*@[\p{L}\p{N}_-]*\s*$/u.test(value));
            setMenuIndex(0);
          }}
          onPaste={event => {
            const file = Array.from(event.clipboardData.items).find(item => item.kind === "file" && item.type.startsWith("image/"))?.getAsFile();
            if (!file) return;
            event.preventDefault();
            if (sending) return;
            setFailure("");
            if (file.size > 16 * 1024 * 1024 || !/^image\/(png|jpeg|gif|webp|bmp)$/.test(file.type)) {
              setFailure("Paste a PNG, JPEG, GIF, WebP or BMP image up to 16 MiB.");
              return;
            }
            setPastedImage({ file, url: URL.createObjectURL(file) });
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
        <label htmlFor={menuId + "-message"} className="composer-label">Write a message…</label>
        {pastedImage && <div className="pasted-image">
          <img src={pastedImage.url} alt="Pasted image ready to send" />
          <button className="icon-button" type="button" disabled={sending} aria-label="Remove pasted image"
            onClick={() => setPastedImage(undefined)}><X size={16} /></button>
        </div>}
        <div className="composer-bar">
          <div className="composer-attachments">
            {assistant ? <span className="ai-composer-target"><ProviderLogo provider={assistant.provider} /> @{assistant.name}</span> : <>
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
            disabled={(!draft.trim() && !pastedImage) || oversized || sending || (!!assistant && (!view || !prompt))}
            aria-label={assistant ? "Choose AI context" : sending ? "Sending message" : "Send message"}>
            {assistant ? <>Choose context <ArrowRight size={16} /></>
              : <><span>{sending ? "Sending…" : "Send"}</span><Send size={16} /></>}
          </button>
        </div>
      </div>
    </form>
    {view && request && <AiRequest view={view} draft={request.id} assistant={request.assistant} prompt={request.prompt}
      messages={page?.items ?? []} hasMore={page?.has_more ?? false} loading={loading} older={older} conversation={conversation}
      close={closeRequest} answered={reply => {
        aiChat.answered(reply);
        setDraft(current => current.trim() === request.draft ? "" : current);
        closeRequest();
      }} />}
  </>;
}
