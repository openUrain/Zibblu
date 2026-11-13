import { listen } from "@tauri-apps/api/event";
import {
  ArrowRight, CircleUserRound, FolderOpen, Laptop, LogOut, MessageCircle, MessageSquare, Moon,
  QrCode, Radar, ScanLine, Search, Settings, Sun, UserPlus, UserRoundMinus, UsersRound, X,
} from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import type { FormEvent, ReactNode } from "react";
import { ai } from "./ai-api";
import type { AiReply, Assistant } from "./ai-api";
import { AssistantSettings } from "./ai";
import { api, errorMessage, isExpired } from "./api";
import type { Conversation, ConversationRequest, Directory, DropOffer, Session, ShareSource, Theme, Transfer, User } from "./api";
import { Avatar, Blank, Brand, Dialog, chatName } from "./common";
import { ChatList, ChatView } from "./chat";
import { FilesView } from "./files";
import { NearbyDevices, PrivateChatView, PrivateInvitationOverlay, PrivateTargets, usePrivateRooms } from "./private-chat";
import { ScanQr, ShareQr } from "./qr";
import type { SharedCode } from "./qr";

const pages = [
  { name: "Chats", Icon: MessageSquare }, { name: "Contacts", Icon: UsersRound },
  { name: "Files", Icon: FolderOpen }, { name: "Settings", Icon: Settings },
] as const;
type Navigation = typeof pages[number]["name"];

export function Workspace({ session, theme, setTheme, privateQuiet, setPrivateQuiet, signedOut }: {
  session: Session; theme: Theme; setTheme: (theme: Theme) => Promise<void>; signedOut: (notice?: string) => void;
  privateQuiet: boolean; setPrivateQuiet: (quiet: boolean) => Promise<void>;
}) {
  const [nav, setNav] = useState<Navigation>("Chats");
  const [assistants, setAssistants] = useState<Assistant[]>([]);
  const [assistantFailure, setAssistantFailure] = useState("");
  const [aiReplies, setAiReplies] = useState<AiReply[]>([]);
  const [directory, setDirectory] = useState<Directory>({ contacts: [], conversations: [], pending_contacts: [] });
  const [recalled, setRecalled] = useState<ReadonlySet<number>>(new Set());
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
    .map(user => [user.id, user])).values()].sort((a, b) => a.handle.localeCompare(b.handle));
  const offeredChat = directory.conversations.find(chat => chat.id === offer?.conversation);
  const connected = connection === "Connected";
  const invitation = privateRooms.find(room => room.id === prompt);

  return <main className="shell">
    <nav className="rail" aria-label="Main navigation">
      <div className="rail-brand"><Brand /></div>
      <div className="rail-links">{pages.map(({ name, Icon }) =>
        <button key={name} className={nav === name ? "nav-active" : ""} aria-label={name}
          aria-current={nav === name ? "page" : undefined} onClick={() => navigate(name)}>
          <Icon size={22} strokeWidth={1.9} /><span>{name}</span>
        </button>)}</div>
      <button className="rail-account" title={"@" + session.user.handle} aria-label="Account settings" onClick={() => navigate("Settings")}>
        <Avatar name={session.user.handle} />
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
            back={() => { setPrivateId(undefined); if (!active) setMobileChat(false); }} showCode={showPrivateCode} />
          : active ? <ChatView key={active.id} chat={active} own={session.user} revision={revision}
            latestAiSeq={latestAiSeq} recalled={recalled} clockOffset={clockOffset}
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
      {nav === "Files" && <FilesView people={people} revision={revision} recalled={recalled} transfers={transfers} onChanged={changed} />}
      {nav === "Settings" && <SettingsView {...{ session, theme, setTheme, leaving, signOut, privateQuiet, setPrivateQuiet }}
        assistantSettings={<AssistantSettings assistants={assistants} failure={assistantFailure} changed={next => {
          setAssistants(next);
          setAssistantFailure("");
        }} />}
        contactCode={() => setShareCode({ kind: "contact", name: "Your contact QR code", value: session.user.handle })} />}
    </div>
    {privateTargets && <PrivateTargets people={privateTargets.members.filter(user => user.id !== session.user.id)}
      close={() => setPrivateTargets(undefined)} start={invited => startPrivate(privateTargets, invited)} />}
    {nearby && <NearbyDevices own={session.user.id} close={() => setNearby(false)} invited={id => {
      setNearby(false);
      showPrivate(id);
    }} />}
    {shareCode && <ShareQr code={shareCode} server={session.server} close={() => setShareCode(undefined)} />}
    {scanning && <ScanQr session={session} close={() => setScanning(false)}
      contactAdded={() => { setScanning(false); navigate("Contacts"); void refresh(); }}
      groupJoined={id => { setScanning(false); selectChat(id); void refresh(); }}
      privateJoined={id => { setScanning(false); selectPrivate(id); }} />}
    {newChat && <NewChat contacts={directory.contacts} create={create} close={() => setNewChat(false)} findPeople={() => {
      setNewChat(false); navigate("Contacts");
    }} />}
    {!privateOpen && offer && offeredChat && <Dialog title="Share this attachment?" close={() => { if (!sharing) setOffer(undefined); }}>
      <p className="drop-confirm"><strong>{offer.name}</strong> will be shared in <strong>{chatName(offeredChat, session.user.id)}</strong>.</p>
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
    {invitation && <PrivateInvitationOverlay key={invitation.id} room={invitation} />}
  </main>;
}

