import {
  ArrowDown, ArrowLeft, ArrowRight, AtSign, Check, CheckCheck, Copy, Download, FilePlus2, FolderOpen, Ghost, LockKeyhole,
  Image, LogOut, MessageCircle, MoreHorizontal, Paperclip, Plus, QrCode, Reply, RotateCcw, Search, Send, Trash2, Undo2, UsersRound, X,
} from "lucide-react";
import { Fragment, useCallback, useContext, useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import type { FormEvent, KeyboardEvent } from "react";
import { ai } from "./ai-api";
import type { AiReply, Assistant } from "./ai-api";
import { AiRequest, AssistantMenu, ProviderLogo, ShareAiReply } from "./ai";
import { api, errorMessage, fileExpired } from "./api";
import type { Conversation, Draft, Message, MessageReceipt, Page, PrivateSummary, ShareSource, Transfer, User } from "./api";
import { MAX_IMAGE_BYTES } from "./attachment-media";
import { Avatar, Blank, Choice, Dialog, People, PersonAvatar, chatName, dateTimeLabel, displayName, messageTimeLabel, startsTimeGroup, usePage } from "./common";
import { copyText, useMenu } from "./context-menu";
import { FilesView, TransferList, useAttachmentSave } from "./files";
import { ChatAttachment, MediaHistory, MediaViewer } from "./chat-media";
import { privatePhaseLabel, privateTitle } from "./private-chat";
import { ios } from "./ios";

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
  const [drawer, setDrawer] = useState(false);
  const [search, setSearch] = useState<"text" | "media">();
  const [media, setMedia] = useState<Message>();
  const [leave, setLeave] = useState(false);
  const [leaving, setLeaving] = useState(false);
  const [leaveError, setLeaveError] = useState("");
  const { remarks, open } = useContext(People);
  const name = chatName(chat, own.id, remarks);
  const peer = chat.title ? undefined : chat.members.find(user => user.id !== own.id);

  function keepDrawerFocus(event: KeyboardEvent<HTMLDialogElement>) {
    if (event.key !== "Tab") return;
    const buttons = event.currentTarget.querySelectorAll<HTMLButtonElement>("button:not(:disabled)");
    const first = buttons[0];
    const last = buttons[buttons.length - 1];
    if (document.activeElement === (event.shiftKey ? first : last)) {
      event.preventDefault();
      (event.shiftKey ? last : first).focus();
    }
  }

  return <section className="chat-view" aria-label={name}>
    <header className="chat-header">
      <button className="icon-button mobile-back" onClick={back} aria-label="Back to chats"><ArrowLeft size={20} /></button>
      <div className="chat-heading"><h2>{peer ? <button className="person-link" onClick={() => open?.(peer)}>{name}</button> : name}</h2></div>
      {showFiles && <button className="button subtle small" onClick={() => setShowFiles(false)}><ArrowLeft size={15} /> Messages</button>}
      <button className="icon-button" aria-label="Open private chat" title="Open private chat"
        onClick={openPrivate}><Ghost size={20} /></button>
      <button className="icon-button" aria-label={chat.title ? "Group details" : "Conversation details"}
        title={chat.title ? "Group details" : "Conversation details"} aria-haspopup="dialog" aria-expanded={drawer}
        onClick={event => { event.currentTarget.focus(); setDrawer(true); }}><MoreHorizontal size={22} /></button>
    </header>
    {showFiles
      ? <FilesView conversation={chat.id} people={chat.members} transfers={transfers} revision={revision} recalled={recalled} hidden={hidden} onChanged={changed} />
      : <ChatMessages chat={chat} own={own} revision={revision} latestAiSeq={latestAiSeq} recalled={recalled} clockOffset={clockOffset} transfers={transfers}
        changed={changed} refresh={refresh} share={share} aiChat={aiChat} openMedia={setMedia} {...{ hidden, hide, readReceipts, draft, setDraft }} />}
    {drawer && <Dialog title={chat.title ? "Group details" : "Conversation details"} className="conversation-drawer"
      close={() => setDrawer(false)} onKeyDown={keepDrawerFocus}>
      {chat.title ? <div className="drawer-identity"><Avatar name={name} group large /><strong>{name}</strong></div>
        : peer && <button className="drawer-identity drawer-action" onClick={() => { setDrawer(false); open?.(peer); }}>
          <PersonAvatar user={peer} large />
          <span><strong>{name}</strong><small>View profile</small></span>
          <ArrowRight size={17} />
        </button>}
      {chat.title && <section className="drawer-members">
        <h3>Members <small>{chat.members.length}</small></h3>
        <ul>{chat.members.map(member => <li key={member.id}>
          <button className="drawer-action" onClick={() => { setDrawer(false); open?.(member); }}>
            <PersonAvatar user={member} /><span>{displayName(member, remarks)}</span>
          </button>
        </li>)}</ul>
        <button className="drawer-action" onClick={() => { setDrawer(false); groupCode(); }}><QrCode size={19} /> Group invite QR code</button>
      </section>}
      <div className="drawer-actions">
        <button className="drawer-action" onClick={() => { setDrawer(false); setSearch("text"); }}><Search size={19} /> Search chat</button>
        <button className="drawer-action" onClick={() => { setDrawer(false); setShowFiles(true); }} disabled={showFiles}><FolderOpen size={19} /> Shared files</button>
        <button className="drawer-action" onClick={() => { setDrawer(false); setSearch("media"); }}><Image size={19} /> Photos and videos</button>
        {chat.title && <button className="drawer-action danger" onClick={() => {
          setDrawer(false); setLeaveError(""); setLeave(true);
        }}><LogOut size={19} /> Leave group</button>}
      </div>
    </Dialog>}
    {leave && <Dialog title="Leave group" close={() => setLeave(false)} closeDisabled={leaving}>
      <p>You will stop receiving messages from {name}.</p>
      {leaveError && <p className="inline-error" role="alert">{leaveError}</p>}
      <div className="dialog-actions">
        <button className="button subtle" disabled={leaving} onClick={() => setLeave(false)}>Cancel</button>
        <button className="button danger" disabled={leaving} onClick={async () => {
          setLeaving(true);
          try { await api.leaveGroup(chat.id); await refresh(); setLeave(false); back(); }
          catch (error) { setLeaveError(errorMessage(error)); }
          finally { setLeaving(false); }
        }}>Leave group</button>
      </div>
    </Dialog>}
    {search && <ChatSearch conversation={chat.id} initialTab={search} {...{ revision, latestAiSeq, recalled, hidden }}
      openMedia={setMedia} close={() => setSearch(undefined)} />}
    {media && <MediaViewer message={media} conversation={chat.id} {...{ revision, recalled, hidden, transfers }}
      open={setMedia} close={() => setMedia(undefined)} onChanged={changed} />}
  </section>;
}

