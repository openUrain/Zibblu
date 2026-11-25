import { ChevronLeft, ChevronRight, Image, LoaderCircle, Play, RotateCcw, Video } from "lucide-react";
import { useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import { api, errorMessage, fileExpired } from "./api";
import type { Attachment, ImageSize, Message, Transfer } from "./api";
import { canPreview, mediaKind, readAttachmentPage } from "./attachment-media";
import { Blank, Dialog, usePage } from "./common";
import { AttachmentDownload, SharedFile, TransferList, UploadOverlay } from "./files";
import { thumbnailKey, useWorkspaceImage, videoCoverKey, WorkspaceImages } from "./workspace-image";

type ChatMediaScope = {
  conversation: string; revision: number; recalled: ReadonlySet<number>; hidden: ReadonlySet<number>;
};
type PreviewState = "loading" | "ready" | "failed";

function thumbnailStyle(size?: ImageSize) {
  if (!size || !size[0] || !size[1]) return;
  const [width, height] = size;
  return { width: Math.min(1, width / height) * 320, aspectRatio: `${width} / ${height}` };
}

function MediaTile({ message, open, fitImage, size: hint }: {
  message: Message; open: () => void; fitImage?: boolean; size?: ImageSize;
}) {
  const file = message.attachment!;
  const kind = mediaKind(file);
  const resources = useContext(WorkspaceImages);
  const [state, setState] = useState<PreviewState>(canPreview(file) ? "loading" : "failed");
  const [visible, setVisible] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const [onlineSize, setOnlineSize] = useState<ImageSize>();
  const [cachedLocally, setCachedLocally] = useState(Boolean(message.local_file));
  const [initialClockOffset] = useState(() => resources?.clockOffset ?? 0);
  const access = resources?.previewAccess(message.conversation_id, message.seq);
  const allowed = !resources || !!access;
  const cachedMedia = (kind === "image" || kind === "video") && access === "cached" && canPreview(file);
  const key = cachedMedia && allowed
    ? (kind === "video" ? videoCoverKey(message, file) : thumbnailKey(message, file))
    : undefined;
  const expires = !message.local_file && !cachedLocally && message.expires_at != null
    ? message.expires_at - initialClockOffset : Infinity;
  const readCover = useCallback(() => api.previewVideoCoverBytes(file.id), [file.id]);
  const { url, size, failed } = useWorkspaceImage(
    key,
    file.id,
    undefined,
    expires,
    attempt,
    visible,
    kind === "video" ? readCover : undefined,
  );
  useEffect(() => {
    if (message.local_file || url) setCachedLocally(true);
  }, [message.local_file, url]);
  // Retry briefly if a freshly sent video's cover is still being transcoded on the server.
  useEffect(() => {
    if (kind !== "video" || !cachedMedia || !failed || attempt >= 4 || !visible) return;
    const timer = window.setTimeout(() => setAttempt(value => value + 1), 1500 * (attempt + 1));
    return () => window.clearTimeout(timer);
  }, [kind, cachedMedia, failed, attempt, visible]);
  const status = !allowed ? "failed" : cachedMedia ? (url ? "ready" : failed ? "failed" : "loading") : state;
  // WebView can reuse a decoded image at the same URL even when the response says no-store.
  const onlineQuery = useMemo(() => access === "online" ? `view=${crypto.randomUUID()}` : "", [access]);
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
  }, [file.id]);

  return <button ref={element} type="button" className="media-tile" data-ready={status === "ready" ? "" : undefined}
    data-cached-image={cachedMedia ? "" : undefined}
    data-fit-image={fitImage ? "" : undefined} style={fitImage ? thumbnailStyle(hint ?? size ?? onlineSize) : undefined}
    onClick={() => {
      if (status === "failed" && canPreview(file)) {
        setState("loading");
        setAttempt(value => value + 1);
      }
      open();
    }}
    aria-label={`Open ${kind === "video" ? "video" : "image"} ${file.name}`}>
    {cachedMedia ? url && <img key="cached" src={url} alt="" /> : visible && status !== "failed" && (kind === "image"
      ? <img key="online" src={api.mediaUrl(file.id, "thumbnail") + (onlineQuery ? "&" + onlineQuery : "")
        + (attempt ? "&retry=" + attempt : "")} alt="" onLoad={event => {
        if (fitImage) setOnlineSize([event.currentTarget.naturalWidth, event.currentTarget.naturalHeight]);
        setState("ready");
      }} onError={() => setState("failed")} />
      : <img key="video-cover-online" src={api.mediaUrl(file.id, "video-cover") + (onlineQuery ? "&" + onlineQuery : "")
        + (attempt ? "&retry=" + attempt : "")} alt="" onLoad={() => setState("ready")} onError={() => setState("failed")} />)}
    {status === "failed" ? <span className="media-tile-fallback">
      {kind === "video" ? <Video size={22} /> : <Image size={22} />}
      <span>{file.name}</span><small>{canPreview(file) ? (kind === "video" ? "Tap to play" : "Preview unavailable") : "Download to open"}</small>
    </span> : <>
      {status === "loading" && <span className="media-tile-status" role="status" aria-label="Loading preview"><LoaderCircle className="preview-spinner" size={16} aria-hidden /></span>}
      {kind === "video" && status === "ready" && <span className="media-play" aria-hidden="true"><Play size={20} fill="currentColor" /></span>}
    </>}
  </button>;
}