function Contacts({ own, directory, refresh, create }: {
  own: User; directory: Directory; refresh: () => Promise<void>; create: (request: ConversationRequest) => Promise<void>;
}) {
  const { contacts, pending_contacts } = directory;
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
        <Avatar name={found.handle} />
        <div className="contact-name"><strong>@{found.handle}</strong></div>
        {found.id === own.id ? <span className="muted">Your account</span> : contacts.some(user => user.id === found.id)
          ? <span className="muted">In contacts</span>
          : pending ? <span className="muted">{pending.incoming ? "Incoming request" : "Request sent"}</span>
            : <button className="button subtle" disabled={busy} onClick={() => void perform(async () => {
              await api.addContact(found.id); await refresh();
            })}><UserPlus size={16} /> Send request</button>}
      </div>}
      {failure && <p className="inline-error" role="alert">{failure}</p>}
    </div>
    {pending_contacts.length > 0 && <>
      <h2 className="section-title">Contact requests</h2>
      <div className="surface contacts">
        {pending_contacts.map(({ user, incoming }) => <div className="contact-row" key={user.id}>
          <Avatar name={user.handle} />
          <div className="contact-name"><strong>{user.handle}</strong><small>{incoming ? "Wants to connect" : "Waiting for approval"}</small></div>
          {incoming ? <>
            <button className="button subtle" disabled={busy} onClick={() => void perform(async () => {
              await api.respondContact(user.id, false); await refresh();
            })}>Decline</button>
            <button className="button primary" disabled={busy} onClick={() => void perform(async () => {
              await api.respondContact(user.id, true); await refresh();
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
        <Avatar name={user.handle} /><div className="contact-name"><strong>{user.handle}</strong><small>@{user.handle}</small></div>
        <button className="button subtle" disabled={busy}
          onClick={() => void perform(() => create({ kind: "direct", contact_id: user.id }))}><MessageSquare size={16} /> Message</button>
        <button className="icon-button" disabled={busy} title="Remove contact" aria-label={"Remove " + user.handle}
          onClick={() => void perform(async () => { await api.removeContact(user.id); await refresh(); })}><UserRoundMinus size={18} /></button>
      </div>)}
    </div> : <Blank icon={<UsersRound size={27} />} title="No contacts yet"><p>Find a username above and send a request.</p></Blank>}
  </section>;
}

function NewChat({ contacts, create, close, findPeople }: {
  contacts: User[]; create: (request: ConversationRequest) => Promise<void>; close: () => void; findPeople: () => void;
}) {
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
      <div className="tabs">
        <button className={!group ? "active-tab" : ""} aria-pressed={!group} disabled={busy} onClick={() => { setGroup(false); setFailure(""); }}>Direct message</button>
        <button className={group ? "active-tab" : ""} aria-pressed={group} disabled={busy} onClick={() => { setGroup(true); setFailure(""); }}>Group</button>
      </div>
      {group ? <form className="group-form" onSubmit={submit}>
        <label>Group name<input required maxLength={64} value={title} onChange={event => setTitle(event.target.value)} /></label>
        <fieldset><legend>Choose 2–15 contacts</legend><div className="dialog-contacts">{contacts.map(user => {
          const checked = members.includes(user.id);
          return <label key={user.id} className="check-contact">
            <input type="checkbox" checked={checked} disabled={busy || (!checked && members.length >= 15)}
              onChange={() => setMembers(current => checked ? current.filter(id => id !== user.id) : [...current, user.id])} />
            <Avatar name={user.handle} /> @{user.handle}
          </label>;
        })}</div></fieldset>
        {failure && <p className="inline-error" role="alert">{failure}</p>}
        <button className="button primary" type="submit" disabled={busy || members.length < 2 || !title.trim()}>Create group</button>
      </form> : <>
        <div className="dialog-contacts">{contacts.map(user => <button className="pick-contact" key={user.id} disabled={busy}
          onClick={() => void start({ kind: "direct", contact_id: user.id })}><Avatar name={user.handle} /> @{user.handle}<ArrowRight size={16} /></button>)}</div>
        {failure && <p className="inline-error" role="alert">{failure}</p>}
      </>}
    </>}
  </Dialog>;
}

function SettingsView({ session, theme, setTheme, leaving, signOut, contactCode, privateQuiet, setPrivateQuiet, assistantSettings }: {
  session: Session; theme: Theme; setTheme: (theme: Theme) => Promise<void>; leaving: boolean; signOut: () => Promise<void>;
  contactCode: () => void; privateQuiet: boolean; setPrivateQuiet: (quiet: boolean) => Promise<void>;
  assistantSettings: ReactNode;
}) {
  const [failure, setFailure] = useState("");
  const appearances = [{ value: "system", label: "System", Icon: Laptop }, { value: "light", label: "Light", Icon: Sun },
    { value: "dark", label: "Dark", Icon: Moon }] as const;
  return <section className="settings-page">
    <header className="workspace-heading"><h1>Settings</h1><p>Manage your account, appearance and assistants.</p></header>
    <div className="surface settings-card">
      <h2><CircleUserRound size={20} /> Account</h2>
      <div className="account-row"><Avatar name={session.user.handle} large /><div><strong>@{session.user.handle}</strong>
        <p className="muted server-address">{session.server}</p></div></div>
      <p className="muted">Normal chats use your server to deliver and store messages and attachments.</p>
      <div className="chat-actions">
        <button className="button subtle" onClick={contactCode}><QrCode size={17} /> Your QR code</button>
        <button className="button subtle" disabled={leaving} onClick={() => void signOut()}><LogOut size={16} />{leaving ? "Signing out…" : "Sign out"}</button>
      </div>
    </div>
    {assistantSettings}
    <div className="surface settings-card"><h2><Sun size={20} /> Appearance</h2>
      <div className="appearance">{appearances.map(({ value, label, Icon }) => <button className={"button subtle" + (theme === value ? " toggled" : "")}
        aria-pressed={theme === value} key={value} onClick={async () => {
          setFailure("");
          try { await setTheme(value); } catch (error) { setFailure(errorMessage(error)); }
        }}><Icon size={17} />{label}</button>)}</div>
      {failure && <p className="inline-error" role="alert">{failure}</p>}
    </div>
    <div className="surface settings-card">
      <label className="check-contact">
        <input type="checkbox" checked={privateQuiet} onChange={async event => {
          setFailure("");
          try { await setPrivateQuiet(event.target.checked); } catch (error) { setFailure(errorMessage(error)); }
        }} /> Pause private invitation pop-ups
      </label>
      <p className="muted">Use this while sharing your screen. Pending invitations stay in Private chats.</p>
    </div>
  </section>;
}
