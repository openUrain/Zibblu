import { FloatingField } from "./floating-field";
import { Archive, Check, CircleAlert, Clock, Download, ExternalLink, File, FileSpreadsheet, FileText, FileType2, FolderOpen, Image, LoaderCircle, Pause, Play, Presentation, RotateCcw, Search, Share, Trash2, Upload, Video } from "lucide-react";
import { useCallback, useContext, useId, useState } from "react";
import type { ChangeEvent } from "react";
import { api, errorMessage, fileExpired } from "./api";
import type { Attachment, FileFilter, Message, Transfer } from "./api";
import { mediaKind, readAttachmentPage } from "./attachment-media";
import { Blank, People, PersonAvatar, bytes, dateTimeLabel, displayName, usePage } from "./common";
import { windows } from "./window-chrome";
import { ios } from "./ios";
import { actions, t } from "./locale";
import { WorkspaceSearchField } from "./workspace-search";
import { Selector } from "./selector";
import { decimalSize, splitFileName } from "./file-labels";

export function UploadOverlay({ upload }: { upload?: Transfer }) {
  if (!upload || upload.direction !== "upload") return null;
  if (upload.status === "running") {
    return <span className="upload-overlay" aria-hidden="true">
      <LoaderCircle className="preview-spinner" size={18} />
    </span>;
  }
  if (upload.status === "queued") {
    return <span className="upload-overlay upload-overlay-queued" aria-hidden="true">
      <Clock size={15} />
    </span>;
  }
  return null;
}

export function FileIcon({ file, library = false, upload }: { file: Attachment; library?: boolean; upload?: Transfer }) {
  const media = mediaKind(file);
  const type = file.name.split(".").at(-1)?.toLowerCase() ?? "";
  const archive = /^(zip|7z|rar|tar|gz|bz2|xz)$/.test(type);
  const document = /^(pdf|docx?|xlsx?|pptx?|odt|txt|rtf|csv|md)$/.test(type);
  const standard = file.kind === "folder" ? FolderOpen : archive ? Archive : media === "image" ? Image
    : media === "video" ? Video : document ? FileText : File;
  const format = !library || file.kind === "folder" ? undefined : /^(docx?|odt)$/.test(type) ? "word"
    : /^(xlsx?|csv|ods)$/.test(type) ? "sheet" : /^(pptx?|odp)$/.test(type) ? "slides" : type === "pdf" ? "pdf" : undefined;
  const Icon = format === "sheet" ? FileSpreadsheet : format === "slides" ? Presentation
    : format === "pdf" ? FileType2 : library && !format && document ? File : standard;
  const tone = file.kind === "folder" || archive ? "archive" : media ? "media" : !library && document ? "document" : undefined;
  return <span className="file-icon" data-tone={format ?? tone} aria-hidden="true">
    <Icon size={20} />
    <UploadOverlay upload={upload} />
  </span>;
}

function FileName({ name }: { name: string }) {
  const [stem, extension] = splitFileName(name);
  return <span className="file-name" title={name} tabIndex={0} dir="ltr">
    <strong dir="auto">{stem}</strong>{extension && <strong>{extension}</strong>}
  </span>;
}

export function SharedFile({ message, transfer, onChanged, previewStatus, upload, expired = fileExpired(message) }: {
  message: Message; transfer?: Transfer; onChanged: () => void; previewStatus?: string; expired?: boolean; upload?: Transfer;
}) {
  const file = message.attachment!;
  return <article className="shared-file chat-file-card">
    <div className="file-main">
      <FileIcon file={file} upload={upload} />
      <div className="file-info">
        <FileName name={file.name} />
        <small>{file.kind === "folder" ? "Folder" : (splitFileName(file.name)[1].slice(1).toUpperCase() || "File")} · {bytes(file.size)}</small>
      </div>
    </div>
    {expired || transfer?.status === "expired"
      ? <small className="file-preview-status" role="status">This file expired. Ask the sender to share it again.</small>
      : <>
        {previewStatus && <small className="file-preview-status" role="status">{previewStatus}</small>}
        {upload ? <div className="attachment-download" aria-hidden="true" style={{ visibility: "hidden" }}>
          <div className="file-download-actions"><button className="button subtle small" disabled><Download size={16} /> {actions.save}</button></div>
        </div> : <AttachmentDownload file={file} transfer={transfer} onChanged={onChanged} />}
      </>}
  </article>;
}

