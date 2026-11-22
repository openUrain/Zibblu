import { FloatingField } from "./floating-field";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import {
  ArrowRight, CircleUserRound, FolderOpen, Info, LogOut, Menu, MessageCircle, MessageSquare,
  Radar, Search, Settings, Shield, UserRoundPlus, UsersRound, X,
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
import { QrContent, ScanQr, ShareQr } from "./qr";
import type { SharedCode } from "./qr";
import { ProfileView } from "./profile";
import { useMenu, useMenuAnchor, useRowMenu } from "./context-menu";
import { CloseActionChoices, WindowControls, WindowDrag, windows } from "./window-chrome";
import { ios, phone, useIpadSplit, useIosBack } from "./ios";
import { WorkspaceSearchField } from "./workspace-search";
import { ChatHistoryCache } from "./chat-history-cache";
import { ImageCache, ProfileCache } from "./workspace-cache";
import { WorkspaceImages } from "./workspace-image";

const pages = [
  { name: "Chats", Icon: MessageSquare }, { name: "Contacts", Icon: UsersRound },
  { name: "Files", Icon: FolderOpen },
] as const;
type Navigation = typeof pages[number]["name"] | "Me" | "Settings";
const tabLabels = { Chats: t("Chats", "聊天"), Contacts: t("Contacts", "联系人"), Files: t("Files", "文件"), Me: t("Me", "我的") };
const hour = 60 * 60 * 1000;
const currentHour = () => Math.floor(Date.now() / hour) * hour;

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
  const [historyCache] = useState(() => new ChatHistoryCache());
  const [profiles] = useState(() => new ProfileCache());
  const [images] = useState(() => new ImageCache());
  useLayoutEffect(() => {
    historyCache.retain(new Set(directory.conversations.map(chat => chat.id)));
  }, [directory.conversations, historyCache]);
  const [recalled, setRecalled] = useState<ReadonlySet<number>>(new Set());
  const [hidden, setHidden] = useState<ReadonlySet<number>>(new Set());
  const [drafts, setDrafts] = useState<Record<string, Draft>>({});
  const [profile, setProfile] = useState<User>();
  const [selectedContact, setSelectedContact] = useState<{ id: string; narrowDetail: boolean }>();
  const [quickProfile, setQuickProfile] = useState<{ user: User; point: { x: number; y: number }; fromContacts?: boolean }>();
  const profilePoint = useRef({ x: 0, y: 0 });
  const profileTrigger = useRef<HTMLElement>(null);
  const refreshInFlight = useRef<Promise<void> | null>(null);
  const syncRetryDelay = useRef(0);
  const appliedRevision = useRef(0);
  const [clockOffset, setClockOffset] = useState(0);
  const [avatarValidated, setAvatarValidated] = useState(currentHour);
  const [selected, setSelected] = useState<string>();
  const [chatPage, setChatPage] = useState<{ id: string; page: "details" | "search" }>();
  const [mobileChat, setMobileChat] = useState(false);
  const [privateView, setPrivateView] = useState<{ id: string; conversation?: string | null }>();
  const privateId = privateView?.id;
  const privateSelection = useRef(0);
  const [privateTargets, setPrivateTargets] = useState<Conversation>();
  const [nearby, setNearby] = useState(false);
  const [shareCode, setShareCode] = useState<SharedCode>();
  const [scanning, setScanning] = useState(false);
  const { rooms: privateRooms, prompt } = usePrivateRooms();
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
  const previewAccess = useMemo(() => {
    const conversations = new Map(directory.conversations.map(chat => [chat.id, chat.joined_after ?? null]));
    return (id: string, seq: number) => {
      const cutoff = conversations.get(id);
      if (cutoff === undefined || recalled.has(seq) || hidden.has(seq)) return;
      if (cutoff === null) return "online";
      if (seq > cutoff) return "cached";
    };
  }, [directory.conversations, recalled, hidden]);

  useLayoutEffect(() => {
    const people = new Set(directory.contacts.map(user => user.id));
    profiles.retain(key => {
      const [id, relation] = key.split("/");
      return relation === "own" ? id === session.user.id : people.has(id);
    });
    images.retain(key => {
      const [type, id, version] = key.split("/");
      if (type === "avatar") return directory.avatars[id] === version;
      if (type === "cover") return (id === session.user.id || people.has(id))
        && profiles.get(id + (id === session.user.id ? "/own" : "/contact"))?.profile.cover === version;
      return previewAccess(id, Number(version)) === "cached";
    });
  }, [directory.contacts, directory.avatars, previewAccess, images, profiles, session.user.id]);
  useLayoutEffect(() => () => {
    profiles.retain(() => false);
    images.retain(() => false);
  }, [profiles, images]);

  useEffect(() => {
    if (!ios) return;
    function clearPrivateView() {
      privateSelection.current += 1;
      setPrivateView(undefined);
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
    if (windows) {
      setQuickProfile({ user, point: profilePoint.current });
      return;
    }
    openDetails(user);
  }

  const directoryContact = directory.contacts.find(person => person.id === selectedContact?.id);
  const invalidateContact = useCallback((id: string) => {
    profiles.retain(key => !key.startsWith(id + "/"));
    images.retain(key => !key.startsWith("cover/" + id + "/"));
    setSelectedContact(current => current?.id === id ? undefined : current);
    setQuickProfile(current => current?.fromContacts && current.user.id === id ? undefined : current);
  }, [profiles, images]);
  useEffect(() => {
    if (selectedContact && !directoryContact) invalidateContact(selectedContact.id);
  }, [selectedContact, directoryContact, invalidateContact]);

  function openContactCard(user: User) {
    setSelectedContact({ id: user.id, narrowDetail: false });
    setQuickProfile({ user, point: profilePoint.current, fromContacts: true });
  }

  function openDetails(user: User) {
    if (windows && nav === "Contacts") {
      setSelectedContact({ id: user.id, narrowDetail: true });
      requestAnimationFrame(() => {
        document.querySelector<HTMLButtonElement>(
          `.contacts-layout > [data-contact-detail="${CSS.escape(user.id)}"] .profile-header button`,
        )?.focus({ preventScroll: true });
      });
      return;
    }
    if (document.activeElement instanceof HTMLElement) profileTrigger.current = document.activeElement;
    setProfile(user);
  }

  function closeProfile() {
    const trigger = profileTrigger.current;
    setProfile(undefined);
    requestAnimationFrame(() => {
      if (trigger?.isConnected) trigger.focus({ preventScroll: true });
    });
  }

  function closeContactDetails(id: string) {
    const narrow = window.matchMedia("(max-width: 740px)").matches;
    setSelectedContact(current => {
      if (current?.id !== id) return current;
      return narrow ? { id, narrowDetail: false } : undefined;
    });
    requestAnimationFrame(() => {
      const row = document.querySelector<HTMLButtonElement>(`.contacts-screen [data-contact-id="${CSS.escape(id)}"]`);
      const list = row?.closest(".contacts-list");
      if (row && list) {
        const rowBounds = row.getBoundingClientRect();
        const listBounds = list.getBoundingClientRect();
        const stickyHeader = Number.parseFloat(getComputedStyle(list).scrollPaddingBlockStart) || 0;
        if (rowBounds.top >= listBounds.top + stickyHeader && rowBounds.bottom <= listBounds.bottom) {
          row.focus({ preventScroll: true });
          return;
        }
      }
      document.querySelector<HTMLInputElement>('.contacts-screen input[type="search"]')?.focus({ preventScroll: true });
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
    setDirectory(previous => mergeDirectory(previous, update.directory));
    const offset = update.server_now - Date.now();
    setClockOffset(previous => Math.abs(previous - offset) < 250 ? previous : offset);
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
        setAvatarValidated(currentHour());
        syncRetryDelay.current = 0;
        setConnection("Connected");
        setSyncProblem("");
      } catch (error) {
        if (isExpired(error)) {
          if (!signingOut.current) {
            signingOut.current = true;
            signedOut(errorMessage(error));
          }
        } else {
          if (windows) syncRetryDelay.current = Math.min(syncRetryDelay.current * 2 || 1700, 6800);
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
    return () => { void updated.then(unlisten => unlisten()); };
  }, [refreshLocal]);

  const changed = useCallback(() => {
    void api.transfers().then(setTransfers).catch(error => setFailure(errorMessage(error)));
  }, []);

  useEffect(() => {
    let closed = false;
    let polling = false;
    let poll: number | undefined;
    async function syncLoop() {
      if (closed || polling) return;
      polling = true;
      await refresh();
      polling = false;
      const retry = syncRetryDelay.current;
      if (!closed) poll = window.setTimeout(syncLoop, retry ? retry + Math.random() * 500 : 1700);
    }
    function online() {
      if (!windows) {
        void refresh();
        return;
      }
      window.clearTimeout(poll);
      void syncLoop();
    }
    window.addEventListener("online", online);
    void syncLoop();
    return () => {
      closed = true;
      window.clearTimeout(poll);
      window.removeEventListener("online", online);
    };
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
  const pairedPrivate = !!active && privateView?.conversation === active.id;
  const availablePrivate = active && privateRooms.find(room => room.conversation === active.id);
  const activeId = nav === "Chats" && !privateOpen && !profile && !quickProfile ? active?.id : undefined;
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
    privateSelection.current += 1;
    setProfile(undefined);
    setOpenAssistants(assistants);
    setPrivateView(undefined);
    setNav(page);
    if (page === "Chats") setMobileChat(false);
  }

  function selectChat(id: string, page?: "details" | "search") {
    if (privateView?.conversation === id && !page) {
      setMobileChat(true);
      return;
    }
    privateSelection.current += 1;
    setProfile(undefined);
    setPrivateView(undefined);
    setSelected(id);
    setChatPage(page ? { id, page } : undefined);
    setNav("Chats");
    setMobileChat(true);
  }

  const showPrivate = useCallback((id: string, conversation?: string | null) => {
    privateSelection.current += 1;
    if (conversation) setSelected(conversation);
    setOffer(undefined);
    setProfile(undefined);
    setChatPage(undefined);
    setNav("Chats");
    setPrivateView({ id, conversation });
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
      setQuickProfile(undefined);
      showPrivate(payload.id, payload.conversation);
    });
    return () => { void accepted.then(unlisten => unlisten()); };
  }, [showPrivate]);

  function selectPrivate(id: string) {
    showPrivate(id, privateRooms.find(room => room.id === id)?.conversation);
  }

  async function startPrivate(chat: Conversation, invited: string[]) {
    const selection = privateSelection.current;
    const id = await api.startPrivate(chat.id, invited);
    setPrivateTargets(undefined);
    if (selection === privateSelection.current) showPrivate(id, chat.id);
  }

  async function invitePrivate(chat: Conversation) {
    const selection = privateSelection.current;
    try {
      const { rooms } = await api.privateRooms();
      if (selection !== privateSelection.current) return;
      const existing = rooms.find(room => room.conversation === chat.id
        && room.phase !== "ended" && room.phase !== "missed" && room.phase !== "failed");
      if (existing) showPrivate(existing.id, chat.id);
      else if (chat.title) setPrivateTargets(chat);
      else await startPrivate(chat, chat.members.filter(person => person.id !== session.user.id).map(person => person.id));
    } catch (error) {
      setFailure(errorMessage(error));
    }
  }

  async function showPrivateCode(id: string) {
    const selection = privateSelection.current;
    const code = await api.privateCode(id);
    if (selection !== privateSelection.current) return;
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
    await refreshLocal();
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
  const offeredChat = directory.conversations.find(chat => chat.id === offer?.conversation);
  const connected = connection === "Connected";
  const invitation = privateRooms.find(room => room.id === prompt && room.id !== privateId);
  const attention: Partial<Record<Navigation, string>> = {
    Chats: directory.conversations.some(chat => chat.unread > 0) ? "Unread messages" : undefined,
    Contacts: directory.pending_contacts.some(request => request.incoming) ? "Pending contact requests" : undefined,
    Files: transfers.some(transfer => transfer.status !== "complete" && !mediaKind(transfer.attachment)) ? "Unfinished transfers" : undefined,
  };

  const conversationOpen = nav === "Chats" && mobileChat && (!!active || privateOpen);
  function backFromChat() {
    privateSelection.current += 1;
    if (privateOpen) setPrivateView(undefined);
    setMobileChat(false);
  }
  function returnToNormal() {
    privateSelection.current += 1;
    setPrivateView(undefined);
    setMobileChat(pairedPrivate);
  }
  function privateClosed(id: string, selection: number) {
    setDraft("private/" + id, () => ({ text: "" }));
    if (selection !== privateSelection.current) return;
    setShareCode(code => code?.kind === "private" ? undefined : code);
    returnToNormal();
  }
  useIosBack(conversationOpen && !ipadSplit ? backFromChat : undefined);
  const splitProfile = ipadSplit && (nav === "Contacts" || nav === "Me");
  const desktopContacts = !ios && nav === "Contacts";
  const chatProfile = nav === "Chats";
  const profileVersion = (id: string) => JSON.stringify([directory.names[id], directory.remarks[id], directory.avatars[id]]);
  const profilePage = profile && <ProfileView key={profile.id} user={profile} session={session}
    cache={profiles} directoryVersion={profileVersion(profile.id)}
    reconnecting={connection === "Reconnecting"}
    relationVersion={directory.contacts.some(person => person.id === profile.id) ? "contact"
      : directory.pending_contacts.find(request => request.user.id === profile.id)?.request_id ?? "other"}
    refresh={refresh} close={closeProfile} edited={profileEdited}
    message={() => create({ kind: "direct", contact_id: profile.id })} />;
  const viewSelection = privateSelection.current;
  const imageScope = useMemo(() => windows ? { cache: images, clockOffset, previewAccess,
    avatarExpires: avatarValidated + 24 * hour } : undefined, [images, clockOffset, previewAccess, avatarValidated]);
  return <WorkspaceImages.Provider value={imageScope}>
    <People.Provider value={{ remarks: names, avatars: directory.avatars, online: connected, open: openProfile }}>
  <main onClickCapture={event => {
    const target = (event.target as Element).closest("button")?.getBoundingClientRect();
    profilePoint.current = event.detail ? { x: event.clientX, y: event.clientY }
      : { x: target?.right ?? 0, y: target?.top ?? 0 };
  }} className={"shell" + (conversationOpen ? " conversation-open" : "") + (profile ? " profile-open" : "")
    + (ipadSplit ? " ipad-split" : "")}>
    <nav className="rail" aria-label="Main navigation">
      <button className="rail-brand icon-button" aria-label={t("Your profile", "我的资料")} title={t("Your profile", "我的资料")}
        aria-haspopup={windows ? "dialog" : undefined} aria-expanded={windows ? quickProfile?.user.id === session.user.id : undefined}
        onClick={() => openProfile(session.user)}><PersonAvatar user={session.user} /></button>
      <div className="rail-links">{pages.map(({ name, Icon }) =>
        <button key={name} aria-label={tabLabels[name]} title={tabLabels[name]}
          aria-describedby={attention[name] ? `rail-status-${name}` : undefined}
          aria-current={nav === name ? "page" : undefined} onClick={() => navigate(name)}>
          <span className="rail-icon">{nav !== name ? <Icon size={22} strokeWidth={1.9} />
            : windows ? <WindowsActiveRailIcon name={name} /> : <ActiveRailIcon name={name} />}
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
        <WindowDrag className="app-name">Zibblu</WindowDrag>
        <div className="chat-actions">
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
        + (desktopContacts && directoryContact && selectedContact?.narrowDetail ? " contact-detail-open" : "")
        + (profile && !splitProfile && !chatProfile && !desktopContacts ? " profile-replaces-root" : "")}>
      {nav === "Chats" && <div className={"chat-layout" + (mobileChat && (active || privateOpen) ? " mobile-chat-open" : "")
        + (profile && chatProfile ? " chat-profile-open" : "")}>
        <ChatList chats={directory.conversations} own={session.user} selected={privateOpen && !pairedPrivate ? undefined : selected}
          open={selectChat} newChat={() => setNewChat(true)} addContact={() => setNewContact(true)}
          rooms={privateRooms} activePrivate={privateId}
          openPrivate={selectPrivate} createQr={() => void createPrivateCode()}
          scan={() => setScanning(true)} openNearby={() => setNearby(true)} />
        {privateId ? <PrivateChatView key={privateId} id={privateId} own={session.user}
            isContact={id => directory.contacts.some(person => person.id === id)}
            draft={drafts["private/" + privateId]?.text ?? ""}
            setDraft={change => setDraft("private/" + privateId, previous => ({ text: change(previous.text) }))}
            back={backFromChat} showCode={showPrivateCode} onClosed={() => privateClosed(privateId, viewSelection)}
            switchMode={pairedPrivate ? returnToNormal : undefined} />
          : active ? <ChatView key={active.id + ":" + (active.joined_after ?? "")} chat={active}
            own={session.user} server={session.server} revision={revision} profileOpen={!!profile} historyCache={historyCache}
            privateRoom={availablePrivate}
            visibleOnNarrow={mobileChat}
            requested={chatPage?.id === active.id ? chatPage : undefined}
            latestAiSeq={latestAiSeq} recalled={recalled} clockOffset={clockOffset}
            hidden={hidden} hide={seq => hide(active.id, seq)} readReceipts={directory.read_receipts && connected && !profile && !quickProfile}
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
      {(windows || nav === "Contacts") && <Contacts active={nav === "Contacts"} details={openDetails} directory={directory}
        selected={windows ? directoryContact?.id : profile?.id} refresh={refresh} create={create}
        openAvatar={openContactCard} addContact={() => setNewContact(true)} />}
      {nav === "Files" && <FilesView revision={revision} recalled={recalled} hidden={hidden} transfers={transfers} onChanged={changed} />}
      {nav === "Me" && <section className="settings-page phone-me">
        <header className="workspace-heading"><h1>{tabLabels.Me}</h1></header>
        <button className="phone-profile drawer-action" onClick={() => openProfile(session.user)}>
          <PersonAvatar user={session.user} large />
          <span><strong>{displayName(session.user, names)}</strong><small className="muted">@{session.user.handle}</small></span><ChevronRight size={20} />
        </button>
        <div className="phone-me-actions">
          <button className="drawer-action" onClick={() => setNearby(true)}><Radar size={19} /> Nearby <ChevronRight size={17} /></button>
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
      {windows && directoryContact && <ProfileView key={directoryContact.id} active={desktopContacts}
        cache={profiles} directoryVersion={profileVersion(directoryContact.id)}
        user={directoryContact} session={session} relationVersion="contact" reconnecting={connection === "Reconnecting"}
        close={() => closeContactDetails(directoryContact.id)} refresh={refresh} edited={profileEdited}
        onContactUnavailable={invalidateContact} message={() => create({ kind: "direct", contact_id: directoryContact.id })} />}
      {desktopContacts && windows && !directoryContact && <section className="chat-blank contact-detail-empty">
        {(directory.contacts.length > 0 || directory.pending_contacts.length > 0)
          && <p>{t("Select a contact", "选择联系人")}</p>}
      </section>}
      {(splitProfile || (desktopContacts && !windows)) && !profile && <section className="chat-blank">
        <Blank icon={<UserRound size={28} />} title={nav === "Me" ? t("Personal information", "个人信息") : t("Select a contact", "选择联系人")} />
      </section>}
      </div>
    </div>
    {quickProfile && (!quickProfile.fromContacts || directoryContact?.id === quickProfile.user.id)
      && <ProfileView key={quickProfile.user.id} popup point={quickProfile.point} user={quickProfile.user} session={session}
      cache={profiles} directoryVersion={profileVersion(quickProfile.user.id)}
      relationVersion={directory.contacts.some(person => person.id === quickProfile.user.id) ? "contact"
        : directory.pending_contacts.find(request => request.user.id === quickProfile.user.id)?.request_id ?? "other"}
      reconnecting={connection === "Reconnecting"} refresh={refresh} close={() => setQuickProfile(undefined)}
      edited={profileEdited} onContactUnavailable={quickProfile.fromContacts ? invalidateContact : undefined} />}
    {about && <About close={() => setAbout(false)} />}
    {privateTargets && <PrivateTargets people={privateTargets.members.filter(user => user.id !== session.user.id)}
      close={() => setPrivateTargets(undefined)} start={invited => startPrivate(privateTargets, invited)} />}
    {nearby && <NearbyDevices own={session.user.id} close={() => setNearby(false)} invited={id => {
      setNearby(false);
      void api.privateRooms().then(({ rooms }) => {
        showPrivate(id, rooms.find(room => room.id === id)?.conversation);
      }).catch(() => showPrivate(id));
    }} />}
    {shareCode && <ShareQr code={shareCode} server={session.server} close={() => setShareCode(undefined)} />}
    {scanning && <ScanQr session={session} close={() => setScanning(false)}
      contactFound={user => { setScanning(false); openProfile(user); }}
      groupJoined={id => { setScanning(false); selectChat(id); void refresh(); }}
      privateJoined={id => { setScanning(false); selectPrivate(id); }} />}
    {newChat && <NewChat contacts={directory.contacts} create={create} close={() => setNewChat(false)} findPeople={() => {
      setNewChat(false); navigate("Contacts"); setNewContact(true);
    }} />}
    {newContact && <AddContact session={session} close={() => setNewContact(false)} open={user => {
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
  </main></People.Provider></WorkspaceImages.Provider>;
}

function mergeDirectory(previous: Directory, next: Directory): Directory {
  const keep = <T,>(old: T, value: T) => JSON.stringify(old) === JSON.stringify(value) ? old : value;
  const contacts = keep(previous.contacts, next.contacts);
  const conversations = keep(previous.conversations, next.conversations);
  const pending_contacts = keep(previous.pending_contacts, next.pending_contacts);
  const remarks = keep(previous.remarks, next.remarks);
  const names = keep(previous.names, next.names);
  const avatars = keep(previous.avatars, next.avatars);
  return contacts === previous.contacts && conversations === previous.conversations
    && pending_contacts === previous.pending_contacts && remarks === previous.remarks
    && names === previous.names && avatars === previous.avatars && next.read_receipts === previous.read_receipts
    ? previous : { ...next, contacts, conversations, pending_contacts, remarks, names, avatars };
}

function WindowsActiveRailIcon({ name }: { name: typeof pages[number]["name"] }) {
  if (name === "Chats") return <MessageSquare size={22} strokeWidth={1.9} fill="currentColor" />;
  if (name === "Contacts") return <UsersRound size={22} strokeWidth={1.9} className="rail-active-contacts" />;
  return <svg width="22" height="22" viewBox="0 0 24 24" fill="currentColor" stroke="currentColor"
    strokeWidth={1.9} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <path d="M20 10a2 2 0 0 1 1.94 2.5l-1.54 6a2 2 0 0 1-1.95 1.5H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H18a2 2 0 0 1 2 2v2Z" />
    <path className="rail-icon-detail" fill="none" d="m6 14 1.5-2.9A2 2 0 0 1 9.24 10H19" />
  </svg>;
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

function AddContact({ session, close, open }: { session: Session; close: () => void; open: (user: User) => void }) {
  const { remarks } = useContext(People);
  const [handle, setHandle] = useState("");
  const [found, setFound] = useState<User>();
  const [failure, setFailure] = useState("");
  const [busy, setBusy] = useState(false);

  return <Dialog className="add-friend" title={t("Add friend", "添加朋友")} mobilePage close={close} closeDisabled={busy}>
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
      {phone && <QrContent code={{ kind: "contact", name: t("My QR code", "我的二维码"), value: session.user.handle }} server={session.server} />}
      {found && <button className="drawer-action search-result" onClick={() => open(found)}>
        <PersonAvatar user={found} /><span>{displayName(found, remarks)} <small>@{found.handle}</small></span><ChevronRight size={17} />
      </button>}
      {failure && <p className="inline-error" role="alert">{failure}</p>}
    </div>
  </Dialog>;
}

function Contacts({ active, directory, selected, refresh, create, addContact, details, openAvatar }: {
  active: boolean; openAvatar: (user: User) => void;
  details: (user: User) => void;
  directory: Directory; selected?: string; refresh: () => Promise<void>; create: (request: ConversationRequest) => Promise<void>;
  addContact: () => void;
}) {
  const { contacts, pending_contacts } = directory;
  const { remarks, open } = useContext(People);
  const menu = useRowMenu();
  const [search, setSearch] = useState("");
  const list = useRef<HTMLDivElement>(null);
  const scrollTop = useRef(0);
  const [failure, setFailure] = useState("");
  const [busy, setBusy] = useState(false);

  useLayoutEffect(() => {
    if (windows && active && list.current) list.current.scrollTop = scrollTop.current;
  }, [active]);

  async function perform(action: () => Promise<unknown>) {
    if (busy) return;
    setBusy(true);
    setFailure("");
    try { await action(); }
    catch (error) { setFailure(errorMessage(error)); }
    finally { setBusy(false); }
  }

  const windowsContacts = useMemo(() => active && windows ? orderContacts(contacts, remarks) : [], [active, contacts, remarks]);
  if (!active) return null;
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
      if (query && filtered.length === 1) details(filtered[0].user);
    }}>
      <WorkspaceSearchField label={t("Search contacts", "搜索联系人")} otherIconSize={17}
        value={search} onChange={event => {
          if (windows) {
            scrollTop.current = 0;
            if (list.current) list.current.scrollTop = 0;
          }
          setSearch(event.currentTarget.value);
        }} />
      <button type="button" className="icon-button new-conversation" title={t("Add contact", "添加联系人")}
        aria-label={t("Add contact", "添加联系人")} aria-haspopup={windows ? "dialog" : undefined}
        onClick={addContact}><UserRoundPlus size={windows ? 20 : 21} /></button>
    </form>
    <div className="contacts-list" ref={list}
      onScroll={windows ? event => { scrollTop.current = event.currentTarget.scrollTop; } : undefined}>
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
          {windows && <button className="contact-avatar icon-button" type="button" aria-haspopup="dialog" onClick={() => openAvatar(user)}
            aria-label={t("View profile", "查看资料") + " " + name}><PersonAvatar user={user} /></button>}
          <button className="person-link contact-name" aria-current={selected === user.id || undefined}
            data-contact-id={windows ? user.id : undefined} onClick={() => details(user)}
            {...menu([
              { label: t("View profile", "查看资料"), icon: <UserRound size={17} />, action: () => details(user) },
              { label: t("Message", "发消息"), icon: <MessageSquare size={17} />, action: () => void perform(() => create({ kind: "direct", contact_id: user.id })) },
            ])}>
            {!windows && <PersonAvatar user={user} />}
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
  const [group, setGroup] = useState(true);
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
    ...(windows ? [{ id: "general" as const, label: t("General", "通用"), Icon: Settings }] : []),
    { id: "chat", label: t("Chat", "聊天"), Icon: MessageCircle },
    { id: "privacy", label: t("Privacy", "隐私"), Icon: Shield },
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
          setSaved(false);
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
      {section === "general" && <section className="settings-group">
          <h2>{t("Window behavior", "窗口行为")}</h2>
          <div className="settings-row" aria-disabled={!!pending}>
            <CloseActionChoices action={closeAction} choose={action => void run("preferences", () => setCloseAction(action))} />
          </div>
        </section>}
      {section === "chat" && <section className="settings-group">
          <h2>{sectionLabel}</h2>
          <label className="choice setting-switch settings-row">
            <span>{t("Read receipts", "消息回执")}<small>{t("Both people must enable receipts to share delivery and read status in direct chats.", "双方都开启后，才会在单聊中共享送达和已读状态。")}</small></span>
            <input type="checkbox" role="switch" checked={readReceipts} aria-disabled={!!pending}
              onChange={event => { const enabled = event.target.checked; void run("preferences", () => setReadReceipts(enabled)); }} />
            <span className="switch-track" aria-hidden="true" />
          </label>
        </section>}
      {section === "privacy" && <section className="settings-group">
          <h2>{sectionLabel}</h2>
          <label className="choice setting-switch settings-row">
            <span>{t("Pause private invitation pop-ups", "暂停私聊邀请弹窗")}<small>{t("Review pending invitations from the chat’s ghost button or the Private chats list.", "可通过会话的幽灵按钮或私密聊天列表查看待处理邀请。")}</small></span>
            <input type="checkbox" role="switch" checked={privateQuiet} aria-disabled={!!pending}
              onChange={event => { const quiet = event.target.checked; void run("preferences", () => setPrivateQuiet(quiet)); }} />
            <span className="switch-track" aria-hidden="true" />
          </label>
        </section>}
      {["general", "chat", "privacy"].includes(section) && <>
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
