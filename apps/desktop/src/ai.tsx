import { FloatingField } from "./floating-field";
import { Selector } from "./selector";
import {
  AtSign, Bot, ChevronRight, CircleAlert, CircleCheck, Clock3, FileText, ListChecks,
  Copy, LockKeyhole, MessageSquare, Paperclip, Send, Trash2, X,
} from "lucide-react";
import { useContext, useEffect, useId, useMemo, useRef, useState } from "react";
import type { FormEvent } from "react";
import { ai, aiProviders, builtinAssistantId } from "./ai-api";
import type {
  AiFile, AiJob, AiPreview, AiPrompt, AiProvider, AiReply, AiScope, AiSource, Assistant,
} from "./ai-api";
import { api, errorMessage } from "./api";
import { decimalSize, splitFileName } from "./file-labels";
import { actions, t } from "./locale";
import { windows } from "./window-chrome";
import { RichMessage } from "./rich-message";
import type { Attachment, Conversation, Message, User } from "./api";
import { bytes, chatName, Choice, dateTimeLabel, Dialog, People, PersonAvatar, displayName } from "./common";
import { copyText, useRowMenu } from "./context-menu";
import "./ai.css";

const MAX_MESSAGES = 60;
const MAX_FILES = 3;

function attachmentTypeDisplay(file: Attachment): string {
  if (file.kind === "folder") return t("Folder", "文件夹");
  const ext = file.name.split(".").pop()?.toUpperCase();
  return ext && ext !== file.name.toUpperCase() ? ext : t("File", "文件");
}

function FileLabel({ name }: { name: string }) {
  const [stem, ext] = splitFileName(name);
  return <span className="ai-file-name file-name" title={name} dir="ltr">
    <strong dir="auto">{stem}</strong>{ext && <strong>{ext}</strong>}
  </span>;
}

export function ProviderLogo({ provider }: { provider: AiProvider }) {
  const { label, icon } = aiProviders[provider];
  return <span className="ai-provider-icon" title={label}><img src={"/providers/" + icon} alt={label} /></span>;
}

export function AssistantSettings({ assistants, failure, changed }: {
  assistants: Assistant[]; failure: string; changed: (assistants: Assistant[]) => void;
}) {
  const [editing, setEditing] = useState<AiProvider>();
  const [agentError, setAgentError] = useState("");
  return <div className="surface settings-card ai-settings">
    <h2><Bot size={20} /> AI assistants</h2>
    <p className="muted">Give a provider an @name to ask it from a normal chat.</p>
    {failure && <p className="inline-error" role="alert">{failure}</p>}
    {agentError && <p className="inline-error" role="alert">{agentError}</p>}
    {(Object.keys(aiProviders) as AiProvider[]).map(provider => {
      const details = aiProviders[provider];
      const assistant = assistants.find(entry => entry.provider === provider);
      return <div className="ai-settings-provider" key={provider}>
        <button type="button" className="ai-settings-row" aria-expanded={editing === provider}
          onClick={() => setEditing(editing === provider ? undefined : provider)}>
          <ProviderLogo provider={provider} />
          <span className="ai-settings-name"><strong>{details.label}</strong>
            <small>{assistant ? "@" + assistant.name + " · " + assistant.model : "Set up an assistant"}</small>
          </span>
          <ChevronRight size={16} />
        </button>
        {windows && assistant?.agent_enabled && <label className="settings-row">
          <span>{t("Autonomous retrieval", "自主检索")}</span>
          <input type="checkbox" role="switch" checked={true} onChange={() => {
            setAgentError("");
            void ai.enableAgent(assistant, false).then(changed).catch(error => setAgentError(errorMessage(error)));
          }} />
        </label>}
        {editing === provider && <AssistantEditor provider={provider} assistant={assistant} saved={next => {
          changed(next);
          setEditing(undefined);
        }} />}
      </div>;
    })}
  </div>;
}

