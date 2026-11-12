import {
  ArrowLeft, ArrowRight, ArrowUpRight, AtSign, Bot, Check, ChevronRight, Clock3,
  FileText, Globe2, ListChecks, LockKeyhole, MessageSquare, Paperclip, Send, UsersRound, X,
} from "lucide-react";
import { useRef, useState } from "react";
import type { FormEvent } from "react";
import { ai, aiProviders } from "./ai-api";
import type { AiFile, AiPreview, AiProvider, AiReply, AiScope, Assistant } from "./ai-api";
import { api, errorMessage } from "./api";
import type { Message } from "./api";
import { bytes, Dialog } from "./common";
import "./ai.css";

const MAX_MESSAGES = 20;
const MAX_FILES = 3;

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
          <span className="ai-provider-icon"><Bot size={18} /></span>
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
  const key = useRef<HTMLInputElement>(null);
  const [failure, setFailure] = useState("");
  const [busy, setBusy] = useState(false);

  async function save(event: FormEvent) {
    event.preventDefault();
    if (busy) return;
    setBusy(true);
    setFailure("");
    try { saved(await ai.saveAssistant(provider, name, model, key.current?.value.trim() || undefined)); }
    catch (error) { setFailure(errorMessage(error)); }
    finally {
      if (key.current) key.current.value = "";
      setBusy(false);
    }
  }

  async function remove() {
    setBusy(true);
    setFailure("");
    try { saved(await ai.removeAssistant(provider)); }
    catch (error) { setFailure(errorMessage(error)); }
    finally { setBusy(false); }
  }

  return <form className="ai-settings-form" onSubmit={save} autoComplete="off">
    <div className="ai-settings-fields">
      <label>Mention name
        <span className="ai-mention-field"><AtSign size={16} />
          <input aria-label="Mention name" value={name} onChange={event => setName(event.target.value)} maxLength={32} required />
        </span>
      </label>
      <label>Model ID<input value={model} onChange={event => setModel(event.target.value)} maxLength={128} required /></label>
    </div>
    <label>API key
      <input ref={key} type="password" autoComplete="off" spellCheck={false} maxLength={512} required={!assistant}
        placeholder={assistant ? "Saved · enter a new key to replace it" : "Paste your provider API key"} />
    </label>
    {failure && <p className="inline-error" role="alert">{failure}</p>}
    <div className="ai-settings-actions">
      <span className="muted"><LockKeyhole size={14} /> Protected by Windows</span>
      {assistant && <button className="button subtle" type="button" disabled={busy} onClick={() => void remove()}>Remove</button>}
      <button className="button primary" type="submit" disabled={busy}>{busy ? "Saving…" : "Save assistant"}</button>
    </div>
  </form>;
}

const scopes = [
  { kind: "prompt", label: "Prompt only", Icon: MessageSquare },
  { kind: "selected", label: "Select messages", Icon: ListChecks },
  { kind: "recent", label: "Recent", Icon: Clock3 },
] as const;

