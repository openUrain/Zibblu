import { CheckCircle2, Circle, LoaderCircle, Search } from "lucide-react";
import { pinyin } from "pinyin-pro";
import { useEffect, useMemo, useRef, useState } from "react";
import { api, errorMessage } from "./api";
import type { Conversation, ForwardTarget, User, ViewerForwardItem, ViewerForwardTargets } from "./api";
import { Avatar, Dialog, chatName, displayName } from "./common";
import { t } from "./locale";
import "./media-forward-picker.css";

export const MAX_FORWARD_TARGETS = 9;

type PickerCandidate = {
  key: string;
  kind: "conversation" | "contact";
  section: "recent" | "contact" | "group";
  conversationId?: string;
  contactId?: string;
  title: string;
  subtitle?: string;
  group: boolean;
  searchTokens: string;
  avatarName: string;
  avatarUser?: User;
  avatarVersion?: string | null;
};

export type ResolvedForwardAttempt = {
  batchId: string;
  items: ViewerForwardItem[];
};

type Props = {
  open: boolean;
  submitting: boolean;
  onClose: () => void;
  onConfirm: (batchId: string, targets: ForwardTarget[]) => Promise<ResolvedForwardAttempt>;
  onRetryFailed: (batchId: string, fullTargets: ForwardTarget[]) => Promise<ResolvedForwardAttempt>;
  onDoneAfterPartial: () => void;
};

function buildSearchTokens(...parts: Array<string | undefined>): string {
  const tokens: string[] = [];
  for (const part of parts) {
    const trimmed = part?.trim();
    if (!trimmed) continue;
    tokens.push(trimmed.toLowerCase());
    const full = pinyin(trimmed, { toneType: "none", nonZh: "consecutive", type: "array" })
      .join("")
      .normalize("NFD")
      .replace(/\p{M}/gu, "")
      .toLowerCase();
    if (full) tokens.push(full);
    const initials = pinyin(trimmed, { pattern: "first", toneType: "none", nonZh: "consecutive", type: "array" })
      .join("")
      .normalize("NFD")
      .replace(/\p{M}/gu, "")
      .toLowerCase();
    if (initials) tokens.push(initials);
  }
  return tokens.join(" ");
}

function directPeer(chat: Conversation, ownId: string): User | undefined {
  if (chat.title !== null) return undefined;
  return chat.members.find(member => member.id !== ownId) ?? chat.members[0];
}