export function useAttachmentSave(onChanged: () => void | Promise<void>) {
  const [saving, setSaving] = useState(false);
  const [failure, setFailure] = useState("");

  async function save(id: string) {
    if (saving) return;
    setSaving(true);
    setFailure("");
    try {
      if (await api.save(id)) {
        await onChanged();
        return true;
      }
    }
    catch (error) { setFailure(errorMessage(error)); }
    finally { setSaving(false); }
  }

  return { save, saving, failure };
}

export function AttachmentDownload({ file, transfer, onChanged, hideSave = false }: {
  file: Attachment; transfer?: Transfer; onChanged: () => void; hideSave?: boolean;
}) {
  const { save, saving, failure } = useAttachmentSave(onChanged);
  const share = ios && transfer?.status === "complete";
  const action = ios ? (share ? t("Share", "分享") : t("Download", "下载"))
    : transfer?.status === "complete" ? t("Save as…", "另存为…") : actions.save;
  const label = saving ? actions.saving : failure ? actions.retry : action;
  return <div className="attachment-download">
    {(!transfer || transfer.status === "complete") && (!hideSave || transfer) && <div className="file-download-actions">
      {transfer && <span className="verified" role="status"><Check size={14} /> {actions.saved}</span>}
      {!hideSave && <button className="button subtle small" onClick={() => void save(file.id)} disabled={saving} aria-label={`${label} ${file.name}`}>
        {share ? <Share size={16} /> : <Download size={16} />} {label}
      </button>}
    </div>}
    {transfer && transfer.status !== "complete" &&
      <TransferProgress transfer={transfer} onChanged={onChanged} chooseNew={ios ? undefined : () => void save(file.id)} choosing={saving} />}
    {failure && <p className="inline-error" role="alert">{failure}</p>}
  </div>;
}

function LibraryError({ error, id }: { error?: string | null; id: string }) {
  return <span className="library-error" title={error || undefined}>
    {error && <><CircleAlert size={16} aria-hidden /><span className="sr-only" id={id} role="alert">{error}</span></>}
  </span>;
}

