import { FloatingField } from "./floating-field";
import {
  ArrowDown, ArrowLeft, ArrowRight, Clock, Check, CheckCheck, ChevronRight, Copy, Download, FolderOpen, Ghost, LockKeyhole,
  Image, LoaderCircle, LogOut, MessageCircle, MoreHorizontal, Paperclip, Plus, QrCode, Reply, RotateCcw, ScanLine, Search, Trash2, Undo2, UserRoundPlus, UsersRound, X,
} from "lucide-react";
import { Fragment, useCallback, useContext, useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import type { FormEvent, KeyboardEvent, ReactNode } from "react";
import { ai } from "./ai-api";
import type { AiReply, Assistant } from "./ai-api";
import { AiRequest, AssistantMenu, ProviderLogo, ShareAiReply } from "./ai";
import { api, errorMessage, fileExpired } from "./api";
import type { Conversation, Draft, Message, MessageReceipt, Page, PrivateSummary, ShareSource, Transfer, User } from "./api";
import { canPreview, mediaKind, MAX_IMAGE_BYTES } from "./attachment-media";
import { Avatar, Blank, Choice, Dialog, People, PersonAvatar, chatName, dateTimeLabel, displayName, messageTimeLabel, startsTimeGroup, usePage } from "./common";
import { copyText, useEditMenu, useMenu, useMenuAnchor, useRowMenu } from "./context-menu";
import { FilesView, TransferList, useAttachmentSave } from "./files";
import { ChatAttachment, MediaHistory, MediaViewer } from "./chat-media";
import { privatePhaseLabel, privateTitle } from "./private-chat";
import { phone, ios, useIosBack } from "./ios";
import { actions as actionLabels, t } from "./locale";
import { ShareQr } from "./qr";
import type { SharedCode } from "./qr";
import { windows } from "./window-chrome";
import { WorkspaceSearchField } from "./workspace-search";
import type { ChatHistoryCache } from "./chat-history-cache";

type ChatPage = "details" | "search";

export function ChatList({ chats, own, selected, open, newChat, addContact, rooms, activePrivate, openPrivate, createQr, scan }: {
  chats: Conversation[]; own: User; selected?: string; open: (id: string, page?: ChatPage) => void; newChat: () => void;
  addContact: () => void;
  rooms: PrivateSummary[]; activePrivate?: string; openPrivate: (id: string) => void; createQr: () => void;
  scan: () => void;
}) {
  const { remarks, open: showPerson } = useContext(People);
  const menu = useRowMenu();
  const openMenu = useMenu();
  const anchor = useMenuAnchor();
  const plus = useRef<HTMLButtonElement>(null);
  const [search, setSearch] = useState("");
  const query = search.trim().toLocaleLowerCase();
  const filtered = chats.filter(chat => chatName(chat, own.id, remarks).toLocaleLowerCase().includes(query));
  const paired = rooms.find(room => room.id === activePrivate)?.conversation;
  const standalone = rooms.filter(room => !chats.some(chat => chat.id === room.conversation));
  const add = <button className="icon-button new-conversation" type="button" ref={plus}
    aria-label={t("Add", "添加")} title={t("Add", "添加")} aria-haspopup="menu" aria-expanded={!!anchor && anchor === plus.current}
    onClick={event => openMenu(event.currentTarget, [
      { label: t("Start group chat", "发起群聊"), icon: <UsersRound size={18} />, action: newChat },
      ...(phone ? [{ label: t("Scan QR code", "扫一扫"), icon: <ScanLine size={18} />, action: scan }] : []),
      { label: t("Add friend", "添加朋友"), icon: <UserRoundPlus size={18} />, action: addContact },
      { label: t("Private chat QR code", "私聊二维码"), icon: <QrCode size={18} />, action: createQr },
    ])}><Plus size={22} /></button>;
  return <aside className="chat-list">
    {ios && <header className="chat-list-title"><h1>{t("Chats", "聊天")}</h1>{add}</header>}
    <form className="chat-list-head" onSubmit={event => {
      event.preventDefault();
      if (query && filtered.length === 1) open(filtered[0].id);
    }}>
      <WorkspaceSearchField label={t("Search chats", "搜索聊天")} otherIconSize={18}
        value={search} onChange={event => setSearch(event.target.value)} />
      {!ios && add}
    </form>
    <div className="chat-rows">
      {filtered.map(chat => {
        const name = chatName(chat, own.id, remarks);
        const peer = chat.members.find(user => user.id !== own.id);
        return <div className="conversation-entry" key={chat.id}><button type="button" className={"chat-row conversation-row" + (chat.id === selected ? " selected" : "")} {...menu([
          { label: t("Chat info", "聊天信息"), icon: <MoreHorizontal size={18} />, action: () => open(chat.id, "details") },
          { label: t("Search chat", "查找聊天内容"), icon: <Search size={18} />, action: () => open(chat.id, "search") },
        ])}
          aria-current={chat.id === selected ? "true" : undefined} onClick={() => open(chat.id)}>
          {chat.title || !peer ? <Avatar name={name} group={!!chat.title} /> : <PersonAvatar user={peer} />}
          <span className="chat-row-text"><strong title={name}>{name}</strong><small>{paired === chat.id
            ? t("Private chat", "私密聊天") : chat.preview || t("New conversation", "新会话")}</small></span>
          <span className="chat-row-status"><time dateTime={new Date(chat.activity_at).toISOString()} title={dateTimeLabel(chat.activity_at)}>{messageTimeLabel(chat.activity_at)}</time>
            {chat.unread > 0 && <span className="unread-count">
              <span aria-hidden="true">{chat.unread > 99 ? "99+" : chat.unread}</span>
              <span className="sr-only">{t(`${chat.unread} unread messages`, `${chat.unread} 条未读消息`)}</span>
            </span>}
          </span>
        </button>
          {windows && !chat.title && peer && <button type="button" className="conversation-avatar"
            aria-label={t("View profile", "查看资料") + " " + name} onClick={() => showPerson?.(peer)}>
            <PersonAvatar user={peer} /></button>}
        </div>;
      })}
      {filtered.length === 0 && <p className="list-empty muted">{search ? "No matching chats." : "Start a chat with a contact."}</p>}
      {standalone.length > 0 && <div className="private-rows" aria-label="Private chats">
        <h2>Private chats</h2>
        {standalone.map(room => <button key={room.id} className={"chat-row" + (room.id === activePrivate ? " selected" : "")}
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

function DrawerAction({ icon, children, onClick, className = "", next = true, busy }: {
  icon: ReactNode; children: ReactNode; onClick: (button: HTMLButtonElement) => void; className?: string; next?: boolean; busy?: boolean;
}) {
  return <button type="button" className={`drawer-action ${className}`} aria-disabled={busy || undefined}
    onClick={event => { if (!busy) onClick(event.currentTarget); }}>
    {icon}<span className="drawer-label">{children}</span>
    {next && <ChevronRight className="drawer-chevron" size={16} aria-hidden="true" />}
  </button>;
}

export function ChatView({ chat, own, server, revision, latestAiSeq, recalled, clockOffset, transfers, changed, refresh, back, share, pasteFiles, openPrivate, aiChat,
  hidden, hide, readReceipts, draft, setDraft, profileOpen, visibleOnNarrow, requested, historyCache, privateRoom }: {
  chat: Conversation; own: User; server: string; revision: number; latestAiSeq: number; recalled: ReadonlySet<number>; clockOffset: number; transfers: Transfer[]; changed: () => void;
  refresh: () => Promise<void>; back: () => void; share: (source: ShareSource) => Promise<void>; pasteFiles: () => Promise<boolean>; openPrivate: () => void;
  aiChat: ChatAssistants; hidden: ReadonlySet<number>; hide: (seq: number) => Promise<void>; readReceipts: boolean;
  profileOpen: boolean; visibleOnNarrow: boolean;
  requested?: { page: ChatPage };
  historyCache: ChatHistoryCache;
  privateRoom?: PrivateSummary;
} & ChatDraft) {
  const [showFiles, setShowFiles] = useState(false);
  const [drawer, setDrawer] = useState(false);
  const [search, setSearch] = useState<"text" | "media">();
  const [media, setMedia] = useState<Message>();
  const [mediaError, setMediaError] = useState("");
  const [leave, setLeave] = useState(false);
  const [leaving, setLeaving] = useState(false);
  const [leaveError, setLeaveError] = useState("");
  const [invite, setInvite] = useState<SharedCode>();
  const [invitePending, setInvitePending] = useState(false);
  const [inviteError, setInviteError] = useState("");
  const wantInvite = useRef(false);
  const { remarks, open } = useContext(People);
  const name = chatName(chat, own.id, remarks);
  const count = new Intl.NumberFormat(navigator.language).format(chat.members.length);
  const memberCount = t(` (${count})`, `（${count}）`);
  const heading = chat.title ? name + memberCount : name;
  const details = chat.title ? t("Group details", "群聊信息") : t("Conversation details", "聊天信息");
  const privateAction = privateRoom?.phase === "invited" ? "Review private invitation"
    : privateRoom?.unread ? "Open private chat, unread messages" : "Open private chat";
  const peer = chat.title ? undefined : chat.members.find(user => user.id !== own.id);
  const groupMember = !!chat.title && chat.members.some(member => member.id === own.id);
  const chatView = useRef<HTMLElement>(null);
  const detailsButton = useRef<HTMLButtonElement>(null);
  const filesBack = useRef<HTMLButtonElement>(null);
  const drawerReturn = useRef<HTMLButtonElement>(null);
  const detailsVisible = drawer && !profileOpen && !search && !showFiles && !leave;
  function openMedia(message: Message) {
    setMediaError("");
    const file = message.attachment;
    if (windows && file && mediaKind(file) === "image" && canPreview(file)) {
      setMedia(undefined);
      setSearch(undefined);
      void api.openImage(file.id).catch(error => setMediaError(errorMessage(error)));
    } else {
      setMedia(message);
    }
  }
  useIosBack(showFiles && !profileOpen ? () => setShowFiles(false) : undefined, 5);

  useEffect(() => {
    if (ios || requested || profileOpen || !visibleOnNarrow || chatView.current?.querySelector(".conversation-drawer[open]")) return;
    const field = chatView.current?.querySelector<HTMLTextAreaElement>(".composer textarea");
    if (field?.getClientRects().length) field.focus({ preventScroll: true });
  }, [requested, profileOpen, visibleOnNarrow]);

  useEffect(() => {
    if (requested?.page === "details") setDrawer(true);
    if (requested?.page === "search") setSearch("text");
  }, [requested]);

  useLayoutEffect(() => {
    if (detailsVisible) drawerReturn.current?.focus({ preventScroll: true });
    else wantInvite.current = false;
  }, [detailsVisible]);
  useLayoutEffect(() => {
    if (windows && showFiles) filesBack.current?.focus();
  }, [showFiles]);

  useEffect(() => {
    if (!windows || !detailsVisible) return;
    function dismissOutside(event: PointerEvent) {
      if (document.querySelector("dialog:modal") || !(event.target instanceof Element)) return;
      if (detailsButton.current?.contains(event.target) || event.target.closest(".conversation-drawer")) return;
      drawerReturn.current = null;
      setDrawer(false);
    }
    document.addEventListener("pointerdown", dismissOutside, true);
    return () => document.removeEventListener("pointerdown", dismissOutside, true);
  }, [detailsVisible]);

  function closeDrawer() {
    drawerReturn.current = null;
    setDrawer(false);
    if (windows) detailsButton.current?.focus({ preventScroll: true });
  }

  function fromDrawer(button: HTMLButtonElement, action: () => void) {
    drawerReturn.current = button;
    action();
  }

  async function showGroupInvite() {
    wantInvite.current = true;
    setInvitePending(true);
    setInviteError("");
    try {
      const code = await api.groupInvite(chat.id);
      if (wantInvite.current) setInvite({ kind: "group", name, value: code.code, expiresAt: code.expires_at });
    } catch (error) {
      if (wantInvite.current) setInviteError(errorMessage(error));
    } finally {
      setInvitePending(false);
    }
  }

  function keepDrawerFocus(event: KeyboardEvent<HTMLDialogElement>) {
    if (windows && event.key === "Escape") {
      event.preventDefault();
      closeDrawer();
      return;
    }
    if (event.key !== "Tab") return;
    const buttons = event.currentTarget.querySelectorAll<HTMLButtonElement>("button:not(:disabled)");
    const first = buttons[0];
    const last = buttons[buttons.length - 1];
    if (document.activeElement === (event.shiftKey ? first : last)) {
      event.preventDefault();
      (event.shiftKey ? last : first).focus();
    }
  }

  return <section className="chat-view" ref={chatView} aria-label={heading} onKeyDown={event => {
    if (windows && showFiles && event.key === "Escape" && !event.defaultPrevented && !document.querySelector("dialog:modal")) {
      event.preventDefault();
      setShowFiles(false);
    }
  }}>
    <header className="chat-header">
      <button className="icon-button mobile-back" onClick={() => { closeDrawer(); back(); }} aria-label="Back to chats"><ArrowLeft size={20} /></button>
      <div className="chat-heading"><h2>{peer && open
        ? <button className="person-link" type="button" title={name} onClick={() => open(peer)}>{name}</button>
        : <><span className="chat-title" title={name}>{name}</span>{chat.title && <span className="chat-member-count">{memberCount}</span>}</>}</h2></div>
      {showFiles && <button className="button subtle small" ref={filesBack} onClick={() => setShowFiles(false)}>
        <ArrowLeft size={16} /> {t("Messages", "消息")}
      </button>}
      <button className="icon-button" aria-label={privateAction} title={privateAction} onClick={openPrivate}>
        <span className="rail-icon"><Ghost size={23} />
          {(privateRoom?.unread || privateRoom?.phase === "invited") && <span className="rail-dot" aria-hidden="true" />}
        </span>
      </button>
      <button className="icon-button" ref={detailsButton} aria-label={details}
        title={details} aria-haspopup="dialog" aria-expanded={detailsVisible}
        onClick={event => {
          if (detailsVisible) closeDrawer();
          else { event.currentTarget.focus(); setShowFiles(false); setDrawer(true); }
        }}><MoreHorizontal size={25} /></button>
    </header>
    {mediaError && <p className="inline-error" role="alert">{mediaError}</p>}
    <div className="chat-content">
      <div className="chat-messages" hidden={showFiles}>
        <ChatMessages chat={chat} own={own} revision={revision} latestAiSeq={latestAiSeq} recalled={recalled} clockOffset={clockOffset} transfers={transfers}
          visible={!showFiles && !profileOpen} changed={changed} refresh={refresh} share={share} pasteFiles={pasteFiles} aiChat={aiChat} openMedia={openMedia}
          {...{ hidden, hide, readReceipts, draft, setDraft, historyCache }} />
      </div>
      {showFiles && <FilesView conversation={chat.id} transfers={transfers} revision={revision} recalled={recalled} hidden={hidden} onChanged={changed} />}
      {(windows || detailsVisible) && <Dialog mobilePage title={details} className="conversation-drawer"
        modal={!windows} visible={detailsVisible} close={closeDrawer} onKeyDown={keepDrawerFocus}>
        {chat.title ? <div className="drawer-identity"><Avatar name={name} group large /><strong>{name}</strong></div>
          : peer && open && <DrawerAction className="drawer-identity" icon={<PersonAvatar user={peer} large />}
            onClick={button => fromDrawer(button, () => open(peer))}>
            <strong>{name}</strong><small>{t("View profile", "查看资料")}</small>
          </DrawerAction>}
        {chat.title && <section className="drawer-members">
          <h3>{t("Members", "群成员")} <small>{count}</small></h3>
          <ul>{chat.members.map(member => <li key={member.id}>
            {open ? <DrawerAction icon={<PersonAvatar user={member} />}
              onClick={button => fromDrawer(button, () => open(member))}>{displayName(member, remarks)}</DrawerAction>
              : <><PersonAvatar user={member} /> {displayName(member, remarks)}</>}
          </li>)}</ul>
          {groupMember && <DrawerAction icon={<QrCode size={20} />} busy={invitePending} onClick={() => void showGroupInvite()}>
            {invitePending ? t("Generating QR code…", "正在生成二维码…") : t("Group invite QR code", "群邀请二维码")}
          </DrawerAction>}
          {inviteError && <p className="inline-error" role="alert">{inviteError}</p>}
        </section>}
        <div className="drawer-actions">
          <DrawerAction icon={<Search size={20} />} onClick={button => fromDrawer(button, () => setSearch("text"))}>
            {t("Search chat", "查找聊天内容")}
          </DrawerAction>
          <DrawerAction icon={<FolderOpen size={20} />} onClick={button => fromDrawer(button, () => setShowFiles(true))}>
            {t("Shared files", "共享文件")}
          </DrawerAction>
          <DrawerAction icon={<Image size={20} />} onClick={button => fromDrawer(button, () => setSearch("media"))}>
            {t("Photos and videos", "图片与视频")}
          </DrawerAction>
        </div>
        {groupMember && <div className="drawer-actions">
          <DrawerAction className="danger" icon={<LogOut size={20} />} next={false} onClick={button => fromDrawer(button, () => {
            setLeaveError(""); setLeave(true);
          })}>{t("Leave group", "退出群聊")}</DrawerAction>
        </div>}
      </Dialog>}
    </div>
    {invite && <ShareQr code={invite} server={server} close={() => setInvite(undefined)} />}
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
    {search && <ChatSearch conversation={chat.id} initialTab={search} {...{ revision, latestAiSeq, recalled, hidden, transfers, changed }}
      openMedia={openMedia} close={() => setSearch(undefined)} />}
    {media && <MediaViewer message={media} conversation={chat.id} {...{ revision, recalled, hidden, transfers }}
      open={openMedia} close={() => setMedia(undefined)} onChanged={changed} />}
  </section>;
}

function ChatSearch({ conversation, initialTab, revision, latestAiSeq, recalled, hidden, openMedia, close, transfers, changed }: {
  conversation: string; revision: number; latestAiSeq: number; recalled: ReadonlySet<number>; hidden: ReadonlySet<number>; close: () => void;
  openMedia: (message: Message) => void; initialTab: "text" | "media"; transfers: Transfer[]; changed: () => void;
}) {
  const [query, setQuery] = useState("");
  const [search, setSearch] = useState("");
  const [tab, setTab] = useState(initialTab);
  return <Dialog title={t("Search chat", "查找聊天内容")} mobilePage close={close}>
    <fieldset className="tabs">
      <legend className="sr-only">Search chat by</legend>
      <Choice type="radio" name="chat-search" checked={tab === "text"} onChange={() => setTab("text")}>Messages</Choice>
      <Choice type="radio" name="chat-search" checked={tab === "media"} onChange={() => setTab("media")}>Photos and videos</Choice>
    </fieldset>
    {tab === "media" ? <MediaHistory {...{ conversation, revision, recalled, hidden, transfers }} open={openMedia} onChanged={changed} /> : <>
      <form className="chat-search" onSubmit={event => { event.preventDefault(); setSearch(query.trim()); }}>
        <FloatingField fieldClass="search" label="Search message text" prefixIcon={<Search size={16} />} value={query}
          onChange={event => setQuery(event.target.value)} autoFocus />
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

function ChatMessages({ chat, own, revision, latestAiSeq, recalled, clockOffset, transfers, visible, changed, refresh, share, pasteFiles, aiChat,
  hidden, hide, readReceipts, draft, setDraft, openMedia, historyCache }: {
  chat: Conversation; own: User; revision: number; latestAiSeq: number; recalled: ReadonlySet<number>; clockOffset: number; transfers: Transfer[]; changed: () => void;
  refresh: () => Promise<void>; share: (source: ShareSource) => Promise<void>; pasteFiles: () => Promise<boolean>; aiChat: ChatAssistants;
  hidden: ReadonlySet<number>; hide: (seq: number) => Promise<void>; readReceipts: boolean; visible: boolean;
  historyCache: ChatHistoryCache;
  openMedia: (message: Message) => void;
} & ChatDraft) {
  const read = useCallback((before?: number) => api.messages(chat.id, before), [chat.id]);
  const savePage = useCallback((page: Page) => historyCache.save(chat.id, page), [chat.id, historyCache]);
  const { page, failure, loading, older, newestAtOpen, loadedRevision } =
    usePage(read, revision, recalled, true, latestAiSeq, hidden, false, historyCache.get(chat.id), savePage);
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
    const view = viewport.current;
    if (visible && nearBottom.current && view?.getClientRects().length) view.scrollTop = view.scrollHeight;
  }, [visible, lastSeq, pending.length, aiChat.replies.length]);

  useEffect(() => {
    const view = viewport.current;
    if (!view) return;
    const resize = new ResizeObserver(() => {
      if (nearBottom.current && view.getClientRects().length) view.scrollTop = view.scrollHeight;
    });
    resize.observe(view);
    return () => resize.disconnect();
  }, []);

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
    if (!visible || !element?.getClientRects().length) return;
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
      {!page && !failure && <div className="history-placeholder" role="status" aria-label="Loading messages">
        <LoaderCircle className="preview-spinner" size={16} aria-hidden="true" />
      </div>}
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
          function actions(selection?: string) {
            const copy = selection || text;
            return [
              ...(file && !expiredFile && (!download || download.status === "complete") ? [{ label: ios
                ? download?.status === "complete" ? t("Share", "分享") : t("Download", "下载") : actionLabels.save, icon: <Download size={15} />,
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
              tabIndex={0} aria-haspopup="menu"
              onContextMenu={event => {
                window.clearTimeout(press.current?.timer);
                const selection = window.getSelection()?.toString();
                event.preventDefault();
                if (!openedByPress(event.currentTarget)) openMenu(event.currentTarget, actions(selection),
                  event.clientX || event.clientY ? { x: event.clientX, y: event.clientY } : undefined);
              }}
              onPointerDown={event => {
                window.clearTimeout(press.current?.timer);
                press.current = undefined;
                if (event.pointerType === "mouse" || !event.isPrimary
                  || !(event.target instanceof Element && event.target.closest(
                    ".bubble, .media-tile, .file-main"
                  ))) return;
                const anchor = event.currentTarget;
                const point = { x: event.clientX, y: event.clientY };
                const timer = window.setTimeout(() => {
                  if (!press.current || !anchor.isConnected) return;
                  press.current.opened = true;
                  openMenu(anchor, actions(), point);
                }, 550);
                press.current = { ...point, anchor, timer, opened: false };
              }}
              onPointerMove={event => {
                const active = press.current;
                if (active && Math.hypot(event.clientX - active.x, event.clientY - active.y) > 10) window.clearTimeout(active.timer);
              }}
              onPointerUp={() => {
                if (press.current) {
                  window.clearTimeout(press.current.timer);
                  press.current.released = Date.now();
                }
              }}
              onPointerLeave={() => window.clearTimeout(press.current?.timer)}
              onPointerCancel={() => {
                window.clearTimeout(press.current?.timer);
                press.current = undefined;
              }}
              onClickCapture={event => {
                if (openedByPress(event.currentTarget) && event.detail > 0) {
                  event.preventDefault();
                  event.stopPropagation();
                  press.current = undefined;
                }
              }}>
              {!mine && avatar}
              <div className={"message-body" + (file ? " attachment-body" : "")}
                ref={viewed !== undefined && message?.seq === viewed ? viewedBody : undefined}>
                {chat.title && !provider && !mine && <span className="message-author">{displayName(message!.sender, remarks)}</span>}
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
                    <small role="status">{message.pending.error ?? <span className="status-icon" title={t("Queued", "排队中")} aria-label={t("Queued", "排队中")} role="img" tabIndex={0}><Clock size={14} aria-hidden /></span>}</small>
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
    <Composer conversation={chat.id} refresh={refresh} share={share} pasteFiles={pasteFiles} changed={changed} aiChat={aiChat}
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

function syncComposerScrollEdges(editor: HTMLTextAreaElement) {
  const { scrollTop, scrollHeight, clientHeight } = editor;
  editor.toggleAttribute("data-clipped-top", scrollTop > 0);
  editor.toggleAttribute("data-clipped-bottom", scrollHeight - clientHeight - scrollTop > 1);
}

function Composer({ conversation, refresh, share, pasteFiles, changed, aiChat, page, loading, older, draft: saved, setDraft, quoteUnavailable }: {
  conversation: string; refresh: () => Promise<void>; share: (source: ShareSource) => Promise<void>; pasteFiles: () => Promise<boolean>; changed: () => void;
  aiChat: ChatAssistants; page?: Page; loading: boolean; older: () => Promise<void>; quoteUnavailable: boolean;
} & ChatDraft) {
  const { remarks } = useContext(People);
  const openTools = useMenu();
  const openEditMenu = useEditMenu();
  const toolsAnchor = useMenuAnchor();
  const draft = saved.text;
  const quote = quoteUnavailable ? undefined : saved.quote;
  const pastedImage = saved.image;
  const [imageUrl, setImageUrl] = useState<string>();
  const [view, setView] = useState<string>();
  const [menu, setMenu] = useState(false);
  const [menuIndex, setMenuIndex] = useState(0);
  const [request, setRequest] = useState<{ id: string; assistant: Assistant; prompt: string; text: string }>();
  const textarea = useRef<HTMLTextAreaElement>(null);
  const clipboardFileCheck = useRef<Promise<boolean> | undefined>(undefined);
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
    if (windows && textarea.current) syncComposerScrollEdges(textarea.current);
  }, [draft]);

  useEffect(() => {
    const editor = textarea.current;
    if (!windows || !editor) return;
    const resize = new ResizeObserver(() => syncComposerScrollEdges(editor));
    resize.observe(editor);
    return () => resize.disconnect();
  }, []);

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
  const maxTextKiB = assistant ? 8 : 64;
  const oversized = new TextEncoder().encode(draft.trim()).length > maxTextKiB * 1024;

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
    setCheckingImage(true);
    try {
      if (file.size > MAX_IMAGE_BYTES) {
        setFailure("Pasted images can be up to 16 MiB.");
        return;
      }
      const type = await api.checkPastedImage(await file.arrayBuffer());
      setDraft(previous => ({ ...previous, image: new File([file], file.name, { type }) }));
    } catch (error) { setFailure(errorMessage(error)); }
    finally { setCheckingImage(false); }
  }

  async function checkClipboardFiles() {
    try { return await pasteFiles(); }
    catch (error) {
      setFailure(errorMessage(error));
      return true;
    }
  }

  async function pasteClipboardAttachment() {
    if (sending || checkingImage || await checkClipboardFiles()) return;
    setFailure("");
    setCheckingImage(true);
    try {
      const bytes = await api.readClipboardImage();
      if (!bytes.byteLength) {
        setFailure(t("There’s nothing to paste from the clipboard.", "剪贴板里没有可粘贴的内容。"));
        return;
      }
      setDraft(previous => ({ ...previous, image: new File([bytes], "clipboard.png", { type: "image/png" }) }));
    } catch (error) { setFailure(errorMessage(error)); }
    finally { setCheckingImage(false); }
  }

  return <>
    <form className="composer" onSubmit={send} onBlur={event => {
      if (!event.currentTarget.contains(event.relatedTarget)) setMenu(false);
    }}>
      {(failure || oversized) && <div className="inline-error" role="alert">{oversized
        ? t(`Text can be up to ${maxTextKiB} KiB.`, `文本最多 ${maxTextKiB} KiB。`) : failure}</div>}
      {ambiguous && <p className="inline-error" role="alert">Rename one of the matching assistants in Settings.</p>}
      {menu && <AssistantMenu id={menuId} assistants={suggestions} active={menuIndex}
        choose={choose} settings={aiChat.settings} />}
      <div className="composer-box">
        <div className={"composer-input" + (assistant ? " ai-composer" : "")}>
          {!assistant && <button className="icon-button composer-plus" type="button" title={t("Message tools", "消息工具")}
            aria-label={t("Message tools", "消息工具")} aria-haspopup="menu" aria-expanded={toolsAnchor?.classList.contains("composer-plus") ?? false}
            onClick={event => openTools(event.currentTarget, [
              { label: t("Attach a file", "附件"), icon: <Paperclip size={18} />, disabled: preparing, action: () => void pick("file") },
              ...(!ios ? [{ label: t("Attach a folder", "文件夹"), icon: <FolderOpen size={18} />, disabled: preparing, action: () => void pick("folder") }] : []),
            ])}><Plus size={20} /></button>}
          <FloatingField multiline ref={textarea} id={menuId + "-message"} label={assistant ? t("AI prompt", "AI 提示词") : t("Message", "消息")}
            className="composer-text"
            aria-autocomplete="list" aria-controls={menu ? menuId : undefined}
            aria-activedescendant={menu && activeOption ? menuId + "-" + menuIndex : undefined}
            rows={windows ? 1 : 2}
            onScroll={windows ? event => syncComposerScrollEdges(event.currentTarget) : undefined}
            onContextMenu={windows ? event => openEditMenu(event, pasteClipboardAttachment) : undefined}
            value={draft} onChange={event => {
              const value = event.target.value;
              setDraft(previous => ({ ...previous, text: value, nonce: undefined }));
              const input = event.nativeEvent as InputEvent;
              const typedAt = input.inputType === "insertText" && input.data === "@";
              setMenu((menu || typedAt) && /^\s*@[\p{L}\p{N}_-]*$/u.test(value));
              setMenuIndex(0);
            }}
            onPaste={event => {
              const file = Array.from(event.clipboardData.items).find(item => item.kind === "file" && item.type.startsWith("image/"))?.getAsFile();
              if (!file) return;
              event.preventDefault();
              if (sending || checkingImage) return;
              const files = clipboardFileCheck.current;
              void (async () => {
                if (!windows || !(await (files ?? checkClipboardFiles()))) await preparePastedImage(file);
              })();
            }}
            onKeyDown={event => {
              if (event.nativeEvent.isComposing) return;
              if (windows && event.nativeEvent.isTrusted && event.ctrlKey && !event.altKey && !event.shiftKey
                  && event.key.toLowerCase() === "v" && !event.repeat && !sending && !checkingImage) {
                // Explorer's native file list may not reach the WebView's paste event.
                const files = checkClipboardFiles();
                clipboardFileCheck.current = files;
                void files.then(() => {
                  if (clipboardFileCheck.current === files) clipboardFileCheck.current = undefined;
                });
              }
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
          <button className="button primary send-button" type="submit"
            disabled={(!draft.trim() && !pastedImage) || ambiguous || quoteUnavailable || oversized || sending || checkingImage || (!!assistant && (!view || !prompt))}
            aria-label={assistant ? "Choose AI context" : sending ? "Sending message" : "Send message"}>
            {assistant ? <>Choose context <ArrowRight size={16} /></>
              : <span>{sending ? t("Sending…", "发送中…") : t("Send", "发送")}</span>}
          </button>
        </div>
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
        {preparing && <span className="drop-hint" role="status">Preparing attachment…</span>}
        {checkingImage && <span className="drop-hint" role="status">Checking image…</span>}
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
