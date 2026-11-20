import { FloatingField } from "./floating-field";
import { Archive, ArrowDown, ArrowUp, Check, Download, File, FileText, FolderOpen, Image, MessageSquare, Pause, Play, RotateCcw, Search, Share, Trash2, Video } from "lucide-react";
import { useCallback, useContext, useId, useState } from "react";
import type { KeyboardEvent } from "react";
import { api, errorMessage, fileExpired } from "./api";
import type { Attachment, FileFilter, Message, Transfer, User } from "./api";
import { mediaKind, readAttachmentPage } from "./attachment-media";
import { Blank, Dialog, People, PersonAvatar, bytes, dateTimeLabel, displayName, usePage } from "./common";
import { windows } from "./window-chrome";
import { ios } from "./ios";
import { actions, t } from "./locale";

export function FileIcon({ file }: { file: Attachment }) {
  const media = mediaKind(file);
  const archive = /\.(zip|7z|rar|tar|gz|bz2|xz)$/i.test(file.name);
  const document = /\.(pdf|docx?|xlsx?|pptx?|odt|txt|rtf|csv|md)$/i.test(file.name);
  const Icon = file.kind === "folder" ? FolderOpen : archive ? Archive : media === "image" ? Image
    : media === "video" ? Video : document ? FileText : File;
  const tone = file.kind === "folder" || archive ? "archive" : media ? "media" : document ? "document" : undefined;
  return <span className="file-icon" data-tone={tone} aria-hidden="true"><Icon size={20} /></span>;
}

function extensionStart(name: string) {
  const dot = name.match(/\.tar\.(gz|bz2|xz)$/i)?.index ?? name.lastIndexOf(".");
  return dot > 0 && dot < name.length - 1 ? dot : name.length;
}

function FileName({ name }: { name: string }) {
  const [open, setOpen] = useState(false);
  const split = extensionStart(name);
  return <>
    <button className="file-name" type="button" title={name} aria-label={name} aria-haspopup="dialog" onClick={() => setOpen(true)}>
      <strong dir="auto">{name.slice(0, split)}</strong>{split < name.length && <strong dir="ltr">{name.slice(split)}</strong>}
    </button>
    {open && <Dialog title={t("File name", "文件名")} close={() => setOpen(false)}>
      <p className="full-file-name" dir="auto">{name}</p>
    </Dialog>}
  </>;
}

export function SharedFile({ message, transfer, onChanged, source, inChat = false, previewStatus, expired = fileExpired(message) }: {
  message: Message; transfer?: Transfer; onChanged: () => void; source?: string; inChat?: boolean; previewStatus?: string; expired?: boolean;
}) {
  const file = message.attachment!;
  const { remarks, open } = useContext(People);
  const sender = displayName(message.sender, remarks);
  const extension = file.name.slice(extensionStart(file.name) + 1).toUpperCase();
  const media = mediaKind(file);
  const type = file.kind === "folder" ? "Folder ZIP" : media === "image" ? "Image"
    : media === "video" ? "Video" : extension.length > 8 || !extension ? "File" : extension;
  return <article className={"shared-file" + (inChat ? " chat-file-card" : "")} aria-label={file.name}>
    <div className="file-main">
      <FileIcon file={file} />
      <div className="file-info">
        <FileName name={file.name} />
        <small>{type} · {bytes(file.size)}</small>
        {!inChat && <div className="file-origin">
          <button className="person-link file-sharer" type="button" onClick={() => open?.(message.sender)}
            title={`View ${sender}'s profile`}><PersonAvatar user={message.sender} /><span>{sender}</span></button>
          {source && <span className="file-source" title={source}><MessageSquare size={12} aria-hidden="true" />
            <span className="sr-only">Conversation </span><span>{source}</span>
          </span>}
          <time dateTime={new Date(message.created_at).toISOString()}>{dateTimeLabel(message.created_at)}</time>
        </div>}
      </div>
    </div>
    {expired || transfer?.status === "expired"
      ? <small className="file-preview-status" role="status">This file expired. Ask the sender to share it again.</small>
      : <>
        {previewStatus && <small className="file-preview-status" role="status">{previewStatus}</small>}
        <AttachmentDownload file={file} transfer={transfer} onChanged={onChanged} />
      </>}
  </article>;
}