function TransferProgress({ transfer, onChanged, chooseNew, choosing, compact = false, pickerError }: {
  transfer: Transfer; onChanged: () => void; chooseNew?: () => void; choosing?: boolean; compact?: boolean; pickerError?: string;
}) {
  const errorId = useId();
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState("");
  const running = transfer.status === "running";
  const queued = transfer.status === "queued";
  const failed = transfer.status === "failed";
  const expired = transfer.status === "expired";
  const complete = transfer.status === "complete";
  const total = transfer.attachment.size;
  const percentage = total > 0 ? Math.floor(transfer.received * 100 / total) : complete ? 100 : 0;
  const status = complete ? (transfer.direction === "upload" ? t("Shared", "已发送") : actions.saved)
    : expired ? "Expired" : failed ? "Failed" : running
      ? percentage === 100 ? "Verifying…" : transfer.direction === "upload" ? "Uploading" : "Downloading"
      : queued ? "Queued" : "Paused";

  async function action(name: "pause" | "resume" | "discard") {
    setBusy(true);
    setFailure("");
    try { await api.transferAction(name, transfer); onChanged(); }
    catch (error) { setFailure(errorMessage(error)); }
    finally { setBusy(false); }
  }

  const pause = running || queued;
  const recovery = failed ? t("Retry", "重试") : t("Resume", "继续");
  const error = pickerError || failure || transfer.error;
  if (transfer.direction === "upload") {
    const uploadStatus = complete ? t("Shared", "已发送")
      : expired ? t("Expired", "已过期")
        : failed ? t("Upload failed", "上传失败")
          : running ? t("Uploading", "上传中")
            : queued ? t("Queued", "排队中")
              : t("Waiting to upload", "待上传");
    const canRetryUpload = failed || transfer.status === "paused";
    if (compact) {
      return <div className="library-transfer">
        <div className="library-progress" title={uploadStatus}>
          <span role="status" className={error ? "danger" : ""}>{uploadStatus}</span>
        </div>
        {!complete && <button className="icon-button danger library-discard" type="button" disabled={busy}
          title={t("Delete", "删除")} aria-label={t("Delete", "删除") + " " + transfer.attachment.name}
          onClick={() => void action("discard")}><Trash2 size={16} /></button>}
        <LibraryError error={error} id={errorId} />
        {canRetryUpload && <button className={(windows ? "button subtle small" : "icon-button") + " library-transfer-action"} type="button" disabled={busy}
          aria-label={t("Upload", "上传") + " " + transfer.attachment.name} aria-describedby={error ? errorId : undefined} title={t("Upload", "上传")}
          onClick={() => void action("resume")}>
          <Upload size={16} />
          {windows && t("Upload", "上传")}
        </button>}
      </div>;
    }
    return <div className="transfer-progress">
      <div className="transfer-status"><span role="status" className={complete ? "verified" : failed || expired ? "danger" : ""}>
        {complete && <Check size={14} />}{uploadStatus}
      </span></div>
      {!complete && <>
        {error && <p className="inline-error" role="alert">{error}</p>}
        <div className="transfer-buttons">
          {canRetryUpload && <button className="button subtle small" onClick={() => void action("resume")} disabled={busy}>
            <Upload size={14} /> {t("Upload", "上传")}
          </button>}
          <button className="icon-button danger" aria-label={t("Delete", "删除") + " " + transfer.attachment.name}
            title={t("Delete", "删除")} onClick={() => void action("discard")} disabled={busy}><Trash2 size={15} /></button>
        </div>
      </>}
    </div>;
  }
  if (compact) {
    const compactStatus = expired ? t("Expired", "已过期") : failed ? t("Failed", "失败")
      : queued ? t("Queued", "排队中") : running
        ? percentage === 100 ? t("Verifying", "校验中")
          : t("Downloading", "下载中")
        : t("Paused", "已暂停");
    const actionLabel = pause ? t("Pause", "暂停") : recovery;
    return <div className="library-transfer">
      <div className="library-progress" title={compactStatus + " " + percentage + "%"}>
        <span role="status" className={error ? "danger" : ""}>
          {compactStatus} {percentage}%
        </span>
        <progress max={100} value={percentage} aria-label={compactStatus + " " + transfer.attachment.name} />
      </div>
      {!pause && <button className="icon-button danger library-discard" type="button" disabled={busy} title={t("Discard transfer", "丢弃传输")}
        aria-label={t("Discard transfer", "丢弃传输")} onClick={() => void action("discard")}><Trash2 size={16} /></button>}
      {!pause && !expired && chooseNew && <button className="icon-button library-choose" type="button" disabled={busy || choosing}
        title={t("Choose save location", "选择保存位置")} aria-label={t("Choose save location", "选择保存位置")}
        onClick={chooseNew}><Download size={16} /></button>}
      <LibraryError error={error} id={errorId} />
      {!expired && <button className={(windows ? "button subtle small" : "icon-button") + " library-transfer-action"} type="button" disabled={busy}
        aria-label={`${actionLabel} ${transfer.attachment.name}`} aria-describedby={error ? errorId : undefined} title={actionLabel}
        onClick={() => void action(pause ? "pause" : "resume")}>
        {pause ? <Pause size={16} /> : failed ? <RotateCcw size={16} /> : <Play size={16} />}
        {windows && actionLabel}
      </button>}
    </div>;
  }

  return <div className="transfer-progress">
    <div className="transfer-status"><span role="status" className={complete ? "verified" : failed || expired ? "danger" : ""}>
      {complete && <Check size={14} />}{queued ? <span className="status-icon" role="img" aria-label={status} title={status} tabIndex={0}><Clock size={14} aria-hidden /></span> : status}
    </span>{!complete && <span>{bytes(transfer.received)} of {bytes(total)} · {percentage}%</span>}</div>
    {!complete && <>
      <progress max={100} value={percentage} aria-label={status + " " + transfer.attachment.name} />
      {(failure || transfer.error) && <p className="inline-error" role="alert">{failure || transfer.error}</p>}
      <div className="transfer-buttons">
        {running || queued
          ? <button className="button subtle small" onClick={() => void action("pause")} disabled={busy}><Pause size={14} /> Pause</button>
          : <>
            {!expired && <button className="button subtle small" onClick={() => void action("resume")} disabled={busy}>
              {failed ? <RotateCcw size={14} /> : <Play size={14} />}{failed ? "Retry" : "Resume"}
            </button>}
            {chooseNew && !expired && <button className="button subtle small" disabled={busy || choosing} onClick={chooseNew}>
              {choosing ? actions.saving : t("Save as…", "另存为…")}
            </button>}
            <button className="icon-button danger" aria-label={"Discard transfer for " + transfer.attachment.name}
              title="Discard partial transfer" onClick={() => void action("discard")} disabled={busy}><Trash2 size={15} /></button>
          </>}
      </div>
    </>}
  </div>;
}

