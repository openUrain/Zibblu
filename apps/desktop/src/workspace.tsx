import { FloatingField } from "./floating-field";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import {
  ArrowRight, CircleUserRound, FolderOpen, Info, LogOut, Menu, MessageCircle, MessageSquare,
  Radar, ScanLine, Search, Settings, UserRoundPlus, UsersRound, X,
  ChevronLeft, ChevronRight, UserRound,
} from "lucide-react";
import { useCallback, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { FormEvent, ReactNode } from "react";
import { ai } from "./ai-api";
import type { AiReply, Assistant } from "./ai-api";
import { AssistantSettings } from "./ai";
import { api, errorMessage, isExpired } from "./api";
import type { CloseAction, Conversation, ConversationRequest, Device, Directory, Draft, DropOffer, Profile, Session, ShareSource, SyncUpdate, Transfer, User } from "./api";
import { mediaKind } from "./attachment-media";
import { Blank, Brand, Choice, Dialog, People, PersonAvatar, chatName, dateTimeLabel, displayName } from "./common";
import { orderContacts } from "./contact-order";
import { actions, t } from "./locale";
import { ChatList, ChatView } from "./chat";
import { FilesView } from "./files";
import { NearbyDevices, PrivateChatView, PrivateInvitationOverlay, PrivateTargets, usePrivateRooms } from "./private-chat";
import { ScanQr, ShareQr } from "./qr";
import type { SharedCode } from "./qr";
import { ProfileView } from "./profile";
import { useMenu, useMenuAnchor, useRowMenu } from "./context-menu";
import { CloseActionChoices, WindowControls, windows } from "./window-chrome";
import { ios, useIpadSplit, useIosBack } from "./ios";

const pages = [
  { name: "Chats", Icon: MessageSquare }, { name: "Contacts", Icon: UsersRound },
  { name: "Files", Icon: FolderOpen },
] as const;
type Navigation = typeof pages[number]["name"] | "Me" | "Settings";
const tabLabels = { Chats: t("Chats", "聊天"), Contacts: t("Contacts", "联系人"), Files: t("Files", "文件"), Me: t("Me", "我的") };

export function Workspace({ session, privateQuiet, setPrivateQuiet, closeAction, setCloseAction, signedOut }: {
  session: Session; signedOut: (notice?: string) => void;
  privateQuiet: boolean; setPrivateQuiet: (quiet: boolean) => Promise<void>;
  closeAction?: CloseAction; setCloseAction: (action: CloseAction) => Promise<void>;
}) {
  const [nav, setNav] = useState<Navigation>("Chats");
  const [openAssistants, setOpenAssistants] = useState(false);
  const ipadSplit = useIpadSplit();
  const openMenu = useMenu();
  const menuAnchor = useMenuAnchor();
  const railMenu = useRef<HTMLButtonElement>(null);
  const [about, setAbout] = useState(false);
  const [assistants, setAssistants] = useState<Assistant[]>([]);
  const [assistantFailure, setAssistantFailure] = useState("");
  const [aiReplies, setAiReplies] = useState<AiReply[]>([]);
  const [directory, setDirectory] = useState<Directory>({ contacts: [], conversations: [], pending_contacts: [], remarks: {}, names: {}, avatars: {}, read_receipts: false });
  const [recalled, setRecalled] = useState<ReadonlySet<number>>(new Set());
  const [hidden, setHidden] = useState<ReadonlySet<number>>(new Set());
  const [drafts, setDrafts] = useState<Record<string, Draft>>({});
  const [profile, setProfile] = useState<User>();
  const [selectedContact, setSelectedContact] = useState<string>();
  const [ownProfileOpen, setOwnProfileOpen] = useState(false);
  const profileTrigger = useRef<HTMLElement>(null);
  const refreshInFlight = useRef<Promise<void> | null>(null);
  const appliedRevision = useRef(0);
  const [clockOffset, setClockOffset] = useState(0);
  const [selected, setSelected] = useState<string>();
  const [chatPage, setChatPage] = useState<{ id: string; page: "details" | "search" }>();
  const [mobileChat, setMobileChat] = useState(false);
  const [privateId, setPrivateId] = useState<string>();
  const [privateTargets, setPrivateTargets] = useState<Conversation>();
  const [nearby, setNearby] = useState(false);
  const [shareCode, setShareCode] = useState<SharedCode>();
  const [scanning, setScanning] = useState(false);
  const { rooms: privateRooms, prompt } = usePrivateRooms();
  const joined = useRef(new Set<string>());
  const privateOpen = privateId !== undefined;
  const [revision, setRevision] = useState(0);
  const [latestAiSeq, setLatestAiSeq] = useState(0);
  const [connection, setConnection] = useState<"Connecting" | "Connected" | "Reconnecting">("Connecting");
  const [transfers, setTransfers] = useState<Transfer[]>([]);
  const [failure, setFailure] = useState(session.notice ?? "");
  const [syncProblem, setSyncProblem] = useState("");
  const [newChat, setNewChat] = useState(false);
  const [newContact, setNewContact] = useState(false);
  const [offer, setOffer] = useState<DropOffer & { conversation: string }>();
  const [sharing, setSharing] = useState(false);
  const [leaving, setLeaving] = useState(false);
  const signingOut = useRef(false);

  useEffect(() => {
    if (!ios) return;
    function clearPrivateView() {
      setPrivateId(undefined);
      setPrivateTargets(undefined);
      setScanning(false);
      setNearby(false);
      setShareCode(code => code?.kind === "private" ? undefined : code);
      setDrafts(previous => Object.fromEntries(Object.entries(previous).filter(([key]) => !key.startsWith("private/"))));
    }
    const invalidated = listen("ios-private-reset", clearPrivateView);
    const visibility = () => { if (document.hidden) clearPrivateView(); };
    document.addEventListener("visibilitychange", visibility);
    return () => {
      document.removeEventListener("visibilitychange", visibility);
      void invalidated.then(unlisten => unlisten());
    };
  }, []);

  function openProfile(user: User) {
    if (windows && user.id === session.user.id) {
      setOwnProfileOpen(true);
      return;
    }
    if (document.activeElement instanceof HTMLElement) profileTrigger.current = document.activeElement;
    if (windows && nav === "Contacts") setSelectedContact(user.id);
    setProfile(user);
  }

  function closeProfile() {
    const trigger = profileTrigger.current;
    const contactId = windows && nav === "Contacts" ? profile?.id : undefined;
    setProfile(undefined);
    requestAnimationFrame(() => {
      if (trigger?.isConnected) {
        trigger.focus({ preventScroll: true });
      } else if (contactId) {
        const row = document.querySelector<HTMLButtonElement>(`.contacts-screen [data-contact-id="${CSS.escape(contactId)}"]`);
        if (row) row.focus();
        else document.querySelector<HTMLInputElement>('.contacts-screen input[type="search"]')?.focus({ preventScroll: true });
      }
    });
  }

  function profileEdited(id: string, patch: Partial<Profile>) {
    setDirectory(previous => {
      const next = { ...previous, names: { ...previous.names }, avatars: { ...previous.avatars }, remarks: { ...previous.remarks } };
      if (patch.display_name !== undefined) {
        if (patch.display_name) next.names[id] = patch.display_name;
        else delete next.names[id];
      }
      if (patch.avatar) next.avatars[id] = patch.avatar;
      if (patch.notes) {
        if (patch.notes.remark) next.remarks[id] = patch.notes.remark;
        else delete next.remarks[id];
      }
      return next;
    });
  }

  function setDraft(key: string, change: (draft: Draft) => Draft) {
    setDrafts(previous => {
      const next = change(previous[key] ?? { text: "" });
      if (next.text || next.quote || next.image) return { ...previous, [key]: next };
      if (!previous[key]) return previous;
      const remaining = { ...previous };
      delete remaining[key];
      return remaining;
    });
  }

  useEffect(() => {
    const connected = new Set(privateRooms.filter(room => room.phase === "connected").map(room => "private/" + room.id));
    setDrafts(previous => {
      const expired = Object.keys(previous).filter(key => key.startsWith("private/") && !connected.has(key));
      if (!expired.length) return previous;
      const next = { ...previous };
      expired.forEach(key => delete next[key]);
      return next;
    });
  }, [privateRooms]);

  useEffect(() => {
    let current = true;
    void ai.assistants().then(value => { if (current) setAssistants(value); })
      .catch(error => { if (current) setAssistantFailure(errorMessage(error)); });
    return () => { current = false; };
  }, []);

  const apply = useCallback((update: SyncUpdate) => {
    if (signingOut.current || update.revision < appliedRevision.current) return;
    appliedRevision.current = update.revision;
    setDirectory(update.directory);
    setClockOffset(update.server_now - Date.now());
    setRecalled(previous => previous.size === update.recalled.length ? previous : new Set(update.recalled));
    setRevision(update.revision);
    setLatestAiSeq(update.latest_ai_seq);
  }, []);

  const refreshLocal = useCallback(() => api.localUpdate().then(apply).catch(() => {}), [apply]);

  const refresh = useCallback(() => {
    const local = refreshLocal();
    if (refreshInFlight.current) return refreshInFlight.current;
    const sync = async () => {
      await local;
      try {
        apply(await api.sync());
        setConnection("Connected");
        setSyncProblem("");
      } catch (error) {
        if (isExpired(error)) {
          if (!signingOut.current) {
            signingOut.current = true;
            signedOut(errorMessage(error));
          }
        } else {
          setConnection("Reconnecting");
          if (typeof error === "object" && error !== null && "status" in error && error.status === 409) {
            setSyncProblem("This device's sync position is no longer valid. Keep this profile and sign in from a new data directory.");
          }
          void refreshLocal();
        }
      } finally {
        refreshInFlight.current = null;
      }
    };
    refreshInFlight.current = sync();
    return refreshInFlight.current;
  }, [apply, refreshLocal, signedOut]);

  useEffect(() => {
    const updated = listen("message-outbox-changed", () => void refreshLocal());
    const online = () => void refresh();
    window.addEventListener("online", online);
    return () => {
      void updated.then(unlisten => unlisten());
      window.removeEventListener("online", online);
    };
  }, [refreshLocal, refresh]);

  const changed = useCallback(() => {
    void api.transfers().then(setTransfers).catch(error => setFailure(errorMessage(error)));
  }, []);

  useEffect(() => {
    let closed = false;
    let poll: number | undefined;
    async function syncLoop() {
      if (closed) return;
      await refresh();
      if (!closed) poll = window.setTimeout(syncLoop, 1700);
    }
    void syncLoop();
    return () => { closed = true; window.clearTimeout(poll); };
  }, [refresh]);

  useEffect(() => {
    let closed = false;
    let poll: number | undefined;
    async function transferLoop() {
      try {
        const active = await api.transfers();
        if (!closed) setTransfers(active);
      } catch (error) {
        if (!closed) setFailure(errorMessage(error));
      }
      if (!closed) poll = window.setTimeout(transferLoop, 450);
    }
    void transferLoop();
    return () => { closed = true; window.clearTimeout(poll); };
  }, []);

  const active = directory.conversations.find(chat => chat.id === selected);
  const activeId = nav === "Chats" && !privateOpen && !profile && !ownProfileOpen ? active?.id : undefined;
  useEffect(() => {
    const connected = privateRooms.filter(room => room.phase === "connected");
    const opened = connected.find(room => !joined.current.has(room.id) && room.conversation === selected);
    joined.current = new Set(connected.map(room => room.id));
    if (opened && nav === "Chats" && !privateId) {
      setOffer(undefined);
      setPrivateId(opened.id);
      setMobileChat(true);
    }
  }, [nav, privateId, privateRooms, selected]);
  useEffect(() => {
    const drop = listen<DropOffer>("attachment-drop", event => {
      if (privateOpen) return;
      const chatVisible = mobileChat || !window.matchMedia("(max-width: 740px)").matches;
      if (activeId && chatVisible) setOffer({ ...event.payload, conversation: activeId });
      else setFailure("Open a chat before dropping a file or folder.");
    });
    const error = listen<string>("attachment-drop-error", event => {
      if (!privateOpen) setFailure(event.payload);
    });
    return () => { void drop.then(unlisten => unlisten()); void error.then(unlisten => unlisten()); };
  }, [activeId, mobileChat, privateOpen]);

  function navigate(page: Navigation, assistants = false) {
    setProfile(undefined);
    setOpenAssistants(assistants);
    setPrivateId(undefined);
    setNav(page);
    if (page === "Chats") setMobileChat(false);
  }

  function selectChat(id: string, page?: "details" | "search") {
    setProfile(undefined);
    setPrivateId(undefined);
    setSelected(id);
    setChatPage(page ? { id, page } : undefined);
    setNav("Chats");
    setMobileChat(true);
  }

  const showPrivate = useCallback((id: string, conversation?: string | null) => {
    if (conversation) setSelected(conversation);
    setOffer(undefined);
    setNav("Chats");
    setPrivateId(id);
    setMobileChat(true);
  }, []);

  useEffect(() => {
    const accepted = listen<{ id: string; conversation: string | null }>("private-invitation-accepted", ({ payload }) => {
      setPrivateTargets(undefined);
      setNearby(false);
      setShareCode(undefined);
      setScanning(false);
      setNewChat(false);
      setNewContact(false);
      setProfile(undefined);
      setOwnProfileOpen(false);
      showPrivate(payload.id, payload.conversation);
    });
    return () => { void accepted.then(unlisten => unlisten()); };
  }, [showPrivate]);

  function selectPrivate(id: string) {
    showPrivate(id, privateRooms.find(room => room.id === id)?.conversation);
  }

  async function startPrivate(chat: Conversation, invited: string[]) {
    const id = await api.startPrivate(chat.id, invited);
    setPrivateTargets(undefined);
    showPrivate(id, chat.id);
  }

  async function invitePrivate(chat: Conversation) {
    if (chat.title) {
      setPrivateTargets(chat);
      return;
    }
    try {
      const { rooms } = await api.privateRooms();
      const existing = rooms.find(room => room.conversation === chat.id
        && room.phase !== "ended" && room.phase !== "missed" && room.phase !== "failed");
      if (existing) showPrivate(existing.id, chat.id);
      else await startPrivate(chat, chat.members.filter(person => person.id !== session.user.id).map(person => person.id));
    } catch (error) {
      setFailure(errorMessage(error));
    }
  }

  async function showPrivateCode(id: string) {
    const code = await api.privateCode(id);
    setShareCode({ kind: "private", name: "Private chat QR code",
      value: JSON.stringify(code), expiresAt: code.expires_at });
  }

  async function createPrivateCode() {
    try {
      const id = await api.startPrivate(null, [], true);
      selectPrivate(id);
      await showPrivateCode(id);
    } catch (error) { setFailure(errorMessage(error)); }
  }

  async function create(request: ConversationRequest) {
    const id = await api.create(request);
    selectChat(id);
    setNewChat(false);
    await refresh();
  }

  async function hide(conversation: string, seq: number) {
    await api.hideMessage(conversation, seq);
    setHidden(previous => new Set(previous).add(seq));
    setDraft(conversation, previous => previous.quote?.seq === seq ? { ...previous, quote: undefined, nonce: undefined } : previous);
    changed();
    setDirectory(await api.directory());
  }

  async function share(conversation: string, source: ShareSource) {
    setFailure("");
    try { await api.share(conversation, source); }
    catch (error) { setFailure(errorMessage(error)); }
    finally { changed(); }
  }

  async function pasteFiles(conversation: string) {
    const clipboard = await api.offerClipboardFiles();
    if (!clipboard) return false;
    setOffer({ ...clipboard, conversation });
    return true;
  }

  function cancelOffer() {
    if (offer) void api.cancelAttachmentOffer(offer.ticket).catch(() => {});
    setOffer(undefined);
  }

  async function signOut() {
    if (signingOut.current) return;
    signingOut.current = true;
    setLeaving(true);
    try { await api.logout(); signedOut(); }
    catch (error) { signedOut(errorMessage(error)); }
  }

  const names = useMemo(() => ({ ...directory.names, ...directory.remarks }), [directory.names, directory.remarks]);
  const people = [...new Map([session.user, ...directory.conversations.flatMap(chat => chat.members)]
    .map(user => [user.id, user])).values()].sort((a, b) => displayName(a, names).localeCompare(displayName(b, names)));
  const offeredChat = directory.conversations.find(chat => chat.id === offer?.conversation);
  const connected = connection === "Connected";
  const invitation = privateRooms.find(room => room.id === prompt);
  const attention: Partial<Record<Navigation, string>> = {
    Chats: directory.conversations.some(chat => chat.unread > 0) ? "Unread messages" : undefined,
    Contacts: directory.pending_contacts.some(request => request.incoming) ? "Pending contact requests" : undefined,
    Files: transfers.some(transfer => transfer.status !== "complete" && !mediaKind(transfer.attachment)) ? "Unfinished transfers" : undefined,
  };

  const conversationOpen = nav === "Chats" && mobileChat && (!!active || privateOpen);
  function backFromChat() {
    if (privateOpen) setPrivateId(undefined);
    setMobileChat(false);
  }
  useIosBack(conversationOpen && !ipadSplit ? backFromChat : undefined);
  const splitProfile = ipadSplit && (nav === "Contacts" || nav === "Me");
  const desktopContacts = !ios && nav === "Contacts";
  const chatProfile = nav === "Chats";
  const profilePage = profile && <ProfileView key={profile.id} user={profile} session={session}
    reconnecting={connection === "Reconnecting"}
    relationVersion={directory.contacts.some(person => person.id === profile.id) ? "contact"
      : directory.pending_contacts.find(request => request.user.id === profile.id)?.request_id ?? "other"}
    refresh={refresh} close={closeProfile} edited={profileEdited}
    message={() => create({ kind: "direct", contact_id: profile.id })} />;
  return <People.Provider value={{ remarks: names, avatars: directory.avatars, online: connected, open: openProfile }}>
  <main className={"shell" + (conversationOpen ? " conversation-open" : "") + (profile ? " profile-open" : "")
    + (ipadSplit ? " ipad-split" : "")}>
    <nav className="rail" aria-label="Main navigation">
      <button className="rail-brand icon-button" aria-label={t("Your profile", "我的资料")} title={t("Your profile", "我的资料")}
        aria-haspopup={windows ? "dialog" : undefined} aria-expanded={windows ? ownProfileOpen : undefined}
        onClick={() => openProfile(session.user)}><PersonAvatar user={session.user} /></button>
      <div className="rail-links">{pages.map(({ name, Icon }) =>
        <button key={name} aria-label={tabLabels[name]} title={tabLabels[name]}
          aria-describedby={attention[name] ? `rail-status-${name}` : undefined}
          aria-current={nav === name ? "page" : undefined} onClick={() => navigate(name)}>
          <span className="rail-icon">{nav === name ? <ActiveRailIcon name={name} /> : <Icon size={22} strokeWidth={1.9} />}
            {attention[name] && <span className="rail-dot" aria-hidden="true" />}
          </span><span>{tabLabels[name]}</span>
        </button>)}</div>
      <button className="rail-menu icon-button" ref={railMenu} title="Menu" aria-label="Menu" aria-haspopup="menu"
        aria-expanded={menuAnchor !== undefined && menuAnchor === railMenu.current}
        onClick={event => openMenu(event.currentTarget, [
          { label: "Settings", icon: <Settings size={17} />, action: () => navigate("Settings") },
          { label: "About Zibblu", icon: <Info size={17} />, action: () => setAbout(true) },
          { label: "Feedback (unavailable)", icon: <MessageSquare size={17} />, action: () => {}, disabled: true },
          { label: "Sign out", icon: <LogOut size={17} />, action: () => void signOut(), disabled: leaving },
        ])}><Menu size={23} /></button>
    </nav>
    {ios && <nav className="phone-tabs" aria-label="Main navigation">
      {[...pages, { name: "Me", Icon: UserRound } as const].map(({ name, Icon }) => {
        const selectedTab = name === nav || (name === "Me" && nav === "Settings");
        return <button key={name} aria-current={selectedTab ? "page" : undefined}
          aria-describedby={attention[name] ? `rail-status-${name}` : undefined} onClick={() => navigate(name)}>
          <span className="rail-icon">{selectedTab && name !== "Me" ? <ActiveRailIcon name={name} />
            : <Icon size={22} fill={selectedTab ? "currentColor" : "none"} />}
            {attention[name] && <span className="rail-dot" aria-hidden="true" />}
          </span><span>{tabLabels[name]}</span>
        </button>;
      })}
    </nav>}
    <div className="sr-only" aria-live="polite" aria-atomic="true">
      {pages.map(({ name }) => <span key={name} id={`rail-status-${name}`}>{attention[name] && `${attention[name]}. `}</span>)}
    </div>
    <div className="shell-main">
      <header className={"topbar" + (windows ? " windows-topbar" : "")}>
        <div className="app-name window-drag" data-tauri-drag-region={windows ? "" : undefined}>Zibblu</div>
        <div className="chat-actions">
        <button className="icon-button" aria-label="Nearby devices" title="Nearby devices"
          onClick={() => setNearby(true)}><Radar size={20} /></button>
        <button className="icon-button" aria-label="Scan QR code" title="Scan QR code"
          onClick={() => setScanning(true)}><ScanLine size={20} /></button>
        <div className={"connection " + (connected ? "connected" : "disconnected")} role="status">
          <span className="connection-dot" />{connection}
        </div></div>
        {windows && <WindowControls />}
      </header>
      {failure && <div className="workspace-error" role="alert">{failure}
        <button className="icon-button" aria-label="Dismiss error" onClick={() => setFailure("")}><X size={17} /></button>
      </div>}
      {syncProblem && <div className="workspace-error" role="alert">{syncProblem}</div>}
      {ios && !connected && <div className="mobile-connection" role="status">{connection}</div>}
      <div className={"workspace-content" + (splitProfile ? " ipad-profile-layout" : "")
        + (desktopContacts ? " contacts-layout" : "")
        + (profile && !splitProfile && !chatProfile && !desktopContacts ? " profile-replaces-root" : "")}>
      {nav === "Chats" && <div className={"chat-layout" + (mobileChat && (active || privateOpen) ? " mobile-chat-open" : "")
        + (profile && chatProfile ? " chat-profile-open" : "")}>
        <ChatList chats={directory.conversations} own={session.user} selected={privateOpen ? undefined : selected}
          open={selectChat} newChat={() => setNewChat(true)} addContact={() => setNewContact(true)}
          rooms={privateRooms} activePrivate={privateId}
          openPrivate={selectPrivate} createQr={() => void createPrivateCode()}
          nearby={() => setNearby(true)} scan={() => setScanning(true)} />
        {privateId ? <PrivateChatView key={privateId} id={privateId} own={session.user}
            isContact={id => directory.contacts.some(person => person.id === id)}
            draft={drafts["private/" + privateId]?.text ?? ""}
            setDraft={change => setDraft("private/" + privateId, previous => ({ text: change(previous.text) }))}
            back={backFromChat} showCode={showPrivateCode} />
          : active ? <ChatView key={active.id} chat={active} own={session.user} server={session.server} revision={revision} profileOpen={!!profile}
            visibleOnNarrow={mobileChat}
            requested={chatPage?.id === active.id ? chatPage : undefined}
            latestAiSeq={latestAiSeq} recalled={recalled} clockOffset={clockOffset}
            hidden={hidden} hide={seq => hide(active.id, seq)} readReceipts={directory.read_receipts && connected && !profile}
            draft={drafts[active.id] ?? { text: "" }} setDraft={change => setDraft(active.id, change)}
            transfers={transfers} changed={changed} refresh={refresh} back={() => setMobileChat(false)}
            pasteFiles={() => pasteFiles(active.id)}
            share={source => share(active.id, source)} openPrivate={() => void invitePrivate(active)}
            aiChat={{
              assistants, settings: () => navigate("Settings", true), replies: aiReplies.filter(reply => reply.conversation === active.id),
              answered: reply => setAiReplies(current => [...current, reply]),
              dismiss: id => setAiReplies(current => current.filter(reply => reply.id !== id)),
            }} />
          : <section className="chat-blank"><Blank icon={<MessageCircle size={28} />} title={t("Select a chat", "选择会话")} /></section>}
        {chatProfile && profilePage}
      </div>}
      {nav === "Contacts" && <Contacts directory={directory} selected={profile?.id ?? (windows ? selectedContact : undefined)} refresh={refresh} create={create}
        addContact={() => setNewContact(true)} />}
      {nav === "Files" && <FilesView people={people} revision={revision} recalled={recalled} hidden={hidden} transfers={transfers}
        sources={new Map(directory.conversations.map(chat => [chat.id, chatName(chat, session.user.id, names)]))} onChanged={changed} />}
      {nav === "Me" && <section className="settings-page phone-me">
        <header className="workspace-heading"><h1>{tabLabels.Me}</h1></header>
        <button className="phone-profile drawer-action" onClick={() => openProfile(session.user)}>
          <PersonAvatar user={session.user} large />
          <span><strong>{displayName(session.user, names)}</strong><small className="muted">@{session.user.handle}</small></span><ChevronRight size={20} />
        </button>
        <div className="phone-me-actions">
          <button className="drawer-action" onClick={() => navigate("Settings")}><Settings size={19} /> Settings <ChevronRight size={17} /></button>
          <button className="drawer-action" onClick={() => setAbout(true)}><Info size={19} /> About Zibblu <ChevronRight size={17} /></button>
        </div>
        <button className="drawer-action danger" disabled={leaving} onClick={() => void signOut()}><LogOut size={19} /> Sign out</button>
      </section>}
      {nav === "Settings" && <SettingsView {...{ session, privateQuiet, setPrivateQuiet, closeAction, setCloseAction }}
        back={ios ? () => navigate("Me") : undefined} openAssistants={openAssistants}
        readReceipts={directory.read_receipts} setReadReceipts={async enabled => {
          await api.setReadReceipts(enabled);
          await refreshInFlight.current;
          setDirectory(previous => ({ ...previous, read_receipts: enabled }));
          await refresh();
        }}
        assistantSettings={<AssistantSettings assistants={assistants} failure={assistantFailure} changed={next => {
          setAssistants(next);
          setAssistantFailure("");
        }} />} />}
      {!chatProfile && profilePage}
      {desktopContacts && windows && !profile && <section className="chat-blank contact-detail-empty">
        {(directory.contacts.length > 0 || directory.pending_contacts.length > 0)
          && <p>{t("Select a contact", "选择联系人")}</p>}
      </section>}
      {(splitProfile || (desktopContacts && !windows)) && !profile && <section className="chat-blank">
        <Blank icon={<UserRound size={28} />} title={nav === "Me" ? t("Personal information", "个人信息") : t("Select a contact", "选择联系人")} />
      </section>}
      </div>
    </div>
    {ownProfileOpen && <ProfileView popup user={session.user} session={session} relationVersion="other"
      reconnecting={connection === "Reconnecting"} refresh={refresh} close={() => setOwnProfileOpen(false)}
      edited={profileEdited} />}
    {about && <About close={() => setAbout(false)} />}
    {privateTargets && <PrivateTargets people={privateTargets.members.filter(user => user.id !== session.user.id)}
      close={() => setPrivateTargets(undefined)} start={invited => startPrivate(privateTargets, invited)} />}
    {nearby && <NearbyDevices own={session.user.id} close={() => setNearby(false)} invited={id => {
      setNearby(false);
      showPrivate(id);
    }} />}
    {shareCode && <ShareQr code={shareCode} server={session.server} close={() => setShareCode(undefined)} />}
    {scanning && <ScanQr session={session} close={() => setScanning(false)}
      contactFound={user => { setScanning(false); openProfile(user); }}
      groupJoined={id => { setScanning(false); selectChat(id); void refresh(); }}
      privateJoined={id => { setScanning(false); selectPrivate(id); }} />}
    {newChat && <NewChat contacts={directory.contacts} create={create} close={() => setNewChat(false)} findPeople={() => {
      setNewChat(false); navigate("Contacts"); setNewContact(true);
    }} />}
    {newContact && <AddContact close={() => setNewContact(false)} open={user => {
      setNewContact(false);
      openProfile(user);
    }} />}
    {!privateOpen && offer && offeredChat && <Dialog title={offer.names.length === 1 ? "Share this attachment?" : "Share these attachments?"} close={() => { if (!sharing) cancelOffer(); }}>
      <p className="drop-confirm"><strong>{offer.names.join(", ")}</strong> will be shared in <strong>{chatName(offeredChat, session.user.id, names)}</strong>.</p>
      <div className="dialog-actions">
        <button className="button subtle" onClick={cancelOffer} disabled={sharing}>Cancel</button>
        <button className="button primary" disabled={sharing} onClick={async () => {
          setSharing(true);
          await share(offer.conversation, { kind: "dropped", ticket: offer.ticket });
          setSharing(false);
          setOffer(undefined);
        }}>{sharing ? "Preparing…" : "Share"}</button>
      </div>
    </Dialog>}
    {invitation && <PrivateInvitationOverlay key={invitation.id} room={invitation} />}
  </main></People.Provider>;
}

function ActiveRailIcon({ name }: { name: typeof pages[number]["name"] }) {
  const paths = {
    Chats: "M5 3h14a3 3 0 0 1 3 3v11a3 3 0 0 1-3 3H8l-5 2V6a3 3 0 0 1 2-3Z",
    Contacts: "M10 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8Zm0 2a8 8 0 0 0-8 7h16a8 8 0 0 0-8-7Zm7-9a3.5 3.5 0 0 1 0 7h-1a6 6 0 0 0 0-7h1Zm1 9a7 7 0 0 1 5 7h-3a10 10 0 0 0-3-7h1Z",
    Files: "M2 6a2 2 0 0 1 2-2h5l3 3h7a2 2 0 0 1 2 2v1H7a3 3 0 0 0-3 2l-2 5V6Zm5 6h14a1.5 1.5 0 0 1 1.4 2l-2.2 5a2 2 0 0 1-1.8 1H4l2.4-7a.7.7 0 0 1 .6-1Z",
  };
  return <svg width="22" height="22" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d={paths[name]} /></svg>;
}

function About({ close }: { close: () => void }) {
  const [version, setVersion] = useState("");
  useEffect(() => { void getVersion().then(setVersion).catch(() => {}); }, []);
  return <Dialog title="About Zibblu" close={close}>
    <div className="about-app"><Brand /><div><strong>Zibblu {version}</strong><p className="muted">Chat freely. Share privately.</p></div></div>
  </Dialog>;
}

function AddContact({ close, open }: { close: () => void; open: (user: User) => void }) {
  const { remarks } = useContext(People);
  const [handle, setHandle] = useState("");
  const [found, setFound] = useState<User>();
  const [failure, setFailure] = useState("");
  const [busy, setBusy] = useState(false);

  return <Dialog title={t("Add contact", "添加联系人")} mobilePage close={close} closeDisabled={busy}>
    <div className="contact-search">
      <form onSubmit={async event => {
        event.preventDefault();
        if (busy || !handle.trim()) return;
        setBusy(true);
        setFailure("");
        setFound(undefined);
        try { setFound(await api.findUser(handle.trim())); }
        catch (error) { setFailure(errorMessage(error)); }
        finally { setBusy(false); }
      }}>
        <FloatingField fieldClass="search" label={t("Contact username", "联系人账号")} prefixIcon={<Search size={17} />} value={handle}
          onChange={event => { setHandle(event.target.value); setFound(undefined); setFailure(""); }} maxLength={32} required readOnly={busy} autoFocus />
        <button className="button primary" disabled={busy || !handle.trim()} type="submit">
          {busy ? t("Searching…", "搜索中…") : t("Search", "搜索")}
        </button>
      </form>
      {found && <button className="drawer-action search-result" onClick={() => open(found)}>
        <PersonAvatar user={found} /><span>{displayName(found, remarks)} <small>@{found.handle}</small></span><ChevronRight size={17} />
      </button>}
      {failure && <p className="inline-error" role="alert">{failure}</p>}
    </div>
  </Dialog>;
}

function Contacts({ directory, selected, refresh, create, addContact }: {
  directory: Directory; selected?: string; refresh: () => Promise<void>; create: (request: ConversationRequest) => Promise<void>;
  addContact: () => void;
}) {
  const { contacts, pending_contacts } = directory;
  const { remarks, open } = useContext(People);
  const menu = useRowMenu();
  const [search, setSearch] = useState("");
  const list = useRef<HTMLDivElement>(null);
  const [failure, setFailure] = useState("");
  const [busy, setBusy] = useState(false);

  async function perform(action: () => Promise<unknown>) {
    if (busy) return;
    setBusy(true);
    setFailure("");
    try { await action(); }
    catch (error) { setFailure(errorMessage(error)); }
    finally { setBusy(false); }
  }

  const windowsContacts = useMemo(() => windows ? orderContacts(contacts, remarks) : [], [contacts, remarks]);
  const query = search.trim().toLocaleLowerCase();
  const matches = (name: string) => name.toLocaleLowerCase().includes(query);
  const filtered = (windows ? windowsContacts : contacts.map(user => ({
    user, name: displayName(user, remarks), letter: "", showHandle: false,
  })))
    .filter(({ user, name }) => matches(user.handle) || matches(name));
  if (!windows) filtered.sort((a, b) => a.name.localeCompare(b.name, navigator.language));
  const groups = new Map<string, typeof filtered>();
  for (const entry of filtered) {
    const group = groups.get(entry.letter);
    if (group) group.push(entry);
    else groups.set(entry.letter, [entry]);
  }
  const emptyText = query ? t("No matching contacts", "没有匹配的联系人") : t("No contacts yet", "暂无联系人");
  return <section className="settings-page contacts-screen">
    {ios && <header className="workspace-heading"><h1>{tabLabels.Contacts}</h1></header>}
    <form className="chat-list-head" role={windows ? "search" : undefined} onSubmit={event => {
      event.preventDefault();
      if (query && filtered.length === 1) open?.(filtered[0].user);
    }}>
      <FloatingField fieldClass="search" label={t("Search contacts", "搜索联系人")} prefixIcon={<Search size={17} />}
        type="search" value={search} onChange={event => {
          if (windows && list.current) list.current.scrollTop = 0;
          setSearch(event.currentTarget.value);
        }} />
      <button type="button" className="icon-button new-conversation" title={t("Add contact", "添加联系人")}
        aria-label={t("Add contact", "添加联系人")} aria-haspopup={windows ? "dialog" : undefined}
        onClick={addContact}><UserRoundPlus size={windows ? 20 : 21} /></button>
    </form>
    <div className="contacts-list" ref={list}>
    {failure && <p className="inline-error" role="alert">{failure}</p>}
    {!query && pending_contacts.length > 0 && <section>
      <h2 className="section-title">{t("Contact requests", "新的朋友")}</h2>
      <div className="contact-requests">
        {pending_contacts.map(({ user, incoming, greeting, request_id }) => <div className="contact-row" key={user.id}>
          <button className="person-link contact-name" aria-current={selected === user.id || undefined} onClick={() => open?.(user)}>
            <PersonAvatar user={user} /><span><strong>{displayName(user, remarks)}</strong>
              <small>{incoming ? "Wants to connect" : "Waiting for approval"}</small>
              {greeting && <span className="contact-greeting">{greeting}</span>}
            </span>
          </button>
          {incoming ? <>
            <button className="button subtle" disabled={busy} onClick={() => void perform(async () => {
              await api.respondContact(user.id, false, request_id); await refresh();
            })}>Decline</button>
            <button className="button primary" disabled={busy} onClick={() => void perform(async () => {
              await api.respondContact(user.id, true, request_id); await refresh();
            })}>Approve</button>
          </> : <button className="button subtle" disabled={busy} onClick={() => void perform(async () => {
            await api.removeContact(user.id); await refresh();
          })}>Cancel request</button>}
        </div>)}
      </div>
    </section>}
    {filtered.length ? <div className="contacts" aria-label={tabLabels.Contacts}>
      {[...groups].map(([letter, entries]) => <div className={letter ? "contact-group" : undefined} key={letter}>
        {letter && <h2 className="contact-group-letter">{letter}</h2>}
        {entries.map(({ user, name, showHandle }) => <div className="contact-row" key={user.id}>
          <button className="person-link contact-name" aria-current={selected === user.id || undefined}
            data-contact-id={windows ? user.id : undefined} onClick={() => open?.(user)}
            {...menu([
              { label: t("View profile", "查看资料"), icon: <UserRound size={17} />, action: () => open?.(user) },
              { label: t("Message", "发消息"), icon: <MessageSquare size={17} />, action: () => void perform(() => create({ kind: "direct", contact_id: user.id })) },
            ])}>
            <PersonAvatar user={user} />
            {showHandle ? <span className="contact-identity"><strong>{name}</strong><small>@{user.handle}</small></span> : <strong>{name}</strong>}
            {ios && <ChevronRight size={20} aria-hidden />}
          </button>
        </div>)}
      </div>)}
    </div> : <p className="list-empty muted" role={windows ? "status" : undefined}>{windows ? emptyText
      : query ? t("No matching contacts.", "没有匹配的联系人。") : t("No contacts yet.", "暂无联系人。")}</p>}
    </div>
  </section>;
}

function NewChat({ contacts, create, close, findPeople }: {
  contacts: User[]; create: (request: ConversationRequest) => Promise<void>; close: () => void; findPeople: () => void;
}) {
  const { remarks } = useContext(People);
  const [group, setGroup] = useState(false);
  const [title, setTitle] = useState("");
  const [members, setMembers] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState("");

  async function start(request: ConversationRequest) {
    if (busy) return;
    setBusy(true);
    setFailure("");
    try { await create(request); }
    catch (error) { setFailure(errorMessage(error)); }
    finally { setBusy(false); }
  }

  function submit(event: FormEvent) {
    event.preventDefault();
    void start({ kind: "group", title, contact_ids: members });
  }

  return <Dialog title="New conversation" mobilePage closeDisabled={busy} close={close}>
    {contacts.length === 0 ? <div className="dialog-empty">
      <p>Add a contact to start a conversation.</p>
      <button className="button primary" onClick={findPeople}>Find contacts <ArrowRight size={16} /></button>
    </div> : <>
      <fieldset className="tabs" disabled={busy}>
        <legend className="sr-only">Conversation type</legend>
        <Choice type="radio" name="conversation-type" checked={!group} onChange={() => { setGroup(false); setFailure(""); }}>Direct message</Choice>
        <Choice type="radio" name="conversation-type" checked={group} onChange={() => { setGroup(true); setFailure(""); }}>Group</Choice>
      </fieldset>
      {group ? <form className="group-form" onSubmit={submit}>
        <FloatingField label="Group name" required maxLength={64} value={title} onChange={event => setTitle(event.target.value)} />
        <fieldset className="dialog-contacts"><legend>Choose 2–15 contacts</legend>{contacts.map(user => {
          const checked = members.includes(user.id);
          return <Choice key={user.id} checked={checked} disabled={busy || (!checked && members.length >= 15)}
            onChange={() => setMembers(current => checked ? current.filter(id => id !== user.id) : [...current, user.id])}>
            <PersonAvatar user={user} /><span>{displayName(user, remarks)}</span>
          </Choice>;
        })}</fieldset>
        {failure && <p className="inline-error" role="alert">{failure}</p>}
        <button className="button primary" type="submit" disabled={busy || members.length < 2 || !title.trim()}>Create group</button>
      </form> : <>
        <div className="dialog-contacts">{contacts.map(user => <button className="pick-contact" key={user.id} disabled={busy}
          onClick={() => void start({ kind: "direct", contact_id: user.id })}><PersonAvatar user={user} /> {displayName(user, remarks)}<ArrowRight size={16} /></button>)}</div>
        {failure && <p className="inline-error" role="alert">{failure}</p>}
      </>}
    </>}
  </Dialog>;
}

function SettingsView({ session, privateQuiet, setPrivateQuiet, readReceipts, setReadReceipts, closeAction, setCloseAction, assistantSettings, back, openAssistants }: {
  session: Session;
  closeAction?: CloseAction; setCloseAction: (action: CloseAction) => Promise<void>;
  privateQuiet: boolean; setPrivateQuiet: (quiet: boolean) => Promise<void>;
  readReceipts: boolean; setReadReceipts: (enabled: boolean) => Promise<void>;
  assistantSettings: ReactNode;
  back?: () => void;
  openAssistants: boolean;
}) {
  const { remarks } = useContext(People);
  const sections = [
    { id: "account", label: t("Account and devices", "账号与设备"), Icon: CircleUserRound },
    { id: "general", label: t("General", "通用"), Icon: Settings },
    { id: "assistants", label: t("AI assistants", "AI 助理"), Icon: MessageSquare },
  ] as const;
  type Section = typeof sections[number]["id"];
  type Operation = "preferences" | "devices" | "retire";
  const compact = () => ios || (windows && window.matchMedia("(max-width: 740px)").matches);
  const [section, setSection] = useState<Section>(openAssistants ? "assistants" : "account");
  const [detailOpen, setDetailOpen] = useState(() => openAssistants || !compact());
  const [pending, setPending] = useState<Operation>();
  const [saved, setSaved] = useState(false);
  const [failure, setFailure] = useState<{ operation: Operation; message: string; retry: () => void }>();
  const [deviceList, setDeviceList] = useState<Device[]>();
  const [retiring, setRetiring] = useState<Device>();
  const heading = useRef<HTMLHeadingElement>(null);
  const nav = useRef<HTMLElement>(null);
  const focusAfterNavigation = useRef(false);
  useLayoutEffect(() => {
    if (!focusAfterNavigation.current) return;
    focusAfterNavigation.current = false;
    if (detailOpen) heading.current?.focus();
    else nav.current?.querySelector<HTMLButtonElement>("[aria-current]")?.focus();
  }, [detailOpen, section]);

  async function run(operation: Operation, change: () => Promise<void>) {
    if (pending) return;
    setPending(operation);
    setFailure(undefined);
    setSaved(false);
    try {
      await change();
      if (operation === "preferences") setSaved(true);
    } catch (error) {
      setFailure({ operation, message: errorMessage(error), retry: () => void run(operation, change) });
    } finally { setPending(undefined); }
  }
  function goBack() {
    if (pending) return;
    if (detailOpen) {
      focusAfterNavigation.current = true;
      setDetailOpen(false);
    } else back?.();
  }
  useIosBack(back ? goBack : undefined, 5);
  const sectionLabel = sections.find(item => item.id === section)?.label;
  const settingTitle = t("Settings", "设置");
  const manageDevices = () => run("devices", async () => setDeviceList(await api.devices()));
  const retryError = (operation: Operation) => failure?.operation === operation && <div className="settings-feedback">
    <p className="inline-error" role="alert">{failure.message}</p>
    <button className="button subtle small" type="button" onClick={failure.retry}>{actions.retry}</button>
  </div>;
  return <section className={"settings-page settings-screen" + (detailOpen ? " settings-detail-open" : "")}>
    <header className="workspace-heading settings-heading">
      {(windows || back) && <button className="icon-button settings-back" aria-label={actions.back} onClick={goBack}
        aria-disabled={!!pending}><ChevronLeft size={24} /></button>}
      <h1 ref={heading} tabIndex={-1}>
        <span className="settings-root-title">{settingTitle}</span>
        <span className="settings-section-title">{sectionLabel}</span>
      </h1>
    </header>
    <nav className="settings-nav" ref={nav} aria-label={settingTitle}>
      {sections.map(({ id, label, Icon }) => <button key={id} className="drawer-action" type="button"
        aria-current={id === section ? "page" : undefined} aria-disabled={!!pending} onClick={() => {
          if (pending) return;
          focusAfterNavigation.current = compact() && !detailOpen;
          setSection(id);
          setDetailOpen(true);
        }}>
        <Icon size={20} /><span>{label}</span><ChevronRight className="settings-chevron" size={16} />
      </button>)}
    </nav>
    <div className="settings-detail" key={section}>
      {section === "account" && <section className="settings-group">
        <h2>{t("Account", "账号")}</h2>
        <div className="settings-row settings-identity">
          <PersonAvatar user={session.user} large />
          <div><strong>{displayName(session.user, remarks)}</strong><small>@{session.user.handle}</small></div>
        </div>
        <dl className="settings-row settings-server">
          <dt>{t("Server address", "服务器地址")}</dt><dd>{session.server}</dd>
        </dl>
        <button className="drawer-action settings-row" type="button" aria-haspopup="dialog" aria-disabled={!!pending}
          onClick={() => void manageDevices()}>
          <span>{t("Devices", "设备管理")}</span><ChevronRight size={16} />
        </button>
        {pending === "devices" && <p className="settings-feedback muted" role="status">{t("Loading devices…", "正在加载设备…")}</p>}
        {retryError("devices")}
      </section>}
      {section === "general" && <>
        {windows && <section className="settings-group">
          <h2>{t("Window behavior", "窗口行为")}</h2>
          <div className="settings-row" aria-disabled={!!pending}>
            <CloseActionChoices action={closeAction} choose={action => void run("preferences", () => setCloseAction(action))} />
          </div>
        </section>}
        <section className="settings-group">
          <h2>{t("Chat and privacy", "聊天与隐私")}</h2>
          <label className="choice setting-switch settings-row">
            <span>{t("Read receipts", "消息回执")}<small>{t("Both people must enable receipts to share delivery and read status in direct chats.", "双方都开启后，才会在单聊中共享送达和已读状态。")}</small></span>
            <input type="checkbox" role="switch" checked={readReceipts} aria-disabled={!!pending}
              onChange={event => { const enabled = event.target.checked; void run("preferences", () => setReadReceipts(enabled)); }} />
            <span className="switch-track" aria-hidden="true" />
          </label>
          <label className="choice setting-switch settings-row">
            <span>{t("Pause private invitation pop-ups", "暂停私聊邀请弹窗")}<small>{t("Pending invitations stay in Private chats.", "待处理的邀请仍保留在私密聊天中。")}</small></span>
            <input type="checkbox" role="switch" checked={privateQuiet} aria-disabled={!!pending}
              onChange={event => { const quiet = event.target.checked; void run("preferences", () => setPrivateQuiet(quiet)); }} />
            <span className="switch-track" aria-hidden="true" />
          </label>
        </section>
        <div className="settings-feedback settings-save-status" role="status">
          {pending === "preferences" ? actions.saving : saved ? actions.saved : ""}
        </div>
        {retryError("preferences")}
      </>}
      {section === "assistants" && assistantSettings}
    </div>
    {deviceList && <Dialog title={t("Devices", "设备管理")} mobilePage close={() => {
      setDeviceList(undefined); setRetiring(undefined); setFailure(undefined);
    }} closeDisabled={!!pending}>
      {deviceList.map(device => <div className="drawer-action" key={device.id}>
        <span>{device.current ? "This device" : "Device " + device.id.slice(0, 8)} · {dateTimeLabel(device.last_seen_at)}</span>
        {!device.current && <button className="button subtle small" disabled={!!pending} onClick={() => {
          setRetiring(device); setFailure(undefined);
        }}>Retire</button>}
      </div>)}
      {retiring && <div className="file-download-actions">
        <p>End sign-in for device {retiring.id.slice(0, 8)}?</p>
        <button className="button subtle" onClick={() => { setRetiring(undefined); setFailure(undefined); }} disabled={!!pending}>Cancel</button>
        <button className="button danger" disabled={!!pending} onClick={() => void run("retire", async () => {
          await api.retireDevice(retiring.id);
          setDeviceList(current => current?.filter(device => device.id !== retiring.id));
          setRetiring(undefined);
        })}>Retire device</button>
      </div>}
      {failure?.operation === "retire" && <p className="inline-error" role="alert">{failure.message}</p>}
    </Dialog>}
  </section>;
}
