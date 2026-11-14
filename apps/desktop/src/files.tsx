import { Archive, ArrowDown, ArrowUp, Check, Download, File, FolderOpen, Pause, Play, RotateCcw, Search, Trash2 } from "lucide-react";
import { useCallback, useContext, useState } from "react";
import { api, errorMessage } from "./api";
import type { Attachment, FileFilter, Message, Transfer, User } from "./api";
import { mediaKind, readAttachmentPage } from "./attachment-media";
import { Blank, Choice, People, PersonAvatar, bytes, dateTimeLabel, displayName, usePage } from "./common";

export function FileIcon({ file }: { file: Attachment }) {
  const Icon = file.kind === "folder" ? FolderOpen : file.name.toLowerCase().endsWith(".zip") ? Archive : File;
  return <span className={"file-icon" + (file.kind === "folder" ? " folder-icon" : "")}><Icon size={21} /></span>;
}

export function SharedFile({ message, transfer, onChanged, inChat = false, previewStatus }: {
  message: Message; transfer?: Transfer; onChanged: () => void; inChat?: boolean; previewStatus?: string;
}) {
  const file = message.attachment!;
  const { remarks, open } = useContext(People);
  const extension = file.name.includes(".") ? file.name.split(".").at(-1)!.toUpperCase() : "";
  const type = file.kind === "folder" ? "Folder ZIP" : mediaKind(file) === "image" ? "Image"
    : mediaKind(file) === "video" ? "Video" : extension.length > 8 || !extension ? "File" : extension;
  return <article className={"shared-file" + (inChat ? " chat-file-card" : "")} aria-label={file.name}>
    <div className="file-main">
      {!inChat && <button className="person-link file-sharer" onClick={() => open?.(message.sender)}
        title={`View ${displayName(message.sender, remarks)}'s profile`}>
        <PersonAvatar user={message.sender} /><span>{displayName(message.sender, remarks)}</span>
      </button>}
      <FileIcon file={file} />
      <div className="file-info">
        <strong title={file.name}>{file.name}</strong>
        <small>{type} · {bytes(file.size)}{!inChat && ` · ${dateTimeLabel(message.created_at)}`}</small>
      </div>
    </div>
    {previewStatus && <small className="file-preview-status" role="status">{previewStatus}</small>}
    <AttachmentDownload file={file} transfer={transfer} onChanged={onChanged} />
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
  return <div className="attachment-download">
    {(!transfer || transfer.status === "complete") && (!hideSave || transfer) && <div className="file-download-actions">
      {transfer && <span className="verified"><Check size={14} /> Download verified</span>}
      {!hideSave && <button className="button subtle small" onClick={() => void save(file.id)} disabled={saving} aria-label={`Save ${file.name}`}>
        <Download size={15} /> {saving ? "Choosing…" : transfer ? "Save again" : "Save"}
      </button>}
    </div>}
    {transfer && transfer.status !== "complete" &&
      <TransferProgress transfer={transfer} onChanged={onChanged} chooseNew={() => void save(file.id)} choosing={saving} />}
    {failure && <p className="inline-error" role="alert">{failure}</p>}
  </div>;
}

function TransferProgress({ transfer, onChanged, chooseNew, choosing }: {
  transfer: Transfer; onChanged: () => void; chooseNew?: () => void; choosing?: boolean;
}) {
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState("");
  const running = transfer.status === "running";
  const failed = transfer.status === "failed";
  const complete = transfer.status === "complete";
  const total = transfer.attachment.size;
  const percentage = total > 0 ? Math.floor(transfer.received * 100 / total) : complete ? 100 : 0;
  const status = complete ? (transfer.direction === "upload" ? "Shared and verified" : "Download verified")
    : failed ? "Failed" : running
      ? percentage === 100 ? "Verifying…" : transfer.direction === "upload" ? "Uploading" : "Downloading"
      : "Paused";

  async function action(name: "pause" | "resume" | "discard") {
    setBusy(true);
    setFailure("");
    try { await api.transferAction(name, transfer); onChanged(); }
    catch (error) { setFailure(errorMessage(error)); }
    finally { setBusy(false); }
  }

  return <div className="transfer-progress">
    <div className="transfer-status"><span className={complete ? "verified" : failed ? "danger" : ""}>
      {complete && <Check size={14} />}{status}
    </span>{!complete && <span>{bytes(transfer.received)} of {bytes(total)} · {percentage}%</span>}</div>
    {!complete && <>
      <progress max={100} value={percentage} aria-label={status + " " + transfer.attachment.name} />
      {(failure || transfer.error) && <p className="inline-error" role="alert">{failure || transfer.error}</p>}
      <div className="transfer-buttons">
        {running
          ? <button className="button subtle small" onClick={() => void action("pause")} disabled={busy}><Pause size={14} /> Pause</button>
          : <>
            <button className="button subtle small" onClick={() => void action("resume")} disabled={busy}>
              {failed ? <RotateCcw size={14} /> : <Play size={14} />}{failed ? "Retry" : "Resume"}
            </button>
            {chooseNew && <button className="button subtle small" disabled={busy || choosing} onClick={chooseNew}>Save as…</button>}
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
      <div className="file-main">{!inChat && transfer.sharer && <span className="file-sharer">
        <PersonAvatar user={transfer.sharer} /><span>{displayName(transfer.sharer, remarks)}</span></span>}<FileIcon file={transfer.attachment} />
        <div className="file-info"><strong title={transfer.attachment.name}>{transfer.attachment.name}</strong>
          <small>{transfer.direction === "upload" && transfer.status !== "complete" ? "Not yet shared"
            : transfer.direction === "upload" ? "Upload" : "Download"} · {bytes(transfer.attachment.size)}</small>
        </div>
      </div>
      <TransferProgress transfer={transfer} onChanged={onChanged} />
    </div>)}</div>;
}

