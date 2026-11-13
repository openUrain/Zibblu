import {
  ArrowDown, ArrowLeft, ArrowRight, AtSign, CheckCheck, Copy, FilePlus2, FolderOpen, Ghost, LockKeyhole,
  MessageCircle, MoreHorizontal, Paperclip, Plus, QrCode, Reply, Search, Send, Trash2, Undo2, UsersRound, X,
} from "lucide-react";
import { Fragment, useCallback, useContext, useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import type { FormEvent } from "react";
import { ai } from "./ai-api";
import type { AiReply, Assistant } from "./ai-api";
import { AiRequest, AssistantMenu, ProviderLogo, ShareAiReply } from "./ai";
import { api, errorMessage } from "./api";
import type { Conversation, Draft, Message, Page, PrivateSummary, ShareSource, Transfer, User } from "./api";
import { Avatar, Blank, Dialog, People, PersonAvatar, chatName, dateTimeLabel, displayName, messageTimeLabel, startsTimeGroup, usePage } from "./common";
import { useMenu } from "./context-menu";
import { FilesView, SharedFile, TransferList } from "./files";
import { privatePhaseLabel, privateTitle } from "./private-chat";

export function ChatList({ chats, own, selected, open, newChat, rooms, activePrivate, openPrivate, createQr }: {
  chats: Conversation[]; own: User; selected?: string; open: (id: string) => void; newChat: () => void;
  rooms: PrivateSummary[]; activePrivate?: string; openPrivate: (id: string) => void; createQr: () => void;
}) {
  const { remarks } = useContext(People);
  const [search, setSearch] = useState("");
  const query = search.trim().toLocaleLowerCase();
  const filtered = chats.filter(chat => chatName(chat, own.id, remarks).toLocaleLowerCase().includes(query));
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
        const name = chatName(chat, own.id, remarks);
        const peer = chat.members.find(user => user.id !== own.id);
        return <button key={chat.id} className={"chat-row" + (chat.id === selected ? " selected" : "")}
          aria-current={chat.id === selected ? "true" : undefined} onClick={() => open(chat.id)}>
          {chat.title || !peer ? <Avatar name={name} group={!!chat.title} /> : <PersonAvatar user={peer} />}
          <span className="chat-row-text"><strong>{name}</strong><small>{chat.preview || "New conversation"}</small></span>
          <span className="chat-row-status"><time title={dateTimeLabel(chat.activity_at)}>{messageTimeLabel(chat.activity_at)}</time>
            {chat.unread > 0 && <span className="unread-count" aria-label={`${chat.unread} unread messages`}>{chat.unread > 99 ? "99+" : chat.unread}</span>}
          </span>
        </button>;
      })}
      {filtered.length === 0 && <p className="list-empty muted">{search ? "No matching chats." : "Start a chat with a contact."}</p>}
      {rooms.length > 0 && <div className="private-rows" aria-label="Private chats">
        <h2>Private chats</h2>
        {rooms.map(room => <button key={room.id} className={"chat-row" + (room.id === activePrivate ? " selected" : "")}
          aria-current={room.id === activePrivate ? "true" : undefined} onClick={() => openPrivate(room.id)}>
          <Ghost size={20} className="private-ghost" />
          <span className="chat-row-text"><strong>{privateTitle(room, own.id, remarks)}</strong><small>
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

type ChatDraft = { draft: Draft; setDraft: (change: (draft: Draft) => Draft) => void };

export function ChatView({ chat, own, revision, latestAiSeq, recalled, clockOffset, transfers, changed, refresh, back, share, openPrivate, groupCode, aiChat,
  hidden, hide, readReceipts, draft, setDraft }: {
  chat: Conversation; own: User; revision: number; latestAiSeq: number; recalled: ReadonlySet<number>; clockOffset: number; transfers: Transfer[]; changed: () => void;
  refresh: () => Promise<void>; back: () => void; share: (source: ShareSource) => Promise<void>; openPrivate: () => void;
  groupCode: () => void; aiChat: ChatAssistants; hidden: ReadonlySet<number>; hide: (seq: number) => Promise<void>; readReceipts: boolean;
} & ChatDraft) {
  const [showFiles, setShowFiles] = useState(false);
  const [search, setSearch] = useState(false);
  const openMenu = useMenu();
  const { remarks, open } = useContext(People);
  const name = chatName(chat, own.id, remarks);
  const peer = chat.title ? undefined : chat.members.find(user => user.id !== own.id);
  return <section className="chat-view" aria-label={name}>
    <header className="chat-header">
      <button className="icon-button mobile-back" onClick={back} aria-label="Back to chats"><ArrowLeft size={20} /></button>
      <div className="chat-heading"><h2>{peer ? <button className="person-link" onClick={() => open?.(peer)}>{name}</button> : name}</h2></div>
      {showFiles && <button className="button subtle small" onClick={() => setShowFiles(false)}><ArrowLeft size={15} /> Messages</button>}
      {chat.title && <button className="icon-button" aria-label="Group invite QR code" title="Group invite QR code"
        onClick={groupCode}><QrCode size={20} /></button>}
      <button className="icon-button" aria-label="Open private chat" title="Open private chat"
        onClick={openPrivate}><Ghost size={20} /></button>
      <button className="icon-button" aria-label="Conversation actions" title="Conversation actions" aria-haspopup="menu"
        onClick={event => openMenu(event.currentTarget, [
          { label: "Search chat", icon: <Search size={15} />, action: () => setSearch(true) },
          { label: "Shared files", icon: <FolderOpen size={15} />, action: () => setShowFiles(true), disabled: showFiles },
        ])}><MoreHorizontal size={22} /></button>
    </header>
    {showFiles
      ? <FilesView conversation={chat.id} people={chat.members} transfers={transfers} revision={revision} recalled={recalled} hidden={hidden} onChanged={changed} />
      : <ChatMessages chat={chat} own={own} revision={revision} latestAiSeq={latestAiSeq} recalled={recalled} clockOffset={clockOffset} transfers={transfers}
        changed={changed} refresh={refresh} share={share} aiChat={aiChat} {...{ hidden, hide, readReceipts, draft, setDraft }} />}
    {search && <ChatSearch conversation={chat.id} revision={revision} latestAiSeq={latestAiSeq} recalled={recalled} hidden={hidden} close={() => setSearch(false)} />}
  </section>;
}

function ChatSearch({ conversation, revision, latestAiSeq, recalled, hidden, close }: {
  conversation: string; revision: number; latestAiSeq: number; recalled: ReadonlySet<number>; hidden: ReadonlySet<number>; close: () => void;
}) {
  const [query, setQuery] = useState("");
  const [search, setSearch] = useState("");
  return <Dialog title="Search chat" close={close}>
    <form className="chat-search" onSubmit={event => { event.preventDefault(); setSearch(query.trim()); }}>
      <label className="search"><Search size={16} /><input aria-label="Search message text" value={query}
        onChange={event => setQuery(event.target.value)} placeholder="Search message text" autoFocus /></label>
      <button className="button primary" type="submit" disabled={!query.trim()}>Search</button>
    </form>
    {search && <SearchResults key={search} {...{ conversation, search, revision, latestAiSeq, recalled, hidden }} />}
  </Dialog>;
}

function SearchResults({ conversation, search, revision, latestAiSeq, recalled, hidden }: {
  conversation: string; search: string; revision: number; latestAiSeq: number; recalled: ReadonlySet<number>; hidden: ReadonlySet<number>;
}) {
  const read = useCallback((before?: number) => api.messages(conversation, before, search), [conversation, search]);
  const { remarks } = useContext(People);
  const { page, failure, loading, older } = usePage(read, revision, recalled, false, latestAiSeq, hidden);
  return <div className="chat-search-results">
    {failure && <p className="inline-error" role="alert">{failure}</p>}
    {!page && !failure && <p className="muted" role="status">Searching…</p>}
    {page?.items.length === 0 && <p className="muted">No matching messages.</p>}
    {page?.items.map(message => <article key={message.seq}>
      <small>{message.ai_provider ? <ProviderLogo provider={message.ai_provider} /> : displayName(message.sender, remarks)}
        {" · "}{dateTimeLabel(message.created_at)}</small>
      <p>{message.text}</p>
    </article>)}
    {page?.has_more && <button className="button subtle" disabled={loading} onClick={() => void older()}>Load older matches</button>}
  </div>;
}

function ChatMessages({ chat, own, revision, latestAiSeq, recalled, clockOffset, transfers, changed, refresh, share, aiChat,
  hidden, hide, readReceipts, draft, setDraft }: {
  chat: Conversation; own: User; revision: number; latestAiSeq: number; recalled: ReadonlySet<number>; clockOffset: number; transfers: Transfer[]; changed: () => void;
  refresh: () => Promise<void>; share: (source: ShareSource) => Promise<void>; aiChat: ChatAssistants;
  hidden: ReadonlySet<number>; hide: (seq: number) => Promise<void>; readReceipts: boolean;
} & ChatDraft) {
  const read = useCallback((before?: number) => api.messages(chat.id, before), [chat.id]);
  const { page, failure, loading, older, newestAtOpen, loadedRevision } = usePage(read, revision, recalled, true, latestAiSeq, hidden);
  const { remarks, open } = useContext(People);
  const viewport = useRef<HTMLDivElement>(null);
  const nearBottom = useRef(true);
  const [away, setAway] = useState(false);
  const [recallError, setRecallError] = useState("");
  const [recalling, setRecalling] = useState<number>();
  const [sharingReply, setSharingReply] = useState<AiReply>();
  const [seen, setSeen] = useState<ReadonlySet<number>>(new Set());
  const openMenu = useMenu();
  const lastSeq = page?.items[0]?.seq;
  const pending = transfers.filter(job => job.direction === "upload" && job.conversation_id === chat.id && job.status !== "complete");
  const downloads = new Map(transfers.filter(job => job.direction === "download").map(job => [job.attachment.id, job]));
  const history = [
    ...(page?.items.map(message => ({ message, reply: undefined, time: message.created_at, key: String(message.seq) })) ?? []),
    ...aiChat.replies.map(reply => ({ message: undefined, reply, time: reply.createdAt, key: reply.id })),
  ].sort((a, b) => a.time - b.time || (a.message?.seq ?? Infinity) - (b.message?.seq ?? Infinity));
  const bySequence = new Map(page?.items.map(message => [message.seq, message]));
  const sent = JSON.stringify(readReceipts ? page?.items.filter(message => message.sender.id === own.id && !message.recalled)
    .map(message => message.seq) ?? [] : []);
  const readThrough = !away && loadedRevision === revision && chat.latest_seq <= revision && chat.latest_seq > chat.read_through
    ? chat.latest_seq : undefined;
  let groupStart: number | undefined;

  useEffect(() => {
    const sequences: number[] = JSON.parse(sent);
    if (!sequences.length) setSeen(new Set());
    if (!readThrough && !sequences.length) return;
    let disposed = false;
    let timer: number | undefined;
    let marking = readThrough;
    async function update() {
      try {
        const element = viewport.current;
        const through = element?.getClientRects().length && document.visibilityState === "visible" && document.hasFocus()
          && !document.querySelector("dialog[open]")
          ? marking : undefined;
        if (!through && !sequences.length) return;
        const requests = [];
        for (let offset = 0; offset < sequences.length || offset === 0; offset += 200) {
          requests.push(api.readUpdate(chat.id, offset === 0 ? through : undefined, sequences.slice(offset, offset + 200)));
        }
        const results = await Promise.all(requests);
        if (disposed) return;
        setSeen(new Set(results.flat()));
        if (through) {
          marking = undefined;
          void refresh();
        }
      } catch { if (!disposed) setSeen(new Set()); }
      finally {
        if (!disposed && (marking || sequences.length)) timer = window.setTimeout(update, 1700);
      }
    }
    void update();
    return () => { disposed = true; window.clearTimeout(timer); };
  }, [chat.id, readThrough, sent, refresh]);

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

  async function deleteForMe(seq: number) {
    setRecallError("");
    try { await hide(seq); } catch (error) { setRecallError(errorMessage(error)); }
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
        {history.map(({ message, reply, key, time }) => {
          const provider = message?.ai_provider ?? reply?.provider;
          const mine = message?.sender.id === own.id;
          const newGroup = startsTimeGroup(time, groupStart);
          if (newGroup) groupStart = time;
          const animated = !!reply || (!!message && newestAtOpen !== undefined && message.seq > newestAtOpen);
          const canRecall = !!message && mine && !provider && !message.recalled && Date.now() + clockOffset - time <= 5 * 60_000;
          const avatar = provider ? <ProviderLogo provider={provider} /> : <button className="person-link message-avatar"
            aria-label={`View ${displayName(message!.sender, remarks)}'s profile`} onClick={() => open?.(message!.sender)}>
            <PersonAvatar user={message!.sender} />
          </button>;
          const text = message?.text ?? reply?.text;
          function actions(selection?: string) {
            const copy = selection || text;
            return [
              ...(copy ? [{ label: "Copy", icon: <Copy size={15} />, action: () => {
                void navigator.clipboard.writeText(copy).catch(error => setRecallError(errorMessage(error)));
              } }] : []),
              ...(message && !message.recalled ? [{ label: "Quote", icon: <Reply size={15} />, action: () => {
                setDraft(previous => ({ ...previous, quote: message, nonce: undefined }));
                viewport.current?.closest(".chat-view")?.querySelector<HTMLTextAreaElement>(".composer textarea")?.focus();
              } }] : []),
              ...(canRecall ? [{ label: "Recall message", icon: <Undo2 size={15} />, disabled: recalling !== undefined,
                action: () => void recall(message.seq) }] : []),
              ...(message ? [{ label: "Delete for me", icon: <Trash2 size={15} />, action: () => void deleteForMe(message.seq) }] : []),
            ];
          }
          return <Fragment key={key}>
            {newGroup && <div className="day-divider"><time dateTime={new Date(time).toISOString()}>{messageTimeLabel(time)}</time></div>}
            <div className={"message-row" + (mine ? " mine" : "") + (animated ? " message-arrival" : "")}
              onContextMenu={event => {
                const selection = window.getSelection()?.toString();
                event.preventDefault();
                openMenu(event.currentTarget, actions(selection), { x: event.clientX, y: event.clientY });
              }}>
              {!mine && !message?.attachment && avatar}
              <div className="message-body">
                {!provider && !mine && !message?.attachment && <span className="message-author">{displayName(message!.sender, remarks)}</span>}
                {message?.reply_to !== undefined && !message.recalled && <QuotedMessage conversation={chat.id} seq={message.reply_to}
                  known={bySequence.get(message.reply_to)} hidden={hidden.has(message.reply_to) || recalled.has(message.reply_to)} />}
                {message?.recalled ? <div className="bubble recalled-message">Message recalled</div>
                  : message?.attachment ? <SharedFile message={message} transfer={downloads.get(message.attachment.id)} onChanged={changed} compact />
                    : <div className="bubble">{message?.text ?? reply?.text}</div>}
                <div className="message-meta">
                  {message && readReceipts && seen.has(message.seq) && !message.recalled && <span className="read-mark" aria-label="Read" title="Read"><CheckCheck size={15} /></span>}
                  {reply && <>
                    <small><LockKeyhole size={12} /> Only you</small>
                    <button type="button" className="text-button" onClick={() => setSharingReply(reply)}><UsersRound size={13} /> Visible to</button>
                    <button className="icon-button" aria-label="Dismiss AI reply" onClick={() => aiChat.dismiss(reply.id)}><X size={14} /></button>
                  </>}
                  <button className="icon-button message-menu" aria-label="Message actions" aria-haspopup="menu"
                    onClick={event => openMenu(event.currentTarget, actions())}><MoreHorizontal size={16} /></button>
                </div>
              </div>
              {mine && !message?.attachment && avatar}
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
      page={page} loading={loading} older={older} draft={draft} setDraft={setDraft}
      quoteUnavailable={!!draft.quote && (recalled.has(draft.quote.seq) || hidden.has(draft.quote.seq))} />
    {sharingReply && <ShareAiReply key={sharingReply.id} reply={sharingReply}
      members={chat.members.filter(user => user.id !== own.id)} close={() => setSharingReply(undefined)} published={async () => {
        await refresh();
        aiChat.dismiss(sharingReply.id);
        setSharingReply(undefined);
      }} />}
  </>;
}

function QuotedMessage({ conversation, seq, known, hidden }: { conversation: string; seq: number; known?: Message; hidden: boolean }) {
  const [loaded, setLoaded] = useState<Message | null>(null);
  const { remarks } = useContext(People);
  useEffect(() => {
    if (known || hidden) return;
    let current = true;
    void api.quotedMessage(conversation, seq).then(value => { if (current) setLoaded(value); }).catch(() => {});
    return () => { current = false; };
  }, [conversation, seq, known, hidden]);
  const quote = known ?? loaded;
  return <div className="message-quote"><Reply size={14} />{quote && !hidden && !quote.recalled
    ? <span><strong>{displayName(quote.sender, remarks)}</strong> {quote.text ?? quote.attachment?.name}</span>
    : <span>Message unavailable</span>}</div>;
}

function Composer({ conversation, refresh, share, changed, aiChat, page, loading, older, draft: saved, setDraft, quoteUnavailable }: {
  conversation: string; refresh: () => Promise<void>; share: (source: ShareSource) => Promise<void>; changed: () => void;
  aiChat: ChatAssistants; page?: Page; loading: boolean; older: () => Promise<void>; quoteUnavailable: boolean;
} & ChatDraft) {
  const { remarks } = useContext(People);
  const draft = saved.text;
  const quote = quoteUnavailable ? undefined : saved.quote;
  const pastedImage = saved.image;
  const [imageUrl, setImageUrl] = useState<string>();
  const [view, setView] = useState<string>();
  const [menu, setMenu] = useState(false);
  const [menuIndex, setMenuIndex] = useState(0);
  const [request, setRequest] = useState<{ id: string; assistant: Assistant; prompt: string; text: string }>();
  const textarea = useRef<HTMLTextAreaElement>(null);
  const menuId = useId();
  const mention = /^\s*@([\p{L}\p{N}_-]*)(?=\s|$)/u.exec(draft);
  const prefix = mention?.[1].toLowerCase() ?? "";
  const prompt = mention ? draft.slice(mention[0].length).trim() : draft.trim();
  const matches = aiChat.assistants.filter(entry => entry.name.toLowerCase() === prefix);
  const assistant = !quote && matches.length === 1 ? matches[0] : undefined;
  const ambiguous = !quote && matches.length > 1;
  const suggestions = aiChat.assistants.filter(entry => entry.name.toLowerCase().startsWith(prefix));
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
    setDraft(previous => ({ ...previous, text: "@" + entry.name + " " + prompt, nonce: undefined, quote: undefined }));
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
  useLayoutEffect(() => {
    const url = pastedImage ? URL.createObjectURL(pastedImage) : undefined;
    setImageUrl(url);
    return () => { if (url) URL.revokeObjectURL(url); };
  }, [pastedImage]);
  const oversized = new TextEncoder().encode(draft.trim()).length > 8192;

  async function send(event: FormEvent) {
    event.preventDefault();
    const text = draft.trim();
    if (oversized || sending || request || ambiguous || quoteUnavailable || (!text && !pastedImage)) return;
    if (pastedImage && !assistant) {
      setSending(true);
      setFailure("");
      try {
        await api.pasteImage(conversation, await pastedImage.arrayBuffer());
        setDraft(previous => previous.image === pastedImage ? { ...previous, image: undefined } : previous);
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
        setRequest({ id, assistant, prompt, text });
      } catch (error) { setFailure(errorMessage(error)); }
      finally { setSending(false); }
      return;
    }
    const nonce = saved.nonce ?? crypto.randomUUID();
    setDraft(previous => ({ ...previous, nonce }));
    setSending(true);
    setFailure("");
    try {
      await api.send(conversation, text, nonce, quote?.seq);
      setDraft(previous => previous.nonce === nonce ? { ...previous, text: "", quote: undefined, nonce: undefined } : previous);
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
      {ambiguous && <p className="inline-error" role="alert">Rename one of the matching assistants in Settings.</p>}
      {menu && <AssistantMenu id={menuId} assistants={suggestions} active={menuIndex}
        choose={choose} settings={aiChat.settings} />}
      <fieldset className={"composer-box floating-field" + (assistant ? " ai-composer" : "")}>
        <legend><label htmlFor={menuId + "-message"}>{assistant ? "Write a prompt…" : "Write a message…"}</label></legend>
        <textarea ref={textarea} id={menuId + "-message"} aria-label={assistant ? "AI prompt" : "Message"}
          aria-autocomplete="list" aria-controls={menu ? menuId : undefined}
          aria-activedescendant={menu && activeOption ? menuId + "-" + menuIndex : undefined}
          rows={2} value={draft} onChange={event => {
            const value = event.target.value;
            setDraft(previous => ({ ...previous, text: value, nonce: undefined }));
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
            setDraft(previous => ({ ...previous, image: file }));
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
        {saved.quote && <div className="message-quote composer-quote"><Reply size={15} />
          {quote ? <span><strong>{displayName(quote.sender, remarks)}</strong> {quote.text ?? quote.attachment?.name}</span> : <span>Message unavailable</span>}
          <button className="icon-button" type="button" aria-label="Remove quote" disabled={sending}
            onClick={() => setDraft(previous => ({ ...previous, quote: undefined, nonce: undefined }))}><X size={15} /></button>
        </div>}
        {imageUrl && <div className="pasted-image">
          <img src={imageUrl} alt="Pasted image ready to send" />
          <button className="icon-button" type="button" disabled={sending} aria-label="Remove pasted image"
            onClick={() => setDraft(previous => ({ ...previous, image: undefined }))}><X size={16} /></button>
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
            disabled={(!draft.trim() && !pastedImage) || ambiguous || quoteUnavailable || oversized || sending || (!!assistant && (!view || !prompt))}
            aria-label={assistant ? "Choose AI context" : sending ? "Sending message" : "Send message"}>
            {assistant ? <>Choose context <ArrowRight size={16} /></>
              : <><span>{sending ? "Sending…" : "Send"}</span><Send size={16} /></>}
          </button>
        </div>
      </fieldset>
    </form>
    {view && request && <AiRequest view={view} draft={request.id} assistant={request.assistant} prompt={request.prompt}
      messages={page?.items ?? []} hasMore={page?.has_more ?? false} loading={loading} older={older} conversation={conversation}
      close={closeRequest} answered={reply => {
        aiChat.answered(reply);
        setDraft(previous => previous.text.trim() === request.text ? { ...previous, text: "", nonce: undefined } : previous);
        closeRequest();
      }} />}
  </>;
}