export function AiRequest({ view, draft, assistant, prompt, messages, hasMore, loading, older, chat, close, answered }: {
  view: string; draft: string; assistant: Assistant; prompt: string;
  messages: Message[]; hasMore: boolean; loading: boolean; older: () => Promise<void>;
  chat: string; close: () => void; answered: (reply: AiReply) => void;
}) {
  const [kind, setKind] = useState<AiScope["kind"]>("prompt");
  const [sequences, setSequences] = useState<number[]>([]);
  const [limit, setLimit] = useState(5);
  const [files, setFiles] = useState<AiFile[]>([]);
  const [share, setShare] = useState(false);
  const [preview, setPreview] = useState<AiPreview>();
  const [busy, setBusy] = useState<"file" | "preview" | "send">();
  const [failure, setFailure] = useState("");
  const [spent, setSpent] = useState(false);
  const chosenAssistant = preview?.assistant ?? assistant;
  const provider = aiProviders[chosenAssistant.provider].label;

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

  async function review() {
    setBusy("preview");
    setFailure("");
    const scope: AiScope = kind === "selected" ? { kind, sequences }
      : kind === "recent" ? { kind, limit } : { kind };
    try {
      setPreview(await ai.prepare(view, draft, {
        provider: assistant.provider, prompt, scope, files: files.map(file => file.id),
      }));
    } catch (error) { setFailure(errorMessage(error)); }
    finally { setBusy(undefined); }
  }

  async function send() {
    if (!preview || spent) return;
    setBusy("send");
    setSpent(true);
    setFailure("");
    try {
      const text = await ai.send(view, draft, preview.id);
      answered({ id: preview.id, assistant: preview.assistant, text, share });
    } catch (error) { setFailure(errorMessage(error) + ". Start a new request to retry."); }
    finally { setBusy(undefined); }
  }

  return <Dialog className="ai-request" title={preview ? "Review AI disclosure" : "Ask @" + assistant.name}
    close={() => { if (!busy) close(); }}>
    <div className="ai-request-model">
      <span className="ai-provider-icon"><Bot size={18} /></span>
      <div><strong>{provider}</strong><span>{chosenAssistant.model}</span></div>
      <span className="ai-read-only"><LockKeyhole size={14} /> Read-only</span>
    </div>
    {preview ? <>
      <div className="ai-disclosure">
        <Globe2 size={18} />
        <p>The content below will leave Zibblu and be sent to <strong>{provider}</strong>.
          <span>{new URL(preview.endpoint).hostname}</span>
        </p>
      </div>
      <div className="ai-snapshot">
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
      </div>
      <p className="ai-visibility-note">{share ? <UsersRound size={15} /> : <LockKeyhole size={15} />}
        {share ? "You’ll review the reply before posting it to " + chat + "." : "Only you will see the reply in Zibblu."}
      </p>
      {failure && <p className="inline-error" role="alert">{failure}</p>}
      <div className="dialog-actions">
        <button className="button subtle" disabled={!!busy} onClick={() => {
          if (spent) close();
          else { setPreview(undefined); setFailure(""); }
        }}>{spent ? "Close" : <><ArrowLeft size={16} /> Edit context</>}</button>
        <button className="button primary" disabled={!!busy || spent} onClick={() => void send()}>
          {busy === "send" ? "Waiting for reply…" : "Send to " + provider} <Send size={15} />
        </button>
      </div>
    </> : <>
      <fieldset className="ai-section">
        <legend>Message context</legend>
        <div className="ai-segments">
          {scopes.map(({ kind: value, label, Icon }) => <button key={value} type="button" aria-pressed={kind === value}
            onClick={() => setKind(value)} disabled={!!busy}><Icon size={16} />{label}</button>)}
        </div>
        {kind === "recent" && <label className="ai-recent">Include the latest
          <select value={limit} onChange={event => setLimit(Number(event.target.value))} disabled={!!busy}>
            {[5, 10, 20].map(count => <option key={count} value={count}>{count} text messages</option>)}
          </select>
        </label>}
        {kind === "selected" && <div className="ai-pick-messages">
          <p className="muted">{sequences.length} of {MAX_MESSAGES} selected</p>
          <div className="ai-message-options">
            {messages.filter(message => message.text !== null).map(message => {
              const checked = sequences.includes(message.seq);
              return <label key={message.seq} className="ai-message-option">
                <input type="checkbox" checked={checked} disabled={!!busy || (!checked && sequences.length >= MAX_MESSAGES)}
                  onChange={() => setSequences(current => checked ? current.filter(seq => seq !== message.seq) : [...current, message.seq])} />
                <span><strong>@{message.sender.handle}</strong><span>{message.text}</span></span>
              </label>;
            })}
            {!messages.some(message => message.text !== null) && <p className="muted">No text messages to select.</p>}
            {hasMore && <button className="button subtle" type="button" disabled={loading || !!busy}
              onClick={() => void older()}>{loading ? "Loading…" : "Load older messages"}</button>}
          </div>
        </div>}
      </fieldset>
      <div className="ai-section">
        <div className="ai-section-line">
          <h3>Files <span className="muted">Optional</span></h3>
          <button type="button" className="button subtle" onClick={() => void pick()} disabled={!!busy || files.length >= MAX_FILES}>
            <Paperclip size={15} /> Add text file
          </button>
        </div>
        {files.length > 0 ? <ul className="ai-file-chips">
          {files.map(file => <li key={file.id}><FileText size={15} /><span className="ai-file-name" title={file.name}>{file.name}</span>
            <small>{bytes(file.bytes)}</small>
            <button className="icon-button" type="button" disabled={!!busy} aria-label={"Remove " + file.name}
              onClick={() => void remove(file)}><X size={14} /></button>
          </li>)}
        </ul> : <p className="muted">Up to three UTF-8 text files, 32 KiB each.</p>}
      </div>
      <fieldset className="ai-section">
        <legend>Reply visibility</legend>
        <div className="ai-segments">
          <button type="button" aria-pressed={!share} onClick={() => setShare(false)} disabled={!!busy}>
            <LockKeyhole size={15} /> Only me
          </button>
          <button type="button" aria-pressed={share} onClick={() => setShare(true)} disabled={!!busy}>
            <UsersRound size={16} /> Review to share
          </button>
        </div>
      </fieldset>
      {failure && <p className="inline-error" role="alert">{failure}</p>}
      <div className="dialog-actions">
        <button className="button subtle" onClick={close} disabled={!!busy}>Cancel</button>
        <button className="button primary" disabled={!!busy || (kind === "selected" && sequences.length === 0)}
          onClick={() => void review()}>{busy === "preview" ? "Preparing…" : "Review disclosure"} <ArrowRight size={16} /></button>
      </div>
    </>}
  </Dialog>;
}

