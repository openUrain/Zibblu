import { listen } from "@tauri-apps/api/event";
import { ArrowLeft, Bot, Send, Settings } from "lucide-react";
import { Fragment, useEffect, useLayoutEffect, useRef, useState } from "react";
import { AiJobBubble, AiPromptBubble, AiSources, builtinAssistantName, ProviderLogo, ShareAiReply, useAiMessageMenu } from "./ai";
import { ai, aiProviders, builtinAssistantId } from "./ai-api";
import { useAiTerms } from "./ai-terms";
import type { AiJob, AiPrompt, AiReply, Assistant, BuiltinModel, BuiltinSelection } from "./ai-api";
import { errorMessage } from "./api";
import type { Conversation, User } from "./api";
import { Blank, Dialog, messageTimeLabel, startsTimeGroup } from "./common";
import { clearMessageSelectionOnBlank } from "./context-menu";
import { FloatingField } from "./floating-field";
import { t } from "./locale";
import { MediaSplitButton } from "./media-split-button";
import { RichMessage } from "./rich-message";
import { Selector } from "./selector";

const effortLabels: Record<string, [string, string]> = {
  none: ["Off", "关闭"], low: ["Low", "低"], medium: ["Medium", "中"],
  high: ["High", "高"], xhigh: ["Extra high", "很高"], max: ["Max", "最高"],
};

