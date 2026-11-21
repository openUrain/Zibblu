import { FloatingField } from "./floating-field";
import { ChevronLeft, ChevronRight, Image, LoaderCircle, Play, RotateCcw, Search, Video } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { api, errorMessage, fileExpired } from "./api";
import type { Attachment, Message, Transfer } from "./api";
import { canPreview, mediaKind, readAttachmentPage } from "./attachment-media";
import { Blank, Dialog, usePage } from "./common";
import { AttachmentDownload, SharedFile, TransferList } from "./files";

type ChatMediaScope = {
  conversation: string; revision: number; recalled: ReadonlySet<number>; hidden: ReadonlySet<number>;
};
type PreviewState = "loading" | "ready" | "failed";

function MediaTile({ file, open }: { file: Attachment; open: () => void }) {
  const kind = mediaKind(file);
  const [state, setState] = useState<PreviewState>(canPreview(file) ? "loading" : "failed");
  const [visible, setVisible] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const element = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    const target = element.current;
    if (!target || !canPreview(file)) return;
    const observer = new IntersectionObserver(entries => {
      if (entries.some(entry => entry.isIntersecting)) {
        setVisible(true);
        observer.disconnect();
      }
    });
    observer.observe(target);
    return () => observer.disconnect();
  }, [file]);

  return <button ref={element} type="button" className="media-tile" data-ready={state === "ready" ? "" : undefined}
    onClick={() => {
      if (state === "failed" && canPreview(file)) {
        setState("loading");
        setAttempt(value => value + 1);
      }
      open();
    }}
    aria-label={`Open ${kind === "video" ? "video" : "image"} ${file.name}`}>
    {visible && state !== "failed" && (kind === "image"
      ? <img src={api.mediaUrl(file.id, "thumbnail") + (attempt ? "&retry=" + attempt : "")} alt="" onLoad={() => setState("ready")} onError={() => setState("failed")} />
      : <video src={api.mediaUrl(file.id)} muted playsInline preload="metadata"
        onLoadedMetadata={() => setState("ready")} onError={() => setState("failed")} aria-hidden="true" />)}
    {state === "failed" ? <span className="media-tile-fallback">
      {kind === "video" ? <Video size={22} /> : <Image size={22} />}
      <span>{file.name}</span><small>{canPreview(file) ? "Preview unavailable" : "Download to open"}</small>
    </span> : <>
      {state === "loading" && <span className="media-tile-status" role="status" aria-label="Loading preview"><LoaderCircle className="preview-spinner" size={16} aria-hidden /></span>}
      {kind === "video" && state === "ready" && <span className="media-play" aria-hidden="true"><Play size={20} fill="currentColor" /></span>}
    </>}
  </button>;
}

export function ChatAttachment({ message, transfer, onChanged, open, expired }: {
  message: Message; transfer?: Transfer; onChanged: () => void; open: () => void; expired: boolean;
}) {
  const file = message.attachment!;
  if (expired || !mediaKind(file) || !canPreview(file)) {
    const previewStatus = mediaKind(file) ? "Download to open" : undefined;
    return <SharedFile {...{ message, transfer, onChanged, previewStatus, expired }} />;
  }
  return <div className="chat-media">
    <MediaTile key={file.id} file={file} open={open} />
    {transfer && <AttachmentDownload file={file} transfer={transfer} onChanged={onChanged} hideSave />}
  </div>;
}

export function MediaHistory({ conversation, revision, recalled, hidden, open, transfers, onChanged }: ChatMediaScope & {
  open: (message: Message) => void; transfers: Transfer[]; onChanged: () => void;
}) {
  const [name, setName] = useState("");
  const downloads = transfers.filter(job => job.conversation_id === conversation && job.direction === "download"
    && job.status !== "complete" && mediaKind(job.attachment));
  return <>
    <FloatingField fieldClass="search media-search" label="Search media by file name" prefixIcon={<Search size={16} />} type="search"
        value={name} onChange={event => setName(event.target.value)} />
    {downloads.length > 0 && <details className="media-transfers">
      <summary>Transfers ({downloads.length})</summary>
      <TransferList transfers={downloads} onChanged={onChanged} inChat />
    </details>}
    <MediaResults key={name} {...{ conversation, revision, recalled, hidden, name, open }} />
  </>;
}

function MediaResults({ conversation, revision, recalled, hidden, name, open }: ChatMediaScope & {
  name: string; open: (message: Message) => void;
}) {
  const read = useCallback((before?: number) => readAttachmentPage(
    cursor => api.files({ conversation_id: conversation, name: name.trim() || undefined, oldest_first: false }, cursor),
    true, recalled, hidden, before), [conversation, name, recalled, hidden]);
  const { page, failure, loading, older } = usePage(read, revision, recalled, false, undefined, hidden);
  return <div className="chat-search-results">
    {failure && <p className="inline-error" role="alert">{failure}</p>}
    {!page && !failure && <p className="muted" role="status">Loading photos and videos…</p>}
    {page?.items.length === 0 && <Blank icon={<Image size={25} />} title="No matching photos or videos" />}
    {page && page.items.length > 0 && <div className="media-grid">{page.items.map(message =>
      fileExpired(message)
        ? <span className="media-tile-fallback" key={message.seq}>{message.attachment!.name}<small>File expired</small></span>
        : <MediaTile key={message.seq} file={message.attachment!} open={() => open(message)} />)}</div>}
    {page?.has_more && <button className="button subtle more" disabled={loading} onClick={() => void older()}>
      {loading ? "Loading…" : "Load older media"}
    </button>}
  </div>;
}