function AssistantEditor({ provider, assistant, saved }: {
  provider: AiProvider; assistant?: Assistant; saved: (assistants: Assistant[]) => void;
}) {
  const [name, setName] = useState(assistant?.name ?? aiProviders[provider].name);
  const [model, setModel] = useState(assistant?.model ?? aiProviders[provider].model);
  const [baseUrl, setBaseUrl] = useState(assistant?.base_url ?? "");
  const [models, setModels] = useState<string[]>([]);
  const [connection, setConnection] = useState("");
  const key = useRef<HTMLInputElement>(null);
  const modelList = useId();
  const [failure, setFailure] = useState("");
  const [busy, setBusy] = useState<"save" | "test" | "remove">();

  function resetTest() {
    setModels([]);
    setConnection("");
    setFailure("");
  }

  async function test() {
    setBusy("test");
    resetTest();
    try {
      const result = await ai.testConnection(provider, baseUrl, key.current?.value.trim() || undefined);
      setModels(result.models);
      setConnection(result.authenticated
        ? result.models.length ? "Connection verified. Choose a detected model or enter its ID." : "Connection verified. Enter the model ID manually."
        : "The server responded, but its model list is unavailable. Enter the model ID manually.");
    } catch (error) { setFailure(errorMessage(error)); }
    finally { setBusy(undefined); }
  }

  async function save(event: FormEvent) {
    event.preventDefault();
    if (busy) return;
    setBusy("save");
    setFailure("");
    try { saved(await ai.saveAssistant(provider, name, model, baseUrl, key.current?.value.trim() || undefined)); }
    catch (error) { setFailure(errorMessage(error)); }
    finally {
      if (key.current) key.current.value = "";
      setBusy(undefined);
    }
  }

  async function remove() {
    setBusy("remove");
    setFailure("");
    try { saved(await ai.removeAssistant(provider)); }
    catch (error) { setFailure(errorMessage(error)); }
    finally { setBusy(undefined); }
  }

  return <form className="ai-settings-form" onSubmit={save} autoComplete="off">
    <fieldset className="ai-editor-fields" disabled={!!busy}>
    <div className="ai-settings-fields">
      <FloatingField label="Mention name" prefixIcon={<AtSign size={16} />} value={name} onChange={event => setName(event.target.value)} maxLength={32} required />
      <FloatingField label="Model ID" value={model} onChange={event => setModel(event.target.value)} maxLength={128} required
        list={models.length ? modelList : undefined} />
      {models.length > 0 && <datalist id={modelList}>{models.map(id => <option key={id} value={id} />)}</datalist>}
    </div>
    <FloatingField label="API key" ref={key} type="password" autoComplete="off" spellCheck={false} maxLength={512} required={!assistant}
        onChange={resetTest} placeholder={assistant ? "Saved · enter a new key to replace it" : "Paste your provider API key"} />
    <details className="ai-advanced" open={assistant?.base_url ? true : undefined}>
      <summary>Advanced connection settings</summary>
      <FloatingField label="Base URL" type="url" value={baseUrl} placeholder={aiProviders[provider].base} maxLength={2048}
        onChange={event => { setBaseUrl(event.target.value); resetTest(); }} />
      <p className="muted">Use the HTTPS API base path. The key is sent to this address when you test or ask the assistant.</p>
    </details>
    </fieldset>
    {connection && <p className="muted" role="status">{connection}</p>}
    {failure && <p className="inline-error" role="alert">{failure}</p>}
    <div className="ai-settings-actions">
      <span className="muted"><LockKeyhole size={14} /> Protected by Windows</span>
      <button className="button subtle" type="button" disabled={!!busy} onClick={() => void test()}>
        {busy === "test" ? "Testing…" : "Test connection"}
      </button>
      {assistant && <button className="button subtle" type="button" disabled={!!busy} onClick={() => void remove()}>Remove</button>}
      <button className="button primary" type="submit" disabled={!!busy}>{busy === "save" ? actions.saving : actions.save}</button>
    </div>
  </form>;
}

const scopes = [
  { kind: "prompt", label: "Prompt only", Icon: MessageSquare },
  { kind: "selected", label: "Select messages", Icon: ListChecks },
  { kind: "recent", label: "Recent", Icon: Clock3 },
] as const;

