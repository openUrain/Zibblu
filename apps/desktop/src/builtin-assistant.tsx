import { listen } from "@tauri-apps/api/event";
import { ArrowLeft, Bot, Check, ChevronDown, MessageSquare, Pencil, Plus, RefreshCw, Settings, Sparkles, X } from "lucide-react";
import { Fragment, useContext, useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import type { FormEvent } from "react";
import { AiJobBubble, AiPromptBubble, AiSources, builtinAssistantName, ProviderLogo, ShareAiReply, useAiMessageMenu } from "./ai";
import { ai, aiProviders, builtinAssistantId } from "./ai-api";
import { useAiTerms } from "./ai-terms";
import type { AiJob, AiPrompt, AiReply, Assistant, BuiltinModel, BuiltinSelection } from "./ai-api";
import { errorMessage } from "./api";
import type { Conversation, User } from "./api";
import { Blank, chatName, Dialog, messageTimeLabel, People, startsTimeGroup } from "./common";
import { clearMessageSelectionOnBlank, useEditMenu, useMenu, useMenuAnchor } from "./context-menu";
import type { MenuItem } from "./context-menu";
import { FloatingField } from "./floating-field";
import { ios } from "./ios";
import { t } from "./locale";
import { RichMessage } from "./rich-message";
import { windows } from "./window-chrome";

const MAX_ATTACHED_CHATS = 8;

const effortLabels: Record<string, [string, string]> = {
  none: ["Off", "关闭"], low: ["Low", "低"], medium: ["Medium", "中"],
  high: ["High", "高"], xhigh: ["Extra high", "很高"], max: ["Max", "最高"],
};

function syncScrollEdges(editor: HTMLTextAreaElement) {
  const { scrollTop, scrollHeight, clientHeight } = editor;
  editor.toggleAttribute("data-clipped-top", scrollTop > 0);
  editor.toggleAttribute("data-clipped-bottom", scrollHeight - clientHeight - scrollTop > 1);
}

export function BuiltinAssistant({ own, assistants, chats, prompts, replies, jobs, settings, refresh, back }: {
  own: User; assistants: Assistant[]; chats: Conversation[];
  prompts: AiPrompt[]; replies: AiReply[]; jobs: AiJob[];
  settings: () => void; refresh: () => Promise<void>; back: () => void;
}) {
  const { remarks } = useContext(People);
  const [model, setModel] = useState<BuiltinModel | null>(null);
  const selection = model?.selection;
  const provider = assistants.find(entry => entry.provider === selection?.provider);
  const [models, setModels] = useState<string[]>([]);
  const [manual, setManual] = useState<string>();
  const [pickerOpen, setPickerOpen] = useState(false);
  const [discovering, setDiscovering] = useState(false);
  const [attachedIds, setAttachedIds] = useState<string[]>([]);
  const [question, setQuestion] = useState("");
  const [sharing, setSharing] = useState<AiReply>();
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState("");
  const terms = useAiTerms();
  const openMenu = useMenu();
  const openEditMenu = useEditMenu();
  const menuAnchor = useMenuAnchor();
  const log = useRef<HTMLDivElement>(null);
  const textarea = useRef<HTMLTextAreaElement>(null);
  const pickerRef = useRef<HTMLDivElement>(null);
  const pickerButton = useRef<HTMLButtonElement>(null);
  const atBottom = useRef(true);
  const discoverOrder = useRef(0);
  const activeTarget = useRef<{ provider: string; baseUrl: string } | undefined>(undefined);
  activeTarget.current = provider ? { provider: provider.provider, baseUrl: provider.base_url ?? "" } : undefined;
  const pickerId = useId();
  const failed = (error: unknown) => setFailure(errorMessage(error));
  const menu = useAiMessageMenu(failed);
  const attachableChats = chats.filter(chat => chat.id !== builtinAssistantId);
  const attachedChats = attachedIds
    .map(id => attachableChats.find(chat => chat.id === id))
    .filter((chat): chat is Conversation => Boolean(chat));

  useEffect(() => {
    let current = true;
    let order = 0;
    const load = () => {
      const ticket = ++order;
      void ai.builtinModel().then(next => { if (current && ticket === order) setModel(next); })
        .catch(error => { if (current && ticket === order) setFailure(errorMessage(error)); });
    };
    load();
    const unlisten = listen("zibblu-builtin-settings", load);
    return () => { current = false; void unlisten.then(stop => stop()); };
  }, [assistants]);

  useEffect(() => {
    discoverOrder.current += 1;
    setModels([]);
    setDiscovering(false);
  }, [provider?.provider, provider?.base_url]);

  useEffect(() => {
    const valid = new Set(chats.filter(chat => chat.id !== builtinAssistantId).map(chat => chat.id));
    setAttachedIds(current => {
      const next = current.filter(id => valid.has(id));
      return next.length === current.length ? current : next;
    });
  }, [chats]);

  useEffect(() => {
    if (!pickerOpen) return;
    function dismissOutside(event: globalThis.PointerEvent) {
      if (event.target instanceof Node && !pickerRef.current?.contains(event.target)) {
        setPickerOpen(false);
      }
    }
    function dismissKey(event: globalThis.KeyboardEvent) {
      if (event.key === "Escape") {
        event.preventDefault();
        setPickerOpen(false);
        pickerButton.current?.focus({ preventScroll: true });
      }
    }
    document.addEventListener("pointerdown", dismissOutside, true);
    document.addEventListener("keydown", dismissKey);
    return () => {
      document.removeEventListener("pointerdown", dismissOutside, true);
      document.removeEventListener("keydown", dismissKey);
    };
  }, [pickerOpen]);

  useLayoutEffect(() => {
    if (windows && textarea.current) syncScrollEdges(textarea.current);
  }, [question]);

  useEffect(() => {
    const editor = textarea.current;
    if (!windows || !editor) return;
    const resize = new ResizeObserver(() => syncScrollEdges(editor));
    resize.observe(editor);
    return () => resize.disconnect();
  }, []);

  useEffect(() => {
    if (ios || pickerOpen || manual !== undefined || sharing) return;
    const editor = textarea.current;
    if (editor?.getClientRects().length) editor.focus({ preventScroll: true });
  }, [pickerOpen, manual, sharing]);

  async function save(next: BuiltinSelection) {
    setBusy(true);
    setFailure("");
    try {
      setModel(await ai.builtinModel(next));
      setManual(undefined);
    } catch (error) { failed(error); }
    finally { setBusy(false); }
  }

  async function discover() {
    if (!provider || discovering) return;
    const targetProvider = provider.provider;
    const targetBase = provider.base_url ?? "";
    const ticket = ++discoverOrder.current;
    const isCurrent = () =>
      discoverOrder.current === ticket
      && activeTarget.current?.provider === targetProvider
      && activeTarget.current?.baseUrl === targetBase;
    setDiscovering(true);
    setFailure("");
    try {
      if (!await terms.requireAcceptance()) return;
      if (!isCurrent()) return;
      const available = (await ai.testConnection(targetProvider, targetBase)).models;
      if (!isCurrent()) return;
      setModels(available);
      if (!available.length) setFailure(t("The provider returned no model IDs. Enter an ID to continue.", "提供商未返回模型 ID，可以手动输入。"));
    } catch (error) {
      if (isCurrent()) failed(error);
    } finally {
      if (discoverOrder.current === ticket) setDiscovering(false);
    }
  }

  function toggleAttachedChat(id: string) {
    setAttachedIds(current => {
      if (current.includes(id)) return current.filter(item => item !== id);
      if (current.length >= MAX_ATTACHED_CHATS) return current;
      return [...current, id];
    });
  }

  async function submit(event?: FormEvent) {
    event?.preventDefault();
    const trimmed = question.trim();
    if (!selection || !provider || !trimmed || busy) return;
    if (new TextEncoder().encode(trimmed).length > 8192) {
      setFailure(t("Enter a question up to 8 KiB.", "问题不能超过 8 KiB。"));
      return;
    }
    setFailure("");
    setBusy(true);
    try {
      if (!await terms.requireAcceptance()) return;
      await ai.startBuiltin(selection, provider.base_url ?? null, trimmed, attachedChats.map(chat => chat.id));
      setQuestion("");
      setAttachedIds([]);
      atBottom.current = true;
    } catch (error) { failed(error); }
    finally { setBusy(false); }
  }

  const remove = (id: string, kind: "prompt" | "reply" | "job") =>
    void ai.deleteLocal(builtinAssistantId, id, kind).catch(failed);
  const timeline = [
    ...prompts.map(item => ({ type: "prompt" as const, item })),
    ...replies.map(item => ({ type: "reply" as const, item })),
    ...jobs.map(item => ({ type: "job" as const, item })),
  ].sort((a, b) => a.item.created_at - b.item.created_at
    || Number(a.type !== "prompt") - Number(b.type !== "prompt")
    || a.item.id.localeCompare(b.item.id));
  let previous: number | undefined;

  useLayoutEffect(() => {
    if (log.current && atBottom.current) log.current.scrollTop = log.current.scrollHeight;
  }, [prompts, replies, jobs]);

  const modelChoices = selection
    ? [...new Set([selection.model, provider?.model ?? selection.model, ...models])]
    : [];
  const hasEfforts = !!selection && !!model?.efforts.length;
  const activeEffortText = selection?.effort && effortLabels[selection.effort]
    ? t(...effortLabels[selection.effort])
    : t("Default", "默认");
  const oversized = new TextEncoder().encode(question.trim()).length > 8192;

  return <section className="chat-view builtin-assistant" aria-label={builtinAssistantName()}>
    <header className="chat-header">
      <button className="icon-button mobile-back" type="button" aria-label={t("Back to chats", "返回聊天列表")} onClick={back}>
        <ArrowLeft size={20} />
      </button>
      <div className="chat-heading builtin-heading">
        <h2><span className="chat-title">{builtinAssistantName()}</span></h2>
        {selection && provider && <div className="builtin-model-picker" ref={pickerRef}>
          <button ref={pickerButton} type="button" className="builtin-model-trigger"
            aria-haspopup="dialog" aria-expanded={pickerOpen} aria-controls={pickerOpen ? pickerId : undefined}
            disabled={busy} title={`${aiProviders[selection.provider].label} · ${selection.model}`}
            onClick={() => setPickerOpen(open => !open)}>
            <ProviderLogo provider={selection.provider} />
            <span className="builtin-model-label">{selection.model}</span>
            <ChevronDown size={14} aria-hidden="true" />
          </button>
          {pickerOpen && <div id={pickerId} className="builtin-model-popover" role="dialog" aria-label={t("Model selection", "模型选择")}>
            <div className="builtin-popover-section">
              <div className="builtin-popover-title">{t("Provider", "提供商")}</div>
              <div className="builtin-popover-list">
                {assistants.map(entry => {
                  const selected = entry.provider === selection.provider;
                  return <button key={entry.provider} type="button" className="builtin-popover-item"
                    aria-pressed={selected} disabled={busy}
                    onClick={() => {
                      if (!selected) void save({ provider: entry.provider, model: entry.model, effort: null });
                    }}>
                    <ProviderLogo provider={entry.provider} />
                    <span className="builtin-popover-copy">
                      <strong>{aiProviders[entry.provider].label}</strong>
                      <small>{entry.model}</small>
                    </span>
                    {selected && <Check size={15} className="builtin-popover-check" aria-hidden="true" />}
                  </button>;
                })}
              </div>
            </div>
            <div className="builtin-popover-divider" role="separator" />
            <div className="builtin-popover-section">
              <div className="builtin-popover-header">
                <span className="builtin-popover-title">{t("Model", "模型")}</span>
                <div className="builtin-popover-inline-actions">
                  <button type="button" className="text-button" disabled={busy || discovering}
                    onClick={() => void discover()}>
                    <RefreshCw size={12} className={discovering ? "preview-spinner" : undefined} aria-hidden="true" />
                    {discovering ? t("Loading…", "获取中…") : t("Load models", "获取列表")}
                  </button>
                  <button type="button" className="text-button" disabled={busy}
                    onClick={() => { setPickerOpen(false); setManual(selection.model); }}>
                    <Pencil size={12} aria-hidden="true" />
                    {t("Custom ID", "手动输入")}
                  </button>
                </div>
              </div>
              <div className="builtin-popover-list builtin-popover-models">
                {modelChoices.map(id => {
                  const selected = id === selection.model;
                  return <button key={id} type="button" className="builtin-popover-item"
                    aria-pressed={selected} disabled={busy}
                    onClick={() => {
                      setPickerOpen(false);
                      if (!selected) void save({ ...selection, model: id, effort: null });
                    }}>
                    <span className="builtin-popover-model-id" title={id}>{id}</span>
                    {selected && <Check size={15} className="builtin-popover-check" aria-hidden="true" />}
                  </button>;
                })}
              </div>
              {!hasEfforts && <p className="builtin-popover-note">{t("Thinking effort is managed by the provider for this model.", "当前模型的思考强度由提供商决定。")}</p>}
            </div>
            <div className="builtin-popover-divider" role="separator" />
            <button type="button" className="builtin-popover-item builtin-popover-footer"
              onClick={() => { setPickerOpen(false); settings(); }}>
              <Settings size={15} aria-hidden="true" />
              <span>{t("AI assistant settings", "AI 助理设置")}</span>
            </button>
          </div>}
        </div>}
      </div>
      <button type="button" className="icon-button" aria-label={t("AI assistant settings", "AI 助理设置")}
        title={t("AI assistant settings", "AI 助理设置")} onClick={settings}>
        <Settings size={20} />
      </button>
    </header>
    <div className="chat-content">
      <div className="chat-messages">
        <div className="timeline-wrap">
          <div className="timeline" ref={log} onPointerDown={clearMessageSelectionOnBlank} onScroll={event => {
            const element = event.currentTarget;
            atBottom.current = element.scrollHeight - element.scrollTop - element.clientHeight < 100;
          }}>
            {failure && <p className="inline-error" role="alert">{failure}</p>}
            {!timeline.length && <Blank icon={<Bot size={24} />} title={t("Ask Zibblu Assistant", "向 Zibblu 助手提问")}>
              {!provider && <>
                <p>{t("Configure an AI provider in Settings to start chatting.", "先在设置中配置 AI 提供商，即可开始对话。")}</p>
                <button type="button" className="button primary" onClick={settings}>{t("Configure provider", "前往配置")}</button>
              </>}
            </Blank>}
            <div className="messages" role="log" aria-label={t("Assistant messages", "助手消息")}>
              {timeline.map(({ item, type }) => {
                const time = item.created_at;
                const heading = startsTimeGroup(time, previous);
                previous = time;
                return <Fragment key={type + item.id}>
                  {heading && <div className="day-divider"><time dateTime={new Date(time).toISOString()}>{messageTimeLabel(time)}</time></div>}
                  {type === "prompt" ? <AiPromptBubble builtin prompt={item} sender={own} menu={menu(item.prompt_text, () => remove(item.id, type))} />
                    : type === "job" ? <AiJobBubble job={item} onCancel={id => void ai.cancelJob(id).catch(failed)}
                        onDismiss={id => remove(id, "job")} menu={menu(undefined, () => remove(item.id, type))} />
                    : <div className="message-row" tabIndex={0} aria-haspopup="menu"
                        {...menu(item.text, () => remove(item.id, type), () => setSharing(item))}>
                        <ProviderLogo provider={item.provider} />
                        <div className="message-body">
                          <div className="message-content"><RichMessage text={item.text} cacheKey={item} /></div>
                          <AiSources sources={item.sources ?? []} failed={failed} open={number => {
                            void ai.source(item.id, number).catch(failed);
                          }} />
                        </div>
                      </div>}
                </Fragment>;
              })}
            </div>
          </div>
        </div>
        <form className="composer" onSubmit={event => void submit(event)}>
          {oversized && <div className="inline-error" role="alert">{t("Text can be up to 8 KiB.", "文本最多 8 KiB。")}</div>}
          <div className="composer-box">
            {attachedChats.length > 0 && <div className="builtin-attached-chats" role="list"
              aria-label={t("Attached ordinary chats", "本次关联的普通聊天")}>
              {attachedChats.map(chat => {
                const label = chatName(chat, own.id, remarks);
                return <span key={chat.id} className="builtin-chat-chip" role="listitem">
                  <MessageSquare size={13} aria-hidden="true" />
                  <span title={label}>{label}</span>
                  <button type="button" className="builtin-chat-chip-remove" disabled={busy}
                    aria-label={t("Remove", "移除") + " " + label}
                    onClick={() => toggleAttachedChat(chat.id)}>
                    <X size={13} aria-hidden="true" />
                  </button>
                </span>;
              })}
            </div>}
            <div className="composer-input builtin-composer-input">
              <div className="builtin-composer-actions">
                <button type="button" className="icon-button builtin-attach-button"
                  aria-label={t("Attach ordinary chat", "关联普通聊天")}
                  title={attachableChats.length ? t("Attach ordinary chat", "关联普通聊天") : t("No ordinary chats available", "暂无可关联的普通聊天")}
                  aria-haspopup="menu"
                  aria-expanded={menuAnchor?.classList.contains("builtin-attach-button") ?? false}
                  disabled={busy || !attachableChats.length}
                  onClick={event => {
                    const items: MenuItem[] = attachableChats.map(chat => {
                      const selected = attachedIds.includes(chat.id);
                      return {
                        label: chatName(chat, own.id, remarks),
                        icon: selected ? <Check size={15} /> : <span className="builtin-menu-spacer" aria-hidden="true" />,
                        disabled: !selected && attachedIds.length >= MAX_ATTACHED_CHATS,
                        action: () => toggleAttachedChat(chat.id),
                      };
                    });
                    if (attachedIds.length) {
                      items.push({
                        label: t("Clear attached chats", "清除已关联聊天"),
                        icon: <X size={15} />,
                        divider: true,
                        action: () => setAttachedIds([]),
                      });
                    }
                    openMenu(event.currentTarget, items);
                  }}>
                  <Plus size={18} aria-hidden="true" />
                </button>
                {hasEfforts && selection && <button type="button" className="builtin-effort-button"
                  aria-label={t("Thinking effort", "思考强度") + ": " + activeEffortText}
                  title={t("Thinking effort", "思考强度")}
                  aria-haspopup="menu"
                  aria-expanded={menuAnchor?.classList.contains("builtin-effort-button") ?? false}
                  disabled={busy}
                  onClick={event => openMenu(event.currentTarget, [
                    {
                      label: t("Default effort", "默认强度"),
                      icon: !selection.effort ? <Check size={15} /> : <span className="builtin-menu-spacer" aria-hidden="true" />,
                      action: () => void save({ ...selection, effort: null }),
                    },
                    ...(model?.efforts ?? []).map(effort => ({
                      label: t(...effortLabels[effort]),
                      icon: selection.effort === effort ? <Check size={15} /> : <span className="builtin-menu-spacer" aria-hidden="true" />,
                      action: () => void save({ ...selection, effort }),
                    })),
                  ])}>
                  <Sparkles size={15} aria-hidden="true" />
                  <span>{activeEffortText}</span>
                </button>}
              </div>
              <FloatingField multiline ref={textarea} label={t("Ask the assistant", "向助手提问")}
                className="composer-text" rows={windows ? 1 : 2}
                value={question}
                onScroll={windows ? event => syncScrollEdges(event.currentTarget) : undefined}
                onContextMenu={windows ? event => openEditMenu(event, async () => {}) : undefined}
                onChange={event => setQuestion(event.currentTarget.value)}
                onKeyDown={event => {
                  if (!ios && event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing && event.keyCode !== 229) {
                    event.preventDefault();
                    void submit();
                  }
                }} />
              <button type="submit" className="button primary send-button" disabled={!provider || !question.trim() || oversized || busy}
                aria-label={busy ? t("Sending question", "正在发送问题") : t("Send question", "发送问题")}>
                <span>{busy ? t("Sending…", "发送中…") : t("Send", "发送")}</span>
              </button>
            </div>
          </div>
        </form>
      </div>
    </div>
    {manual !== undefined && selection && <Dialog title={t("Model ID", "模型 ID")} close={() => setManual(undefined)} closeDisabled={busy}>
      <form onSubmit={event => { event.preventDefault(); void save({ ...selection, model: manual, effort: null }); }}>
        <FloatingField label={t("Model ID", "模型 ID")} value={manual} onChange={event => setManual(event.currentTarget.value)} maxLength={256} required />
        <div className="dialog-actions">
          <button type="button" className="button subtle" disabled={busy} onClick={() => setManual(undefined)}>{t("Cancel", "取消")}</button>
          <button type="submit" className="button primary" disabled={busy || !manual.trim()}>{t("Save", "保存")}</button>
        </div>
      </form>
    </Dialog>}
    {terms.dialog}
    {sharing && <ShareAiReply reply={sharing} members={[]} own={own.id} destinations={chats} close={() => setSharing(undefined)} published={async () => {
      await refresh(); setSharing(undefined);
    }} />}
  </section>;
}
