import { ArrowLeft, Ghost, Send } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { FormEvent } from "react";
import { api, errorMessage } from "./api";
import type { PrivateMessage, PrivateStatus, User } from "./api";
import { Blank } from "./common";

type Phase = PrivateStatus | { state: "ready" };
const titles: Record<Phase["state"], string> = {
  ready: "Start a private chat",
  discovering: "Finding your contact on this LAN…",
  connecting: "Establishing an encrypted connection…",
  verify: "Verify your contact",
  waiting_for_peer: "Waiting for your contact to confirm…",
  connected: "Verified private connection",
  ended: "Private session ended",
  failed: "Private connection failed",
};

export function PrivateChatView({ conversation, peer, back }: {
  conversation: string; peer: User; back: () => void;
}) {
  const [phase, setPhase] = useState<Phase>({ state: "ready" });
  const [started, setStarted] = useState(false);
  const [session, setSession] = useState<string>();
  const [messages, setMessages] = useState<PrivateMessage[]>([]);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState("");
  const endRequested = useRef(false);
  const timeline = useRef<HTMLDivElement>(null);

  function finish(status: PrivateStatus) {
    setMessages([]);
    setDraft("");
    setFailure("");
    setSession(undefined);
    setPhase(status);
    setStarted(false);
  }

  useEffect(() => {
    if (!started) return;
    let disposed = false;
    let timer: number | undefined;
    let id: string | undefined;
    let after = 0;
    const current = () => !disposed && !endRequested.current;
    const end = (value: string) => void api.endPrivate(value).catch(() => {});

    async function poll(value: string) {
      try {
        const { messages: incoming, ...status } = await api.viewPrivate(value, after);
        if (!current()) return;
        if (status.state === "ended" || status.state === "failed") {
          finish(status);
          return;
        }
        setPhase(status);
        if (incoming.length) {
          after = incoming[incoming.length - 1].seq;
          setMessages(previous => [...previous, ...incoming].slice(-100));
        }
        timer = window.setTimeout(() => void poll(value), 250);
      } catch (error) {
        if (current()) finish({ state: "failed", reason: errorMessage(error) });
      }
    }

    void api.startPrivate(conversation).then(value => {
      if (!current()) { end(value); return; }
      id = value;
      setSession(value);
      void poll(value);
    }).catch(error => {
      if (current()) finish({ state: "failed", reason: errorMessage(error) });
    });
    return () => {
      disposed = true;
      window.clearTimeout(timer);
      if (id) end(id);
    };
  }, [conversation, started]);

  useLayoutEffect(() => {
    if (timeline.current) timeline.current.scrollTop = timeline.current.scrollHeight;
  }, [messages]);

  async function act(action: (id: string) => Promise<void>) {
    if (!session || busy) return;
    setBusy(true);
    setFailure("");
    try { await action(session); }
    catch (error) { setFailure(errorMessage(error)); }
    finally { setBusy(false); }
  }

  const oversized = new TextEncoder().encode(draft.trim()).length > 2048;
  function send(event: FormEvent) {
    event.preventDefault();
    const text = draft.trim();
    if (!text || oversized || phase.state !== "connected") return;
    void act(async id => {
      await api.sendPrivate(id, text);
      setDraft(current => current.trim() === text ? "" : current);
    });
  }

  const terminal = phase.state === "ended" || phase.state === "failed";
  return <section className="chat-view private-chat" aria-label="Private chat"
    onCopy={event => event.preventDefault()} onCut={event => event.preventDefault()}
    onDragStart={event => event.preventDefault()} onDrop={event => event.preventDefault()}>
    <header className="chat-header">
      <Ghost className="private-ghost" size={23} aria-hidden="true" />
      <div className="chat-heading"><h2>Private chat with @{peer.handle}</h2>
        <span role="status">{phase.state === "connected" ? titles.connected : "LAN only"}</span>
      </div>
      {phase.state === "ready" || terminal
        ? <button className="button subtle" onClick={back}><ArrowLeft size={16} /> Normal chat</button>
        : <button className="button subtle" onClick={() => {
          endRequested.current = true;
          finish({ state: "ended" });
        }}>End session</button>}
    </header>
    {phase.state === "connected" ? <>
      <div className="timeline-wrap">
        <div className="timeline private-timeline" ref={timeline} role="log" aria-label="Private messages">
          {messages.map(message => <div key={message.seq} className={"message-row" + (message.mine ? " mine" : "")}>
            <div className="message-body">
              <span className="message-author">{message.mine ? "You" : "@" + peer.handle}</span>
              <div className="bubble">{message.text}</div>
            </div>
          </div>)}
        </div>
      </div>
      <form className="composer" onSubmit={send}>
        {(failure || oversized) && <p className="inline-error" role="alert">
          {oversized ? "Private messages can be up to 2 KiB." : failure}
        </p>}
        <div className="composer-box">
          <textarea aria-label="Private message" placeholder="Write a private message…" rows={2} value={draft}
            autoComplete="off" spellCheck={false} onChange={event => setDraft(event.target.value)}
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
      <Blank icon={<Ghost size={27} />} title={titles[phase.state]}>
        {phase.state === "ready" && <>
          <p>Both people must open Private chat and start discovery on the same reachable LAN. Messages are cleared when the session ends.</p>
          <button className="button primary" onClick={() => {
            setPhase({ state: "discovering" });
            setStarted(true);
          }}>Start discovery</button>
        </>}
        {phase.state === "discovering" && <p>Ask @{peer.handle} to start discovery in your one-to-one chat.</p>}
        {(phase.state === "verify" || phase.state === "waiting_for_peer") && <>
          <p>Compare all eight groups with @{peer.handle} in person or on a trusted call. Confirm only if every group matches.</p>
          <code className="private-code" aria-label={"Verification code " + phase.code}>
            {phase.code.split(" ").map((group, index) => <span key={index}>{group}</span>)}
          </code>
          {phase.state === "verify" && <div className="private-actions">
            <button className="button subtle" disabled={busy} onClick={() => void act(id => api.verifyPrivate(id, false))}>Decline</button>
            <button className="button primary" disabled={busy} onClick={() => void act(id => api.verifyPrivate(id, true))}>Codes match</button>
          </div>}
        </>}
        {phase.state === "failed" && <p role="alert">{phase.reason}</p>}
        {phase.state === "ended" && <p>The private conversation has been cleared.</p>}
        {failure && <p className="inline-error" role="alert">{failure}</p>}
      </Blank>
    </div>}
  </section>;
}