export function AssistantReply({ reply, conversation, chat, refresh, dismiss }: {
  reply: AiReply; conversation: string; chat: string; refresh: () => Promise<void>; dismiss: () => void;
}) {
  const nonce = useRef(crypto.randomUUID());
  const [posting, setPosting] = useState(false);
  const [failure, setFailure] = useState("");
  const label = aiProviders[reply.assistant.provider].label;
  const message = "AI reply from @" + reply.assistant.name + " (" + label + ")\n\n" + reply.text;
  const tooLong = new TextEncoder().encode(message).length > 8192;

  async function post() {
    setPosting(true);
    setFailure("");
    try {
      await api.send(conversation, message, nonce.current);
      await refresh();
      dismiss();
    } catch (error) { setFailure(errorMessage(error)); }
    finally { setPosting(false); }
  }

  return <article className="ai-reply" aria-label={"AI reply from @" + reply.assistant.name}>
    <header>
      <span className="ai-provider-icon"><Bot size={19} /></span>
      <div><strong>@{reply.assistant.name}</strong><small>{label}</small></div>
      <span className="ai-read-only"><LockKeyhole size={13} /> Only you</span>
      <button className="icon-button" type="button" aria-label="Dismiss AI reply" disabled={posting} onClick={dismiss}><X size={16} /></button>
    </header>
    <p>{reply.text}</p>
    {reply.share && <footer>
      {tooLong ? <span className="muted">This reply exceeds the chat message limit.</span> : <>
        <span className="muted">Everyone in {chat} can see the posted reply.</span>
        <button className="button subtle" disabled={posting} onClick={() => void post()}>
          {posting ? "Posting…" : "Post reply"} <ArrowUpRight size={15} />
        </button>
      </>}
    </footer>}
    {failure && <p className="inline-error" role="alert">{failure}</p>}
  </article>;
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
        <Bot size={17} />
        <span><strong>@{assistant.name}</strong><small>{aiProviders[assistant.provider].label} · {assistant.model}</small></span>
        {active === index && <Check size={15} />}
      </button>)}
    </div>
    {assistants.length === 0 && <button className="ai-mention-option" type="button" onClick={settings}>
      <Bot size={17} /><span>Manage assistants</span><ChevronRight size={15} />
    </button>}
  </div>;
}
