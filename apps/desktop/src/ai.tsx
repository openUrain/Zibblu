import {
  AtSign, Bot, Check, ChevronRight, Clock3, FileText, ListChecks,
  LockKeyhole, MessageSquare, Paperclip, Send, X,
} from "lucide-react";
import { useContext, useEffect, useId, useMemo, useRef, useState } from "react";
import type { FormEvent } from "react";
import { ai, aiProviders } from "./ai-api";
import type { AiFile, AiPreview, AiProvider, AiReply, AiScope, Assistant } from "./ai-api";
import { api, errorMessage } from "./api";
import type { Message, User } from "./api";
import { bytes, Dialog, People, displayName } from "./common";
import "./ai.css";

const MAX_MESSAGES = 60;
const MAX_FILES = 3;

export function ProviderLogo({ provider }: { provider: AiProvider }) {
  const { label, icon } = aiProviders[provider];
  return <span className="ai-provider-icon" title={label}><img src={"/providers/" + icon} alt={label} /></span>;
}

export function AssistantSettings({ assistants, failure, changed }: {
  assistants: Assistant[]; failure: string; changed: (assistants: Assistant[]) => void;
}) {
  const [editing, setEditing] = useState<AiProvider>();
  return <div className="surface settings-card ai-settings">
    <h2><Bot size={20} /> AI assistants</h2>
    <p className="muted">Give a provider an @name to ask it from a normal chat.</p>
    {failure && <p className="inline-error" role="alert">{failure}</p>}
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
      <label>Mention name
        <span className="ai-mention-field"><AtSign size={16} />
          <input aria-label="Mention name" value={name} onChange={event => setName(event.target.value)} maxLength={32} required />
        </span>
      </label>
      <label>Model ID<input value={model} onChange={event => setModel(event.target.value)} maxLength={128} required
        list={models.length ? modelList : undefined} /></label>
      {models.length > 0 && <datalist id={modelList}>{models.map(id => <option key={id} value={id} />)}</datalist>}
    </div>
    <label>API key
      <input ref={key} type="password" autoComplete="off" spellCheck={false} maxLength={512} required={!assistant}
        onChange={resetTest} placeholder={assistant ? "Saved · enter a new key to replace it" : "Paste your provider API key"} />
    </label>
    <details className="ai-advanced" open={assistant?.base_url ? true : undefined}>
      <summary>Advanced connection settings</summary>
      <label>Base URL<input type="url" value={baseUrl} placeholder={aiProviders[provider].base} maxLength={2048}
        onChange={event => { setBaseUrl(event.target.value); resetTest(); }} /></label>
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
      <button className="button primary" type="submit" disabled={!!busy}>{busy === "save" ? "Saving…" : "Save assistant"}</button>
    </div>
  </form>;
}

const scopes = [
  { kind: "prompt", label: "Prompt only", Icon: MessageSquare },
  { kind: "selected", label: "Select messages", Icon: ListChecks },
  { kind: "recent", label: "Recent", Icon: Clock3 },
] as const;

