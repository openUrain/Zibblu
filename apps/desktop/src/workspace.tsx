import { listen } from "@tauri-apps/api/event";
import {
  ArrowRight, CircleUserRound, FolderOpen, LogOut, MessageCircle, MessageSquare,
  Radar, ScanLine, Search, Settings, UserRoundMinus, UsersRound, X,
} from "lucide-react";
import { useCallback, useContext, useEffect, useRef, useState } from "react";
import type { FormEvent, ReactNode } from "react";
import { ai } from "./ai-api";
import type { AiReply, Assistant } from "./ai-api";
import { AssistantSettings } from "./ai";
import { api, errorMessage, isExpired } from "./api";
import type { Conversation, ConversationRequest, Directory, Draft, DropOffer, Session, ShareSource, Transfer, User } from "./api";
import { Blank, Brand, Choice, Dialog, People, PersonAvatar, chatName, displayName } from "./common";
import { ChatList, ChatView } from "./chat";
import { FilesView } from "./files";
import { NearbyDevices, PrivateChatView, PrivateInvitationOverlay, PrivateTargets, usePrivateRooms } from "./private-chat";
import { ScanQr, ShareQr } from "./qr";
import type { SharedCode } from "./qr";
import { ProfileView } from "./profile";

const pages = [
  { name: "Chats", Icon: MessageSquare }, { name: "Contacts", Icon: UsersRound },
  { name: "Files", Icon: FolderOpen }, { name: "Settings", Icon: Settings },
] as const;
type Navigation = typeof pages[number]["name"];