export function TransferList({ transfers, onChanged, inChat = false }: {
  transfers: Transfer[]; onChanged: () => void; inChat?: boolean;
}) {
  const { remarks } = useContext(People);
  return <div className="transfer-list">{transfers.map(transfer =>
    <div className="shared-file" key={transfer.direction + transfer.attachment.id}>
      <div className="file-main"><FileIcon file={transfer.attachment} upload={transfer.direction === "upload" ? transfer : undefined} />
        <div className="file-info"><FileName name={transfer.attachment.name} />
          <small>{transfer.direction === "upload" && transfer.status !== "complete" ? "Not yet shared"
            : transfer.direction === "upload" ? "Upload" : "Download"} · {bytes(transfer.attachment.size)}</small>
          {!inChat && transfer.sharer && <div className="file-origin"><span className="file-sharer">
            <PersonAvatar user={transfer.sharer} /><span>{displayName(transfer.sharer, remarks)}</span>
          </span></div>}
        </div>
      </div>
      <TransferProgress transfer={transfer} onChanged={onChanged} />
    </div>)}</div>;
}

export function FilesView({ conversation, transfers, revision, recalled, hidden, onChanged }: {
  conversation?: string; transfers: Transfer[]; revision: number; recalled: ReadonlySet<number>;
  hidden: ReadonlySet<number>; onChanged: () => void;
}) {
  const [kind, setKind] = useState<Attachment["kind"] | "">("");
  const [oldestFirst, setOldestFirst] = useState(false);
  const [name, setName] = useState("");
  const desktopLibrary = windows && !conversation;
  const search = { value: name, onChange: (event: ChangeEvent<HTMLInputElement>) => setName(event.target.value) };
  return <div className={"files-view" + (conversation ? " chat-files" : "")}>
    {!conversation && <header className={desktopLibrary ? "sr-only" : "workspace-heading"}><h1>{t("Shared files", "共享文件")}</h1></header>}
    {desktopLibrary && <div className="chat-list-head">
      <WorkspaceSearchField {...search} label={t("Search files", "搜索文件")} />
    </div>}
    <div className="file-filters">
      {!desktopLibrary && <FloatingField {...search} label={t("Search file names", "按文件名搜索")}
        fieldClass="search file-search" prefixIcon={<Search size={16} />} type="search" />}
      <Selector aria-label={t("File type", "文件类型")} value={kind} onChange={event => setKind(event.target.value as typeof kind)}>
        <option value="">{t("All types", "所有类型")}</option><option value="file">{t("Files", "文件")}</option><option value="folder">{t("Folders", "文件夹")}</option>
      </Selector>
      <button className="file-sort" type="button" onClick={() => setOldestFirst(value => !value)}
        aria-label="时间排序" aria-pressed={oldestFirst} title={oldestFirst ? "最早优先" : "最新优先"}>
        时间排序<span className="file-sort-arrows" aria-hidden><i /><i /></span>
      </button>
    </div>
    <SharedHistory key={JSON.stringify([conversation, kind, oldestFirst, name])} {...{
      conversation, kind, oldestFirst, name, revision, recalled, hidden, transfers, onChanged,
    }} />
  </div>;
}