function ChatSearch({ conversation, initialTab, revision, latestAiSeq, recalled, hidden, openMedia, close }: {
  conversation: string; revision: number; latestAiSeq: number; recalled: ReadonlySet<number>; hidden: ReadonlySet<number>; close: () => void;
  openMedia: (message: Message) => void; initialTab: "text" | "media";
}) {
  const [query, setQuery] = useState("");
  const [search, setSearch] = useState("");
  const [tab, setTab] = useState(initialTab);
  return <Dialog title="Search chat" close={close}>
    <fieldset className="tabs">
      <legend className="sr-only">Search chat by</legend>
      <Choice type="radio" name="chat-search" checked={tab === "text"} onChange={() => setTab("text")}>Messages</Choice>
      <Choice type="radio" name="chat-search" checked={tab === "media"} onChange={() => setTab("media")}>Photos and videos</Choice>
    </fieldset>
    {tab === "media" ? <MediaHistory {...{ conversation, revision, recalled, hidden }} open={openMedia} /> : <>
      <form className="chat-search" onSubmit={event => { event.preventDefault(); setSearch(query.trim()); }}>
        <label className="search"><Search size={16} /><input aria-label="Search message text" value={query}
          onChange={event => setQuery(event.target.value)} placeholder="Search message text" autoFocus /></label>
        <button className="button primary" type="submit" disabled={!query.trim()}>Search</button>
      </form>
      {search && <SearchResults key={search} {...{ conversation, search, revision, latestAiSeq, recalled, hidden }} />}
    </>}
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
  hidden, hide, readReceipts, draft, setDraft, openMedia }: {
  chat: Conversation; own: User; revision: number; latestAiSeq: number; recalled: ReadonlySet<number>; clockOffset: number; transfers: Transfer[]; changed: () => void;
  refresh: () => Promise<void>; share: (source: ShareSource) => Promise<void>; aiChat: ChatAssistants;
  hidden: ReadonlySet<number>; hide: (seq: number) => Promise<void>; readReceipts: boolean;
  openMedia: (message: Message) => void;
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
  const [receipt, setReceipt] = useState<MessageReceipt | null>(null);
  const viewedBody = useRef<HTMLDivElement>(null);
  const openMenu = useMenu();
  const { save, saving, failure: saveFailure } = useAttachmentSave(changed);
  const press = useRef<{ anchor: HTMLElement; x: number; y: number; timer: number; opened: boolean; released?: number }>(undefined);
  const lastSeq = page?.items[0]?.seq;
  const pending = transfers.filter(job => job.direction === "upload" && job.conversation_id === chat.id && job.status !== "complete");
  const downloads = new Map(transfers.filter(job => job.direction === "download").map(job => [job.attachment.id, job]));
  const history = [
    ...(page?.items.map(message => ({ message, reply: undefined, time: message.created_at, key: String(message.seq) })) ?? []),
    ...aiChat.replies.map(reply => ({ message: undefined, reply, time: reply.createdAt, key: reply.id })),
  ].sort((a, b) => a.time - b.time || (a.message?.seq ?? Infinity) - (b.message?.seq ?? Infinity));
  const bySequence = new Map(page?.items.map(message => [message.seq, message]));
  const lastMessage = history.at(-1)?.message;
  const receiptMessage = readReceipts && !chat.title && lastMessage
    && lastMessage.seq > 0 && lastMessage.seq === chat.preview_seq && !lastMessage.recalled
    && !lastMessage.expired && !lastMessage.ai_provider ? lastMessage : undefined;
  const sent = receiptMessage?.sender.id === own.id ? receiptMessage.seq : undefined;
  const viewed = !away && receiptMessage?.sender.id !== own.id ? receiptMessage?.seq : undefined;
  const receiptLabel = receipt && receipt.seq === sent ? (receipt.status === "read" ? "Read" : "Delivered") : undefined;
  const readThrough = !away && loadedRevision === revision && chat.latest_seq > chat.read_through
    ? chat.latest_seq : undefined;
  let previousTime: number | undefined;

  useEffect(() => () => window.clearTimeout(press.current?.timer), []);

  useEffect(() => {
    if (!sent) setReceipt(null);
    if (!readThrough && !sent && !viewed) return;
    let disposed = false;
    let timer: number | undefined;
    let marking = readThrough;
    async function update() {
      try {
        const element = viewport.current;
        const active = element?.getClientRects().length && document.visibilityState === "visible" && document.hasFocus()
          && !document.querySelector("dialog[open]");
        const through = active ? marking : undefined;
        let shown: number | undefined;
        if (active && viewed && element) {
          const view = element.getBoundingClientRect();
          const content = viewedBody.current?.querySelector(".bubble, .chat-media, .chat-file-card")?.getBoundingClientRect();
          if (content && Math.min(content.bottom, view.bottom, window.innerHeight) > Math.max(content.top, view.top, 0)) {
            shown = viewed;
          }
        }
        if (!through && !sent && !shown) return;
        const result = await api.readUpdate(chat.id, through, sent, shown);
        if (disposed) return;
        setReceipt(result);
        if (through) {
          marking = undefined;
          void refresh();
        }
      } catch { if (!disposed) setReceipt(null); }
      finally {
        if (!disposed && (marking || viewed || sent)) timer = window.setTimeout(update, 1700);
      }
    }
    void update();
    return () => { disposed = true; window.clearTimeout(timer); };
  }, [chat.id, readThrough, sent, viewed, refresh]);

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

  async function retry(nonce: string) {
    setRecallError("");
    try { await api.retryMessage(nonce); } catch (error) { setRecallError(errorMessage(error)); }
    finally { void refresh(); }
  }

  function checkScroll() {
    const element = viewport.current;
    if (!element) return;
    nearBottom.current = element.scrollHeight - element.scrollTop - element.clientHeight < 100;
    setAway(!nearBottom.current);
  }

  function openedByPress(anchor: HTMLElement) {
    const active = press.current;
    return active?.anchor === anchor && active.opened
      && (active.released === undefined || Date.now() - active.released < 500);
  }

  return <><div className="timeline-wrap">
    <div className="timeline" ref={viewport} onScroll={checkScroll}>
      {failure && <p className="inline-error" role="alert">{failure}</p>}
      {recallError && <p className="inline-error" role="alert">{recallError}</p>}
      {saveFailure && <p className="inline-error" role="alert">{saveFailure}</p>}
      {!page && !failure && <p className="muted loading" role="status">Loading messages…</p>}
      {page?.has_more && <button className="button subtle more" disabled={loading} onClick={() => void older()}>Load older messages</button>}
      {page && history.length === 0 && pending.length === 0 && <Blank icon={<MessageCircle size={27} />} title="Start the conversation">
        <p>Say hello or share a file.</p>
      </Blank>}
      <div className="messages" role="log" aria-label="Messages">
        {history.map(({ message, reply, key, time }) => {
          const provider = message?.ai_provider ?? reply?.provider;
          const mine = message?.sender.id === own.id;
          const newGroup = startsTimeGroup(time, previousTime);
          previousTime = time;
          const animated = !!reply || (!!message && newestAtOpen !== undefined && message.seq > newestAtOpen);
          const now = Date.now() + clockOffset;
          const canRecall = !!message && message.seq > 0 && mine && !provider && !message.recalled && now - time <= 5 * 60_000;
          const avatar = provider ? <ProviderLogo provider={provider} /> : <button className="person-link message-avatar"
            aria-label={`View ${displayName(message!.sender, remarks)}'s profile`} onClick={() => open?.(message!.sender)}>
            <PersonAvatar user={message!.sender} />
          </button>;
          const text = message?.text ?? reply?.text;
          const file = message?.attachment;
          const download = file ? downloads.get(file.id) : undefined;
          const expiredFile = !!message && (fileExpired(message, now) || download?.status === "expired");
          const touchMenu = !!file || ios;
          function actions(selection?: string) {
            const copy = selection || text;
            return [
              ...(file && !expiredFile && (!download || download.status === "complete") ? [{ label: "Save attachment", icon: <Download size={15} />,
                disabled: saving, action: () => void save(file.id) }] : []),
              ...(copy ? [{ label: "Copy", icon: <Copy size={15} />, action: () => {
                void copyText(copy).catch(error => setRecallError(errorMessage(error)));
              } }] : []),
              ...(message && message.seq > 0 && !message.recalled && !message.expired && !expiredFile ? [{ label: "Quote", icon: <Reply size={15} />, action: () => {
                setDraft(previous => ({ ...previous, quote: message, nonce: undefined }));
                viewport.current?.closest(".chat-view")?.querySelector<HTMLTextAreaElement>(".composer textarea")?.focus();
              } }] : []),
              ...(canRecall ? [{ label: "Recall message", icon: <Undo2 size={15} />, disabled: recalling !== undefined,
                action: () => void recall(message.seq) }] : []),
              ...(message && message.seq > 0 ? [{ label: "Delete for me", icon: <Trash2 size={15} />, action: () => void deleteForMe(message.seq) }] : []),
            ];
          }
          return <Fragment key={key}>
            {newGroup && <div className="day-divider"><time dateTime={new Date(time).toISOString()}>{messageTimeLabel(time)}</time></div>}
            <div className={"message-row" + (mine ? " mine" : "") + (animated ? " message-arrival" : "")}
              onContextMenu={event => {
                window.clearTimeout(press.current?.timer);
                const selection = window.getSelection()?.toString();
                event.preventDefault();
                if (!openedByPress(event.currentTarget)) openMenu(event.currentTarget, actions(selection),
                  event.clientX || event.clientY ? { x: event.clientX, y: event.clientY } : undefined);
              }}
              onPointerDown={touchMenu ? event => {
                window.clearTimeout(press.current?.timer);
                press.current = undefined;
                if (event.pointerType === "mouse" || !event.isPrimary
                  || !(event.target instanceof Element && event.target.closest(
                    ios ? ".bubble, .media-tile, .file-main" : ".media-tile, .file-main"
                  ))) return;
                const anchor = event.currentTarget;
                const point = { x: event.clientX, y: event.clientY };
                const timer = window.setTimeout(() => {
                  if (!press.current || !anchor.isConnected) return;
                  press.current.opened = true;
                  openMenu(anchor, actions(), point);
                }, 550);
                press.current = { ...point, anchor, timer, opened: false };
              } : undefined}
              onPointerMove={touchMenu ? event => {
                const active = press.current;
                if (active && Math.hypot(event.clientX - active.x, event.clientY - active.y) > 10) window.clearTimeout(active.timer);
              } : undefined}
              onPointerUp={touchMenu ? () => {
                if (press.current) {
                  window.clearTimeout(press.current.timer);
                  press.current.released = Date.now();
                }
              } : undefined}
              onPointerLeave={touchMenu ? () => window.clearTimeout(press.current?.timer) : undefined}
              onPointerCancel={touchMenu ? () => {
                window.clearTimeout(press.current?.timer);
                press.current = undefined;
              } : undefined}
              onClickCapture={touchMenu ? event => {
                if (openedByPress(event.currentTarget) && event.detail > 0) {
                  event.preventDefault();
                  event.stopPropagation();
                  press.current = undefined;
                }
              } : undefined}>
              {!mine && avatar}
              <div className={"message-body" + (file ? " attachment-body" : "")}
                ref={viewed !== undefined && message?.seq === viewed ? viewedBody : undefined}>
                {!provider && !mine && <span className="message-author">{displayName(message!.sender, remarks)}</span>}
                {message?.reply_to !== undefined && !message.recalled && <QuotedMessage conversation={chat.id} seq={message.reply_to}
                  known={bySequence.get(message.reply_to)} hidden={hidden.has(message.reply_to) || recalled.has(message.reply_to)} />}
                {message?.recalled ? <div className="bubble recalled-message">Message recalled</div>
                  : message && file ? <ChatAttachment key={file.id} message={message} transfer={download} expired={expiredFile}
                    onChanged={changed} open={() => openMedia(message)} />
                    : message?.expired ? <div className="bubble recalled-message">
                      {message.expired === "file" ? "This file expired. Ask the sender to share it again." : "This message expired before delivery."}
                    </div>
                    : <div className="bubble">{message?.text ?? reply?.text}</div>}
                <div className="message-meta">
                  {message?.pending && <>
                    <small role="status">{message.pending.error ?? "Queued"}</small>
                    {message.pending.blocked && message.nonce && <button className="text-button" type="button"
                      onClick={() => void retry(message.nonce!)}><RotateCcw size={13} /> Retry</button>}
                  </>}
                  {message === lastMessage && mine && !message?.recalled && !message?.pending && !file
                    && message?.expires_at !== undefined && message.expires_at <= now && !receiptLabel
                    && <small>Online delivery window ended</small>}
                  {message && message.seq === sent && receiptLabel && <span className="read-mark" role="img" aria-label={receiptLabel} title={receiptLabel}>
                    {receiptLabel === "Read" ? <CheckCheck size={15} aria-hidden /> : <Check size={15} aria-hidden />}
                  </span>}
                  {reply && <>
                    <small><LockKeyhole size={12} /> Only you</small>
                    <button type="button" className="text-button" onClick={() => setSharingReply(reply)}><UsersRound size={13} /> Visible to</button>
                    <button className="icon-button" aria-label="Dismiss AI reply" onClick={() => aiChat.dismiss(reply.id)}><X size={14} /></button>
                  </>}
                  {actions().length > 0 && <button className="icon-button message-menu" aria-label="Message actions" aria-haspopup="menu"
                    onClick={event => openMenu(event.currentTarget, actions())}><MoreHorizontal size={16} /></button>}
                </div>
              </div>
              {mine && avatar}
            </div>
          </Fragment>;
        })}
      </div>
    </div>
    {away && <button className="button subtle jump-bottom" onClick={() => {
      if (viewport.current) viewport.current.scrollTop = viewport.current.scrollHeight;
    }}><ArrowDown size={15} /> Latest messages</button>}
  </div>
    {pending.length > 0 && <section className="composer-transfers" aria-label="Pending uploads">
      <TransferList transfers={pending} onChanged={changed} inChat />
    </section>}
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
  return <div className="message-quote"><Reply size={14} />{quote && !hidden && !quote.recalled && !quote.expired
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
  const [checkingImage, setCheckingImage] = useState(false);
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
    if (oversized || sending || checkingImage || request || ambiguous || quoteUnavailable || (!text && !pastedImage)) return;
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
      void refresh();
    } catch (error) { setFailure(errorMessage(error)); }
    finally { setSending(false); }
  }

  async function pick(kind: "file" | "folder") {
    setPreparing(true);
    try { await share({ kind }); }
    finally { setPreparing(false); }
  }

  async function preparePastedImage(file: File) {
    setFailure("");
    if (file.size > MAX_IMAGE_BYTES) {
      setFailure("Pasted images can be up to 16 MiB.");
      return;
    }
    setCheckingImage(true);
    try {
      const type = await api.checkPastedImage(await file.arrayBuffer());
      setDraft(previous => ({ ...previous, image: new File([file], file.name, { type }) }));
    } catch (error) { setFailure(errorMessage(error)); }
    finally { setCheckingImage(false); }
  }

  return <>
    <form className="composer" onSubmit={send} onBlur={event => {
      if (!event.currentTarget.contains(event.relatedTarget)) setMenu(false);
    }}>
      {(failure || oversized) && <div className="inline-error" role="alert">{oversized ? "Messages can be up to 8 KiB." : failure}</div>}
      {ambiguous && <p className="inline-error" role="alert">Rename one of the matching assistants in Settings.</p>}
      {menu && <AssistantMenu id={menuId} assistants={suggestions} active={menuIndex}
        choose={choose} settings={aiChat.settings} />}
      <div className={"composer-box" + (draft.length > 0 ? " has-text" : "") + (assistant ? " ai-composer" : "")}>
        <label className="composer-label" htmlFor={menuId + "-message"}>{assistant ? "Write a prompt" : "Write a message"}</label>
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
            if (!sending && !checkingImage) void preparePastedImage(file);
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
            if (!ios && event.key === "Enter" && !event.shiftKey) {
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
          <button className="icon-button" type="button" disabled={sending || checkingImage} aria-label="Remove pasted image"
            onClick={() => setDraft(previous => ({ ...previous, image: undefined }))}><X size={16} /></button>
        </div>}
        <div className="composer-bar">
          <div className="composer-attachments">
            {assistant ? <span className="ai-composer-target"><ProviderLogo provider={assistant.provider} /> @{assistant.name}</span> : <>
              <button className="icon-button" type="button" aria-label="Attach a file" title="Attach a file"
                onClick={() => void pick("file")} disabled={preparing}><Paperclip size={20} /></button>
              {!ios && <button className="icon-button" type="button" aria-label="Attach a folder" title="Attach a folder"
                onClick={() => void pick("folder")} disabled={preparing}><FilePlus2 size={20} /></button>}
            </>}
            <button className="icon-button ai-mention-button" type="button" aria-label="Mention an AI assistant"
              aria-controls={menu ? menuId : undefined} aria-expanded={menu} title="Mention an AI assistant" onClick={() => {
                setMenu(!menu);
                setMenuIndex(0);
                textarea.current?.focus();
              }}><AtSign size={19} /></button>
            {preparing && <span className="drop-hint" role="status">Preparing attachment…</span>}
            {checkingImage && <span className="drop-hint" role="status">Checking image…</span>}
          </div>
          <button className="button primary send-button" type="submit"
            disabled={(!draft.trim() && !pastedImage) || ambiguous || quoteUnavailable || oversized || sending || checkingImage || (!!assistant && (!view || !prompt))}
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
        setDraft(previous => previous.text.trim() === request.text ? { ...previous, text: "", nonce: undefined } : previous);
        closeRequest();
      }} />}
  </>;
}
