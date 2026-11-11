import { Archive, Check, Download, File, FolderOpen, Pause, Play, RotateCcw, Trash2 } from "lucide-react";
import { useCallback, useState } from "react";
import { api, errorMessage } from "./api";
import type { Attachment, FileFilter, Message, Transfer, User } from "./api";
import { Blank, bytes, dateLabel, usePage } from "./common";

export function FileIcon({ file }: { file: Attachment }) {
  const Icon = file.kind === "folder" ? FolderOpen : file.name.toLowerCase().endsWith(".zip") ? Archive : File;
  return <span className={"file-icon" + (file.kind === "folder" ? " folder-icon" : "")}><Icon size={21} /></span>;
}

export function SharedFile({ message, transfer, onChanged, compact = false }: {
  message: Message; transfer?: Transfer; onChanged: () => void; compact?: boolean;
}) {
  const file = message.attachment!;
  const [saving, setSaving] = useState(false);
  const [failure, setFailure] = useState("");

  async function save() {
    setSaving(true);
    setFailure("");
    try { await api.save(file.id); onChanged(); }
    catch (error) { setFailure(errorMessage(error)); }
    finally { setSaving(false); }
  }

  return <div className={"shared-file" + (compact ? " compact-file" : "")}>
    <div className="file-main">
      <FileIcon file={file} />
      <div className="file-info">
        <strong title={file.name}>{file.name}</strong>
        <small>
          {file.kind === "folder" ? "Folder ZIP · " : ""}{bytes(file.size)}
          {!compact && <> · @{message.sender.handle} · {dateLabel(message.created_at)}</>}
        </small>
      </div>
      {(!transfer || transfer.status === "complete") &&
        <button className="button subtle small" onClick={save} disabled={saving}>
          <Download size={15} /> {saving ? "Choosing…" : transfer ? "Save again" : "Save"}
        </button>}
    </div>
    {transfer?.status === "complete" && <div className="verified"><Check size={14} /> Download verified</div>}
    {transfer && transfer.status !== "complete" &&
      <TransferProgress transfer={transfer} onChanged={onChanged} chooseNew={save} choosing={saving} />}
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

export function TransferList({ transfers, onChanged }: { transfers: Transfer[]; onChanged: () => void }) {
  return <div className="transfer-list">{transfers.map(transfer =>
    <div className="shared-file" key={transfer.direction + transfer.attachment.id}>
      <div className="file-main"><FileIcon file={transfer.attachment} />
        <div className="file-info"><strong title={transfer.attachment.name}>{transfer.attachment.name}</strong>
          <small>{transfer.direction === "upload" ? "Upload" : "Download"} · {bytes(transfer.attachment.size)}</small>
        </div>
      </div>
      <TransferProgress transfer={transfer} onChanged={onChanged} />
    </div>)}</div>;
}

export function FilesView({ conversation, people, transfers, revision, onChanged }: {
  conversation?: string; people: User[]; transfers: Transfer[]; revision: number; onChanged: () => void;
}) {
  const [tab, setTab] = useState<"shared" | "transfers">("shared");
  const [person, setPerson] = useState("");
  const [kind, setKind] = useState<Attachment["kind"] | "">("");
  const [start, setStart] = useState("");
  const [end, setEnd] = useState("");
  const jobs = transfers.filter(transfer => !conversation || transfer.conversation_id === conversation);
  const pending = jobs.filter(transfer => transfer.status !== "complete").length;
  return <div className={"files-view" + (conversation ? " chat-files" : "")}>
    {!conversation && <header className="workspace-heading"><h1>Files</h1><p>Find attachments shared across your chats.</p></header>}
    <div className="tabs" aria-label="Files view">
      <button className={tab === "shared" ? "active-tab" : ""} aria-pressed={tab === "shared"} onClick={() => setTab("shared")}>Shared files</button>
      <button className={tab === "transfers" ? "active-tab" : ""} aria-pressed={tab === "transfers"} onClick={() => setTab("transfers")}>
        Transfers{pending > 0 && <span className="tab-count">{pending}</span>}
      </button>
    </div>
    {tab === "shared" ? <>
      <div className="filters">
        <label>Shared by<select value={person} onChange={event => setPerson(event.target.value)}>
          <option value="">Everyone</option>
          {people.map(user => <option key={user.id} value={user.id}>@{user.handle}</option>)}
        </select></label>
        <label>Type<select value={kind} onChange={event => setKind(event.target.value as typeof kind)}>
          <option value="">All types</option><option value="file">Files</option><option value="folder">Folders</option>
        </select></label>
        <label>From<input type="date" value={start} max={end || undefined} onChange={event => setStart(event.target.value)} /></label>
        <label>Through<input type="date" value={end} min={start || undefined} onChange={event => setEnd(event.target.value)} /></label>
      </div>
      <SharedHistory key={[conversation, person, kind, start, end].join("|")} {...{
        conversation, person, kind, start, end, revision, jobs, onChanged,
      }} />
    </> : jobs.length
      ? <TransferList transfers={jobs} onChanged={onChanged} />
      : <Blank icon={<Download size={26} />} title="No transfers"><p>Uploads and downloads will appear here.</p></Blank>}
  </div>;
}

function SharedHistory({ conversation, person, kind, start, end, revision, jobs, onChanged }: {
  conversation?: string; person: string; kind: Attachment["kind"] | ""; start: string; end: string;
  revision: number; jobs: Transfer[]; onChanged: () => void;
}) {
  const read = useCallback((before?: number) => {
    const filter: FileFilter = { conversation_id: conversation, sender_id: person || undefined, kind: kind || undefined };
    if (start) filter.start_at = new Date(start + "T00:00:00").getTime();
    if (end) {
      const next = new Date(end + "T00:00:00");
      next.setDate(next.getDate() + 1);
      filter.end_before = next.getTime();
    }
    return api.files(filter, before);
  }, [conversation, person, kind, start, end]);
  const { page, failure, loading, older } = usePage(read, revision);
  const downloads = new Map(jobs.filter(job => job.direction === "download").map(job => [job.attachment.id, job]));
  if (failure) return <p className="inline-error" role="alert">{failure}</p>;
  if (!page) return <p className="muted loading" role="status">Loading shared files…</p>;
  if (page.items.length === 0) return <Blank icon={<FolderOpen size={27} />} title="No shared files found">
    <p>{person || kind || start || end ? "Try changing the filters." : "Files and folders shared in chats will appear here."}</p>
  </Blank>;
  return <div className="shared-list">
    {page.items.map(message => <SharedFile key={message.seq} message={message}
      transfer={downloads.get(message.attachment!.id)} onChanged={onChanged} />)}
    {page.has_more && <button className="button subtle more" disabled={loading} onClick={() => void older()}>Load older files</button>}
  </div>;
}
