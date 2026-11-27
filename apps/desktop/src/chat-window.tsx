import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { MessageCircle, X } from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { ai, builtinAssistantId } from "./ai-api";
import type { AiJob, AiPrompt, AiReply, Assistant } from "./ai-api";
import { api, errorMessage } from "./api";
import type { AddDraftsResult, Directory, Draft, DropOffer, Profile, SyncUpdate, Transfer, User } from "./api";
import { ChatView } from "./chat";
import { builtinAssistantName } from "./ai";
import { BuiltinAssistant } from "./builtin-assistant";
import { ChatHistoryCache } from "./chat-history-cache";
import { Blank, People, chatName } from "./common";
import { t } from "./locale";
import { ProfileView } from "./profile";
import { WindowControls, WindowDrag, windows } from "./window-chrome";
import { ImageCache, ProfileCache } from "./workspace-cache";
import { WorkspaceImages } from "./workspace-image";

export type ChatWindowSession = {
  user: User;
  server: string;
  conversation_id: string;
};

const appWindow = getCurrentWindow();
const hour = 60 * 60 * 1000;

export function ChatWindow() {
  const [session, setSession] = useState<ChatWindowSession>();
  const [bootError, setBootError] = useState("");
  const [loaded, setLoaded] = useState(false);
  const [directory, setDirectory] = useState<Directory>({
    contacts: [],
    conversations: [],
    pending_contacts: [],
    remarks: {},
    names: {},
    avatars: {},
    read_receipts: false,
  });
  const [assistants, setAssistants] = useState<Assistant[]>([]);
  const [aiPrompts, setAiPrompts] = useState<AiPrompt[]>([]);
  const [aiJobs, setAiJobs] = useState<AiJob[]>([]);
  const [aiReplies, setAiReplies] = useState<AiReply[]>([]);
  const [recalled, setRecalled] = useState<ReadonlySet<number>>(new Set());
  const [hidden, setHidden] = useState<ReadonlySet<number>>(new Set());
  const [revision, setRevision] = useState(0);
  const [latestAiSeq, setLatestAiSeq] = useState(0);
  const [clockOffset, setClockOffset] = useState(0);
  const [transfers, setTransfers] = useState<Transfer[]>([]);
  const [draft, setDraftState] = useState<Draft>({ text: "" });
  const [sending, setSending] = useState(false);
  const sendingRef = useRef(false);
  const [failure, setFailure] = useState("");
  const [quickProfile, setQuickProfile] = useState<{ user: User; point: { x: number; y: number } }>();
  const profilePoint = useRef({ x: 0, y: 0 });
  const appliedRevision = useRef(0);
  const [historyCache] = useState(() => new ChatHistoryCache());
  const [profiles] = useState(() => new ProfileCache());
  const [images] = useState(() => new ImageCache());

  useEffect(() => {
    let active = true;
    invoke<ChatWindowSession>("chat_window_session")
      .then(value => {
        if (active) setSession(value);
      })
      .catch(error => {
        if (active) setBootError(errorMessage(error));
      });
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    let active = true;
    const refreshSettings = () => void ai.assistants().then(value => {
      if (active) setAssistants(value);
    }).catch(() => {});
    refreshSettings();
    const unlisten = listen("zibblu-assistant-settings", refreshSettings);
    return () => {
      active = false;
      void unlisten.then(stop => stop());
    };
  }, []);

  const apply = useCallback((update: SyncUpdate) => {
    if (update.revision < appliedRevision.current) return;
    appliedRevision.current = update.revision;
    setDirectory(update.directory);
    const offset = update.server_now - Date.now();
    setClockOffset(previous => Math.abs(previous - offset) < 250 ? previous : offset);
    setRecalled(previous => previous.size === update.recalled.length && update.recalled.every(seq => previous.has(seq))
      ? previous : new Set(update.recalled));
    setHidden(previous => update.deleted.every(seq => previous.has(seq))
      ? previous : new Set([...previous, ...update.deleted]));
    setRevision(update.revision);
    setLatestAiSeq(update.latest_ai_seq);
    setLoaded(true);
  }, []);

  const refreshLocal = useCallback(async () => {
    try {
      apply(await api.localUpdate());
    } catch {
      // Keep last rendered state if a transient local read fails.
    }
  }, [apply]);

  const refresh = useCallback(async () => {
    await refreshLocal();
    try {
      apply(await api.sync());
    } catch {
      // Main window owns background sync; avoid turning standalone window into a sync poller.
    }
  }, [apply, refreshLocal]);

  const changed = useCallback(() => {
    void api.transfers().then(setTransfers).catch(error => setFailure(errorMessage(error)));
  }, []);

  const aiOrder = useRef(0);
  const refreshAi = useCallback(async (convId: string) => {
    const ticket = ++aiOrder.current;
    try {
      const [prompts, jobs, replies] = await Promise.all([
        ai.prompts(convId),
        ai.jobs(convId),
        ai.replies(convId),
      ]);
      if (ticket !== aiOrder.current) return;
      setAiPrompts(prompts);
      setAiJobs(jobs);
      setAiReplies(replies);
    } catch {
      // Keep last known AI state if transient fetch fails.
    }
  }, []);

  useEffect(() => {
    if (!session) return;
    const convId = session.conversation_id;
    void refreshLocal();
    void refreshAi(convId);
    changed();
    const onChat = listen("zibblu-chat-changed", () => void refreshLocal());
    const onOutbox = listen("message-outbox-changed", () => void refreshLocal());
    const onAi = listen<string | undefined>("zibblu-ai-changed", event => {
      if (!event.payload || event.payload === convId) {
        void refreshLocal();
        void refreshAi(convId);
      }
    });
    return () => {
      void onChat.then(unlisten => unlisten());
      void onOutbox.then(unlisten => unlisten());
      void onAi.then(unlisten => unlisten());
    };
  }, [session, refreshLocal, refreshAi, changed]);

  useEffect(() => {
    if (!session) return;
    let disposed = false;
    let timer: number | undefined;
    async function pollTransfers() {
      try {
        const list = await api.transfers();
        if (!disposed) setTransfers(list);
      } catch {
        // Ignore transient transfer read failures in standalone window.
      }
      if (!disposed) timer = window.setTimeout(pollTransfers, 750);
    }
    void pollTransfers();
    return () => {
      disposed = true;
      window.clearTimeout(timer);
    };
  }, [session]);

  const setDraft = useCallback((change: (current: Draft) => Draft) => {
    setDraftState(previous => {
      const next = change(previous);
      const attachments = next.attachments?.length ? next.attachments : undefined;
      const cleaned = attachments === next.attachments ? next : { ...next, attachments };
      if (cleaned.text || cleaned.quote || cleaned.attachments?.length) return cleaned;
      return { text: "" };
    });
  }, []);

  const appendDraftResult = useCallback((result: AddDraftsResult) => {
    if (result.added.length > 0) {
      setDraft(previous => {
        const existing = previous.attachments ?? [];
        const seen = new Set(existing.map(item => item.id));
        const added = result.added.filter(item => !seen.has(item.id));
        return added.length ? { ...previous, attachments: [...existing, ...added] } : previous;
      });
    }
    if (result.errors.length > 0) {
      setFailure(result.errors.join("\n"));
    }
  }, [setDraft]);

  useEffect(() => {
    if (!session) return;
    const convId = session.conversation_id;
    const drop = listen<DropOffer>("attachment-drop", event => {
      setFailure("");
      void api.addDroppedFiles(convId, event.payload.ticket)
        .then(appendDraftResult)
        .catch(error => setFailure(errorMessage(error)));
    });
    const dropError = listen<string>("attachment-drop-error", event => {
      setFailure(event.payload);
    });
    return () => {
      void drop.then(unlisten => unlisten());
      void dropError.then(unlisten => unlisten());
    };
  }, [session, appendDraftResult]);

  const conversationId = session?.conversation_id;
  const active = conversationId
    ? directory.conversations.find(chat => chat.id === conversationId)
    : undefined;

  const previewAccess = useMemo(() => {
    const conversations = new Map(directory.conversations.map(chat => [chat.id, chat.joined_after ?? null]));
    return (id: string, seq: number) => {
      const cutoff = conversations.get(id);
      if (cutoff === undefined || recalled.has(seq) || hidden.has(seq)) return;
      if (cutoff === null || seq > cutoff) return "cached" as const;
    };
  }, [directory.conversations, recalled, hidden]);

  useLayoutEffect(() => {
    if (!session) return;
    const people = new Set(directory.contacts.map(user => user.id));
    profiles.retain(key => {
      const [id, relation] = key.split("/");
      return relation === "own" ? id === session.user.id : people.has(id);
    });
    images.retain(key => {
      const [type, id, version] = key.split("/");
      if (type === "avatar") return directory.avatars[id] === version;
      if (type === "cover") {
        return (id === session.user.id || people.has(id))
          && profiles.get(id + (id === session.user.id ? "/own" : "/contact"))?.profile.cover === version;
      }
      return previewAccess(id, Number(version)) === "cached";
    });
  }, [session, directory.contacts, directory.avatars, previewAccess, images, profiles]);

  useLayoutEffect(() => () => {
    profiles.retain(() => false);
    images.retain(() => false);
  }, [profiles, images]);

  const names = useMemo(() => ({ ...directory.names, ...directory.remarks }), [directory.names, directory.remarks]);
  const windowTitle = session && active
    ? chatName(active, session.user.id, names)
    : conversationId === builtinAssistantId ? builtinAssistantName() : "Zibblu";

  useEffect(() => {
    document.title = windowTitle;
  }, [windowTitle]);

  const beginSending = useCallback(() => {
    if (sendingRef.current) return false;
    sendingRef.current = true;
    setSending(true);
    return true;
  }, []);

  const endSending = useCallback(() => {
    if (!sendingRef.current) return;
    sendingRef.current = false;
    setSending(false);
  }, []);

  async function pickAttachments(kind: "file" | "folder") {
    if (!session) return;
    setFailure("");
    try {
      const result = await api.pickDraftAttachments(session.conversation_id, kind);
      if (result) appendDraftResult(result);
    } catch (error) {
      setFailure(errorMessage(error));
    }
  }

  async function pasteFiles() {
    if (!session) return false;
    const result = await api.addClipboardFiles(session.conversation_id);
    if (!result) return false;
    appendDraftResult(result);
    return true;
  }

  async function hideMessage(seq: number) {
    if (!session) return;
    await api.hideMessage(session.conversation_id, seq);
    setHidden(previous => new Set(previous).add(seq));
    setDraft(previous => previous.quote?.seq === seq ? { ...previous, quote: undefined, nonce: undefined } : previous);
    changed();
    await refreshLocal();
  }

  async function removeHistory(entire: boolean) {
    if (!session) return;
    if (entire) await api.deleteConversation(session.conversation_id);
    else await api.clearHistory(session.conversation_id);
    historyCache.discard(session.conversation_id);
    setDraft(previous => entire ? { text: "" } : { ...previous, quote: undefined, nonce: undefined });
    await refreshLocal();
    changed();
    if (entire) void appWindow.close();
  }

  function profileEdited(id: string, patch: Partial<Profile>) {
    setDirectory(previous => {
      const next = {
        ...previous,
        names: { ...previous.names },
        avatars: { ...previous.avatars },
        remarks: { ...previous.remarks },
      };
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

  const imageScope = useMemo(
    () => windows ? { cache: images, clockOffset, previewAccess, avatarExpires: Date.now() + 24 * hour } : undefined,
    [images, clockOffset, previewAccess],
  );

  const profileVersion = (id: string) => JSON.stringify([directory.names[id], directory.remarks[id], directory.avatars[id]]);

  return <WorkspaceImages.Provider value={imageScope}>
    <People.Provider value={{
      remarks: names,
      avatars: directory.avatars,
      online: true,
      open: user => setQuickProfile({ user, point: profilePoint.current }),
    }}>
      <main
        className="shell standalone-chat-shell"
        style={{ gridTemplateColumns: "minmax(0, 1fr)" }}
        onClickCapture={event => {
          const target = (event.target as Element).closest("button")?.getBoundingClientRect();
          profilePoint.current = event.detail
            ? { x: event.clientX, y: event.clientY }
            : { x: target?.right ?? 0, y: target?.top ?? 0 };
        }}
      >
        <div className="shell-main">
          <header className={"topbar" + (windows ? " windows-topbar" : "")}>
            <WindowDrag className="app-name"><span className="standalone-window-title">{windowTitle}</span></WindowDrag>
            {windows && <WindowControls />}
          </header>
          {failure && <div className="workspace-error" role="alert">
            {failure}
            <button className="icon-button" aria-label={t("Dismiss error", "关闭提示")} onClick={() => setFailure("")}>
              <X size={17} />
            </button>
          </div>}
          <div className="workspace-content" style={windows ? { marginLeft: "var(--shell-inset)" } : undefined}>
            {bootError ? (
              <section className="chat-blank">
                <Blank icon={<MessageCircle size={28} />} title={t("Unable to open chat window", "无法打开独立聊天窗口")}>
                  <p className="inline-error" role="alert">{bootError}</p>
                  <button type="button" className="button primary" onClick={() => void appWindow.close()}>
                    {t("Close window", "关闭窗口")}
                  </button>
                </Blank>
              </section>
            ) : !session || !loaded ? (
              <section className="chat-blank">
                <Blank icon={<MessageCircle size={28} />} title={t("Loading chat…", "正在加载会话…")} />
              </section>
            ) : conversationId === builtinAssistantId ? (
              <BuiltinAssistant own={session.user} assistants={assistants} chats={directory.conversations}
                prompts={aiPrompts} replies={aiReplies} jobs={aiJobs} refresh={refresh}
                back={() => void appWindow.close()} settings={() => void ai.openBuiltinSettings().catch(error => setFailure(errorMessage(error)))} />
            ) : active ? (
              <ChatView
                key={active.id + ":" + (active.joined_after ?? "")}
                standalone
                chat={active}
                own={session.user}
                server={session.server}
                revision={revision}
                latestAiSeq={latestAiSeq}
                recalled={recalled}
                clockOffset={clockOffset}
                transfers={transfers}
                changed={changed}
                refresh={refresh}
                back={() => void appWindow.close()}
                pickAttachments={pickAttachments}
                pasteFiles={pasteFiles}
                openPrivate={() => setFailure(t("Open private chats from the main Zibblu window.", "请在主窗口中发起或查看私密聊天。"))}
                aiChat={{
                  assistants,
                  settings: () => setFailure(t("Manage AI assistants in the main window Settings.", "请在主窗口设置中管理 AI 助理。")),
                  prompts: aiPrompts,
                  jobs: aiJobs,
                  replies: aiReplies,
                  cancelJob: id => {
                    void ai.cancelJob(id).catch(error => setFailure(errorMessage(error)));
                  },
                  dismissJob: id => {
                    void ai.dismissJob(id).catch(error => setFailure(errorMessage(error)));
                  },
                  dismiss: id => {
                    void ai.dismissReply(id).catch(error => setFailure(errorMessage(error)));
                  },
                }}
                hidden={hidden}
                hide={hideMessage}
                removeHistory={removeHistory}
                readReceipts={directory.read_receipts && !quickProfile}
                draft={draft}
                setDraft={setDraft}
                sending={sending}
                beginSending={beginSending}
                endSending={endSending}
                profileOpen={false}
                visibleOnNarrow={true}
                historyCache={historyCache}
              />
            ) : (
              <section className="chat-blank">
                <Blank icon={<MessageCircle size={28} />} title={t("This conversation is no longer available", "该会话已不可用")}>
                  <p className="muted">{t("It may have been deleted or you may have left the group.", "该会话可能已被删除，或你已退出该群聊。")}</p>
                  <button type="button" className="button primary" onClick={() => void appWindow.close()}>
                    {t("Close window", "关闭窗口")}
                  </button>
                </Blank>
              </section>
            )}
          </div>
        </div>
        {session && quickProfile && (
          <ProfileView
            key={quickProfile.user.id}
            popup
            point={quickProfile.point}
            user={quickProfile.user}
            session={{ user: session.user, server: session.server }}
            cache={profiles}
            directoryVersion={profileVersion(quickProfile.user.id)}
            relationVersion={directory.contacts.some(person => person.id === quickProfile.user.id)
              ? "contact"
              : directory.pending_contacts.find(request => request.user.id === quickProfile.user.id)?.request_id ?? "other"}
            reconnecting={false}
            refresh={refresh}
            close={() => setQuickProfile(undefined)}
            edited={profileEdited}
          />
        )}
      </main>
    </People.Provider>
  </WorkspaceImages.Provider>;
}
