import { FloatingField } from "./floating-field";
import { RichMessage } from "./rich-message";
import { ArrowDown, ArrowLeft, Copy, Ghost, QrCode, Send, UserPlus } from "lucide-react";
import { listen } from "@tauri-apps/api/event";
import { Fragment, useCallback, useContext, useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import type { FormEvent } from "react";
import { api, errorMessage } from "./api";
import { copyText, useRowMenu } from "./context-menu";
import { ios } from "./ios";
import { t } from "./locale";
import { windows } from "./window-chrome";
import type { Conversation, Directory, Discovery, NearbyDevice, PrivateList, PrivateMessage, PrivateSummary, User } from "./api";
import { Avatar, Blank, Choice, Dialog, People, PersonAvatar, displayName, messageTimeLabel, startsTimeGroup } from "./common";

export function usePrivateRooms() {
  const [list, setList] = useState<PrivateList>({ rooms: [], prompt: null });
  const generation = useRef(0);
  const forget = useCallback((id: string) => {
    generation.current += 1;
    setList(current => ({ rooms: current.rooms.filter(room => room.id !== id), prompt: current.prompt === id ? null : current.prompt }));
  }, []);
  useEffect(() => {
    let disposed = false;
    let timer: number | undefined;
    const reset = ios ? listen("ios-private-reset", () => {
      generation.current += 1;
      setList({ rooms: [], prompt: null });
    }) : undefined;
    const accepted = listen<{ id: string }>("private-invitation-accepted", ({ payload }) => {
      generation.current += 1;
      setList(current => ({
        rooms: current.rooms.map(room => room.id === payload.id && room.phase === "invited"
          ? { ...room, phase: "authenticating" } : room),
        prompt: current.prompt === payload.id ? null : current.prompt,
      }));
    });
    async function refresh() {
      const current = generation.current;
      try {
        const next = await api.privateRooms();
        if (!disposed && current === generation.current) setList(next);
      } catch {
        // A failed IPC read does not report a room ending.
      } finally {
        if (!disposed) timer = window.setTimeout(refresh, 500);
      }
    }
    void refresh();
    return () => {
      disposed = true;
      window.clearTimeout(timer);
      void reset?.then(unlisten => unlisten());
      void accepted.then(unlisten => unlisten());
    };
  }, []);
  return { ...list, forget };
}

export function privateSessionActive(room: PrivateSummary) {
  return room.phase === "connected" || room.phase === "suspended" || room.phase === "reconnecting";
}

export function exclusivePrivateRoom(rooms: PrivateSummary[], chat: Conversation | undefined) {
  if (!windows || !chat || chat.title != null) return undefined;
  return rooms.find(room => room.conversation === chat.id && privateSessionActive(room));
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
  return <dialog ref={element} className="private-invite-modal" aria-label={t("Private chat invitation", "私密聊天邀请")}
    onCancel={event => event.preventDefault()}>
    <InvitePrompt room={room} />
  </dialog>;
}

const privatePhaseLabels: Record<PrivateSummary["phase"], [string, string]> = {
  discovering: ["Finding LAN devices", "正在查找局域网设备"],
  waiting: ["Waiting for approval", "等待同意"],
  invited: ["Invitation", "邀请"],
  authenticating: ["Verifying account consent", "正在验证账号授权"],
  connected: ["Connected", "已连接"],
  suspended: ["LAN connection interrupted", "局域网连接已中断"],
  reconnecting: ["Reconnecting on LAN", "正在重新连接局域网"],
  missed: ["Missed invitation", "错过邀请"],
  failed: ["Connection interrupted", "连接已中断"],
  ended: ["Ended", "已结束"],
};

export const privatePhaseLabel = (phase: PrivateSummary["phase"]) => t(...privatePhaseLabels[phase]);

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
    {room.invited.length > 1 && <ul className="invitees" aria-label={t("Invited accounts", "受邀账号")} tabIndex={compact ? 0 : undefined}>
      {room.invited.map(person => <li key={person.id}>{displayName(person, names)}</li>)}
    </ul>}
    <div className="private-actions">
      <button className="button subtle" disabled={busy} onClick={() => void respond(false)}>{t("Decline", "拒绝")}</button>
      <button className="button primary" disabled={busy} onClick={() => void respond(true)}>{t("Accept", "接受")}</button>
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
  return <Dialog title={t("Invite to private chat", "邀请加入私密聊天")} close={() => { if (!busy) close(); }}>
    <fieldset className="dialog-contacts"><legend>{t("Group members", "群成员")}</legend>
      {people.map(person => <Choice key={person.id} disabled={busy} checked={selected.includes(person.id)}
        onChange={event => setSelected(previous => event.target.checked
          ? [...previous, person.id] : previous.filter(id => id !== person.id))}>
        <PersonAvatar user={person} /><span>{displayName(person, remarks)}</span>
      </Choice>)}
    </fieldset>
    {failure && <p className="inline-error" role="alert">{failure}</p>}
    <div className="dialog-actions">
      <button className="button subtle" disabled={busy} onClick={close}>{t("Cancel", "取消")}</button>
      <button className="button primary" disabled={busy || selected.length === 0} onClick={async () => {
        setBusy(true);
        try { await start(selected); }
        catch (error) { setFailure(errorMessage(error)); setBusy(false); }
      }}>{t("Send invitation", "发送邀请")}</button>
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

  return <Dialog title={t("Nearby", "附近设备")} close={close} closeDisabled={busy}>
    {devices.length ? <fieldset className="dialog-contacts" disabled={busy}>
      <legend>{t("Seen on LAN · identity unverified", "局域网内发现 · 身份未验证")}</legend>
      {devices.map(device => <Choice key={device.id} type="radio" name="nearby-device" checked={selected?.id === device.id}
        onChange={() => { setChoice(device); setFailure(""); }}>
        <span className="chat-row-text">
          <strong>{device.matched_account ? t(`Possible match: ${displayName(device.matched_account, remarks)}`, `可能是 ${displayName(device.matched_account, remarks)}`) : t("Unknown Zibblu device", "未知 Zibblu 设备")}</strong>
          <small>{t("Device", "设备")}{" "}{device.id.slice(0, 8)}</small>
        </span>
      </Choice>)}
    </fieldset> : <p className="muted" role="status">
      {status === "starting" ? t("Starting LAN discovery…", "正在启动局域网发现…")
        : status === "network_unavailable" ? (ios ? t("Nearby requires an allowed Wi-Fi network.", "附近设备功能需要接入允许的 Wi-Fi 网络。")
          : t("Nearby requires an approved IPv4 LAN. Connect to an allowed Wi-Fi or Ethernet network.", "附近设备功能需要接入允许的 IPv4 局域网。请连接允许的 Wi-Fi 或以太网。"))
        : status === "permission_denied" ? t("Allow Zibblu to use the Local Network in iOS Settings, then return here.", "请在 iOS 设置中允许 Zibblu 访问本地网络，然后返回这里。")
        : status === "discovery_failed" ? t("LAN discovery could not start or the configured discovery service failed.", "无法启动局域网发现，或配置的发现服务出现故障。")
        : t("No Zibblu devices found. Keep both clients open and signed in on the same allowed network.", "未发现 Zibblu 设备。请在同一允许的网络上打开并登录两端客户端。")}
    </p>}
    {selected && !account && <p className="muted">{t("Zibblu verifies the selected device’s account before sending the invitation.", "发送邀请前，Zibblu 会验证所选设备的账号。")}</p>}
    {account === own && <p className="muted">{t("Private invitations go to a different account.", "请向其他账号发送私密聊天邀请。")}</p>}
    {choice && !selected && status === "ready" && <p className="muted" role="status">{t("The selected device is no longer listed for that account.", "该账号下已找不到所选设备。")}</p>}
    {failure && <p className="inline-error" role="alert">{failure}</p>}
    {((status === "ready" && devices.length === 0) || status === "discovery_failed" || failure) && <details className="nearby-help">
      <summary className="muted">{t("Troubleshooting tips", "排查连接问题")}</summary>
      <p className="muted">{ios ? t("Check Local Network access in iOS Settings and make sure Wi-Fi allows devices to communicate.", "请确认 iOS 设置已授权访问本地网络，且 Wi-Fi 允许设备互相通信。")
        : t("Check that Windows Defender Firewall allows Zibblu on the Private profile. For managed networks, ensure mDNS and direct UDP are permitted.", "请确认 Windows Defender 防火墙允许 Zibblu 使用专用网络。在受管理的网络上，还需允许 mDNS 和 UDP 直连。")}</p>
    </details>}
    <div className="dialog-actions">
      <button className="button subtle" disabled={busy} onClick={close}>{t("Close", "关闭")}</button>
      <button className="button primary" disabled={busy || !selected || account === own}
        onClick={() => void invite()}>{busy ? t("Connecting…", "正在连接…") : t("Send private invitation", "发送私密聊天邀请")}</button>
    </div>
  </Dialog>;
}