export function Workspace({ session, privateQuiet, setPrivateQuiet, signedOut }: {
  session: Session; signedOut: (notice?: string) => void;
  privateQuiet: boolean; setPrivateQuiet: (quiet: boolean) => Promise<void>;
}) {
  const [nav, setNav] = useState<Navigation>("Chats");
  const [assistants, setAssistants] = useState<Assistant[]>([]);
  const [assistantFailure, setAssistantFailure] = useState("");
  const [aiReplies, setAiReplies] = useState<AiReply[]>([]);
  const [directory, setDirectory] = useState<Directory>({ contacts: [], conversations: [], pending_contacts: [], remarks: {}, avatars: {}, read_receipts: false });
  const [recalled, setRecalled] = useState<ReadonlySet<number>>(new Set());
  const [hidden, setHidden] = useState<ReadonlySet<number>>(new Set());
  const [drafts, setDrafts] = useState<Record<string, Draft>>({});
  const [profile, setProfile] = useState<User>();
  const [profileNotice, setProfileNotice] = useState(0);
  const refreshInFlight = useRef<Promise<void> | null>(null);
  const [clockOffset, setClockOffset] = useState(0);
  const [selected, setSelected] = useState<string>();
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
  const [connection, setConnection] = useState<"Connecting" | "Connected" | "Reconnecting" | "Session expired">("Connecting");
  const [transfers, setTransfers] = useState<Transfer[]>([]);
  const [failure, setFailure] = useState("");
  const [newChat, setNewChat] = useState(false);
  const [offer, setOffer] = useState<DropOffer & { conversation: string }>();
  const [sharing, setSharing] = useState(false);
  const [leaving, setLeaving] = useState(false);

  useEffect(() => {
    if (!profileNotice) return;
    const timer = window.setTimeout(() => setProfileNotice(0), 3000);
    return () => window.clearTimeout(timer);
  }, [profileNotice]);

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

  const refresh = useCallback(() => {
    if (refreshInFlight.current) return refreshInFlight.current;
    const sync = async () => {
      try {
        const update = await api.sync();
        setDirectory(update.directory);
        setClockOffset(update.server_now - Date.now());
        setRecalled(previous => previous.size === update.recalled.length
          ? previous : new Set(update.recalled));
        setConnection("Connected");
        setRevision(update.last_seq);
        setLatestAiSeq(update.latest_ai_seq);
      } catch (error) {
        setConnection(isExpired(error) ? "Session expired" : "Reconnecting");
      } finally {
        refreshInFlight.current = null;
      }
    };
    refreshInFlight.current = sync();
    return refreshInFlight.current;
  }, []);

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
    api.directory().then(cached => { if (!closed) setDirectory(cached); })
      .catch(error => { if (!closed) setFailure(errorMessage(error)); }).finally(() => void syncLoop());
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
  const activeId = nav === "Chats" && !privateOpen ? active?.id : undefined;
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

  function navigate(page: Navigation) {
    setPrivateId(undefined);
    setNav(page);
    if (page === "Chats") setMobileChat(false);
  }

  function selectChat(id: string) {
    setPrivateId(undefined);
    setSelected(id);
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
      setProfile(undefined);
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

  async function showGroupCode(chat: Conversation) {
    try {
      const invite = await api.groupInvite(chat.id);
      setShareCode({ kind: "group", name: chat.title ?? "Group invite", value: invite.code, expiresAt: invite.expires_at });
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
    try { await api.share(conversation, source); changed(); }
    catch (error) { setFailure(errorMessage(error)); }
  }

  async function signOut() {
    if (leaving) return;
    setLeaving(true);
    try { await api.logout(); signedOut(); }
    catch (error) { signedOut(isExpired(error) ? undefined : "Signed out locally. The service could not revoke this session."); }
  }

  const people = [...new Map([session.user, ...directory.conversations.flatMap(chat => chat.members)]
    .map(user => [user.id, user])).values()].sort((a, b) => displayName(a, directory.remarks).localeCompare(displayName(b, directory.remarks)));
  const offeredChat = directory.conversations.find(chat => chat.id === offer?.conversation);
  const connected = connection === "Connected";
  const invitation = privateRooms.find(room => room.id === prompt);
  const attention: Partial<Record<Navigation, string>> = {
    Chats: directory.conversations.some(chat => chat.unread > 0) ? "Unread messages" : undefined,
    Contacts: directory.pending_contacts.some(request => request.incoming) ? "Pending contact requests" : undefined,
    Files: transfers.some(transfer => transfer.status !== "complete") ? "Unfinished transfers" : undefined,
  };

  return <People.Provider value={{ remarks: directory.remarks, avatars: directory.avatars, open: setProfile }}><main className="shell">
    <nav className="rail" aria-label="Main navigation">
      <div className="rail-brand"><Brand /></div>
      <div className="rail-links">{pages.map(({ name, Icon }) =>
        <button key={name} className={nav === name ? "nav-active" : ""} aria-label={name}
          aria-describedby={attention[name] ? `rail-status-${name}` : undefined}
          aria-current={nav === name ? "page" : undefined} onClick={() => navigate(name)}>
          <span className="rail-icon"><Icon size={22} strokeWidth={1.9} />
            {attention[name] && <span className="rail-dot" aria-hidden="true" />}
          </span><span>{name}</span>
        </button>)}</div>
      <div className="sr-only" aria-live="polite" aria-atomic="true">
        {pages.map(({ name }) => <span key={name} id={`rail-status-${name}`}>{attention[name] && `${attention[name]}. `}</span>)}
      </div>
      <button className="rail-account" title={"@" + session.user.handle} aria-label="Your profile" onClick={() => setProfile(session.user)}>
        <PersonAvatar user={session.user} />
      </button>
    </nav>
    <div className="shell-main">
      <header className="topbar"><span className="app-name">Zibblu</span><div className="chat-actions">
        <button className="icon-button" aria-label="Nearby devices" title="Nearby devices"
          onClick={() => setNearby(true)}><Radar size={20} /></button>
        <button className="icon-button" aria-label="Scan QR code" title="Scan QR code"
          onClick={() => setScanning(true)}><ScanLine size={20} /></button>
        <div className={"connection " + (connected ? "connected" : "disconnected")} role="status">
          <span className="connection-dot" />{connection}
          {connection === "Session expired" && <button className="text-button" disabled={leaving} onClick={() => void signOut()}>Sign in again</button>}
        </div></div>
      </header>
      {failure && <div className="workspace-error" role="alert">{failure}
        <button className="icon-button" aria-label="Dismiss error" onClick={() => setFailure("")}><X size={17} /></button>
      </div>}
      {nav === "Chats" && <div className={"chat-layout" + (mobileChat && (active || privateOpen) ? " mobile-chat-open" : "")}>
        <ChatList chats={directory.conversations} own={session.user} selected={privateOpen ? undefined : selected}
          open={selectChat} newChat={() => setNewChat(true)} rooms={privateRooms} activePrivate={privateId}
          openPrivate={selectPrivate} createQr={() => void createPrivateCode()} />
        {privateId ? <PrivateChatView key={privateId} id={privateId} own={session.user}
            isContact={id => directory.contacts.some(person => person.id === id)}
            draft={drafts["private/" + privateId]?.text ?? ""}
            setDraft={change => setDraft("private/" + privateId, previous => ({ text: change(previous.text) }))}
            back={() => { setPrivateId(undefined); if (!active) setMobileChat(false); }} showCode={showPrivateCode} />
          : active ? <ChatView key={active.id} chat={active} own={session.user} revision={revision}
            latestAiSeq={latestAiSeq} recalled={recalled} clockOffset={clockOffset}
            hidden={hidden} hide={seq => hide(active.id, seq)} readReceipts={directory.read_receipts}
            draft={drafts[active.id] ?? { text: "" }} setDraft={change => setDraft(active.id, change)}
            transfers={transfers} changed={changed} refresh={refresh} back={() => setMobileChat(false)}
            share={source => share(active.id, source)} openPrivate={() => void invitePrivate(active)}
            groupCode={() => void showGroupCode(active)} aiChat={{
              assistants, settings: () => navigate("Settings"), replies: aiReplies.filter(reply => reply.conversation === active.id),
              answered: reply => setAiReplies(current => [...current, reply]),
              dismiss: id => setAiReplies(current => current.filter(reply => reply.id !== id)),
            }} />
          : <section className="chat-blank"><Blank icon={<MessageCircle size={31} />} title="Your conversations">
            <p>Open a chat to read messages or start a new one.</p>
            <button className="button primary" onClick={() => setNewChat(true)}>New conversation <ArrowRight size={16} /></button>
          </Blank></section>}
      </div>}
      {nav === "Contacts" && <Contacts own={session.user} directory={directory} refresh={refresh} create={create} />}
      {nav === "Files" && <FilesView people={people} revision={revision} recalled={recalled} hidden={hidden} transfers={transfers} onChanged={changed} />}
      {nav === "Settings" && <SettingsView {...{ session, leaving, signOut, privateQuiet, setPrivateQuiet }}
        readReceipts={directory.read_receipts} setReadReceipts={async enabled => { await api.setReadReceipts(enabled); await refresh(); }}
        assistantSettings={<AssistantSettings assistants={assistants} failure={assistantFailure} changed={next => {
          setAssistants(next);
          setAssistantFailure("");
        }} />} />}
    </div>
    {privateTargets && <PrivateTargets people={privateTargets.members.filter(user => user.id !== session.user.id)}
      close={() => setPrivateTargets(undefined)} start={invited => startPrivate(privateTargets, invited)} />}
    {nearby && <NearbyDevices own={session.user.id} close={() => setNearby(false)} invited={id => {
      setNearby(false);
      showPrivate(id);
    }} />}
    {shareCode && <ShareQr code={shareCode} server={session.server} close={() => setShareCode(undefined)} />}
    {scanning && <ScanQr session={session} close={() => setScanning(false)}
      contactFound={user => { setScanning(false); setProfile(user); }}
      groupJoined={id => { setScanning(false); selectChat(id); void refresh(); }}
      privateJoined={id => { setScanning(false); selectPrivate(id); }} />}
    {newChat && <NewChat contacts={directory.contacts} create={create} close={() => setNewChat(false)} findPeople={() => {
      setNewChat(false); navigate("Contacts");
    }} />}
    {!privateOpen && offer && offeredChat && <Dialog title="Share this attachment?" close={() => { if (!sharing) setOffer(undefined); }}>
      <p className="drop-confirm"><strong>{offer.name}</strong> will be shared in <strong>{chatName(offeredChat, session.user.id, directory.remarks)}</strong>.</p>
      <div className="dialog-actions">
        <button className="button subtle" onClick={() => setOffer(undefined)} disabled={sharing}>Cancel</button>
        <button className="button primary" disabled={sharing} onClick={async () => {
          setSharing(true);
          await share(offer.conversation, { kind: "dropped", ticket: offer.ticket });
          setSharing(false);
          setOffer(undefined);
        }}>{sharing ? "Preparing…" : "Share"}</button>
      </div>
    </Dialog>}
    {profile && <ProfileView key={profile.id} user={profile}
      relationVersion={directory.contacts.some(person => person.id === profile.id) ? "contact"
        : directory.pending_contacts.find(request => request.user.id === profile.id)?.request_id ?? "other"}
      refresh={refresh} close={() => setProfile(undefined)} saved={() => {
        setProfile(current => current?.id === profile.id ? undefined : current);
        setProfileNotice(Date.now());
      }}
      message={async () => { await create({ kind: "direct", contact_id: profile.id }); setProfile(undefined); }}
      code={() => { setProfile(undefined); setShareCode({ kind: "contact", name: "Your contact QR code", value: session.user.handle }); }} />}
    {invitation && <PrivateInvitationOverlay key={invitation.id} room={invitation} />}
    <div className="profile-notice" role="status">{profileNotice > 0 && <span key={profileNotice} lang="zh-Hans">信息已更新</span>}</div>
  </main></People.Provider>;
}

function Contacts({ own, directory, refresh, create }: {
  own: User; directory: Directory; refresh: () => Promise<void>; create: (request: ConversationRequest) => Promise<void>;
}) {
  const { contacts, pending_contacts } = directory;
  const { remarks, open } = useContext(People);
  const [handle, setHandle] = useState("");
  const [found, setFound] = useState<User>();
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

  const pending = pending_contacts.find(request => request.user.id === found?.id);
  return <section className="settings-page">
    <header className="workspace-heading"><h1>Contacts</h1><p>Find people by their Zibblu username.</p></header>
    <div className="surface contact-search">
      <form onSubmit={event => {
        event.preventDefault();
        setFound(undefined);
        void perform(async () => { setFound(await api.findUser(handle)); });
      }}>
        <label className="search"><Search size={17} /><input aria-label="Contact username" value={handle}
          onChange={event => { setHandle(event.target.value); setFound(undefined); }} placeholder="Enter a username" required disabled={busy} /></label>
        <button className="button primary" disabled={busy || !handle.trim()} type="submit">Find contact</button>
      </form>
      {found && <div className="contact-row search-result">
        <button className="person-link contact-name" onClick={() => open?.(found)}>
          <PersonAvatar user={found} /><strong>{displayName(found, remarks)}</strong>
        </button>
        <span className="muted">{found.id === own.id ? "Your account" : contacts.some(user => user.id === found.id)
          ? "In contacts" : pending ? pending.incoming ? "Incoming request" : "Request sent" : "View profile"}</span>
      </div>}
      {failure && <p className="inline-error" role="alert">{failure}</p>}
    </div>
    {pending_contacts.length > 0 && <>
      <h2 className="section-title">Contact requests</h2>
      <div className="surface contacts">
        {pending_contacts.map(({ user, incoming, greeting, request_id }) => <div className="contact-row" key={user.id}>
          <button className="person-link contact-name" onClick={() => open?.(user)}>
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
    </>}
    <h2 className="section-title">Your contacts</h2>
    {contacts.length ? <div className="surface contacts">
      {contacts.map(user => <div className="contact-row" key={user.id}>
        <button className="person-link contact-name" onClick={() => open?.(user)}>
          <PersonAvatar user={user} /><strong>{displayName(user, remarks)}</strong>
        </button>
        <button className="button subtle" disabled={busy}
          onClick={() => void perform(() => create({ kind: "direct", contact_id: user.id }))}><MessageSquare size={16} /> Message</button>
        <button className="icon-button" disabled={busy} title="Remove contact" aria-label={"Remove " + displayName(user, remarks)}
          onClick={() => void perform(async () => { await api.removeContact(user.id); await refresh(); })}><UserRoundMinus size={18} /></button>
      </div>)}
    </div> : <Blank icon={<UsersRound size={27} />} title="No contacts yet"><p>Find a username above and send a request.</p></Blank>}
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

  return <Dialog title="New conversation" close={() => { if (!busy) close(); }}>
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
        <label>Group name<input required maxLength={64} value={title} onChange={event => setTitle(event.target.value)} /></label>
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

function SettingsView({ session, leaving, signOut, privateQuiet, setPrivateQuiet, readReceipts, setReadReceipts, assistantSettings }: {
  session: Session; leaving: boolean; signOut: () => Promise<void>;
  privateQuiet: boolean; setPrivateQuiet: (quiet: boolean) => Promise<void>;
  readReceipts: boolean; setReadReceipts: (enabled: boolean) => Promise<void>;
  assistantSettings: ReactNode;
}) {
  const [failure, setFailure] = useState("");
  const [saving, setSaving] = useState(false);
  async function save(change: () => Promise<void>) {
    setSaving(true);
    setFailure("");
    try { await change(); } catch (error) { setFailure(errorMessage(error)); }
    finally { setSaving(false); }
  }
  return <section className="settings-page">
    <header className="workspace-heading"><h1>Settings</h1></header>
    <div className="surface settings-card">
      <h2><CircleUserRound size={20} /> Account</h2>
      <p className="muted server-address">{session.server}</p>
      <div className="chat-actions">
        <button className="button subtle" disabled={leaving} onClick={() => void signOut()}><LogOut size={16} />{leaving ? "Signing out…" : "Sign out"}</button>
      </div>
    </div>
    <div className="surface settings-card">
      <label className="choice setting-switch">
        <span>Read receipts<small>Both people must enable receipts to share read status.</small></span>
        <input type="checkbox" role="switch" checked={readReceipts} disabled={saving}
          onChange={event => { const enabled = event.target.checked; void save(() => setReadReceipts(enabled)); }} />
        <span className="switch-track" aria-hidden="true" />
      </label>
      <label className="choice setting-switch">
        <span>Pause private invitation pop-ups<small>Pending invitations stay in Private chats.</small></span>
        <input type="checkbox" role="switch" checked={privateQuiet} disabled={saving}
          onChange={event => { const quiet = event.target.checked; void save(() => setPrivateQuiet(quiet)); }} />
        <span className="switch-track" aria-hidden="true" />
      </label>
      {failure && <p className="inline-error" role="alert">{failure}</p>}
    </div>
    {assistantSettings}
  </section>;
}