export function AiRequest({ view, draft, assistant, prompt, messages, hasMore, loading, older, conversation: _conversation, close, sent, answered }: {
  view: string; draft: string; assistant: Assistant; prompt: string;
  messages: Message[]; hasMore: boolean; loading: boolean; older: () => Promise<void>;
  conversation: string; close: () => void;
  sent?: (result: { id: string; created_at: number }) => void;
  answered?: (reply: AiReply) => void;
}) {
  const [kind, setKind] = useState<AiScope["kind"]>("prompt");
  const [sequences, setSequences] = useState<number[]>([]);
  const [limit, setLimit] = useState(5);
  const [files, setFiles] = useState<AiFile[]>([]);
  const [preview, setPreview] = useState<AiPreview>();
  const [busy, setBusy] = useState<"file" | "send">();
  const [failure, setFailure] = useState("");
  const [previewFailure, setPreviewFailure] = useState("");
  const [preparing, setPreparing] = useState(true);
  const [spent, setSpent] = useState(false);
  const scopeName = useId();
  const { remarks } = useContext(People);
  const chosenAssistant = preview?.assistant ?? assistant;
  const provider = aiProviders[chosenAssistant.provider].label;
  const selectableMessages = useMemo(
    () => messages.filter(message => message.seq > 0 && !message.recalled && !message.expired && (message.text !== null || message.attachment !== null)),
    [messages],
  );
  const selection = useMemo(() => {
    const scope: AiScope = kind === "selected" ? { kind, sequences }
      : kind === "recent" ? { kind, limit } : { kind };
    return { provider: assistant.provider, prompt, scope, files: files.map(file => file.id) };
  }, [assistant.provider, prompt, kind, sequences, limit, files]);

  useEffect(() => {
    if (spent) return;
    let current = true;
    setPreview(undefined);
    setPreviewFailure("");
    const ready = kind !== "selected" || selection.scope.kind === "selected" && selection.scope.sequences.length > 0;
    setPreparing(ready);
    if (!ready) return;
    const timer = window.setTimeout(() => {
      void ai.prepare(view, draft, selection)
        .then(value => { if (current) setPreview(value); })
        .catch(error => { if (current) setPreviewFailure(errorMessage(error)); })
        .finally(() => { if (current) setPreparing(false); });
    }, 150);
    return () => { current = false; window.clearTimeout(timer); };
  }, [view, draft, kind, selection, spent]);

  async function pick() {
    setBusy("file");
    setFailure("");
    try {
      const file = await ai.pickFile(view, draft);
      if (file) setFiles(current => [...current, file]);
    } catch (error) { setFailure(errorMessage(error)); }
    finally { setBusy(undefined); }
  }

  async function remove(file: AiFile) {
    setBusy("file");
    setFailure("");
    try {
      await ai.removeFile(view, draft, file.id);
      setFiles(current => current.filter(item => item.id !== file.id));
    } catch (error) { setFailure(errorMessage(error)); }
    finally { setBusy(undefined); }
  }

  async function send() {
    if (!preview || preparing || spent) return;
    setBusy("send");
    setSpent(true);
    setFailure("");
    try {
      const result = await ai.send(view, draft, preview.id);
      sent?.(result);
      void answered;
      close();
    } catch (error) {
      setFailure(errorMessage(error) + ". Start a new request to retry.");
    } finally {
      setBusy(undefined);
    }
  }

  const disabled = !!busy || spent;
  function updateSelection(change: () => void) {
    setPreview(undefined);
    setPreparing(true);
    setFailure("");
    change();
  }

  return <Dialog className="ai-request" title={t(`Ask @${assistant.name}`, `向 @${assistant.name} 提问`)} mobilePage closeDisabled={!!busy} close={close}>
    <div className="ai-request-model">
      <ProviderLogo provider={chosenAssistant.provider} />
      <div><strong>{provider}</strong><span>{chosenAssistant.model}</span></div>
    </div>
      <fieldset className="ai-section">
        <legend>Message context</legend>
        <div className="ai-segments">
          {scopes.map(({ kind: value, label, Icon }) => <Choice key={value} type="radio" name={scopeName} checked={kind === value}
            onChange={() => updateSelection(() => setKind(value))} disabled={disabled}><Icon size={16} /><span>{label}</span></Choice>)}
        </div>
        {kind === "recent" && <label className="ai-recent">Include the latest
          <Selector value={limit} onChange={event => updateSelection(() => setLimit(Number(event.target.value)))} disabled={disabled}>
            {[5, 10, 20, 40, 60].map(count => <option key={count} value={count}>{count} messages</option>)}
          </Selector>
        </label>}
        {kind === "selected" && <div className="ai-pick-messages">
          <p className="muted">{sequences.length} of {MAX_MESSAGES} selected</p>
          <div className="ai-message-options">
            {selectableMessages.map(message => {
              const checked = sequences.includes(message.seq);
              const att = message.attachment;
              return <Choice key={message.seq} className="ai-message-option" checked={checked}
                disabled={disabled || (!checked && sequences.length >= MAX_MESSAGES)}
                onChange={() => updateSelection(() => setSequences(current => checked ? current.filter(seq => seq !== message.seq) : [...current, message.seq]))}>
                <span>
                  <strong>{displayName(message.sender, remarks)}</strong>
                  {message.text && <span>{message.text}</span>}
                  {att && <span className="ai-message-attachment">
                    <Paperclip size={13} aria-hidden="true" />
                    <FileLabel name={att.name} />
                    <small>{attachmentTypeDisplay(att)} · {decimalSize(att.size)}</small>
                  </span>}
                </span>
              </Choice>;
            })}
            {selectableMessages.length === 0 && <p className="muted">No messages to select.</p>}
            {hasMore && <button className="button subtle" type="button" disabled={loading || disabled}
              onClick={() => void older()}>{loading ? "Loading…" : "Load older messages"}</button>}
          </div>
        </div>}
      </fieldset>
      <div className="ai-section">
        <div className="ai-section-line">
          <small className="muted">Up to 3 UTF-8 files · 32 KiB each</small>
          <button type="button" className="button subtle" onClick={() => void pick()} disabled={disabled || files.length >= MAX_FILES}>
            <Paperclip size={15} /> Add text file
          </button>
        </div>
        {files.length > 0 ? <ul className="ai-file-chips">
          {files.map(file => <li key={file.id}><FileText size={15} /><FileLabel name={file.name} />
            <small>{bytes(file.bytes)}</small>
            <button className="icon-button" type="button" disabled={disabled} aria-label={"Remove " + file.name}
              onClick={() => void remove(file)}><X size={14} /></button>
          </li>)}
        </ul> : null}
      </div>
      {preview && <div className="ai-snapshot">
        <header className="ai-disclosure"><strong>{t(`Content sent to ${provider}`, `将发送给 ${provider} 的内容`)}</strong><small>{preview.endpoint}</small></header>
        <details open>
          <summary><MessageSquare size={15} /> Prompt</summary>
          <p>{preview.snapshot.prompt}</p>
        </details>
        {preview.snapshot.messages.length > 0 && <details open>
          <summary><ListChecks size={15} /> Messages <span>{preview.snapshot.messages.length}</span></summary>
          {preview.snapshot.messages.map((message, index) => <div className="ai-excerpt" key={index}>
            <strong>@{message.sender}</strong>
            {message.text && <p>{message.text}</p>}
            {message.attachment && <p className="ai-excerpt-attachment"><Paperclip size={12} aria-hidden="true" /> {message.attachment}</p>}
          </div>)}
        </details>}
        {preview.snapshot.attachments.length > 0 && <details open>
          <summary><Paperclip size={15} /> {t("Outbound attachments", "外发附件列表")} <span>{preview.snapshot.attachments.length}</span></summary>
          <ul className="ai-attachment-disclosure">
            {preview.snapshot.attachments.map(att => <li key={`${att.seq}-${att.id}`}>
              <FileLabel name={att.name} />
              <span className="ai-attachment-meta">
                <span>{att.file_type}</span>
                <span>{decimalSize(att.size)}</span>
                <span className={att.cached ? "ai-cache-badge cached" : "ai-cache-badge"}>
                  {att.cached ? t("Cached locally", "本地已缓存") : t("Will download on send", "将按需下载")}
                </span>
              </span>
            </li>)}
          </ul>
        </details>}
        {preview.snapshot.files.map((file, index) => <details key={index}>
          <summary><FileText size={15} /> <FileLabel name={file.name} /></summary>
          <p>{file.text}</p>
        </details>)}
      </div>}
      {!preview && preparing && <p className="muted" role="status">Preparing content preview…</p>}
      <p className="ai-visibility-note"><LockKeyhole size={15} /> One-time, read-only request. Reply visible only to you.</p>
      {(failure || previewFailure) && <p className="inline-error" role="alert">{failure || previewFailure}</p>}
      <div className="dialog-actions">
        <button className="button subtle" onClick={close} disabled={!!busy}>{spent ? t("Close", "关闭") : t("Cancel", "取消")}</button>
        <button className="button primary" disabled={disabled || preparing || !preview}
          onClick={() => void send()}>{busy === "send" ? t("Sending…", "发送中…") : t(`Send to ${provider}`, `发送给 ${provider}`)} <Send size={16} /></button>
      </div>
  </Dialog>;
}

