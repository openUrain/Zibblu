import { FloatingField } from "./floating-field";
import { ArrowDown, ArrowLeft, Ghost, QrCode, Send, UserPlus } from "lucide-react";
import { listen } from "@tauri-apps/api/event";
import { Fragment, useContext, useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import type { FormEvent } from "react";
import { api, errorMessage } from "./api";
import { ios } from "./ios";
import type { Directory, Discovery, NearbyDevice, PrivateList, PrivateMessage, PrivateSummary, User } from "./api";
import { Avatar, Blank, Choice, Dialog, People, PersonAvatar, displayName, messageTimeLabel, startsTimeGroup } from "./common";

export function usePrivateRooms() {
  const [list, setList] = useState<PrivateList>({ rooms: [], prompt: null });
  useEffect(() => {
    let disposed = false;
    let generation = 0;
    let timer: number | undefined;
    const reset = ios ? listen("ios-private-reset", () => {
      generation += 1;
      setList({ rooms: [], prompt: null });
    }) : undefined;
    async function refresh() {
      const current = generation;
      try {
        const next = await api.privateRooms();
        if (!disposed && current === generation) setList(next);
      } catch {
        if (!disposed && current === generation) setList({ rooms: [], prompt: null });
      } finally {
        if (!disposed) timer = window.setTimeout(refresh, 500);
      }
    }
    void refresh();
    return () => { disposed = true; window.clearTimeout(timer); void reset?.then(unlisten => unlisten()); };
  }, []);
  return list;
}

export function PrivateInvitationWindow() {
  const { rooms, prompt } = usePrivateRooms();
  const room = rooms.find(room => room.id === prompt);
  return room ? <InvitePrompt key={room.id} room={room} compact /> : null;
}

export function PrivateInvitationOverlay({ room }: { room: PrivateSummary }) {
  const element = useRef<HTMLDialogElement>(null);
  useLayoutEffect(() => {
    const dialog = element.current;
    dialog?.showModal();
    return () => dialog?.close();
  }, []);
  return <dialog ref={element} className="private-invite-modal" aria-label="Private chat invitation"
    onCancel={event => event.preventDefault()}>
    <InvitePrompt room={room} />
  </dialog>;
}

export const privatePhaseLabel: Record<PrivateSummary["phase"], string> = {
  discovering: "Finding LAN devices",
  waiting: "Waiting for approval",
  invited: "Invitation",
  authenticating: "Verifying account consent",
  connected: "Connected",
  missed: "Missed invitation",
  failed: "Connection interrupted",
  ended: "Ended",
};

function InvitePrompt({ room, compact = false }: { room: PrivateSummary; compact?: boolean }) {
  const { remarks } = useContext(People);
  const [compactRemarks, setCompactRemarks] = useState<Directory["remarks"]>({});
  useEffect(() => {
    if (!compact) return;
    let current = true;
    void api.directory().then(directory => { if (current) setCompactRemarks(directory.remarks); }).catch(() => {});
    return () => { current = false; };
  }, [compact]);
  const names = compact ? compactRemarks : remarks;
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState("");

  async function respond(accept: boolean) {
    setBusy(true);
    setFailure("");
    try { await api.respondPrivate(room.id, accept); }
    catch (error) { setFailure(errorMessage(error)); setBusy(false); }
  }

  return <div className={"private-invite" + (compact ? " compact" : "")}>
    <Avatar name={displayName(room.host, names)} large />
    <h2>{displayName(room.host, names)}</h2>
    {room.invited.length > 1 && <ul className="invitees" aria-label="Invited accounts" tabIndex={compact ? 0 : undefined}>
      {room.invited.map(person => <li key={person.id}>{displayName(person, names)}</li>)}
    </ul>}
    <div className="private-actions">
      <button className="button subtle" disabled={busy} onClick={() => void respond(false)}>Decline</button>
      <button className="button primary" disabled={busy} onClick={() => void respond(true)}>Accept</button>
    </div>
    {failure && <p className="inline-error" role="alert">{failure}</p>}
  </div>;
}

export function PrivateTargets({ people, close, start }: {
  people: User[]; close: () => void; start: (ids: string[]) => Promise<void>;
}) {
  const { remarks } = useContext(People);
  const [selected, setSelected] = useState<string[]>([]);
  const [failure, setFailure] = useState("");
  const [busy, setBusy] = useState(false);
  return <Dialog title="Invite to private chat" close={() => { if (!busy) close(); }}>
    <fieldset className="dialog-contacts"><legend>Group members</legend>
      {people.map(person => <Choice key={person.id} disabled={busy} checked={selected.includes(person.id)}
        onChange={event => setSelected(previous => event.target.checked
          ? [...previous, person.id] : previous.filter(id => id !== person.id))}>
        <PersonAvatar user={person} /><span>{displayName(person, remarks)}</span>
      </Choice>)}
    </fieldset>
    {failure && <p className="inline-error" role="alert">{failure}</p>}
    <div className="dialog-actions">
      <button className="button subtle" disabled={busy} onClick={close}>Cancel</button>
      <button className="button primary" disabled={busy || selected.length === 0} onClick={async () => {
        setBusy(true);
        try { await start(selected); }
        catch (error) { setFailure(errorMessage(error)); setBusy(false); }
      }}>Send invitation</button>
    </div>
  </Dialog>;
}

export function NearbyDevices({ own, close, invited }: {
  own: string; close: () => void; invited: (id: string) => void;
}) {
  const { remarks } = useContext(People);
  const [discovery, setDiscovery] = useState<Discovery>({ status: "starting", devices: [] });
  const { devices, status } = discovery;
  const [choice, setChoice] = useState<NearbyDevice>();
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState("");
  useEffect(() => {
    let disposed = false;
    let timer: number | undefined;
    async function refresh() {
      try {
        const next = await api.nearbyDevices();
        if (!disposed) setDiscovery(next);
      } catch {
        if (!disposed) setDiscovery({ status: "discovery_failed", devices: [] });
      } finally {
        if (!disposed) timer = window.setTimeout(refresh, 1000);
      }
    }
    void refresh();
    return () => { disposed = true; window.clearTimeout(timer); };
  }, []);

  const selected = devices.find(device => device.id === choice?.id
    && device.matched_account?.id === choice?.matched_account?.id);
  const account = selected?.matched_account?.id;
  async function invite() {
    if (!selected || account === own || busy) return;
    setBusy(true);
    setFailure("");
    try { invited(await api.inviteNearby(selected.id, account)); }
    catch (error) { setFailure(errorMessage(error)); setBusy(false); }
  }

  return <Dialog title="Nearby devices" close={() => { if (!busy) close(); }}>
    {devices.length ? <fieldset className="dialog-contacts" disabled={busy}>
      <legend>Seen on LAN · identity unverified</legend>
      {devices.map(device => <Choice key={device.id} type="radio" name="nearby-device" checked={selected?.id === device.id}
        onChange={() => { setChoice(device); setFailure(""); }}>
        <span className="chat-row-text">
          <strong>{device.matched_account ? `Possible match: ${displayName(device.matched_account, remarks)}` : "Unknown Zibblu device"}</strong>
          <small>Device {device.id.slice(0, 8)}</small>
        </span>
      </Choice>)}
    </fieldset> : <p className="muted" role="status">
      {status === "starting" ? "Starting LAN discovery…"
        : status === "network_unavailable" ? (ios ? "Connect to an allowed Wi-Fi network to find nearby devices."
          : "No approved IPv4 LAN is available. Connect to an allowed Wi-Fi or Ethernet network.")
        : status === "permission_denied" ? "Allow Zibblu to use the Local Network in iOS Settings, then return here."
        : status === "discovery_failed" ? "LAN discovery could not start or the configured discovery service failed."
        : "No Zibblu devices found. Keep both clients open and signed in on the same allowed network."}
    </p>}
    {selected && !account && <p className="muted">Zibblu verifies the selected device’s account before sending the invitation.</p>}
    {account === own && <p className="muted">Private invitations go to a different account.</p>}
    {choice && !selected && status === "ready" && <p className="muted" role="status">The selected device is no longer listed for that account.</p>}
    {failure && <p className="inline-error" role="alert">{failure}</p>}
    {((status === "ready" && devices.length === 0) || status === "discovery_failed" || failure) && <details className="nearby-help">
      <summary className="muted">Troubleshooting tips</summary>
      <p className="muted">{ios ? "Check Local Network access in iOS Settings and make sure Wi-Fi allows devices to communicate."
        : "Check that Windows Defender Firewall allows Zibblu on the Private profile. For managed networks, ensure mDNS and direct UDP are permitted."}</p>
    </details>}
    <div className="dialog-actions">
      <button className="button subtle" disabled={busy} onClick={close}>Close</button>
      <button className="button primary" disabled={busy || !selected || account === own}
        onClick={() => void invite()}>{busy ? "Connecting…" : "Send private invitation"}</button>
    </div>
  </Dialog>;
}

export function PrivateChatView({ id, own, back, showCode, draft, setDraft, isContact }: {
  id: string; own: User; back: () => void; showCode: (id: string) => Promise<void>;
  draft: string; setDraft: (change: (draft: string) => string) => void; isContact: (id: string) => boolean;
}) {
  const { remarks, open } = useContext(People);
  const inputId = useId();
  const [room, setRoom] = useState<PrivateSummary>();
  const [messages, setMessages] = useState<PrivateMessage[]>([]);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState("");
  const [closed, setClosed] = useState(false);
  const timeline = useRef<HTMLDivElement>(null);
  const latestAtOpen = useRef<number | undefined>(undefined);
  const [away, setAway] = useState(false);
  const nearBottom = useRef(true);
  const clearDraft = useRef(setDraft);
  clearDraft.current = setDraft;

  useEffect(() => {
    let disposed = false;
    let timer: number | undefined;
    let after = 0;
    let connection: string | undefined;
    async function poll() {
      try {
        let result = await api.viewPrivate(id, after);
        if (disposed) return;
        if (result.connection !== connection) {
          if (connection !== undefined) {
            result = await api.viewPrivate(id, 0);
            if (disposed) return;
          }
          connection = result.connection;
          after = 0;
          setMessages([]);
          if (after || latestAtOpen.current !== undefined) clearDraft.current(() => "");
          latestAtOpen.current = undefined;
        }
        const { messages: incoming, ...summary } = result;
        setRoom(summary);
        setClosed(false);
        if (summary.phase !== "connected") {
          setMessages([]);
          clearDraft.current(() => "");
        } else if (incoming.length) {
          after = incoming.at(-1)!.seq;
          setMessages(previous => [...previous, ...incoming].slice(-100));
        }
        if (summary.phase === "connected") latestAtOpen.current ??= incoming.at(-1)?.seq ?? 0;
      } catch {
        if (disposed) return;
        setClosed(true);
        setMessages([]);
        clearDraft.current(() => "");
      } finally {
        if (!disposed) timer = window.setTimeout(poll, 300);
      }
    }
    void poll();
    return () => { disposed = true; window.clearTimeout(timer); };
  }, [id]);

  useLayoutEffect(() => {
    if (nearBottom.current && timeline.current) timeline.current.scrollTop = timeline.current.scrollHeight;
  }, [messages]);

  async function act(action: () => Promise<void>) {
    if (busy) return;
    setBusy(true);
    setFailure("");
    try { await action(); }
    catch (error) { setFailure(errorMessage(error)); }
    finally { setBusy(false); }
  }

  const connected = !closed && room?.phase === "connected";
  const failed = room?.phase === "failed" || (closed && room?.phase !== "connected");
  const terminal = closed || room?.phase === "ended" || room?.phase === "missed" || failed;
  const people = room?.participants ?? [];
  const oversized = new TextEncoder().encode(draft.trim()).length > 2048;
  let previousTime: number | undefined;
  function send(event: FormEvent) {
    event.preventDefault();
    const text = draft.trim();
    if (!text || oversized || !connected) return;
    void act(async () => {
      await api.sendPrivate(id, text);
      setDraft(previous => previous.trim() === text ? "" : previous);
    });
  }

  return <section className="chat-view private-chat" aria-label="Private chat"
    onCopy={event => event.preventDefault()} onCut={event => event.preventDefault()}
    onDragStart={event => event.preventDefault()} onDrop={event => event.preventDefault()}>
    <header className="chat-header">
      <button className="icon-button" onClick={back} aria-label="Return to chats"><ArrowLeft size={19} /></button>
      <Ghost className="private-ghost" size={22} aria-hidden="true" />
      <div className="chat-heading">
        <h2>{room ? privateTitle(room, own.id, remarks) : "Private chat"}</h2>
        <span role="status">{connected ? "Verified private connection" : failed ? "Connection failed" : terminal ? "Session ended" : "LAN only"}
          {connected && ios ? " · LAN only" : ""}{room?.open && !terminal ? " · QR access" : ""}</span>
      </div>
      {!terminal && room?.open && room.hosted && <button className="icon-button" title="Share private QR code"
        aria-label="Share private QR code" onClick={() => void act(() => showCode(id))}><QrCode size={20} /></button>}
      {room && room.phase !== "invited" && <button className="button subtle" onClick={() => void act(async () => {
        await api.endPrivate(id);
        setDraft(() => "");
        back();
      })}>{terminal ? "Dismiss" : "End session"}</button>}
    </header>
    {connected ? <>
      <div className="private-people" aria-label="Current participants">
        {people.map(person => <button className="person-link" key={person.id} onClick={() => open?.(person)}
          title={person.id !== own.id && !isContact(person.id) ? "View profile or add contact" : "View profile"}>
          {person.id !== own.id && !isContact(person.id) && <UserPlus size={14} />}{displayName(person, remarks)}
        </button>)}
      </div>
      <div className="timeline-wrap"><div className="timeline" ref={timeline} role="log" aria-label="Private messages" onScroll={event => {
        const element = event.currentTarget;
        nearBottom.current = element.scrollHeight - element.scrollTop - element.clientHeight < 100;
        setAway(!nearBottom.current);
      }}>
        {messages.map(message => {
          const mine = message.sender.id === own.id;
          const group = startsTimeGroup(message.created_at, previousTime);
          previousTime = message.created_at;
          const avatar = <Avatar name={displayName(message.sender, remarks)} />;
          return <Fragment key={message.seq}>
            {group && <div className="day-divider"><time dateTime={new Date(message.created_at).toISOString()}>{messageTimeLabel(message.created_at)}</time></div>}
            <div className={"message-row" + (mine ? " mine" : "") + (message.seq > (latestAtOpen.current ?? 0) ? " message-arrival" : "")}>
              {!mine && avatar}
              <div className="message-body">
                {!mine && <span className="message-author">{displayName(message.sender, remarks)}</span>}
                <div className="bubble">{message.text}</div>
              </div>
              {mine && avatar}
            </div>
          </Fragment>;
        })}
      </div>{away && <button className="button subtle jump-bottom" onClick={() => {
        if (timeline.current) timeline.current.scrollTop = timeline.current.scrollHeight;
      }}><ArrowDown size={15} /> Latest messages</button>}</div>
      <form className="composer" onSubmit={send}>
        {(failure || oversized) && <p className="inline-error" role="alert">
          {oversized ? "Private messages can be up to 2 KiB." : failure}
        </p>}
        <div className="composer-box">
          <FloatingField multiline id={inputId} label="Private message" rows={2} autoComplete="off"
            spellCheck={false} value={draft} onChange={event => { const value = event.target.value; setDraft(() => value); }}
            onKeyDown={event => {
              if (!ios && event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
                event.preventDefault();
                event.currentTarget.form?.requestSubmit();
              }
            }} />
            <button className="button primary send-button" type="submit" aria-label="Send private message"
              disabled={busy || !draft.trim() || oversized}>{busy ? "Sending…" : "Send"} <Send size={16} /></button>
        </div>
      </form>
    </> : <div className="private-status" aria-live="polite">
      {room?.phase === "invited" && !closed ? <InvitePrompt key={room.id} room={room} /> : <Blank
        icon={<Ghost size={26} />} title={room?.phase === "missed" ? "Missed private invitation"
          : failed ? "Private connection failed" : terminal ? "Private session ended"
          : room?.open && room.hosted ? "Waiting for a QR scan"
          : room ? privatePhaseLabel[room.phase] : "Opening private chat"}>
        {room?.phase === "missed" && <p>{displayName(room.host, remarks)} invited you. The invitation ended before you responded.</p>}
        {failure && <p className="inline-error" role="alert">{failure}</p>}
      </Blank>}
    </div>}
  </section>;
}

export function privateTitle(room: PrivateSummary, own: string, remarks: Directory["remarks"]) {
  if (room.open) return "Private QR chat";
  const others = [room.host, ...room.invited].filter(person => person.id !== own);
  return others.length ? others.map(person => displayName(person, remarks)).join(", ") : "Private chat";
}