export function MediaForwardPicker({
  open,
  submitting,
  onClose,
  onConfirm,
  onRetryFailed,
  onDoneAfterPartial,
}: Props) {
  const [data, setData] = useState<ViewerForwardTargets | null>(null);
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState("");
  const [submitError, setSubmitError] = useState("");
  const [query, setQuery] = useState("");
  const [selectedKeys, setSelectedKeys] = useState<string[]>([]);
  const [resolving, setResolving] = useState(false);
  const [partialResult, setPartialResult] = useState<{
    batchId: string;
    fullTargets: ForwardTarget[];
    items: ViewerForwardItem[];
    labels: Map<string, string>;
  } | null>(null);
  const pendingSubmitRef = useRef<{
    selectionKey: string;
    batchId: string;
    targets: ForwardTarget[];
    labels: Map<string, string>;
  } | null>(null);

  useEffect(() => {
    if (!open) {
      setQuery("");
      setSelectedKeys([]);
      setSubmitError("");
      setLoadError("");
      setPartialResult(null);
      pendingSubmitRef.current = null;
      return;
    }
    let cancelled = false;
    setLoading(true);
    setLoadError("");
    void api.viewerForwardTargets().then(res => {
      if (!cancelled) setData(res);
    }).catch(err => {
      if (!cancelled) setLoadError(errorMessage(err));
    }).finally(() => {
      if (!cancelled) setLoading(false);
    });
    return () => { cancelled = true; };
  }, [open]);

  const candidates = useMemo<PickerCandidate[]>(() => {
    if (!data) return [];
    const { own_id: ownId, current_conversation_id: currentConvId, directory } = data;
    const directContactIds = new Set<string>();
    const list: PickerCandidate[] = [];

    const sortedChats = [...directory.conversations].sort((a, b) => {
      if (a.id === currentConvId && b.id !== currentConvId) return -1;
      if (b.id === currentConvId && a.id !== currentConvId) return 1;
      return b.activity_at - a.activity_at;
    });

    for (const chat of sortedChats) {
      const isGroup = chat.title !== null;
      const peer = directPeer(chat, ownId);
      if (peer) directContactIds.add(peer.id);
      const title = chatName(chat, ownId, directory.remarks);
      const subtitle = chat.id === currentConvId
        ? t("Current chat", "当前会话")
        : isGroup
          ? t(`${chat.members.length} members`, `${chat.members.length} 人群聊`)
          : peer?.handle;
      list.push({
        key: `conv:${chat.id}`,
        kind: "conversation",
        section: isGroup ? "group" : "recent",
        conversationId: chat.id,
        contactId: peer?.id,
        title,
        subtitle,
        group: isGroup,
        searchTokens: buildSearchTokens(title, peer?.handle, peer ? directory.remarks[peer.id] : undefined),
        avatarName: title,
        avatarUser: peer,
        avatarVersion: peer ? directory.avatars[peer.id] : undefined,
      });
    }

    for (const contact of directory.contacts) {
      if (directContactIds.has(contact.id)) continue;
      const title = displayName(contact, directory.remarks);
      list.push({
        key: `contact:${contact.id}`,
        kind: "contact",
        section: "contact",
        contactId: contact.id,
        title,
        subtitle: directory.remarks[contact.id] ? `@${contact.handle}` : undefined,
        group: false,
        searchTokens: buildSearchTokens(title, contact.handle, directory.remarks[contact.id]),
        avatarName: title,
        avatarUser: contact,
        avatarVersion: directory.avatars[contact.id],
      });
    }

    return list;
  }, [data]);

  const filtered = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return candidates;
    return candidates.filter(item => item.searchTokens.includes(needle));
  }, [candidates, query]);

  const recentItems = filtered.filter(item => item.section === "recent");
  const groupItems = filtered.filter(item => item.section === "group");
  const contactItems = filtered.filter(item => item.section === "contact");

  const selectedSet = useMemo(() => new Set(selectedKeys), [selectedKeys]);
  const atLimit = selectedKeys.length >= MAX_FORWARD_TARGETS;
  const busy = submitting || resolving;

  function toggleKey(key: string) {
    if (busy) return;
    setSubmitError("");
    setSelectedKeys(prev => {
      if (prev.includes(key)) {
        return prev.filter(item => item !== key);
      }
      if (prev.length >= MAX_FORWARD_TARGETS) return prev;
      return [...prev, key];
    });
  }

  async function submitSelected() {
    if (busy || selectedKeys.length === 0) return;
    setSubmitError("");
    setResolving(true);
    try {
      const selectionKey = selectedKeys.join("|");
      let attempt = pendingSubmitRef.current;
      if (!attempt || attempt.selectionKey !== selectionKey) {
        const candidateMap = new Map(candidates.map(item => [item.key, item]));
        const chosen = selectedKeys.map(key => candidateMap.get(key)).filter(Boolean) as PickerCandidate[];
        const convTargets = new Map<string, { label: string }>();

        for (const item of chosen) {
          let convId = item.conversationId;
          if (!convId && item.contactId) {
            convId = await api.viewerResolveDirectTarget(item.contactId);
          }
          if (convId && !convTargets.has(convId)) {
            convTargets.set(convId, { label: item.title });
          }
        }

        if (convTargets.size === 0) {
          throw new Error(t("Select at least one valid target", "请选择至少一个有效会话"));
        }
        if (convTargets.size > MAX_FORWARD_TARGETS) {
          throw new Error(t("You can forward to at most 9 conversations", "最多只能转发到 9 个会话"));
        }

        const targets: ForwardTarget[] = [];
        const labels = new Map<string, string>();
        for (const [conversation_id, meta] of convTargets.entries()) {
          targets.push({ conversation_id, attachment_id: crypto.randomUUID() });
          labels.set(conversation_id, meta.label);
        }
        attempt = {
          selectionKey,
          batchId: crypto.randomUUID(),
          targets,
          labels,
        };
        pendingSubmitRef.current = attempt;
      }

      const result = await onConfirm(attempt.batchId, attempt.targets);
      pendingSubmitRef.current = null;
      if (result.items.some(item => item.status === "failed")) {
        setPartialResult({
          batchId: result.batchId,
          fullTargets: attempt.targets,
          items: result.items,
          labels: attempt.labels,
        });
      }
    } catch (err) {
      setSubmitError(errorMessage(err));
    } finally {
      setResolving(false);
    }
  }

  async function retryFailed() {
    if (!partialResult || busy) return;
    if (!partialResult.items.some(item => item.status === "failed")) return;
    setSubmitError("");
    setResolving(true);
    try {
      const retried = await onRetryFailed(partialResult.batchId, partialResult.fullTargets);
      const merged = partialResult.items.map(existing => {
        const updated = retried.items.find(item => item.target.conversation_id === existing.target.conversation_id);
        return updated ?? existing;
      });
      if (merged.some(item => item.status === "failed")) {
        setPartialResult({ ...partialResult, items: merged });
      } else {
        onDoneAfterPartial();
      }
    } catch (err) {
      setSubmitError(errorMessage(err));
    } finally {
      setResolving(false);
    }
  }

  if (!open) return null;

  const renderSection = (title: string, items: PickerCandidate[]) => {
    if (items.length === 0) return null;
    return <>
      <div className="media-forward-section">{title}</div>
      {items.map(item => {
        const selected = selectedSet.has(item.key);
        const disabled = busy || (!selected && atLimit);
        return <button
          key={item.key}
          type="button"
          className="media-forward-row"
          aria-pressed={selected}
          disabled={disabled}
          onClick={() => toggleKey(item.key)}
        >
          <Avatar
            name={item.avatarName}
            group={item.group}
            src={item.avatarUser && item.avatarVersion ? api.profileImageUrl(item.avatarUser.id, item.avatarVersion) : undefined}
          />
          <span className="media-forward-row-info">
            <span className="media-forward-row-title">{item.title}</span>
            {item.subtitle && <span className="media-forward-row-sub">{item.subtitle}</span>}
          </span>
          {selected
            ? <CheckCircle2 className="media-forward-check" size={18} aria-hidden />
            : <Circle className="media-forward-check" size={18} aria-hidden />}
        </button>;
      })}
    </>;
  };

  return (
    <Dialog
      title={partialResult ? t("Forward summary", "转发结果") : t("Forward to…", "转发给…")}
      close={busy ? () => {} : partialResult ? onDoneAfterPartial : onClose}
      closeDisabled={busy}
      className="media-forward-dialog"
    >
      {partialResult ? (
        <div className="media-forward-body">
          <div className="media-forward-results">
            {partialResult.items.map(item => {
              const name = partialResult.labels.get(item.target.conversation_id) ?? item.target.conversation_id;
              const statusText = item.status === "sent"
                ? t("Sent", "已发送")
                : item.status === "queued"
                  ? t("Queued", "已加入队列")
                  : item.status === "cancelled"
                    ? t("Cancelled", "已取消")
                    : (item.error ? errorMessage(item.error) : t("Failed", "失败"));
              return (
                <div key={item.target.conversation_id} className="media-forward-result-row">
                  <span className="media-forward-row-title">{name}</span>
                  <span className="media-forward-result-status" data-status={item.status}>
                    {statusText}
                  </span>
                </div>
              );
            })}
          </div>
          {submitError && <p className="inline-error" role="alert">{submitError}</p>}
          <div className="media-forward-footer">
            <div className="media-forward-actions">
              <button type="button" className="button subtle" disabled={busy} onClick={onDoneAfterPartial}>
                {t("Close", "关闭")}
              </button>
              {partialResult.items.some(item => item.status === "failed") && (
                <button type="button" className="button primary" disabled={busy} onClick={() => void retryFailed()}>
                  {busy && <LoaderCircle className="preview-spinner" size={15} aria-hidden />}
                  {t(
                    `Retry failed (${partialResult.items.filter(item => item.status === "failed").length})`,
                    `重试失败项 (${partialResult.items.filter(item => item.status === "failed").length})`,
                  )}
                </button>
              )}
            </div>
          </div>
        </div>
      ) : (
        <div className="media-forward-body">
          <div className="media-forward-top">
            <Search size={16} aria-hidden />
            <input
              type="search"
              className="media-forward-search"
              value={query}
              onChange={event => setQuery(event.currentTarget.value)}
              placeholder={t("Search chats, contacts, pinyin…", "搜索会话、联系人或拼音…")}
              aria-label={t("Search targets", "搜索转发对象")}
              autoFocus
            />
            <span className="media-forward-counter" data-full={atLimit}>
              {t(`Selected ${selectedKeys.length} / 9`, `已选 ${selectedKeys.length} / 9`)}
            </span>
          </div>
          {loadError ? (
            <p className="inline-error" role="alert">{loadError}</p>
          ) : loading ? (
            <div className="media-forward-empty">
              <LoaderCircle className="preview-spinner" aria-label={t("Loading contacts", "加载会话列表")} />
            </div>
          ) : (
            <div className="media-forward-list" role="group" aria-label={t("Conversations and contacts", "会话与联系人")}>
              {recentItems.length === 0 && groupItems.length === 0 && contactItems.length === 0 ? (
                <div className="media-forward-empty">{t("No matching chats or contacts", "没有匹配的会话或联系人")}</div>
              ) : (
                <>
                  {renderSection(t("Recent chats", "最近会话"), recentItems)}
                  {renderSection(t("Groups", "群聊"), groupItems)}
                  {renderSection(t("Contacts", "联系人"), contactItems)}
                </>
              )}
            </div>
          )}
          {submitError && <p className="inline-error" role="alert">{submitError}</p>}
          <div className="media-forward-footer">
            <div className="media-forward-actions">
              <button type="button" className="button subtle" disabled={busy} onClick={onClose}>
                {t("Cancel", "取消")}
              </button>
              <button
                type="button"
                className="button primary"
                disabled={busy || selectedKeys.length === 0}
                onClick={() => void submitSelected()}
              >
                {busy && <LoaderCircle className="preview-spinner" size={15} aria-hidden />}
                {t(`Confirm (${selectedKeys.length})`, `确认转发 (${selectedKeys.length})`)}
              </button>
            </div>
          </div>
        </div>
      )}
    </Dialog>
  );
}