export function FilesView({ conversation, people, transfers, revision, recalled, hidden, onChanged }: {
  conversation?: string; people: User[]; transfers: Transfer[]; revision: number; recalled: ReadonlySet<number>;
  hidden: ReadonlySet<number>; onChanged: () => void;
}) {
  const { remarks } = useContext(People);
  const [tab, setTab] = useState<"shared" | "transfers">("shared");
  const [person, setPerson] = useState("");
  const [kind, setKind] = useState<Attachment["kind"] | "">("");
  const [oldestFirst, setOldestFirst] = useState(false);
  const [name, setName] = useState("");
  const jobs = transfers.filter(transfer => !conversation || transfer.conversation_id === conversation);
  const pending = jobs.filter(transfer => transfer.status !== "complete").length;
  return <div className={"files-view" + (conversation ? " chat-files" : "")}>
    {!conversation && <header className="workspace-heading"><h1>Files</h1></header>}
    {!conversation && <fieldset className="tabs">
      <legend className="sr-only">Files view</legend>
      <Choice type="radio" name="files-view" checked={tab === "shared"} onChange={() => setTab("shared")}>Shared files</Choice>
      <Choice type="radio" name="files-view" checked={tab === "transfers"} onChange={() => setTab("transfers")}>
        Transfers{pending > 0 && <span className="tab-count">{pending}</span>}
      </Choice>
    </fieldset>}
    {tab === "shared" ? <>
      <div className="filters">
        <label className="file-search">File name<span className="search"><Search size={16} />
          <input type="search" value={name} onChange={event => setName(event.target.value)} placeholder="Search file names" />
        </span></label>
        {!conversation && <label>Shared by<select value={person} onChange={event => setPerson(event.target.value)}>
          <option value="">Everyone</option>
          {people.map(user => <option key={user.id} value={user.id}>{displayName(user, remarks)}</option>)}
        </select></label>}
        <label>Type<select value={kind} onChange={event => setKind(event.target.value as typeof kind)}>
          <option value="">All types</option><option value="file">Files</option><option value="folder">Folders</option>
        </select></label>
        <div className="sort-filter"><span>Time</span>
          <button className="button subtle file-sort" type="button" onClick={() => setOldestFirst(value => !value)}
            aria-label={`Sort by time, ${oldestFirst ? "oldest" : "newest"} first`}
            title={oldestFirst ? "Show newest first" : "Show oldest first"}>
            {oldestFirst ? <ArrowUp size={16} /> : <ArrowDown size={16} />}{oldestFirst ? "Oldest first" : "Newest first"}
          </button>
        </div>
      </div>
      <SharedHistory key={JSON.stringify([conversation, person, kind, oldestFirst, name])} {...{
        conversation, person, kind, oldestFirst, name, revision, recalled, hidden, jobs, onChanged,
      }} />
    </> : jobs.length
      ? <TransferList transfers={jobs} onChanged={onChanged} />
      : <Blank icon={<Download size={26} />} title="No transfers"><p>Uploads and downloads will appear here.</p></Blank>}
  </div>;
}

function SharedHistory({ conversation, person, kind, oldestFirst, name, revision, recalled, hidden, jobs, onChanged }: {
  conversation?: string; person: string; kind: Attachment["kind"] | ""; oldestFirst: boolean; name: string;
  revision: number; recalled: ReadonlySet<number>; hidden: ReadonlySet<number>; jobs: Transfer[]; onChanged: () => void;
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
    {page.items.map(message => <SharedFile key={message.seq} message={message}
      transfer={downloads.get(message.attachment!.id)} onChanged={onChanged} />)}
    {page.has_more && <button className="button subtle more" disabled={loading} onClick={() => void older()}>Load more files</button>}
  </div>;
}