export function MediaViewer({ message, conversation, revision, recalled, hidden, transfers, open, close, onChanged }: ChatMediaScope & {
  message: Message; transfers: Transfer[]; open: (message: Message) => void; close: () => void; onChanged: () => void;
}) {
  const [neighbors, setNeighbors] = useState<{ seq: number; previous?: Message; next?: Message; error?: string }>();
  const [retry, setRetry] = useState(0);
  const file = message.attachment!;
  const unavailable = message.recalled || recalled.has(message.seq) || hidden.has(message.seq);
  const current = neighbors?.seq === message.seq ? neighbors : undefined;

  useEffect(() => {
    if (unavailable) return;
    let active = true;
    const find = async (oldest_first: boolean) => (await readAttachmentPage(
      cursor => api.files({ conversation_id: conversation, oldest_first }, cursor),
      true, recalled, hidden, message.seq, 1)).items[0];
    void Promise.all([find(false), find(true)]).then(([previous, next]) => {
      if (active) setNeighbors({ seq: message.seq, previous, next });
    }).catch(error => { if (active) setNeighbors({ seq: message.seq, error: errorMessage(error) }); });
    return () => { active = false; };
  }, [conversation, message.seq, revision, recalled, hidden, unavailable, retry]);

  function stillVisible(item?: Message) {
    return item && !recalled.has(item.seq) && !hidden.has(item.seq) ? item : undefined;
  }
  const previous = stillVisible(current?.previous);
  const next = stillVisible(current?.next);
  const transfer = transfers.find(job => job.direction === "download" && job.attachment.id === file.id);
  const expired = transfer?.status === "expired" || (transfer?.status !== "complete" && fileExpired(message));

  return <Dialog title="Media viewer" className="media-viewer" close={close} onKeyDown={event => {
    if (event.defaultPrevented || (event.target instanceof Element && event.target.closest("video, input, .media-viewer-stage"))) return;
    const item = event.key === "ArrowLeft" ? previous : event.key === "ArrowRight" ? next : undefined;
    if (item && !unavailable) { event.preventDefault(); open(item); }
  }}>
    {unavailable ? <p className="inline-error" role="alert">This attachment is no longer available.</p> :
      expired ? <p className="inline-error" role="alert">This file expired. Ask the sender to share it again.</p> : <>
      <ViewerMedia key={"preview:" + file.id} file={file} />
      <div className="media-viewer-footer">
        <strong title={file.name}>{file.name}</strong>
        <div className="media-nav">
          <button className="icon-button" type="button" title="Previous media" aria-label="Previous media"
            disabled={!previous} onClick={() => { if (previous) open(previous); }}><ChevronLeft size={20} /></button>
          <button className="icon-button" type="button" title="Next media" aria-label="Next media"
            disabled={!next} onClick={() => { if (next) open(next); }}><ChevronRight size={20} /></button>
        </div>
      </div>
      {current?.error && <div className="media-viewer-error"><p className="inline-error" role="alert">{current.error}</p>
        <button className="button subtle small" type="button" onClick={() => { setNeighbors(undefined); setRetry(value => value + 1); }}>
          <RotateCcw size={15} /> Retry media history
        </button>
      </div>}
      <AttachmentDownload key={"download:" + file.id} {...{ file, transfer, onChanged }} />
    </>}
  </Dialog>;
}

function ViewerMedia({ file }: { file: Attachment }) {
  const supported = canPreview(file);
  const video = mediaKind(file) === "video";
  const [state, setState] = useState<PreviewState>(supported ? "loading" : "failed");
  const [attempt, setAttempt] = useState(0);
  const [zoom, setZoom] = useState(100);
  const [thumbnailReady, setThumbnailReady] = useState(false);

  return <>
    <div className="media-viewer-stage" data-ready={state === "ready" ? "" : undefined}
      tabIndex={zoom > 100 ? 0 : undefined} aria-label={zoom > 100 ? "Pan zoomed image" : undefined}>
      {!video && supported && state !== "failed" && <img className="media-viewer-thumbnail" alt="" aria-hidden="true"
        src={api.mediaUrl(file.id, "cached-thumbnail")} onLoad={() => setThumbnailReady(true)} />}
      {state !== "failed" && (video
        ? <video key={attempt} src={api.mediaUrl(file.id)} controls controlsList="nodownload noremoteplayback"
          playsInline preload="auto" disablePictureInPicture aria-label={file.name}
          onLoadedData={() => setState("ready")} onError={() => setState("failed")} />
        : <div className="media-viewer-image" style={{ width: zoom + "%", height: zoom + "%" }}>
          <img key={attempt} src={api.mediaUrl(file.id)} alt={file.name}
            onLoad={() => setState("ready")} onError={() => { setZoom(100); setState("failed"); }} />
        </div>)}
      {state === "loading" && !thumbnailReady && <p className="media-viewer-placeholder" role="status" aria-label="Loading preview"><LoaderCircle className="preview-spinner" size={16} aria-hidden /></p>}
      {state === "failed" && <div className="media-viewer-placeholder">
        <p role="status">{supported ? "Preview unavailable. You can try again or save the attachment." : "Download this attachment to open it."}</p>
        {supported && <button className="button subtle small" type="button" onClick={() => {
          setThumbnailReady(false); setState("loading"); setAttempt(value => value + 1);
        }}><RotateCcw size={15} /> Retry preview</button>}
      </div>}
    </div>
    {!video && state === "ready" && <label className="media-zoom">Zoom
      <input type="range" min={100} max={300} step={25} value={zoom} onChange={event => setZoom(Number(event.target.value))} />
      <span>{zoom}%</span>
    </label>}
  </>;
}