export function useAttachmentSave(onChanged: () => void) {
  const [saving, setSaving] = useState(false);
  const [failure, setFailure] = useState("");

  async function save(id: string) {
    if (saving) return;
    setSaving(true);
    setFailure("");
    try { if (await api.save(id)) onChanged(); }
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

function TransferProgress({ transfer, onChanged, chooseNew, choosing }: {
  transfer: Transfer; onChanged: () => void; chooseNew?: () => void; choosing?: boolean;
}) {
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

  return <div className="transfer-progress">
    <div className="transfer-status"><span role="status" className={complete ? "verified" : failed || expired ? "danger" : ""}>
      {complete && <Check size={14} />}{status}
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
      <div className="file-main"><FileIcon file={transfer.attachment} />
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

const fileTabs = { shared: "Shared files", transfers: "Transfers" } as const;
type FileTab = keyof typeof fileTabs;

export function FilesView({ conversation, people, transfers, revision, recalled, hidden, onChanged, sources }: {
  conversation?: string; people: User[]; transfers: Transfer[]; revision: number; recalled: ReadonlySet<number>;
  hidden: ReadonlySet<number>; onChanged: () => void; sources?: ReadonlyMap<string, string>;
}) {
  const { remarks } = useContext(People);
  const [tab, setTab] = useState<FileTab>("shared");
  const tabsId = useId();
  const [person, setPerson] = useState("");
  const [kind, setKind] = useState<Attachment["kind"] | "">("");
  const [oldestFirst, setOldestFirst] = useState(false);
  const [name, setName] = useState("");
  const jobs = transfers.filter(transfer => !mediaKind(transfer.attachment)
    && (!conversation || transfer.conversation_id === conversation));
  const pending = jobs.filter(transfer => transfer.status !== "complete").length;
  function navigateTab(event: KeyboardEvent<HTMLButtonElement>) {
    const next = event.key === "Home" ? "shared" : event.key === "End" ? "transfers"
      : ["ArrowLeft", "ArrowRight"].includes(event.key) ? (tab === "shared" ? "transfers" : "shared") : undefined;
    if (!next) return;
    event.preventDefault();
    setTab(next);
    document.getElementById(`${tabsId}-${next}-tab`)?.focus();
  }
  const desktopLibrary = windows && !conversation;
  const searchField = <FloatingField fieldClass="search file-search" label="Search file names" prefixIcon={<Search size={16} />}
             type="search" value={name} onChange={event => setName(event.target.value)} />;
  return <div className={"files-view" + (conversation ? " chat-files" : "")}>
    {desktopLibrary && tab === "shared" && <div className="chat-list-head">{searchField}</div>}
    {!conversation && <header className={desktopLibrary ? "sr-only" : "workspace-heading"}><h1>Files</h1></header>}
    {!conversation && <div className="file-tabs" role="tablist" aria-label="Files view">
      {(Object.keys(fileTabs) as FileTab[]).map(id => <button key={id} type="button" role="tab"
        id={`${tabsId}-${id}-tab`} aria-controls={`${tabsId}-${id}-panel`} aria-selected={tab === id}
        tabIndex={tab === id ? 0 : -1} onClick={() => setTab(id)} onKeyDown={navigateTab}>
        {fileTabs[id]}{id === "transfers" && pending > 0 && <span className="tab-count">{pending}</span>}
      </button>)}
    </div>}
    <div hidden={tab !== "shared"} role={conversation ? undefined : "tabpanel"}
      aria-labelledby={conversation ? undefined : `${tabsId}-shared-tab`} id={`${tabsId}-shared-panel`}>
    {tab === "shared" && <>
      <div className="file-filters">
        {!desktopLibrary && searchField}
        {!conversation && <select aria-label="Shared by" value={person} onChange={event => setPerson(event.target.value)}>
          <option value="">All senders</option>
          {people.map(user => <option key={user.id} value={user.id}>{displayName(user, remarks)}</option>)}
        </select>}
        <select aria-label="Type" value={kind} onChange={event => setKind(event.target.value as typeof kind)}>
          <option value="">All types</option><option value="file">Files</option><option value="folder">Folders</option>
        </select>
          <button className="button subtle file-sort" type="button" onClick={() => setOldestFirst(value => !value)}
            aria-label={`Sort by time, ${oldestFirst ? "oldest" : "newest"} first`}
            title={oldestFirst ? "Show newest first" : "Show oldest first"}>
            {oldestFirst ? <ArrowUp size={16} /> : <ArrowDown size={16} />}{oldestFirst ? "Oldest first" : "Newest first"}
          </button>
      </div>
      <SharedHistory key={JSON.stringify([conversation, person, kind, oldestFirst, name])} {...{
        conversation, person, kind, oldestFirst, name, revision, recalled, hidden, jobs, onChanged, sources,
      }} />
    </>}
    </div>
    {!conversation && <div role="tabpanel" aria-labelledby={`${tabsId}-transfers-tab`} id={`${tabsId}-transfers-panel`}
      hidden={tab !== "transfers"} tabIndex={0}>
      {tab === "transfers" && (jobs.length ? <TransferList transfers={jobs} onChanged={onChanged} />
        : <Blank icon={<Download size={24} />} title="No file transfers" />)}
    </div>}
  </div>;
}

function SharedHistory({ conversation, person, kind, oldestFirst, name, revision, recalled, hidden, jobs, onChanged, sources }: {
  conversation?: string; person: string; kind: Attachment["kind"] | ""; oldestFirst: boolean; name: string;
  revision: number; recalled: ReadonlySet<number>; hidden: ReadonlySet<number>; jobs: Transfer[]; onChanged: () => void;
  sources?: ReadonlyMap<string, string>;
}) {
  const read = useCallback((before?: number) => {
    const filter: FileFilter = { conversation_id: conversation, sender_id: person || undefined, kind: kind || undefined,
      name: name.trim() || undefined, oldest_first: oldestFirst };
    return readAttachmentPage(cursor => api.files(filter, cursor), false, recalled, hidden, before);
  }, [conversation, person, kind, oldestFirst, name, recalled, hidden]);
  const { page, failure, loading, older } = usePage(read, revision, recalled, false, undefined, hidden, oldestFirst);
  const downloads = new Map(jobs.filter(job => job.direction === "download").map(job => [job.attachment.id, job]));
  if (failure) return <p className="inline-error" role="alert">{failure}</p>;
  if (!page) return <p className="muted loading" role="status">Loading shared files…</p>;
  if (page.items.length === 0) return <Blank icon={<FolderOpen size={27} />} title="No shared files found">
    <p>{person || kind || name ? "Try changing the filters." : "Files and folders shared in chats will appear here."}</p>
  </Blank>;
  return <div className="shared-list">
    {page.items.map(message => <SharedFile key={message.seq} message={message} source={sources?.get(message.conversation_id)}
      transfer={downloads.get(message.attachment!.id)} onChanged={onChanged} />)}
    {page.has_more && <button className="button subtle more" disabled={loading} onClick={() => void older()}>Load more files</button>}
  </div>;
}