export const builtinAssistantName = () => t("Zibblu Assistant", "Zibblu 助手");

export function selectedText(anchor: HTMLElement) {
  const selection = window.getSelection();
  if (selection && !selection.isCollapsed && selection.rangeCount && selection.getRangeAt(0).intersectsNode(anchor)) {
    return selection.toString();
  }
}

export function useAiMessageMenu(failed: (error: unknown) => void) {
  const menu = useRowMenu<HTMLDivElement>();
  return (text: string | undefined, remove: () => void, share?: () => void) => menu(anchor => [
    ...(text ? [{ label: t("Copy", "复制"), icon: <Copy size={15} />,
      action: () => void copyText(selectedText(anchor) || text).catch(failed) }] : []),
    ...(share ? [{ label: t("Share reply", "分享回复"), action: share }] : []),
    { label: t("Delete", "删除"), icon: <Trash2 size={15} />, danger: true, action: remove },
  ]);
}

export function AiSources({ sources, open, failed }: {
  sources: AiSource[]; open: (number: number) => void; failed: (error: unknown) => void;
}) {
  const menu = useRowMenu<HTMLSpanElement>();
  if (!sources.length) return null;
  return <ul className="ai-sources" aria-label={t("Sources", "来源")}>
    {sources.map(source => {
      const time = new Date(source.created_at);
      const title = new Intl.DateTimeFormat(undefined, {
        year: "numeric", month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit",
        second: "2-digit", timeZoneName: "longOffset",
      }).format(time);
      return <li key={source.number}><span role="link" tabIndex={0} className="text-button ai-source-link"
        {...menu(anchor => [{
          label: t("Copy", "复制"), icon: <Copy size={15} />,
          action: () => void copyText(selectedText(anchor) || anchor.textContent || "").catch(failed),
        }])}
        onClick={event => { if (!selectedText(event.currentTarget)) open(source.number); }}
        onKeyDown={event => { if (event.key === "Enter") { event.preventDefault(); open(source.number); } }}>
        [{source.number}] {source.label} <time dateTime={time.toISOString()} title={title}>{dateTimeLabel(source.created_at)}</time>
      </span></li>;
    })}
  </ul>;
}

