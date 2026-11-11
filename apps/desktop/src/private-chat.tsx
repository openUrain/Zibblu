import { ArrowLeft, Ghost, QrCode, Send } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { FormEvent } from "react";
import { api, errorMessage } from "./api";
import type { PrivateList, PrivateMessage, PrivateSummary, User } from "./api";
import { Avatar, Blank, Dialog } from "./common";

export function usePrivateRooms() {
  const [list, setList] = useState<PrivateList>({ rooms: [], prompt: null });
  useEffect(() => {
    let disposed = false;
    let timer: number | undefined;
    async function refresh() {
      try {
        const next = await api.privateRooms();
        if (!disposed) setList(next);
      } catch {
        if (!disposed) setList({ rooms: [], prompt: null });
      } finally {
        if (!disposed) timer = window.setTimeout(refresh, 500);
      }
    }
    void refresh();
    return () => { disposed = true; window.clearTimeout(timer); };
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
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState("");

  async function respond(accept: boolean) {
    setBusy(true);
    setFailure("");
    try { await api.respondPrivate(room.id, accept); }
    catch (error) { setFailure(errorMessage(error)); setBusy(false); }
  }

  return <div className={"private-invite" + (compact ? " compact" : "")}>
    <Avatar name={room.host.handle} large />
    <h2>@{room.host.handle}</h2>
    {!compact && room.invited.length > 1 && <ul className="invitees" aria-label="Invited accounts">
      {room.invited.map(person => <li key={person.id}>@{person.handle}</li>)}
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
  const [selected, setSelected] = useState<string[]>([]);
  const [failure, setFailure] = useState("");
  const [busy, setBusy] = useState(false);
  return <Dialog title="Invite to private chat" close={() => { if (!busy) close(); }}>
    <fieldset className="dialog-contacts"><legend>Group members</legend>
      {people.map(person => <label key={person.id} className="check-contact">
        <input type="checkbox" disabled={busy} checked={selected.includes(person.id)}
          onChange={event => setSelected(previous => event.target.checked
            ? [...previous, person.id] : previous.filter(id => id !== person.id))} />
        <Avatar name={person.handle} /> @{person.handle}
      </label>)}
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

export function PrivateChatView({ id, own, back, showCode }: {
  id: string; own: User; back: () => void; showCode: (id: string) => Promise<void>;
}) {
  const [room, setRoom] = useState<PrivateSummary>();
  const [messages, setMessages] = useState<PrivateMessage[]>([]);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState("");
  const [closed, setClosed] = useState(false);
  const timeline = useRef<HTMLDivElement>(null);

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
          setDraft("");
        }
        const { messages: incoming, ...summary } = result;
        setRoom(summary);
        setClosed(false);
        if (summary.phase !== "connected") {
          setMessages([]);
          setDraft("");
        } else if (incoming.length) {
          after = incoming.at(-1)!.seq;
          setMessages(previous => [...previous, ...incoming].slice(-100));
        }
      } catch {
        if (disposed) return;
        setClosed(true);
        setMessages([]);
        setDraft("");
      } finally {
        if (!disposed) timer = window.setTimeout(poll, 300);
      }
    }
    void poll();
    return () => { disposed = true; window.clearTimeout(timer); };
  }, [id]);

  useLayoutEffect(() => {
    if (timeline.current) timeline.current.scrollTop = timeline.current.scrollHeight;
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
        <h2>{room ? privateTitle(room, own.id) : "Private chat"}</h2>
        <span role="status">{connected ? "Verified private connection" : failed ? "Connection failed" : terminal ? "Session ended" : "LAN only"}
          {room?.open && !terminal ? " · QR access" : ""}</span>
      </div>
      {!terminal && room?.open && room.hosted && <button className="icon-button" title="Share private QR code"
        aria-label="Share private QR code" onClick={() => void act(() => showCode(id))}><QrCode size={20} /></button>}
      {room && room.phase !== "invited" && <button className="button subtle" onClick={() => void act(async () => {
        await api.endPrivate(id);
        back();
      })}>{terminal ? "Dismiss" : "End session"}</button>}
    </header>
    {connected ? <>
      <div className="private-people" aria-label="Current participants">
        {people.map(person => <span key={person.id}>@{person.handle}</span>)}
      </div>
      <div className="timeline-wrap"><div className="timeline private-timeline" ref={timeline} role="log" aria-label="Private messages">
        {messages.map(message => <div className={"message-row" + (message.sender.id === own.id ? " mine" : "")} key={message.seq}>
          <div className="message-body">
            <span className="message-author">{message.sender.id === own.id ? "You" : "@" + message.sender.handle}</span>
            <div className="bubble">{message.text}</div>
          </div>
        </div>)}
      </div></div>
      <form className="composer" onSubmit={send}>
        {(failure || oversized) && <p className="inline-error" role="alert">
          {oversized ? "Private messages can be up to 2 KiB." : failure}
        </p>}
        <div className="composer-box">
          <textarea aria-label="Private message" placeholder="Write a private message…" rows={2} autoComplete="off"
            spellCheck={false} value={draft} onChange={event => setDraft(event.target.value)}
            onKeyDown={event => {
              if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
                event.preventDefault();
                event.currentTarget.form?.requestSubmit();
              }
            }} />
          <div className="composer-bar private-composer-bar">
            <button className="button primary send-button" type="submit" aria-label="Send private message"
              disabled={busy || !draft.trim() || oversized}>Send <Send size={16} /></button>
          </div>
        </div>
      </form>
    </> : <div className="private-status" aria-live="polite">
      {room?.phase === "invited" && !closed ? <InvitePrompt key={room.id} room={room} /> : <Blank
        icon={<Ghost size={26} />} title={room?.phase === "missed" ? "Missed private invitation"
          : failed ? "Private connection failed" : terminal ? "Private session ended"
          : room?.open && room.hosted ? "Waiting for a QR scan"
          : room ? privatePhaseLabel[room.phase] : "Opening private chat"}>
        {room?.phase === "missed" && <p>@{room.host.handle} invited you. The invitation ended before you responded.</p>}
        {failure && <p className="inline-error" role="alert">{failure}</p>}
      </Blank>}
    </div>}
  </section>;
}

export function privateTitle(room: PrivateSummary, own: string) {
  if (room.open) return "Private QR chat";
  const others = [room.host, ...room.invited].filter(person => person.id !== own);
  return others.length ? others.map(person => "@" + person.handle).join(", ") : "Private chat";
}