export function BuiltinAssistant({ own, assistants, chats, prompts, replies, jobs, settings, refresh, back }: {
  own: User; assistants: Assistant[]; chats: Conversation[];
  prompts: AiPrompt[]; replies: AiReply[]; jobs: AiJob[];
  settings: () => void; refresh: () => Promise<void>; back: () => void;
}) {
  const [model, setModel] = useState<BuiltinModel | null>(null);
  const selection = model?.selection;
  const provider = assistants.find(entry => entry.provider === selection?.provider);
  const [models, setModels] = useState<string[]>([]);
  const [manual, setManual] = useState<string>();
  const [question, setQuestion] = useState("");
  const [approval, setApproval] = useState<{ selection: BuiltinSelection; endpoint: string | null; prompt: string }>();
  const [sharing, setSharing] = useState<AiReply>();
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState("");
  const terms = useAiTerms();
  const log = useRef<HTMLDivElement>(null);
  const atBottom = useRef(true);
  const failed = (error: unknown) => setFailure(errorMessage(error));
  const menu = useAiMessageMenu(failed);

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

  useEffect(() => { setModels([]); }, [provider?.provider, provider?.base_url]);

  async function save(next: BuiltinSelection) {
    setBusy(true);
    setFailure("");
    try { setModel(await ai.builtinModel(next)); setManual(undefined); }
    catch (error) { failed(error); }
    finally { setBusy(false); }
  }

  async function discover() {
    if (!provider) return;
    setBusy(true);
    setFailure("");
    try {
      if (!await terms.requireAcceptance()) return;
      const available = (await ai.testConnection(provider.provider, provider.base_url ?? "")).models;
      setModels(available);
      if (!available.length) setFailure(t("The provider returned no model IDs. Enter an ID to continue.", "提供商未返回模型 ID，可以手动输入。"));
    } catch (error) { failed(error); }
    finally { setBusy(false); }
  }

  async function confirm() {
    if (!selection || !provider || !question.trim() || busy) return;
    if (new TextEncoder().encode(question.trim()).length > 8192) {
      setFailure(t("Enter a question up to 8 KiB.", "问题不能超过 8 KiB。"));
      return;
    }
    setFailure("");
    setBusy(true);
    try {
      if (await terms.requireAcceptance()) {
        setApproval({ selection, endpoint: provider.base_url ?? null, prompt: question.trim() });
      }
    } catch (error) { failed(error); }
    finally { setBusy(false); }
  }

  async function send() {
    if (!approval || busy) return;
    setBusy(true);
    try {
      await ai.startBuiltin(approval.selection, approval.endpoint, approval.prompt);
      setApproval(undefined);
      setQuestion("");
      atBottom.current = true;
    } catch (error) {
      failed(error);
      setApproval(undefined);
    } finally { setBusy(false); }
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

  return <section className="chat-view builtin-assistant">
    <header className="chat-header">
      <div className="chat-heading">
        <button className="icon-button chat-back" type="button" aria-label={t("Back", "返回")} onClick={back}><ArrowLeft size={20} /></button>
        <h2>{builtinAssistantName()}</h2>
      </div>
      <div className="builtin-model-controls">
        {selection && <>
          <Selector aria-label={t("Provider", "提供商")} title={t("Provider", "提供商")} value={selection.provider} disabled={busy}
            onChange={event => {
              const entry = assistants.find(item => item.provider === event.currentTarget.value);
              if (entry) void save({ provider: entry.provider, model: entry.model, effort: null });
            }}>
            {assistants.map(entry => <option key={entry.provider} value={entry.provider}>{aiProviders[entry.provider].label}</option>)}
          </Selector>
          <Selector aria-label={t("Model", "模型")} title={selection.model} value={selection.model} disabled={busy}
            onChange={event => void save({ ...selection, model: event.currentTarget.value, effort: null })}>
            {[...new Set([selection.model, provider?.model ?? selection.model, ...models])].map(id => <option key={id}>{id}</option>)}
          </Selector>
          <Selector aria-label={t("Thinking effort", "思考强度")} value={selection.effort ?? ""}
            disabled={busy || !model?.efforts.length} onChange={event => void save({ ...selection, effort: event.currentTarget.value || null })}>
            <option value="">{model?.efforts.length ? t("Default effort", "默认强度") : t("Effort managed by provider", "强度由提供商决定")}</option>
            {model?.efforts.map(effort => <option key={effort} value={effort}>{t(...effortLabels[effort])}</option>)}
          </Selector>
        </>}
        <MediaSplitButton className="builtin-model-config" label={t("Model settings", "模型配置")}
          menuLabel={t("Model options", "模型选项")} run={settings} choices={provider ? [
            { label: t("Load models", "获取模型列表"), run: () => void discover() },
            { label: t("Enter model ID", "输入模型 ID"), run: () => setManual(selection?.model ?? "") },
          ] : []}><Settings size={16} /></MediaSplitButton>
      </div>
    </header>
    {failure && <p className="inline-error builtin-feedback" role="alert">{failure}</p>}
    <div className="timeline builtin-timeline" ref={log} onPointerDown={clearMessageSelectionOnBlank} onScroll={event => {
      const element = event.currentTarget;
      atBottom.current = element.scrollHeight - element.scrollTop - element.clientHeight < 100;
    }}>
      {!timeline.length && <Blank icon={<Bot size={28} />} title={t("Ask a question", "问个问题")} />}
      <div className="messages builtin-thread" role="log" aria-label={t("Assistant messages", "助手消息")}>
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
                  <div className="message-body"><RichMessage text={item.text} cacheKey={item} />
                    <AiSources sources={item.sources ?? []} failed={failed} open={number => {
                      void ai.source(item.id, number).catch(failed);
                    }} />
                  </div>
                </div>}
          </Fragment>;
        })}
      </div>
    </div>
    <form className="composer builtin-composer" onSubmit={event => { event.preventDefault(); void confirm(); }}>
      <FloatingField multiline label={t("Ask the assistant", "向助手提问")} value={question} onChange={event => setQuestion(event.currentTarget.value)}
        onKeyDown={event => {
          if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing && event.keyCode !== 229) {
            event.preventDefault();
            void confirm();
          }
        }} />
      <button type="submit" className="button primary" disabled={!provider || !question.trim() || busy}
        aria-label={t("Send question", "发送问题")}><Send size={17} /></button>
    </form>
    {manual !== undefined && selection && <Dialog title={t("Model ID", "模型 ID")} close={() => setManual(undefined)} closeDisabled={busy}>
      <form onSubmit={event => { event.preventDefault(); void save({ ...selection, model: manual, effort: null }); }}>
        <FloatingField label={t("Model ID", "模型 ID")} value={manual} onChange={event => setManual(event.currentTarget.value)} maxLength={256} required />
        <div className="dialog-actions"><button type="submit" className="button primary" disabled={busy || !manual.trim()}>{t("Save", "保存")}</button></div>
      </form>
    </Dialog>}
    {approval && <Dialog title={t("Send to AI provider?", "发送给 AI 提供商？")} close={() => setApproval(undefined)} closeDisabled={busy}>
      <p><strong>{aiProviders[approval.selection.provider].label} · {approval.selection.model}</strong></p>
      <small>{approval.endpoint || aiProviders[approval.selection.provider].base}</small>
      <p>{t("Send this question to the provider. Name any ordinary chat you want to search. Each chat requires separate approval before relevant messages or attachments are sent for this task.",
        "本次问题将发送给上述提供商。请在问题中写出要检索的普通会话名称。每个会话都需单独授权，相关消息或附件才可用于本次任务。")}</p>
      <blockquote>{approval.prompt}</blockquote>
      <div className="dialog-actions">
        <button type="button" className="button subtle" disabled={busy} onClick={() => setApproval(undefined)}>{t("Cancel", "取消")}</button>
        <button type="button" className="button primary" disabled={busy} onClick={() => void send()}>{t("Send", "发送")}</button>
      </div>
    </Dialog>}
    {terms.dialog}
    {sharing && <ShareAiReply reply={sharing} members={[]} own={own.id} destinations={chats} close={() => setSharing(undefined)} published={async () => {
      await refresh(); setSharing(undefined);
    }} />}
  </section>;
}