export function AiRequest({ view, draft, assistant, prompt, messages, hasMore, loading, older, conversation, close, answered }: {
  view: string; draft: string; assistant: Assistant; prompt: string;
  messages: Message[]; hasMore: boolean; loading: boolean; older: () => Promise<void>;
  conversation: string; close: () => void; answered: (reply: AiReply) => void;
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
  const { remarks } = useContext(People);
  const chosenAssistant = preview?.assistant ?? assistant;
  const provider = aiProviders[chosenAssistant.provider].label;
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
      const answer = await ai.send(view, draft, preview.id);
      answered({ id: preview.id, conversation, provider: preview.assistant.provider, text: answer.text, createdAt: answer.created_at });
    } catch (error) { setFailure(errorMessage(error) + ". Start a new request to retry."); }
    finally { setBusy(undefined); }
  }

  const disabled = !!busy || spent;
  function updateSelection(change: () => void) {
    setPreview(undefined);
    setPreparing(true);
    setFailure("");
    change();
  }

  return <Dialog className="ai-request" title={"Ask @" + assistant.name}
    close={() => { if (!busy) close(); }}>
    <div className="ai-request-model">
      <ProviderLogo provider={chosenAssistant.provider} />
      <div><strong>{provider}</strong><span>{chosenAssistant.model}</span></div>
    </div>
      <fieldset className="ai-section">
        <legend>Message context</legend>
        <div className="ai-segments">
          {scopes.map(({ kind: value, label, Icon }) => <button key={value} type="button" aria-pressed={kind === value}
            onClick={() => { if (kind !== value) updateSelection(() => setKind(value)); }} disabled={disabled}><Icon size={16} />{label}</button>)}
        </div>
        {kind === "recent" && <label className="ai-recent">Include the latest
          <select value={limit} onChange={event => updateSelection(() => setLimit(Number(event.target.value)))} disabled={disabled}>
            {[5, 10, 20, 40, 60].map(count => <option key={count} value={count}>{count} text messages</option>)}
          </select>
        </label>}
        {kind === "selected" && <div className="ai-pick-messages">
          <p className="muted">{sequences.length} of {MAX_MESSAGES} selected</p>
          <div className="ai-message-options">
            {messages.filter(message => message.text !== null).map(message => {
              const checked = sequences.includes(message.seq);
              return <label key={message.seq} className="ai-message-option">
                <input type="checkbox" checked={checked} disabled={disabled || (!checked && sequences.length >= MAX_MESSAGES)}
                  onChange={() => updateSelection(() => setSequences(current => checked ? current.filter(seq => seq !== message.seq) : [...current, message.seq]))} />
                <span><strong>{displayName(message.sender, remarks)}</strong><span>{message.text}</span></span>
              </label>;
            })}
            {!messages.some(message => message.text !== null) && <p className="muted">No text messages to select.</p>}
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
          {files.map(file => <li key={file.id}><FileText size={15} /><span className="ai-file-name" title={file.name}>{file.name}</span>
            <small>{bytes(file.bytes)}</small>
            <button className="icon-button" type="button" disabled={disabled} aria-label={"Remove " + file.name}
              onClick={() => void remove(file)}><X size={14} /></button>
          </li>)}
        </ul> : null}
      </div>
      {preview && <div className="ai-snapshot">
        <header className="ai-disclosure"><strong>Content sent to {provider}</strong><small>{preview.endpoint}</small></header>
        <details open>
          <summary><MessageSquare size={15} /> Prompt</summary>
          <p>{preview.snapshot.prompt}</p>
        </details>
        {preview.snapshot.messages.length > 0 && <details open>
          <summary><ListChecks size={15} /> Messages <span>{preview.snapshot.messages.length}</span></summary>
          {preview.snapshot.messages.map((message, index) => <div className="ai-excerpt" key={index}>
            <strong>@{message.sender}</strong><p>{message.text}</p>
          </div>)}
        </details>}
        {preview.snapshot.files.map((file, index) => <details key={index}>
          <summary><FileText size={15} /> <span className="ai-file-name">{file.name}</span></summary>
          <p>{file.text}</p>
        </details>)}
      </div>}
      {!preview && preparing && <p className="muted" role="status">Preparing content preview…</p>}
      <p className="ai-visibility-note"><LockKeyhole size={15} /> One-time, read-only request. Reply visible only to you.</p>
      {(failure || previewFailure) && <p className="inline-error" role="alert">{failure || previewFailure}</p>}
      <div className="dialog-actions">
        <button className="button subtle" onClick={close} disabled={!!busy}>{spent ? "Close" : "Cancel"}</button>
        <button className="button primary" disabled={disabled || preparing || !preview}
          onClick={() => void send()}>{busy === "send" ? "Waiting for reply…" : "Send to " + provider} <Send size={16} /></button>
      </div>
  </Dialog>;
}

export function ShareAiReply({ reply, members, close, published }: {
  reply: AiReply; members: User[]; close: () => void; published: () => Promise<void>;
}) {
  const { remarks } = useContext(People);
  const [readers, setReaders] = useState<string[]>([]);
  const [posting, setPosting] = useState(false);
  const [failure, setFailure] = useState("");
  const lockedReaders = useRef<string[] | undefined>(undefined);
  const tooLong = new TextEncoder().encode(reply.text.trim()).length > 24 * 1024;

  async function post() {
    setPosting(true);
    setFailure("");
    try {
      lockedReaders.current ??= readers;
      await api.publishAiReply(reply.conversation, {
        nonce: reply.id, provider: reply.provider, text: reply.text, created_at: reply.createdAt, readers: lockedReaders.current,
      });
      await published();
    } catch (error) { setFailure(errorMessage(error)); }
    finally { setPosting(false); }
  }

  return <Dialog title="Visible to" close={() => { if (!posting) close(); }}>
    <p className="muted">Share this reply with the selected chat members. You will also be able to see it.</p>
    <fieldset className="dialog-contacts" disabled={posting || !!lockedReaders.current}>
      <legend>Chat members</legend>
      {members.map(user => <label className="check-contact" key={user.id}>
        <input type="checkbox" checked={readers.includes(user.id)} onChange={event => {
          setReaders(event.target.checked ? [...readers, user.id] : readers.filter(id => id !== user.id));
        }} />
        <span>{displayName(user, remarks)}</span>
      </label>)}
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
        {active === index && <Check size={15} />}
      </button>)}
    </div>
    {assistants.length === 0 && <button className="ai-mention-option" type="button" onClick={settings}>
      <Bot size={17} /><span>Manage assistants</span><ChevronRight size={15} />
    </button>}
  </div>;
}
