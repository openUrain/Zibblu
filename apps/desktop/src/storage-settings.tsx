import { ChevronRight, Trash2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { errorMessage } from "./api";
import type { Conversation, Directory } from "./api";
import { Avatar, chatName } from "./common";
import { decimalSize } from "./file-labels";
import { actions, t } from "./locale";
import "./storage-settings.css";

export type ConversationStorage = {
  conversation_id: string;
  media_bytes: number;
  chat_data_bytes: number;
  chat_data_estimated: boolean;
  files_bytes: number;
  total_bytes: number;
};

export type StorageStats = {
  app_bytes: number;
  conversation_media_bytes: number;
  database_bytes: number;
  public_cache_and_temp_bytes: number;
  outbox_bytes: number;
  total_bytes: number;
  conversations: ConversationStorage[];
  volumes?: StorageVolume[];
};

type StorageVolume = {
  name: string;
  zibblu_bytes: number;
  total_bytes: number | null;
  free_bytes: number | null;
};

function DiskUsage({ volume }: { volume: StorageVolume }) {
  const { total_bytes: total, free_bytes: free, zibblu_bytes: zibblu } = volume;
  const used = total !== null && free !== null ? total - free : null;
  const parts = used !== null && free !== null && zibblu <= used
    ? [
        { label: "Zibblu", bytes: zibblu },
        { label: t("Other used space", "其他占用"), bytes: used - zibblu },
        { label: t("Free space", "剩余空间"), bytes: free },
      ]
    : [];
  return <div className="storage-volume">
    <div className="storage-volume-heading">
      <strong>{volume.name || t("Unknown volume", "未知磁盘")}</strong>
      {used !== null && total !== null
        ? <span>{decimalSize(used)} / {decimalSize(total)}</span>
        : <span>{t("Capacity unavailable", "无法读取磁盘容量")}</span>}
    </div>
    {total !== null && parts.length > 0 && <>
      <div className="storage-volume-bar" aria-hidden="true">
        {parts.map(part => <span key={part.label} style={{ width: `${part.bytes / total * 100}%` }} />)}
      </div>
      <ul className="storage-volume-legend">{parts.map(part =>
        <li key={part.label}>{part.label} {decimalSize(part.bytes)}</li>,
      )}</ul>
    </>}
    {parts.length === 0 && <span className="muted">Zibblu {decimalSize(zibblu)}{used !== null
      ? ` · ${t("Usage breakdown unavailable", "无法计算占用分布")}` : ""}</span>}
  </div>;
}

export type CacheCleanupReport = {
  removed_public_files: number;
  removed_temp_files: number;
  reclaimed_bytes: number;
};

export function StorageSettings({
  ownId,
  conversations,
  remarks,
  avatars,
  revision,
  loadStats,
  clearCache,
}: {
  ownId: string;
  conversations: Conversation[];
  remarks: Directory["remarks"];
  avatars: Directory["avatars"];
  revision: number;
  loadStats: () => Promise<StorageStats>;
  clearCache: () => Promise<CacheCleanupReport>;
}) {
  const [stats, setStats] = useState<StorageStats>();
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState("");
  const [clearing, setClearing] = useState(false);
  const [clearError, setClearError] = useState("");
  const [cleanupReport, setCleanupReport] = useState<CacheCleanupReport>();
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const hasStats = useRef(false);

  const loadSeq = useRef(0);
  const loadStatsRef = useRef(loadStats);
  loadStatsRef.current = loadStats;
  const clearCacheRef = useRef(clearCache);
  clearCacheRef.current = clearCache;
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const refreshStats = useCallback(async (background = false) => {
    const seq = ++loadSeq.current;
    if (!background) setLoading(true);
    setLoadError("");
    try {
      const next = await loadStatsRef.current();
      if (!mounted.current || seq !== loadSeq.current) return;
      hasStats.current = true;
      setStats(next);
    } catch (error) {
      if (!mounted.current || seq !== loadSeq.current) return;
      setLoadError(errorMessage(error));
    } finally {
      if (mounted.current && seq === loadSeq.current) {
        setLoading(false);
      }
    }
  }, []);

  useEffect(() => {
    void refreshStats(hasStats.current);
  }, [revision, refreshStats]);

  async function runClearCache() {
    if (clearing) return;
    setClearing(true);
    setClearError("");
    setCleanupReport(undefined);
    try {
      const report = await clearCacheRef.current();
      if (!mounted.current) return;
      setCleanupReport(report);
      await refreshStats();
    } catch (error) {
      if (!mounted.current) return;
      setClearError(errorMessage(error));
    } finally {
      if (mounted.current) {
        setClearing(false);
      }
    }
  }

  const chatById = useMemo(
    () => new Map(conversations.map(chat => [chat.id, chat])),
    [conversations],
  );

  const orderedConversations = useMemo(() => {
    if (!stats) return [];
    return [...stats.conversations].sort((a, b) =>
      b.total_bytes - a.total_bytes || a.conversation_id.localeCompare(b.conversation_id),
    );
  }, [stats]);

  const globalCategories = stats?.volumes ? [
    { key: "app", label: t("Application", "应用程序"), value: stats.app_bytes },
    { key: "media", label: t("Media & files", "媒体与文件"), value: stats.conversation_media_bytes + stats.outbox_bytes },
    { key: "db", label: t("Chat content", "聊天内容"), value: stats.database_bytes },
    { key: "cache", label: t("Cache & temporary content", "缓存与临时内容"), value: stats.public_cache_and_temp_bytes },
  ] : stats ? [
    { key: "app", label: t("Application", "应用程序"), value: stats.app_bytes },
    { key: "media", label: t("Conversation media & files", "普通会话媒体与附件"), value: stats.conversation_media_bytes },
    { key: "db", label: t("Chat database", "聊天数据库"), value: stats.database_bytes },
    { key: "cache", label: t("Public cache & temp files", "公共缓存与临时文件"), value: stats.public_cache_and_temp_bytes },
    { key: "outbox", label: t("Pending sends", "待发送队列"), value: stats.outbox_bytes },
  ] : [];

  const statusText = clearing
    ? t("Clearing cache…", "正在清理缓存…")
    : loading
      ? t("Scanning storage…", "正在统计存储占用…")
      : cleanupReport
        ? cleanupReport.reclaimed_bytes > 0
          ? t(
              `Reclaimed ${decimalSize(cleanupReport.reclaimed_bytes)} (${cleanupReport.removed_public_files} cached images, ${cleanupReport.removed_temp_files} temporary files)`,
              `已释放 ${decimalSize(cleanupReport.reclaimed_bytes)}（公共缓存 ${cleanupReport.removed_public_files} 项，临时文件 ${cleanupReport.removed_temp_files} 项）`,
            )
          : t("No expired cache or temporary files to remove", "没有可清理的过期缓存或临时文件")
        : "";

  return (
    <div className="storage-settings">
      <section className="settings-group" aria-labelledby="storage-overview-title">
        <h2 id="storage-overview-title">{t("Storage", "存储空间")}</h2>
        {stats?.volumes?.map(volume => <DiskUsage key={volume.name} volume={volume} />)}
        <div className={"settings-row storage-summary-row" + (stats?.volumes ? " storage-volume-actions" : "")}>
          {!stats?.volumes && <div className="storage-summary-main">
            <span className="storage-summary-label">{t("Total storage used", "总占用")}</span>
            <strong className="storage-summary-total">{stats ? decimalSize(stats.total_bytes) : "—"}</strong>
          </div>}
          <button
            className="button primary storage-clear-button"
            type="button"
            disabled={clearing}
            onClick={() => void runClearCache()}
          >
            <Trash2 size={16} aria-hidden="true" />
            <span>{clearing ? t("Clearing…", "清理中…") : t("Clear cache", "清理缓存")}</span>
          </button>
        </div>
        <div className="storage-status-line muted" role="status" aria-live="polite">
          {statusText}
        </div>
        {loadError && (
          <div className="settings-feedback">
            <p className="inline-error" role="alert">{loadError}</p>
            <button className="button subtle small" type="button" onClick={() => void refreshStats()}>
              {actions.retry}
            </button>
          </div>
        )}
        {clearError && (
          <div className="settings-feedback">
            <p className="inline-error" role="alert">{clearError}</p>
            <button className="button subtle small" type="button" disabled={clearing} onClick={() => void runClearCache()}>
              {actions.retry}
            </button>
          </div>
        )}
        {stats && (
          <>
          {stats.volumes && <h2>{t("Storage usage details", "空间占用详情")}</h2>}
          <dl className="storage-breakdown">
            {globalCategories.map(item => (
              <div className="settings-row storage-breakdown-row" key={item.key}>
                <dt>{item.label}</dt>
                <dd>{decimalSize(item.value)}</dd>
              </div>
            ))}
          </dl>
          </>
        )}
      </section>

      <section className="settings-group" aria-labelledby="storage-conversations-title">
        <h2 id="storage-conversations-title">{t("Conversations", "会话存储排行")}</h2>
        {!stats && loading ? (
          <p className="settings-row muted">{t("Loading conversation storage…", "正在加载会话存储占用…")}</p>
        ) : orderedConversations.length === 0 ? (
          <p className="settings-row muted">{t("No conversation storage data", "暂无会话存储占用")}</p>
        ) : (
          <div className="storage-conv-list">
            {orderedConversations.map(item => {
              const chat = chatById.get(item.conversation_id);
              const isGroup = chat ? chat.title !== null : false;
              const name = chat
                ? chatName(chat, ownId, remarks)
                : t("Other conversation", "其他本地会话");
              const peer = chat && !isGroup
                ? chat.members.find(member => member.id !== ownId)
                : undefined;
              const avatarVersion = peer ? avatars[peer.id] : undefined;
              const isExpanded = !!expanded[item.conversation_id];
              const detailId = `storage-conv-detail-${item.conversation_id}`;

              return (
                <div className="storage-conv-item" key={item.conversation_id}>
                  <button
                    className="settings-row storage-conv-toggle"
                    type="button"
                    aria-expanded={isExpanded}
                    aria-controls={detailId}
                    onClick={() =>
                      setExpanded(current => ({
                        ...current,
                        [item.conversation_id]: !current[item.conversation_id],
                      }))
                    }
                  >
                    <Avatar
                      name={name}
                      group={isGroup}
                      cached={peer && avatarVersion ? { id: peer.id, version: avatarVersion } : undefined}
                    />
                    <span className="storage-conv-name" title={name}>{name}</span>
                    <span className="storage-conv-total">{decimalSize(item.total_bytes)}</span>
                    <ChevronRight className="storage-conv-chevron" size={16} aria-hidden="true" />
                  </button>
                  {isExpanded && (
                    <dl className="storage-conv-details" id={detailId}>
                      <div className="storage-conv-detail-row">
                        <dt>{t("Photos & videos", "图片与视频")}</dt>
                        <dd>{decimalSize(item.media_bytes)}</dd>
                      </div>
                      <div className="storage-conv-detail-row">
                        <dt>
                          {item.chat_data_estimated
                            ? t("Chat data (estimated)", "聊天数据（估算）")
                            : t("Chat data", "聊天数据")}
                        </dt>
                        <dd>{decimalSize(item.chat_data_bytes)}</dd>
                      </div>
                      <div className="storage-conv-detail-row">
                        <dt>{t("Files & archives", "文件与压缩包")}</dt>
                        <dd>{decimalSize(item.files_bytes)}</dd>
                      </div>
                    </dl>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </section>
    </div>
  );
}