export function ChatAttachment({ message, transfer, onChanged, open, expired, upload }: {
  message: Message; transfer?: Transfer; onChanged: () => void; open: () => void; expired: boolean; upload?: Transfer;
}) {
  const file = message.attachment!;
  const fitImage = useContext(WorkspaceImages) !== undefined && mediaKind(file) === "image";
  const [uploadSize, setUploadSize] = useState<ImageSize>();
  useEffect(() => {
    if (!upload || !fitImage || !canPreview(file)) return;
    let current = true;
    void api.uploadImageSize(file.id).then(size => { if (current) setUploadSize(size); }).catch(() => {});
    return () => { current = false; };
  }, [!!upload, fitImage, file.id]);
  if (expired || !mediaKind(file) || !canPreview(file)) {
    const previewStatus = mediaKind(file) ? "Download to open" : undefined;
    return <SharedFile {...{ message, transfer, onChanged, previewStatus, expired, upload }} />;
  }
  const size = uploadSize ?? message.preview_size;
  return <div className="chat-media">
    {upload ? <div className="media-tile" data-fit-image={fitImage ? "" : undefined}
      style={fitImage ? thumbnailStyle(size) : undefined}><span className="media-tile-fallback">
      {mediaKind(file) === "video" ? <Video size={22} /> : <Image size={22} />}
      <span>{file.name}</span></span><UploadOverlay upload={upload} /></div>
      : <MediaTile key={file.id} message={message} open={open} fitImage={fitImage} size={size} />}
    {transfer && <AttachmentDownload file={file} transfer={transfer} onChanged={onChanged} hideSave />}
  </div>;
}

export function MediaHistory({ conversation, revision, recalled, hidden, open, transfers, onChanged }: ChatMediaScope & {
  open: (message: Message) => void; transfers: Transfer[]; onChanged: () => void;
}) {
  const downloads = transfers.filter(job => job.conversation_id === conversation && job.direction === "download"
    && job.status !== "complete" && mediaKind(job.attachment));
  return <>
    {downloads.length > 0 && <details className="media-transfers">
      <summary>Transfers ({downloads.length})</summary>
      <TransferList transfers={downloads} onChanged={onChanged} inChat />
    </details>}
    <MediaResults {...{ conversation, revision, recalled, hidden, open }} />
  </>;
}

function MediaResults({ conversation, revision, recalled, hidden, open }: ChatMediaScope & {
  open: (message: Message) => void;
}) {
  const read = useCallback((before?: number) => readAttachmentPage(
    cursor => api.files({ conversation_id: conversation, oldest_first: false }, cursor),
    true, recalled, hidden, before), [conversation, recalled, hidden]);
  const { page, failure, loading, older } = usePage(read, revision, recalled, false, undefined, hidden);
  return <div className="chat-search-results">
    {failure && <p className="inline-error" role="alert">{failure}</p>}
    {!page && !failure && <p className="muted" role="status">Loading photos and videos…</p>}
    {page?.items.length === 0 && <Blank icon={<Image size={25} />} title="No photos or videos" />}
    {page && page.items.length > 0 && <div className="media-grid">{page.items.map(message =>
      fileExpired(message)
        ? <span className="media-tile-fallback" key={message.seq}>{message.attachment!.name}<small>File expired</small></span>
        : <MediaTile key={message.seq} message={message} open={() => open(message)} />)}</div>}
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
    if (unavailable) close();
  }, [unavailable, close]);

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
