import { listen } from "@tauri-apps/api/event";
import {
  ArrowRight, CircleUserRound, FolderOpen, Laptop, LogOut, MessageCircle, MessageSquare, Moon,
  Search, Settings, Sun, UserPlus, UserRoundMinus, UsersRound, X,
} from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import type { FormEvent } from "react";
import { api, errorMessage, isExpired } from "./api";
import type { ConversationRequest, Directory, DropOffer, Session, ShareSource, Theme, Transfer, User } from "./api";
import { Avatar, Blank, Brand, Dialog, chatName } from "./common";
import { ChatList, ChatView } from "./chat";
import { FilesView } from "./files";

const pages = [
  { name: "Chats", Icon: MessageSquare }, { name: "Contacts", Icon: UsersRound },
  { name: "Files", Icon: FolderOpen }, { name: "Settings", Icon: Settings },
] as const;
type Navigation = typeof pages[number]["name"];

export function Workspace({ session, theme, setTheme, signedOut }: {
  session: Session; theme: Theme; setTheme: (theme: Theme) => Promise<void>; signedOut: (notice?: string) => void;
}) {
  const [nav, setNav] = useState<Navigation>("Chats");
  const [directory, setDirectory] = useState<Directory>({ contacts: [], conversations: [] });
  const [selected, setSelected] = useState<string>();
  const [mobileChat, setMobileChat] = useState(false);
  const [revision, setRevision] = useState(0);
  const [connection, setConnection] = useState<"Connecting" | "Connected" | "Reconnecting" | "Session expired">("Connecting");
  const [transfers, setTransfers] = useState<Transfer[]>([]);
  const [failure, setFailure] = useState("");
  const [newChat, setNewChat] = useState(false);
  const [offer, setOffer] = useState<DropOffer & { conversation: string }>();
  const [sharing, setSharing] = useState(false);
  const [leaving, setLeaving] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setDirectory(await api.sync());
      setConnection("Connected");
      setRevision(value => value + 1);
    } catch (error) {
      setConnection(isExpired(error) ? "Session expired" : "Reconnecting");
    }
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
  const activeId = nav === "Chats" ? active?.id : undefined;
  useEffect(() => {
    const drop = listen<DropOffer>("attachment-drop", event => {
      if (activeId) setOffer({ ...event.payload, conversation: activeId });
      else setFailure("Open a chat before dropping a file or folder.");
    });
    const error = listen<string>("attachment-drop-error", event => setFailure(event.payload));
    return () => { void drop.then(unlisten => unlisten()); void error.then(unlisten => unlisten()); };
  }, [activeId]);

  function selectChat(id: string) {
    setSelected(id);
    setNav("Chats");
    setMobileChat(true);
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

  return <main className="shell">
    <nav className="rail" aria-label="Main navigation">
      <div className="rail-brand"><Brand /></div>
      <div className="rail-links">{pages.map(({ name, Icon }) =>
        <button key={name} className={nav === name ? "nav-active" : ""} aria-label={name}
          aria-current={nav === name ? "page" : undefined} onClick={() => {
            setNav(name);
            if (name === "Chats") setMobileChat(false);
          }}>
          <Icon size={22} strokeWidth={1.9} /><span>{name}</span>
        </button>)}</div>
      <button className="rail-account" title={"@" + session.user.handle} aria-label="Account settings" onClick={() => setNav("Settings")}>
        <Avatar name={session.user.handle} />
      </button>
    </nav>
    <div className="shell-main">
      <header className="topbar"><span className="app-name">Zibblu</span>
        <div className={"connection " + (connected ? "connected" : "disconnected")} role="status">
          <span className="connection-dot" />{connection}
          {connection === "Session expired" && <button className="text-button" disabled={leaving} onClick={() => void signOut()}>Sign in again</button>}
        </div>
      </header>
      {failure && <div className="workspace-error" role="alert">{failure}
        <button className="icon-button" aria-label="Dismiss error" onClick={() => setFailure("")}><X size={17} /></button>
      </div>}
      {nav === "Chats" && <div className={"chat-layout" + (mobileChat && active ? " mobile-chat-open" : "")}>
        <ChatList chats={directory.conversations} own={session.user} selected={selected} open={selectChat} newChat={() => setNewChat(true)} />
        {active ? <ChatView key={active.id} chat={active} own={session.user} revision={revision}
          transfers={transfers} changed={changed} refresh={refresh} back={() => setMobileChat(false)}
          share={source => share(active.id, source)} />
          : <section className="chat-blank"><Blank icon={<MessageCircle size={31} />} title="Your conversations">
            <p>Open a chat to read messages or start a new one.</p>
            <button className="button primary" onClick={() => setNewChat(true)}>New conversation <ArrowRight size={16} /></button>
          </Blank></section>}
      </div>}
      {nav === "Contacts" && <Contacts own={session.user} contacts={directory.contacts} refresh={refresh} create={create} />}
      {nav === "Files" && <FilesView people={people} revision={revision} transfers={transfers} onChanged={changed} />}
      {nav === "Settings" && <SettingsView {...{ session, theme, setTheme, leaving, signOut }} />}
    </div>
    {newChat && <NewChat contacts={directory.contacts} create={create} close={() => setNewChat(false)} findPeople={() => {
      setNewChat(false); setNav("Contacts");
    }} />}
    {offer && offeredChat && <Dialog title="Share this attachment?" close={() => { if (!sharing) setOffer(undefined); }}>
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
  </main>;
}

function Contacts({ own, contacts, refresh, create }: {
  own: User; contacts: User[]; refresh: () => Promise<void>; create: (request: ConversationRequest) => Promise<void>;
}) {
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

  return <section className="settings-page">
    <header className="workspace-heading"><h1>Contacts</h1><p>Find people by their Zibblu handle.</p></header>
    <div className="surface contact-search">
      <form onSubmit={event => {
        event.preventDefault();
        setFound(undefined);
        void perform(async () => { setFound(await api.findUser(handle)); });
      }}>
        <label className="search"><Search size={17} /><input aria-label="Contact handle" value={handle}
          onChange={event => { setHandle(event.target.value); setFound(undefined); }} placeholder="Enter a handle" required disabled={busy} /></label>
        <button className="button primary" disabled={busy || !handle.trim()} type="submit">Find contact</button>
      </form>
      {found && <div className="contact-row search-result">
        <Avatar name={found.handle} />
        <div className="contact-name"><strong>@{found.handle}</strong></div>
        {found.id === own.id ? <span className="muted">Your account</span> : contacts.some(user => user.id === found.id)
          ? <span className="muted">In contacts</span>
          : <button className="button subtle" disabled={busy} onClick={() => void perform(async () => {
            await api.addContact(found.id); setFound(undefined); setHandle(""); await refresh();
          })}><UserPlus size={16} /> Add contact</button>}
      </div>}
      {failure && <p className="inline-error" role="alert">{failure}</p>}
    </div>
    <h2 className="section-title">Your contacts</h2>
    {contacts.length ? <div className="surface contacts">
      {contacts.map(user => <div className="contact-row" key={user.id}>
        <Avatar name={user.handle} /><div className="contact-name"><strong>{user.handle}</strong><small>@{user.handle}</small></div>
        <button className="button subtle" disabled={busy}
          onClick={() => void perform(() => create({ kind: "direct", contact_id: user.id }))}><MessageSquare size={16} /> Message</button>
        <button className="icon-button" disabled={busy} title="Remove contact" aria-label={"Remove " + user.handle}
          onClick={() => void perform(async () => { await api.removeContact(user.id); await refresh(); })}><UserRoundMinus size={18} /></button>
      </div>)}
    </div> : <Blank icon={<UsersRound size={27} />} title="No contacts yet"><p>Find a handle above and add your first contact.</p></Blank>}
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

function SettingsView({ session, theme, setTheme, leaving, signOut }: {
  session: Session; theme: Theme; setTheme: (theme: Theme) => Promise<void>; leaving: boolean; signOut: () => Promise<void>;
}) {
  const [failure, setFailure] = useState("");
  const appearances = [{ value: "system", label: "System", Icon: Laptop }, { value: "light", label: "Light", Icon: Sun },
    { value: "dark", label: "Dark", Icon: Moon }] as const;
  return <section className="settings-page">
    <header className="workspace-heading"><h1>Settings</h1><p>Manage your account and appearance.</p></header>
    <div className="surface settings-card">
      <h2><CircleUserRound size={20} /> Account</h2>
      <div className="account-row"><Avatar name={session.user.handle} large /><div><strong>@{session.user.handle}</strong>
        <p className="muted server-address">{session.server}</p></div></div>
      <p className="muted">Normal chats use your server to deliver and store messages and attachments.</p>
      <button className="button subtle" disabled={leaving} onClick={() => void signOut()}><LogOut size={16} />{leaving ? "Signing out…" : "Sign out"}</button>
    </div>
    <div className="surface settings-card"><h2><Sun size={20} /> Appearance</h2>
      <div className="appearance">{appearances.map(({ value, label, Icon }) => <button className={"button subtle" + (theme === value ? " toggled" : "")}
        aria-pressed={theme === value} key={value} onClick={async () => {
          setFailure("");
          try { await setTheme(value); } catch (error) { setFailure(errorMessage(error)); }
        }}><Icon size={17} />{label}</button>)}</div>
      {failure && <p className="inline-error" role="alert">{failure}</p>}
    </div>
  </section>;
}