export function AiPromptBubble({ prompt, assistantName, sender, menu, builtin = false }: {
  prompt: AiPrompt; assistantName?: string; sender: User; builtin?: boolean; menu?: React.HTMLAttributes<HTMLDivElement>;
}) {
  const target = assistantName ? `@${assistantName}` : aiProviders[prompt.provider].label;
  return <div className="message-row mine ai-prompt-row" tabIndex={menu ? 0 : undefined} aria-haspopup={menu ? "menu" : undefined} {...menu}>
    <div className="message-body">
      <div className="message-content">
        <div className="bubble ai-prompt-bubble">
          {!builtin && <><span className="ai-prompt-target">{target}</span>{" "}</>}
          <span>{prompt.prompt_text}</span>
        </div>
      </div>
      <div className="message-meta">
        <small className="ai-only-you-badge">
          <LockKeyhole size={12} aria-hidden="true" />
          <span>{t("Only visible to you", "仅自己可见")}</span>
        </small>
      </div>
    </div>
    <span className="message-avatar"><PersonAvatar user={sender} /></span>
  </div>;
}

export function AiJobBubble({ job, onCancel, onDismiss, menu }: {
  job: AiJob; onCancel: (jobId: string) => void; onDismiss?: (jobId: string) => void; menu?: React.HTMLAttributes<HTMLDivElement>;
}) {
  const [busy, setBusy] = useState(false);
  const [accessError, setAccessError] = useState("");
  async function answerAccess(accept: boolean) {
    if (!job.access || busy) return;
    setBusy(true);
    setAccessError("");
    try { await ai.answerAccess(job.id, job.access.id, accept); }
    catch (error) { setAccessError(errorMessage(error)); }
    finally { setBusy(false); }
  }
  const phaseLabel = job.phase === "searching" ? t("Searching messages…", "正在检索消息…")
    : job.phase === "downloading" ? t("Downloading attachment…", "正在下载附件…")
    : job.phase === "preparing" ? t("Analyzing attachment…", "正在分析附件…")
    : t("Generating reply…", "正在生成回复…");
  const failed = job.phase === "failed";
  const progressLabel = job.phase === "downloading" && job.progress
    ? `${decimalSize(job.progress[0])} / ${decimalSize(job.progress[1])}`
    : job.phase === "generating" && job.status_text
      ? t(`Chunk ${job.status_text}`, `分块 ${job.status_text}`)
      : undefined;

  return <div className="message-row ai-job-row" tabIndex={menu ? 0 : undefined} aria-haspopup={menu ? "menu" : undefined} {...menu}>
    <ProviderLogo provider={job.provider} />
    <div className="message-body">
      <div className="message-content">
        {failed ? <div className="bubble ai-job-bubble ai-job-failed" role="alert">
          <CircleAlert className="ai-job-error-icon" size={16} aria-hidden="true" />
          <div className="ai-job-copy">
            <strong>{t("AI request failed", "AI 生成失败")}</strong>
            <span className="ai-job-error-text">{job.error || t("Could not complete the request.", "请求未能完成。")}</span>
          </div>
          {onDismiss && <button type="button" className="icon-button ai-job-cancel"
            aria-label={t("Dismiss failed AI status", "关闭失败提示")} title={t("Dismiss", "关闭")}
            onClick={() => onDismiss(job.id)}><X size={15} /></button>}
        </div> : job.access ? <div className="bubble ai-access-card">
          <strong>{t("Read another conversation?", "允许读取另一个会话？")} · {job.access.name}</strong>
          <p>{t("For this task, relevant ordinary messages and attachments from this conversation may be sent to", "本次任务可能向以下提供商发送该会话中相关的普通消息和附件")}</p>
          <small>{job.access.endpoint} · {t("Only you", "仅自己可见")}</small>
          {accessError && <p className="inline-error" role="alert">{accessError}</p>}
          <div className="dialog-actions">
            <button type="button" className="button subtle" onClick={() => onCancel(job.id)}>{actions.cancel}</button>
            <button type="button" className="button subtle" disabled={busy} onClick={() => void answerAccess(false)}>{t("Deny", "拒绝")}</button>
            <button type="button" className="button primary" disabled={busy} onClick={() => void answerAccess(true)}>{t("Allow this time", "仅允许本次")}</button>
          </div>
        </div> : <div className="bubble ai-job-bubble">
          <span className="ai-dot-spinner" aria-hidden="true">
            <i /><i /><i /><i /><i /><i /><i /><i />
          </span>
          <div className="ai-job-copy" role="status" aria-live="polite">
            <span className="ai-job-primary">{phaseLabel}</span>
            {progressLabel && <small className="ai-job-progress">{progressLabel}</small>}
          </div>
          <button type="button" className="icon-button ai-job-cancel"
            aria-label={t("Cancel AI generation", "取消生成")} title={t("Cancel", "取消")}
            onClick={() => onCancel(job.id)}><X size={15} /></button>
        </div>}
      </div>
    </div>
  </div>;
}