export function PrivateChatView({ id, own, back, showCode, draft, setDraft, isContact, onClosed, switchMode }: {
  id: string; own: User; back: () => void; showCode: (id: string) => Promise<void>;
  draft: string; setDraft: (change: (draft: string) => string) => void; isContact: (id: string) => boolean;
  onClosed: (invalidated?: boolean) => void; switchMode?: () => void;
}) {
  const { remarks, open } = useContext(People);
  const messageMenu = useRowMenu<HTMLDivElement>();
  const inputId = useId();
  const [room, setRoom] = useState<PrivateSummary>();
  const [messages, setMessages] = useState<PrivateMessage[]>([]);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState("");
  const [connectionId, setConnectionId] = useState<string>();
  const marked = useRef<{ connection: string; through: number }>(undefined);
  const timeline = useRef<HTMLDivElement>(null);
  const latestAtOpen = useRef<number | undefined>(undefined);
  const [away, setAway] = useState(false);
  const nearBottom = useRef(true);
  const clearDraft = useRef(setDraft);
  clearDraft.current = setDraft;
  const closeRoom = useRef(onClosed);
  closeRoom.current = onClosed;
  const connected = room?.phase === "connected";
  const [previewReady, setPreviewReady] = useState(!windows);
  useEffect(() => {
    if (!windows) return;
    let current = true;
    void api.privatePreview(true).then(() => { if (current) setPreviewReady(true); })
      .catch(error => { if (current) setFailure(errorMessage(error)); });
    return () => {
      current = false;
      void api.privatePreview(false).catch(() => {});
    };
  }, []);

  useEffect(() => {
    let disposed = false;
    let timer: number | undefined;
    let after = 0;
    let connection: string | undefined;
    function finish() {
      disposed = true;
      setMessages([]);
      closeRoom.current(true);
    }
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
          setConnectionId(connection);
          after = 0;
          setMessages([]);
          if (!windows && latestAtOpen.current !== undefined) clearDraft.current(() => "");
          latestAtOpen.current = undefined;
        }
        const { messages: incoming, ...summary } = result;
        if (summary.phase === "ended" || summary.phase === "failed" || summary.phase === "missed") {
          finish();
          return;
        }
        setRoom(summary);
        if (summary.phase !== "connected" && !(windows && privateSessionActive(summary))) {
          setMessages([]);
          clearDraft.current(() => "");
        } else if (incoming.length) {
          after = incoming.at(-1)!.seq;
          setMessages(previous => [...previous, ...incoming].slice(-100));
        }
        if (summary.phase === "connected") latestAtOpen.current ??= incoming.at(-1)?.seq ?? 0;
      } catch {
        if (disposed) return;
        finish();
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

  useEffect(() => {
    const last = messages.at(-1)?.seq;
    if (!connected || !connectionId || last === undefined) return;
    const connection = connectionId;
    const through = last;
    function readVisible() {
      if (away || document.hidden || !document.hasFocus() || !timeline.current?.getClientRects().length
          || document.querySelector("dialog[open]")) return;
      if (marked.current?.connection === connection && marked.current.through >= through) return;
      marked.current = { connection, through };
      void api.markPrivateRead(id, connection, through).catch(() => false).then(recorded => {
        if (!recorded && marked.current?.connection === connection && marked.current.through === through) marked.current = undefined;
      });
    }
    readVisible();
    window.addEventListener("focus", readVisible);
    document.addEventListener("visibilitychange", readVisible);
    return () => {
      window.removeEventListener("focus", readVisible);
      document.removeEventListener("visibilitychange", readVisible);
    };
  }, [away, connected, connectionId, id, messages]);

  async function act(action: () => Promise<void>) {
    if (busy) return;
    setBusy(true);
    setFailure("");
    try { await action(); }
    catch (error) { setFailure(errorMessage(error)); }
    finally { setBusy(false); }
  }

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

  return <section className="chat-view private-chat" aria-label={t("Private chat", "私密聊天")}
    onCopy={event => {
      const selection = window.getSelection();
      const start = selection?.anchorNode;
      const bubble = (start instanceof Element ? start : start?.parentElement)?.closest(".rich-message");
      if (!bubble || !event.currentTarget.contains(bubble) || !bubble.contains(selection?.focusNode ?? null)) event.preventDefault();
    }} onCut={event => event.preventDefault()}
    onDragStart={event => event.preventDefault()} onDrop={event => event.preventDefault()}>
    <header className="chat-header">
      <button className="icon-button" onClick={back} aria-label={t("Return to chats", "返回聊天")}><ArrowLeft size={19} /></button>
      {switchMode
        ? <button className="icon-button private-ghost" type="button" aria-label={t("Switch to normal chat", "切换到普通聊天")}
          title={t("Switch to normal chat", "切换到普通聊天")} onClick={switchMode}><Ghost size={22} /></button>
        : <Ghost className="private-ghost" size={22} aria-hidden="true" />}
      <div className="chat-heading">
        <h2>{room ? privateTitle(room, own.id, remarks) : t("Private chat", "私密聊天")}</h2>
        <span role="status">{connected ? t("Verified private connection", "已验证私密连接") : room ? privatePhaseLabel(room.phase) : ""}
          {" · "}{t("LAN only", "仅局域网")}{room?.open ? t(" · QR access", " · 扫码加入") : ""}</span>
      </div>
      {room?.open && room.hosted && <button className="icon-button" title={t("Share private QR code", "分享私密聊天二维码")}
        aria-label={t("Share private QR code", "分享私密聊天二维码")} onClick={() => void act(() => showCode(id))}><QrCode size={20} /></button>}
      {room && room.phase !== "invited" && <button className="button subtle" disabled={busy} onClick={() => void act(async () => {
        await api.endPrivate(id);
        closeRoom.current();
      })}>{t("End session", "结束会话")}</button>}
    </header>
    {previewReady && connected ? <>
      <div className="private-people" aria-label={t("Current participants", "当前参与者")}>
        {people.map(person => <button className="person-link" key={person.id} onClick={() => open?.(person)}
          title={person.id !== own.id && !isContact(person.id) ? t("View profile or add contact", "查看资料或添加联系人") : t("View profile", "查看资料")}>
          {person.id !== own.id && !isContact(person.id) && <UserPlus size={14} />}{displayName(person, remarks)}
        </button>)}
      </div>
      <div className="timeline-wrap"><div className="timeline" ref={timeline} role="log" aria-label={t("Private messages", "私密消息")} onScroll={event => {
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
            <div className={"message-row" + (mine ? " mine" : "") + (message.seq > (latestAtOpen.current ?? 0) ? " message-arrival" : "")}
              tabIndex={0} aria-haspopup="menu" {...messageMenu(anchor => {
                const selection = window.getSelection();
                const text = anchor.contains(selection?.anchorNode ?? null) && anchor.contains(selection?.focusNode ?? null)
                  ? selection?.toString() || message.text : message.text;
                return [{
                  label: t("Copy", "复制"), icon: <Copy size={15} />,
                  action: () => { void copyText(text).catch(error => setFailure(errorMessage(error))); },
                }];
              })}>
              {!mine && avatar}
              <div className="message-body">
                {!mine && <span className="message-author">{displayName(message.sender, remarks)}</span>}
                <RichMessage text={message.text} privateRoom />
              </div>
              {mine && avatar}
            </div>
          </Fragment>;
        })}
      </div>{away && <button className="button subtle jump-bottom" onClick={() => {
        if (timeline.current) timeline.current.scrollTop = timeline.current.scrollHeight;
      }}><ArrowDown size={15} />{t("Latest messages", "最新消息")}</button>}</div>
      <form className="composer" onSubmit={send}>
        {(failure || oversized) && <p className="inline-error" role="alert">
          {oversized ? t("Private messages can be up to 2 KiB.", "私密消息最多 2 KiB。") : failure}
        </p>}
        <div className="composer-box">
          <FloatingField multiline id={inputId} label={t("Private message", "私密消息")} rows={2} autoComplete="off"
            spellCheck={false} value={draft} onChange={event => { const value = event.target.value; setDraft(() => value); }}
            onKeyDown={event => {
              if (!ios && event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
                event.preventDefault();
                event.currentTarget.form?.requestSubmit();
              }
            }} />
            <button className="button primary send-button" type="submit" aria-label={t("Send private message", "发送私密消息")}
              disabled={busy || !draft.trim() || oversized}>{busy ? t("Sending…", "正在发送…") : t("Send", "发送")} <Send size={16} /></button>
        </div>
      </form>
    </> : <div className="private-status" aria-live="polite">
      {room?.phase === "invited" ? <InvitePrompt key={room.id} room={room} /> : <Blank
        icon={<Ghost size={26} />} title={room?.open && room.hosted ? t("Waiting for a QR scan", "等待扫码")
          : room ? privatePhaseLabel(room.phase) : t("Opening private chat", "正在打开私密聊天")}>
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