function SharedHistory({ conversation, kind, oldestFirst, name, revision, recalled, hidden, transfers, onChanged }: {
  conversation?: string; kind: Attachment["kind"] | ""; oldestFirst: boolean; name: string;
  revision: number; recalled: ReadonlySet<number>; hidden: ReadonlySet<number>; transfers: Transfer[]; onChanged: () => void;
}) {
  const read = useCallback((before?: number) => {
    const filter: FileFilter = { conversation_id: conversation, kind: kind || undefined,
      name: name.trim() || undefined, oldest_first: oldestFirst, include_transfer_rows: true };
    return readAttachmentPage(cursor => api.files(filter, cursor), false, recalled, hidden, before);
  }, [conversation, kind, oldestFirst, name, recalled, hidden]);
  const { page, failure, loading, older } = usePage(read, revision, recalled, false, undefined, hidden, oldestFirst);
  const downloads = new Map(transfers.filter(job => job.direction === "download").map(job => [job.attachment.id, job]));
  const uploads = new Map(transfers.filter(job => job.direction === "upload" && job.status !== "complete").map(job => [job.attachment.id, job]));
  if (failure) return <p className="inline-error" role="alert">{failure}</p>;
  if (!page) return <p className="muted loading" role="status">Loading shared files…</p>;
  if (page.items.length === 0) return <Blank icon={<FolderOpen size={27} />} title="No shared files found">
    <p>{kind || name ? "Try changing the filters." : "Files and folders shared in chats will appear here."}</p>
  </Blank>;
  // A publication may replace a local pending row while an older page is being appended.
  const items = [...new Map(page.items.map(item => [item.attachment!.id, item])).values()];
  return <div className="shared-list">
    {items.map(message => {
      const id = message.attachment!.id;
      const download = downloads.get(id);
      const transfer = message.seq < 0 ? uploads.get(id) ?? download : download;
      return <LibraryFile key={id} {...{ message, transfer, onChanged }} />;
    })}
    {page.has_more && <button className="button subtle more" disabled={loading} onClick={() => void older()}>Load more files</button>}
  </div>;
}

function LibraryFile({ message, transfer, onChanged }: { message: Message; transfer?: Transfer; onChanged: () => void }) {
  const { remarks } = useContext(People);
  const file = message.attachment!;
  const sender = displayName(message.sender, remarks);
  const { save, saving, failure: saveError } = useAttachmentSave(onChanged);
  const [openError, setOpenError] = useState("");
  const [opening, setOpening] = useState(false);
  const [needsSave, setNeedsSave] = useState(false);
  const errorId = useId();
  const saved = windows && transfer?.status === "complete" && !needsSave;
  const expired = !saved && fileExpired(message);
  const label = saved ? t("Open", "打开") : saveError ? actions.retry : actions.save;
  const error = saveError || openError;
  const action = async () => {
    setOpenError("");
    if (!saved) {
      if (await save(file.id)) setNeedsSave(false);
      return;
    }
    setOpening(true);
    try {
      if (!await api.openSaved(file.id)) setNeedsSave(true);
    } catch (error) { setOpenError(errorMessage(error)); }
    finally { setOpening(false); }
  };
  return <article className="library-file" data-attachment={file.id}>
    <div className="file-main">
      <FileIcon file={file} library upload={transfer?.direction === "upload" ? transfer : undefined} />
      <div className="file-info">
        <FileName name={file.name} />
        <div className="library-meta">
          <PersonAvatar user={message.sender} />
          <span className="library-sender" title={sender}>{sender}</span>
          <span>{decimalSize(file.size)}</span>
          {message.created_at > 0 && <time dateTime={new Date(message.created_at).toISOString()}>
            {dateTimeLabel(message.created_at)}
          </time>}
        </div>
      </div>
    </div>
    <div className="library-actions">
      {transfer && transfer.status !== "complete" ? <TransferProgress transfer={transfer} onChanged={onChanged} compact
        chooseNew={message.seq > 0 && transfer.direction === "download" && !ios ? () => void save(file.id) : undefined}
        choosing={saving} pickerError={error} />
        : expired ? <span role="status" className="library-expired" title="Ask the sender to share this file again.">{t("Expired", "已过期")}</span>
        : message.seq > 0 && (ios ? <AttachmentDownload file={file} transfer={transfer} onChanged={onChanged} /> :
          <>
            <LibraryError error={error} id={errorId} />
            <button className="button subtle small" type="button" disabled={opening || saving} onClick={() => void action()}
              aria-label={`${saving ? actions.saving : label} ${file.name}`} aria-describedby={error ? errorId : undefined}
              aria-busy={opening || saving}>
              {saving ? actions.saving : <>{saved ? <ExternalLink size={15} /> : <Download size={15} />}{label}</>}
            </button>
          </>)}
    </div>
  </article>;
}