export function ShareAiReply({ reply, members, close, published, destinations = [], own }: {
  reply: AiReply; members: User[]; close: () => void; published: () => Promise<void>;
  destinations?: Conversation[]; own?: string;
}) {
  const { remarks } = useContext(People);
  const builtin = reply.conversation_id === builtinAssistantId;
  const [target, setTarget] = useState("");
  const nonce = useRef(builtin ? crypto.randomUUID() : reply.id);
  const targetMembers = builtin
    ? destinations.find(chat => chat.id === target)?.members.filter(member => member.id !== own) ?? []
    : members;
  const [readers, setReaders] = useState<string[]>([]);
  const [posting, setPosting] = useState(false);
  const [failure, setFailure] = useState("");
  const lockedReaders = useRef<string[] | undefined>(undefined);
  const shareText = reply.text;
  const tooLong = new TextEncoder().encode(shareText.trim()).length > 24 * 1024;

  async function post() {
    setPosting(true);
    setFailure("");
    try {
      lockedReaders.current ??= readers;
      await api.publishAiReply(builtin ? target : reply.conversation_id, {
        nonce: nonce.current, provider: reply.provider, text: shareText, created_at: reply.created_at, readers: lockedReaders.current,
      }, builtin ? reply.id : undefined);
      await published();
    } catch (error) { setFailure(errorMessage(error)); }
    finally { setPosting(false); }
  }

  return <Dialog title="Visible to" mobilePage closeDisabled={posting} close={close}>
    <p className="muted">Share this reply with the selected chat members. You will also be able to see it.</p>
    {!!reply.sources?.length && <><p className="muted">{t(
      "Generated text may contain information from other conversations. Review the reply before sharing. Each recipient sees only sources they can access.",
      "生成文本可能包含其他会话的信息。请核对正文后再分享。每位接收者只会看到有权限访问的来源。",
    )}</p><RichMessage text={shareText} /></>}
    {builtin && <label>{t("Conversation", "会话")} <select value={target} disabled={posting || !!lockedReaders.current}
      onChange={event => { setTarget(event.currentTarget.value); setReaders([]); }}>
      <option value="">{t("Choose a conversation", "选择会话")}</option>
      {destinations.map(chat => <option key={chat.id} value={chat.id}>{chatName(chat, own!, remarks)}</option>)}
    </select></label>}
    <fieldset className="dialog-contacts" disabled={posting || !!lockedReaders.current}>
      <legend>Chat members</legend>
      {targetMembers.map(user => <Choice key={user.id} checked={readers.includes(user.id)} onChange={event => {
          setReaders(event.target.checked ? [...readers, user.id] : readers.filter(id => id !== user.id));
        }}>
        <span>{displayName(user, remarks)}</span>
      </Choice>)}
    </fieldset>
    {tooLong && <p className="inline-error">Replies up to 24 KiB can be shared.</p>}
    {failure && <p className="inline-error" role="alert">{failure}</p>}
    <div className="dialog-actions">
      <button type="button" className="button subtle" disabled={posting} onClick={close}>Close</button>
      <button type="button" className="button primary" disabled={posting || tooLong || !readers.length}
        onClick={() => void post()}>{posting ? "Sharing…" : "Share reply"}</button>
    </div>
  </Dialog>;
}

export function AssistantMenu({ id, assistants, active, choose, settings }: {
  id: string; assistants: Assistant[]; active: number; choose: (assistant: Assistant) => void; settings: () => void;
}) {
  return <div className="ai-mention-menu">
    <div className="ai-menu-title">AI assistants</div>
    <div id={id} role="listbox" aria-label="AI assistants">
      {assistants.map((assistant, index) => <button className="ai-mention-option" id={id + "-" + index}
        key={assistant.provider} type="button" role="option" aria-selected={active === index}
        onClick={() => choose(assistant)}>
        <ProviderLogo provider={assistant.provider} />
        <span><strong>@{assistant.name}</strong><small>{aiProviders[assistant.provider].label} · {assistant.model}</small></span>
        {active === index && <CircleCheck size={15} aria-hidden="true" />}
      </button>)}
    </div>
    {assistants.length === 0 && <button className="ai-mention-option" type="button" onClick={settings}>
      <Bot size={17} /><span>Manage assistants</span><ChevronRight size={15} />
    </button>}
  </div>;
}
