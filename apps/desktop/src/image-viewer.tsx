import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  Copy,
  Download,
  LoaderCircle,
  Maximize,
  Minimize,
  Pen,
  RotateCw,
  Shrink,
  X,
} from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import type { CSSProperties, KeyboardEvent, PointerEvent, ReactNode } from "react";
import { api, errorMessage } from "./api";
import type { ForwardTarget, ViewerSelection } from "./api";
import { Dialog } from "./common";
import { decimalSize } from "./file-labels";
import { createEmptyAnnotationDoc, exportAnnotatedImage } from "./image-annotator";
import { ImageAnnotationOverlay, ImageAnnotator } from "./image-annotator-view";
import { t } from "./locale";
import { MediaForwardPicker } from "./media-forward-picker";
import { MediaSplitButton } from "./media-split-button";
import type { ResolvedForwardAttempt } from "./media-forward-picker";
import type {
  ImageAnnotationDoc,
  VideoExportProgress,
  VideoProbeInfo,
  VideoTrimRange,
  ViewTransform,
} from "./media-viewer-types";
import { formatTrimTime } from "./video-trimmer";
import { VideoExportBar, VideoTrimmer } from "./video-trimmer-view";
import { WindowControls, WindowDrag, windows } from "./window-chrome";
import "./image-viewer.css";

type Point = { x: number; y: number };
type View = Point & { scale?: number; turn: number };
type Size = { width: number; height: number };
type CloseGuard = {
  needsConfirm: () => boolean;
  promptClose: () => void;
};
type PendingVideoSend = {
  batchId: string;
  targets: ForwardTarget[];
  fileName: string;
};

const centered: View = { x: 0, y: 0, turn: 0 };
const empty: Size = { width: 0, height: 0 };
const appWindow = getCurrentWindow();

function editedFileName(originalName: string, kind: "image" | "video", ext: string): string {
  const dot = originalName.lastIndexOf(".");
  const stem = (dot > 0 ? originalName.slice(0, dot) : originalName).trim() || (kind === "image" ? "image" : "video");
  const suffix = kind === "image" ? "_annotated" : "_trimmed";
  return `${stem}${suffix}.${ext}`;
}

export function ImageViewer() {
  const [selection, setSelection] = useState<ViewerSelection>();
  const [failure, setFailure] = useState("");
  const [fullscreen, setFullscreen] = useState(false);
  const generation = useRef(0);
  const closeGuardRef = useRef<CloseGuard | undefined>(undefined);
  const bypassCloseGuardRef = useRef(false);

  const registerCloseGuard = useCallback((guard: CloseGuard | undefined) => {
    closeGuardRef.current = guard;
  }, []);

  const forceCloseWindow = useCallback(() => {
    bypassCloseGuardRef.current = true;
    void appWindow.destroy().catch(error => {
      bypassCloseGuardRef.current = false;
      setFailure(errorMessage(error));
    });
  }, []);

  const requestClose = useCallback(() => {
    const guard = closeGuardRef.current;
    if (guard && guard.needsConfirm()) {
      guard.promptClose();
      return;
    }
    forceCloseWindow();
  }, [forceCloseWindow]);

  const refresh = useCallback(async () => {
    const request = ++generation.current;
    try {
      const selected = await invoke<ViewerSelection>("image_viewer_selection");
      if (generation.current === request) {
        setSelection(selected);
        setFailure("");
      }
    } catch (error) {
      if (generation.current === request) {
        setSelection(undefined);
        setFailure(errorMessage(error));
        forceCloseWindow();
      }
    }
  }, [forceCloseWindow]);

  useEffect(() => {
    void refresh();
    const timer = globalThis.setInterval(refresh, 750);
    return () => {
      globalThis.clearInterval(timer);
      generation.current++;
    };
  }, [refresh]);

  useEffect(() => {
    const unlisten = appWindow.onCloseRequested(event => {
      if (bypassCloseGuardRef.current) return;
      const guard = closeGuardRef.current;
      if (guard && guard.needsConfirm()) {
        event.preventDefault();
        guard.promptClose();
      }
    });
    return () => {
      void unlisten.then(stop => stop());
    };
  }, []);

  const changeFullscreen = useCallback(async (value: boolean) => {
    await appWindow.setFullscreen(value);
    setFullscreen(value);
  }, []);
  useEffect(() => {
    function escape(event: globalThis.KeyboardEvent) {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      event.preventDefault();
      void appWindow.isFullscreen().then(active => {
        if (active) return changeFullscreen(false);
        requestClose();
        return undefined;
      }).catch(error => {
        setSelection(undefined);
        setFailure(errorMessage(error));
      });
    }
    document.addEventListener("keydown", escape);
    return () => document.removeEventListener("keydown", escape);
  }, [changeFullscreen, requestClose]);

  return <main className="image-viewer" aria-label="Media viewer">
    {selection ? (selection.media_kind === "video"
      ? <VideoCanvas key={selection.file.id} {...selection} {...{ refresh, fullscreen, changeFullscreen, registerCloseGuard, forceCloseWindow }} />
      : <ImageCanvas key={selection.file.id} {...selection} {...{ refresh, fullscreen, changeFullscreen, registerCloseGuard, forceCloseWindow }} />)
      : <><ViewerToolbar fullscreen={fullscreen} /><div className="image-viewer-note" role={failure ? "alert" : "status"}>
        {failure || <LoaderCircle className="preview-spinner" aria-label="Loading media" />}
      </div></>}
    {selection && failure && <div className="image-action-status" role="alert">{failure}</div>}
  </main>;
}

function ViewerToolbar({ fullscreen, children }: { fullscreen: boolean; children?: ReactNode }) {
  return <header className="image-toolbar">
    {fullscreen ? <div /> : <WindowDrag />}
    <div className="image-tools" role="group" aria-label="Media controls">{children}</div>
    <div className="image-window-end">
      {!fullscreen && <><WindowDrag /><WindowControls /></>}
    </div>
  </header>;
}

function ViewerSplitSendButton({
  disabled,
  busy,
  onSendCurrent,
  onOpenForward,
}: {
  disabled: boolean;
  busy: boolean;
  onSendCurrent: () => void;
  onOpenForward: () => void;
}) {
  return <MediaSplitButton className="viewer-split-send"
    label={t("Send", "发送")} menuLabel={t("Send options", "发送选项")}
    disabled={disabled || busy} run={onSendCurrent}
    choices={[{ label: t("Forward", "转发"), run: onOpenForward }]}>
    {busy && <LoaderCircle className="preview-spinner" size={14} aria-hidden />}
    {t("Send", "发送")}
  </MediaSplitButton>;
}

type CanvasSharedProps = ViewerSelection & {
  refresh: () => Promise<void>;
  fullscreen: boolean;
  changeFullscreen: (value: boolean) => Promise<void>;
  registerCloseGuard: (guard: CloseGuard | undefined) => void;
  forceCloseWindow: () => void;
};

function ImageCanvas({
  conversation_id,
  file,
  previous,
  next,
  download,
  draft,
  refresh,
  fullscreen,
  changeFullscreen,
  registerCloseGuard,
  forceCloseWindow,
}: CanvasSharedProps) {
  const stage = useRef<HTMLDivElement>(null);
  const imageRef = useRef<HTMLImageElement>(null);
  const drag = useRef<(Point & { id: number }) | undefined>(undefined);
  const annotatorSnapshotRef = useRef<ImageAnnotationDoc | null>(null);
  const cachedExportRef = useRef<{ bytes: Uint8Array; fileName: string } | null>(null);
  const pendingCurrentSendRef = useRef<{ key: string; batchId: string; target: ForwardTarget } | null>(null);
  const [viewport, setViewport] = useState<Size>(empty);
  const [image, setImage] = useState<Size>(empty);
  const [view, setView] = useState<View>(centered);
  const [attempt, setAttempt] = useState(0);
  const [loaded, setLoaded] = useState(false);
  const [failed, setFailed] = useState(false);
  const [notice, setNotice] = useState("");
  const [actionError, setActionError] = useState("");
  const [requestedSave, setRequestedSave] = useState(false);
  const [busy, setBusy] = useState(false);
  const [annotating, setAnnotating] = useState(false);
  const [pendingImageEdit, setPendingImageEdit] = useState(false);
  const [annotationDoc, setAnnotationDoc] = useState<ImageAnnotationDoc | null>(null);
  const [pendingDiscardAction, setPendingDiscardAction] = useState<(() => void) | null>(null);
  const [forwardOpen, setForwardOpen] = useState(false);
  const [sendingMedia, setSendingMedia] = useState(false);

  const hasUnsentEdits = Boolean(annotationDoc && annotationDoc.items.length > 0);
  const dirtyInAnnotator = annotating && JSON.stringify(annotationDoc) !== JSON.stringify(annotatorSnapshotRef.current);

  useEffect(() => {
    cachedExportRef.current = null;
    pendingCurrentSendRef.current = null;
  }, [annotationDoc]);

  useEffect(() => {
    registerCloseGuard({
      needsConfirm: () => pendingImageEdit || hasUnsentEdits,
      promptClose: () => {
        setPendingDiscardAction(() => () => {
          setAnnotationDoc(null);
          setAnnotating(false);
          setForwardOpen(false);
          forceCloseWindow();
        });
      },
    });
    return () => registerCloseGuard(undefined);
  }, [pendingImageEdit, forceCloseWindow, hasUnsentEdits, registerCloseGuard]);

  useEffect(() => {
    const element = stage.current!;
    element.focus({ preventScroll: true });
    const observer = new ResizeObserver(() => {
      setViewport({ width: element.clientWidth, height: element.clientHeight });
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  const dimensions = useCallback((turn: number) => turn % 2
    ? { width: image.height, height: image.width } : image, [image]);
  const fit = useCallback((turn: number) => {
    const { width, height } = dimensions(turn);
    return width && height && viewport.width && viewport.height
      ? Math.min(viewport.width / width, viewport.height / height) : 1;
  }, [dimensions, viewport]);
  const clamp = useCallback((current: View) => {
    const size = dimensions(current.turn);
    const scale = current.scale ?? fit(current.turn);
    const x = Math.max(0, (size.width * scale - viewport.width) / 2);
    const y = Math.max(0, (size.height * scale - viewport.height) / 2);
    return { ...current, x: Math.max(-x, Math.min(x, current.x)), y: Math.max(-y, Math.min(y, current.y)) };
  }, [dimensions, fit, viewport]);
  const zoom = useCallback((point: Point, factor: number) => {
    setView(current => {
      const fitted = fit(current.turn);
      const previous = current.scale ?? fitted;
      const scale = Math.max(Math.min(fitted, 0.25), Math.min(Math.max(8, fitted), previous * factor));
      const offset = clamp(current);
      const ratio = scale / previous;
      return clamp({ ...current, scale, x: point.x - (point.x - offset.x) * ratio, y: point.y - (point.y - offset.y) * ratio });
    });
  }, [clamp, fit]);

  useEffect(() => {
    const element = stage.current!;
    function wheel(event: WheelEvent) {
      event.preventDefault();
      if (!loaded) return;
      const rect = element.getBoundingClientRect();
      const step = event.deltaMode === WheelEvent.DOM_DELTA_LINE ? 16
        : event.deltaMode === WheelEvent.DOM_DELTA_PAGE ? rect.height : 1;
      zoom({ x: event.clientX - rect.left - rect.width / 2, y: event.clientY - rect.top - rect.height / 2 },
        Math.exp(-event.deltaY * step / 500));
    }
    element.addEventListener("wheel", wheel, { passive: false });
    return () => element.removeEventListener("wheel", wheel);
  }, [loaded, zoom]);

  const setScale = (scale?: number) => setView(current => ({ ...current, x: 0, y: 0, scale }));
  const transform = clamp(view);
  const scale = view.scale ?? fit(view.turn);
  const size = dimensions(view.turn);
  const pannable = loaded && (size.width * scale > viewport.width || size.height * scale > viewport.height);
  const viewTransform: ViewTransform = { x: transform.x, y: transform.y, scale, turn: view.turn };

  async function operate(action: () => Promise<void>) {
    setActionError("");
    setNotice("");
    try { await action(); }
    catch (error) { setActionError(errorMessage(error)); }
  }
  async function performStep(forward: boolean) {
    if (busy || !(forward ? next : previous)) return;
    setBusy(true);
    await operate(async () => {
      await invoke("image_viewer_step", { id: file.id, forward });
      await refresh();
    });
    setBusy(false);
  }
  function step(forward: boolean) {
    if (busy || sendingMedia || !(forward ? next : previous)) return;
    if (annotating || hasUnsentEdits) {
      setPendingDiscardAction(() => () => {
        setAnnotationDoc(null);
        setAnnotating(false);
        void performStep(forward);
      });
      return;
    }
    void performStep(forward);
  }
  async function copy() {
    if (busy || !loaded) return;
    setBusy(true);
    await operate(async () => {
      await invoke("copy_viewer_image", { id: file.id });
      setNotice("Image copied");
    });
    setBusy(false);
  }
  async function save() {
    if (busy) return;
    setBusy(true);
    await operate(async () => {
      if (hasUnsentEdits && annotationDoc && imageRef.current) {
        const edited = await exportAnnotatedImage(imageRef.current, annotationDoc);
        if (await api.viewerSaveEditedImage(file.id, edited.bytes)) setNotice(t("Edited copy saved", "已保存编辑副本"));
        return;
      }
      const transfer = await api.save(file.id);
      if (transfer) {
        setRequestedSave(true);
        await refresh();
      }
    });
    setBusy(false);
  }
  function enterAnnotator() {
    if (!loaded || failed || draft || sendingMedia || image.width <= 0 || image.height <= 0) return;
    const base = annotationDoc ?? createEmptyAnnotationDoc(image.width, image.height);
    annotatorSnapshotRef.current = structuredClone(base);
    setAnnotationDoc(base);
    setAnnotating(true);
  }
  function exitAnnotator() {
    if (dirtyInAnnotator) {
      setPendingDiscardAction(() => () => {
        const snapshot = annotatorSnapshotRef.current;
        setAnnotationDoc(snapshot && snapshot.items.length > 0 ? snapshot : null);
        setAnnotating(false);
      });
      return;
    }
    const snapshot = annotatorSnapshotRef.current;
    setAnnotationDoc(snapshot && snapshot.items.length > 0 ? snapshot : null);
    setAnnotating(false);
  }

  async function executeSendBatch(batchId: string, targets: ForwardTarget[]): Promise<ResolvedForwardAttempt> {
    if (!hasUnsentEdits || !annotationDoc) {
      const items = await api.viewerSendUnedited(file.id, batchId, targets);
      return { batchId, items };
    }
    const imgEl = imageRef.current;
    if (!imgEl) {
      throw new Error(t("Image is not loaded yet", "图片尚未加载完成"));
    }
    let exported = cachedExportRef.current;
    if (!exported) {
      const result = await exportAnnotatedImage(imgEl, annotationDoc);
      exported = {
        bytes: result.bytes,
        fileName: editedFileName(file.name, "image", result.ext),
      };
      cachedExportRef.current = exported;
    }
    const items = await api.viewerSendAnnotatedImage(
      file.id,
      batchId,
      targets,
      exported.fileName,
      exported.bytes,
    );
    return { batchId, items };
  }

  async function sendCurrentChat() {
    if (sendingMedia || annotating || !loaded || failed || draft) return;
    setActionError("");
    setNotice("");
    setSendingMedia(true);
    try {
      const sendKey = `${file.id}:${hasUnsentEdits ? "edited" : "orig"}`;
      if (!pendingCurrentSendRef.current || pendingCurrentSendRef.current.key !== sendKey) {
        pendingCurrentSendRef.current = {
          key: sendKey,
          batchId: crypto.randomUUID(),
          target: { conversation_id, attachment_id: crypto.randomUUID() },
        };
      }
      const { batchId, target } = pendingCurrentSendRef.current;
      const res = await executeSendBatch(batchId, [target]);
      const failedItem = res.items.find(item => item.status === "failed");
      if (failedItem) {
        throw new Error(failedItem.error || t("Could not send media", "发送失败"));
      }
      pendingCurrentSendRef.current = null;
      forceCloseWindow();
    } catch (error) {
      setActionError(errorMessage(error));
    } finally {
      setSendingMedia(false);
    }
  }

  const full = () => operate(() => changeFullscreen(!fullscreen));
  function keys(event: KeyboardEvent) {
    if (annotating || forwardOpen) return;
    if (event.ctrlKey && event.key.toLowerCase() === "c") {
      event.preventDefault();
      void copy();
    } else if (event.target === stage.current && event.shiftKey && pannable && event.key.startsWith("Arrow")) {
      const x = event.key === "ArrowLeft" ? 32 : event.key === "ArrowRight" ? -32 : 0;
      const y = event.key === "ArrowUp" ? 32 : event.key === "ArrowDown" ? -32 : 0;
      setView(current => clamp({ ...current, x: current.x + x, y: current.y + y }));
      event.preventDefault();
    } else if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
      event.preventDefault();
      step(event.key === "ArrowRight");
    } else if (event.target === stage.current && ["+", "=", "-"].includes(event.key)) {
      event.preventDefault();
      zoom({ x: 0, y: 0 }, event.key === "-" ? 0.8 : 1.25);
    }
  }
  function pointerDown(event: PointerEvent<HTMLDivElement>) {
    if (!pannable || event.button !== 0 || (event.target instanceof Element && event.target.closest("button"))) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    drag.current = { id: event.pointerId, x: event.clientX, y: event.clientY };
  }
  function pointerMove(event: PointerEvent<HTMLDivElement>) {
    const start = drag.current;
    if (!start || start.id !== event.pointerId) return;
    const x = event.clientX - start.x;
    const y = event.clientY - start.y;
    drag.current = { id: start.id, x: event.clientX, y: event.clientY };
    setView(current => {
      const offset = clamp(current);
      return clamp({ ...current, x: offset.x + x, y: offset.y + y });
    });
  }

  const saving = requestedSave && (download?.status === "running" || download?.status === "queued");
  const saveError = requestedSave && download && ["expired", "failed", "paused"].includes(download.status)
    ? download.error || "The image could not be saved. Try again." : "";
  const saveStatus = saving ? "Saving image…" : requestedSave && download?.status === "complete" ? "Image saved" : "";
  const tool = (label: string, icon: ReactNode, action: () => void, disabled = false, pressed?: boolean) =>
    <button type="button" className="icon-button" title={label} aria-label={label} aria-pressed={pressed}
      onClick={action} disabled={disabled}>{icon}</button>;

  return <div className="image-canvas" onKeyDown={keys}>
    <ViewerToolbar fullscreen={fullscreen}>
      <div>
        {tool("Previous media", <ChevronLeft size={18} />, () => step(false), busy || sendingMedia || !previous)}
        {tool("Next media", <ChevronRight size={18} />, () => step(true), busy || sendingMedia || !next)}
      </div>
      <div>
        {tool("Fit to window", <Shrink size={18} />, () => setScale(), !loaded, view.scale === undefined)}
        {tool("Original size", <span aria-hidden>1:1</span>, () => setScale(1), !loaded, view.scale === 1)}
        {tool("Rotate clockwise", <RotateCw size={18} />, () => {
          setView(current => ({ ...current, x: 0, y: 0, turn: (current.turn + 1) % 4 }));
        }, !loaded)}
      </div>
      <div>
        {!draft && tool(t("Annotate image", "标注图片"), <Pen size={18} />, () => {
          if (annotating) exitAnnotator();
          else enterAnnotator();
        }, !loaded || failed || busy || sendingMedia, annotating || hasUnsentEdits)}
        {tool("Copy image", <Copy size={18} />, () => void copy(), !loaded || busy)}
        {!draft && tool("Save image", <Download size={18} />, () => void save(), busy || saving)}
        {tool(fullscreen ? "Exit full screen" : "Full screen", fullscreen ? <Minimize size={18} /> : <Maximize size={18} />, () => void full())}
      </div>
      {!draft && (
        <div>
          <ViewerSplitSendButton
            disabled={!loaded || failed || annotating || busy}
            busy={sendingMedia}
            onSendCurrent={() => void sendCurrentChat()}
            onOpenForward={() => setForwardOpen(true)}
          />
        </div>
      )}
    </ViewerToolbar>
    <div ref={stage} className="image-stage" tabIndex={0} aria-label={file.name}
      data-ready={loaded || undefined} data-pannable={pannable || undefined}
      onPointerDown={pointerDown} onPointerMove={pointerMove} onLostPointerCapture={() => { drag.current = undefined; }}
      onDoubleClick={event => {
        if (!annotating && loaded && event.target instanceof Element && !event.target.closest("button")) {
          setScale(view.scale === 1 ? undefined : 1);
        }
      }}>
      {!loaded && !failed && <img className="image-stage-thumbnail" aria-hidden alt="" draggable={false}
        src={draft ? api.draftMediaUrl(file.id, "thumbnail") : api.mediaUrl(file.id, "cached-thumbnail")} onError={event => { event.currentTarget.hidden = true; }} />}
      {!failed && <img ref={imageRef} key={attempt} className="image-stage-original" draggable={false} alt={file.name}
        crossOrigin={windows ? "anonymous" : undefined}
        style={{
          visibility: loaded ? "visible" : "hidden",
          transform: `translate(-50%, -50%) translate(${transform.x}px, ${transform.y}px) rotate(${view.turn * 90}deg) scale(${scale})`,
        }}
        src={(draft ? api.draftMediaUrl(file.id) : api.mediaUrl(file.id)) + (attempt ? "?original&retry=" + attempt : "")}
        onLoad={event => {
          const { naturalWidth: width, naturalHeight: height } = event.currentTarget;
          setImage({ width, height });
          setLoaded(true);
        }}
        onError={() => { setLoaded(false); setFailed(true); }} />}
      {loaded && !failed && !annotating && annotationDoc && annotationDoc.items.length > 0 && (
        <ImageAnnotationOverlay
          naturalWidth={image.width}
          naturalHeight={image.height}
          viewTransform={viewTransform}
          stageRect={viewport}
          doc={annotationDoc}
        />
      )}
      {loaded && !failed && annotating && annotationDoc && (
        <ImageAnnotator
          imageElement={imageRef.current}
          naturalWidth={image.width}
          naturalHeight={image.height}
          viewTransform={viewTransform}
          stageRect={viewport}
          editing={annotating}
          doc={annotationDoc}
          onChange={setAnnotationDoc}
          onComplete={doc => {
            setAnnotationDoc(doc.items.length > 0 ? doc : null);
            setAnnotating(false);
          }}
          onRequestExit={exitAnnotator}
          onPendingEditChange={setPendingImageEdit}
        />
      )}
      {!loaded && <div className="image-viewer-note">
        {failed ? <><p role="alert">Image unavailable.</p>
          <button type="button" className="button subtle small" onClick={() => {
            setAttempt(value => value + 1);
            setFailed(false);
          }}><RotateCw size={16} /> Retry preview</button></>
          : <LoaderCircle className="preview-spinner" role="status" aria-label="Loading image" />}
      </div>}
      {(notice || actionError || saveStatus || saveError) && <div className="image-action-status" role={actionError || saveError ? "alert" : "status"}>
        {actionError || saveError || notice || saveStatus}
      </div>}
    </div>
    <MediaForwardPicker
      open={forwardOpen}
      submitting={sendingMedia}
      onClose={() => setForwardOpen(false)}
      onConfirm={async (batchId, targets) => {
        setSendingMedia(true);
        try {
          const res = await executeSendBatch(batchId, targets);
          if (!res.items.some(item => item.status === "failed")) {
            forceCloseWindow();
          }
          return res;
        } finally {
          setSendingMedia(false);
        }
      }}
      onRetryFailed={async (batchId, fullTargets) => {
        setSendingMedia(true);
        try {
          return await executeSendBatch(batchId, fullTargets);
        } finally {
          setSendingMedia(false);
        }
      }}
      onDoneAfterPartial={() => forceCloseWindow()}
    />
    {pendingDiscardAction && (
      <Dialog title={t("Discard edits?", "放弃编辑？")} close={() => setPendingDiscardAction(null)}>
        <p className="viewer-dialog-copy">
          {t("Your unsent annotations will be discarded and the original image stays unchanged.", "当前未发送的标注将被丢弃，原图不会受影响。")}
        </p>
        <div className="viewer-dialog-actions">
          <button type="button" className="button subtle" onClick={() => setPendingDiscardAction(null)}>
            {t("Continue viewing", "继续查看")}
          </button>
          <button type="button" className="button danger" onClick={() => {
            const action = pendingDiscardAction;
            setPendingDiscardAction(null);
            action();
          }}>
            {t("Discard edits", "放弃编辑")}
          </button>
        </div>
      </Dialog>
    )}
  </div>;
}

function VideoCanvas({
  conversation_id,
  file,
  previous,
  next,
  download,
  video,
  draft,
  refresh,
  fullscreen,
  changeFullscreen,
  registerCloseGuard,
  forceCloseWindow,
}: CanvasSharedProps) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const resumeState = useRef<{ time: number; play: boolean } | null>(null);
  const prevHasOriginal = useRef(Boolean(video?.has_local_original));
  const pendingVideoSendRef = useRef<PendingVideoSend | null>(null);
  const pendingCurrentSendRef = useRef<{ key: string; batchId: string; target: ForwardTarget } | null>(null);
  const [sourceRevision, setSourceRevision] = useState(0);
  const [loaded, setLoaded] = useState(false);
  const [failed, setFailed] = useState(false);
  const [notice, setNotice] = useState("");
  const [actionError, setActionError] = useState("");
  const [requestedSave, setRequestedSave] = useState(false);
  const [startingOriginal, setStartingOriginal] = useState(false);
  const [busy, setBusy] = useState(false);

  const [trimming, setTrimming] = useState(false);
  const [pendingVideoEdit, setPendingVideoEdit] = useState(false);
  const [previewHeight, setPreviewHeight] = useState(66);
  const [loadingTrimAssets, setLoadingTrimAssets] = useState(false);
  const [probe, setProbe] = useState<VideoProbeInfo | null>(null);
  const [thumbnails, setThumbnails] = useState<string[]>([]);
  const [trimRange, setTrimRange] = useState<VideoTrimRange | null>(null);
  const [confirmOriginalDownload, setConfirmOriginalDownload] = useState(false);
  const [pendingOpenTrimmer, setPendingOpenTrimmer] = useState(false);
  const [pendingDiscardAction, setPendingDiscardAction] = useState<(() => void) | null>(null);
  const [exportProgress, setExportProgress] = useState<VideoExportProgress | null>(null);
  const [confirmingCancelExport, setConfirmingCancelExport] = useState(false);
  const [forwardOpen, setForwardOpen] = useState(false);
  const [sendingMedia, setSendingMedia] = useState(false);

  const hasOriginal = Boolean(video?.has_local_original);
  const origDownload = video?.original_download;
  const origDownloading = startingOriginal
    || origDownload?.status === "downloading"
    || origDownload?.status === "verifying";
  const exportRunning = exportProgress?.status === "running";
  const hasUnsentEdits = trimRange !== null;

  const finalizeCompletedExport = useCallback(async (status: VideoExportProgress) => {
    const sendSpec = pendingVideoSendRef.current;
    if (!sendSpec) return;
    pendingVideoSendRef.current = null;
    try {
      await api.viewerSendTrimmedVideo(
        file.id,
        status.export_id,
        sendSpec.batchId,
        sendSpec.targets,
        sendSpec.fileName,
      );
      forceCloseWindow();
    } catch (error) {
      setActionError(errorMessage(error));
      setSendingMedia(false);
    }
  }, [file.id, forceCloseWindow]);

  const openTrimmer = useCallback(async () => {
    setActionError("");
    setNotice("");
    setPendingOpenTrimmer(false);
    setTrimming(true);
    setLoadingTrimAssets(true);
    try {
      const [probeInfo, frames] = await Promise.all([
        probe ? Promise.resolve(probe) : api.viewerVideoProbe(file.id),
        thumbnails.length > 0 ? Promise.resolve(thumbnails) : api.viewerVideoThumbnails(file.id, 10),
      ]);
      setProbe(probeInfo);
      setThumbnails(frames);
    } catch (error) {
      setTrimming(false);
      setActionError(errorMessage(error));
    } finally {
      setLoadingTrimAssets(false);
    }
  }, [file.id, probe, thumbnails]);

  useEffect(() => {
    registerCloseGuard({
      needsConfirm: () => exportRunning || pendingVideoEdit || hasUnsentEdits,
      promptClose: () => {
        if (exportRunning) {
          setPendingDiscardAction(() => () => {
            pendingVideoSendRef.current = null;
            forceCloseWindow();
          });
          return;
        }
        setPendingDiscardAction(() => () => {
          setTrimRange(null);
          setTrimming(false);
          setForwardOpen(false);
          forceCloseWindow();
        });
      },
    });
    return () => registerCloseGuard(undefined);
  }, [exportRunning, forceCloseWindow, hasUnsentEdits, pendingVideoEdit, registerCloseGuard]);

  // Seamlessly switch playback to the verified local original once downloaded.
  useEffect(() => {
    if (!prevHasOriginal.current && hasOriginal) {
      const el = videoRef.current;
      if (el) {
        resumeState.current = { time: el.currentTime || 0, play: !el.paused };
      }
      setFailed(false);
      setLoaded(false);
      setSourceRevision(value => value + 1);
      setNotice(t("Switched to local original video", "已切换至本地原视频"));
      if (pendingOpenTrimmer) {
        void openTrimmer();
      }
    }
    prevHasOriginal.current = hasOriginal;
  }, [hasOriginal, openTrimmer, pendingOpenTrimmer]);

  useEffect(() => {
    if (!origDownloading) return;
    const timer = globalThis.setInterval(() => {
      void refresh();
    }, 250);
    return () => globalThis.clearInterval(timer);
  }, [origDownloading, refresh]);

  useEffect(() => {
    if (!exportRunning || !exportProgress) return;
    const exportId = exportProgress.export_id;
    const timer = globalThis.setInterval(() => {
      void api.viewerVideoExportStatus(exportId).then(status => {
        if (!status) return;
        setExportProgress(status);
        if (status.status === "complete") {
          setConfirmingCancelExport(false);
          void finalizeCompletedExport(status);
        } else if (status.status !== "running") {
          setConfirmingCancelExport(false);
          pendingVideoSendRef.current = null;
          setSendingMedia(false);
          if (status.error) {
            setActionError(status.error);
          }
        }
      }).catch(() => {});
    }, 250);
    return () => globalThis.clearInterval(timer);
  }, [exportProgress, exportRunning, finalizeCompletedExport]);

  async function operate(action: () => Promise<void>) {
    setActionError("");
    setNotice("");
    try { await action(); }
    catch (error) { setActionError(errorMessage(error)); }
  }
  async function performStep(forward: boolean) {
    if (busy || !(forward ? next : previous)) return;
    setBusy(true);
    await operate(async () => {
      await invoke("image_viewer_step", { id: file.id, forward });
      await refresh();
    });
    setBusy(false);
  }
  function step(forward: boolean) {
    if (busy || sendingMedia || !(forward ? next : previous) || exportRunning) return;
    if (trimming || hasUnsentEdits) {
      setPendingDiscardAction(() => () => {
        setTrimRange(null);
        setTrimming(false);
        void performStep(forward);
      });
      return;
    }
    void performStep(forward);
  }
  async function save() {
    if (busy) return;
    setBusy(true);
    await operate(async () => {
      const transfer = await api.save(file.id);
      if (transfer) {
        setRequestedSave(true);
        await refresh();
      }
    });
    setBusy(false);
  }
  async function downloadOriginal() {
    if (origDownloading) return;
    setStartingOriginal(true);
    setActionError("");
    try {
      await api.viewerDownloadOriginalVideo(file.id);
      await refresh();
    } catch (error) {
      setActionError(errorMessage(error));
    } finally {
      setStartingOriginal(false);
    }
  }
  async function cancelOriginalDownload() {
    setPendingOpenTrimmer(false);
    setActionError("");
    try {
      await api.viewerCancelOriginalVideo(file.id);
      await refresh();
    } catch (error) {
      setActionError(errorMessage(error));
    }
  }
  function clickEditTrim() {
    if (draft || exportRunning || sendingMedia || failed) return;
    if (trimming) {
      setPendingDiscardAction(() => () => setTrimming(false));
      return;
    }
    if (hasOriginal) {
      void openTrimmer();
      return;
    }
    if (origDownloading) {
      setPendingOpenTrimmer(true);
      setNotice(t("Will open trimmer once original video finishes downloading", "原视频下载完成后将自动进入剪辑"));
      return;
    }
    setConfirmOriginalDownload(true);
  }
  async function handleCancelExport() {
    if (!exportProgress) return;
    setConfirmingCancelExport(false);
    pendingVideoSendRef.current = null;
    setSendingMedia(false);
    try {
      const nextStatus = await api.viewerCancelVideoExport(exportProgress.export_id);
      setExportProgress(nextStatus ?? null);
      setNotice(t("Video export cancelled", "已取消视频导出"));
    } catch (error) {
      setActionError(errorMessage(error));
    }
  }

  async function startTrimmedExportAndSend(batchId: string, targets: ForwardTarget[]): Promise<void> {
    if (!trimRange) return;
    setForwardOpen(false);
    setActionError("");
    setNotice("");
    setSendingMedia(true);
    pendingVideoSendRef.current = {
      batchId,
      targets,
      fileName: editedFileName(file.name, "video", "mp4"),
    };
    try {
      const initial = await api.viewerStartVideoExport(
        file.id,
        trimRange.startSec,
        trimRange.endSec,
        trimRange.durationSec,
      );
      setExportProgress(initial);
      if (initial.status === "complete") {
        await finalizeCompletedExport(initial);
      } else if (initial.status !== "running") {
        pendingVideoSendRef.current = null;
        setSendingMedia(false);
        if (initial.error) throw new Error(initial.error);
      }
    } catch (error) {
      pendingVideoSendRef.current = null;
      setSendingMedia(false);
      setActionError(errorMessage(error));
    }
  }

  async function executeSendVideo(batchId: string, targets: ForwardTarget[]): Promise<ResolvedForwardAttempt> {
    if (trimRange !== null) {
      await startTrimmedExportAndSend(batchId, targets);
      return {
        batchId,
        items: targets.map(target => ({ target, status: "queued" })),
      };
    }
    const items = await api.viewerSendUnedited(file.id, batchId, targets);
    return { batchId, items };
  }

  async function sendCurrentChat() {
    if (sendingMedia || exportRunning || trimming || failed || draft) return;
    const sendKey = `${file.id}:${trimRange ? `${trimRange.startSec}-${trimRange.endSec}` : "orig"}`;
    if (!pendingCurrentSendRef.current || pendingCurrentSendRef.current.key !== sendKey) {
      pendingCurrentSendRef.current = {
        key: sendKey,
        batchId: crypto.randomUUID(),
        target: { conversation_id, attachment_id: crypto.randomUUID() },
      };
    }
    const { batchId, target } = pendingCurrentSendRef.current;
    if (trimRange !== null) {
      await startTrimmedExportAndSend(batchId, [target]);
      return;
    }
    setActionError("");
    setNotice("");
    setSendingMedia(true);
    try {
      const res = await api.viewerSendUnedited(file.id, batchId, [target]);
      const failedItem = res.find(item => item.status === "failed");
      if (failedItem) {
        throw new Error(failedItem.error || t("Could not send media", "发送失败"));
      }
      pendingCurrentSendRef.current = null;
      forceCloseWindow();
    } catch (error) {
      setActionError(errorMessage(error));
    } finally {
      setSendingMedia(false);
    }
  }

  const saving = requestedSave && (download?.status === "running" || download?.status === "queued");
  const saveError = requestedSave && download && ["expired", "failed", "paused"].includes(download.status)
    ? download.error || "The video could not be saved. Try again." : "";
  const saveStatus = saving ? "Saving video…" : requestedSave && download?.status === "complete" ? "Video saved" : "";
  const tool = (label: string, icon: ReactNode, action: () => void, disabled = false, pressed?: boolean) =>
    <button type="button" className="icon-button" title={label} aria-label={label} aria-pressed={pressed}
      onClick={action} disabled={disabled}>{icon}</button>;

  const origPct = origDownload && origDownload.total > 0
    ? Math.min(100, Math.round((origDownload.received / origDownload.total) * 100))
    : 0;
  const videoSrc = hasOriginal
    ? api.mediaUrl(file.id, "video-original") + (sourceRevision ? "&rev=" + sourceRevision : "")
    : api.mediaUrl(file.id, "video-stream") + (sourceRevision ? "&rev=" + sourceRevision : "");

  return <div className="image-canvas" onKeyDown={event => {
    if (trimming || forwardOpen || (event.target instanceof Element && event.target.closest("video"))) return;
    if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
      event.preventDefault();
      step(event.key === "ArrowRight");
    }
  }}>
    <ViewerToolbar fullscreen={fullscreen}>
      <div>
        {tool("Previous media", <ChevronLeft size={18} />, () => step(false), busy || sendingMedia || !previous || exportRunning)}
        {tool("Next media", <ChevronRight size={18} />, () => step(true), busy || sendingMedia || !next || exportRunning)}
      </div>
      <div>
        {!draft && tool(t("Trim video", "剪辑视频"), <Pen size={18} />, clickEditTrim, busy || failed || exportRunning || sendingMedia, trimming || hasUnsentEdits)}
        {!draft && tool("Save video", <Download size={18} />, () => void save(), busy || saving)}
        {tool(fullscreen ? "Exit full screen" : "Full screen", fullscreen ? <Minimize size={18} /> : <Maximize size={18} />,
          () => void operate(() => changeFullscreen(!fullscreen)))}
      </div>
      {!draft && (
        <div>
          <ViewerSplitSendButton
            disabled={failed || trimming || exportRunning || busy}
            busy={sendingMedia}
            onSendCurrent={() => void sendCurrentChat()}
            onOpenForward={() => setForwardOpen(true)}
          />
        </div>
      )}
    </ViewerToolbar>
    <div className="video-workspace">
    <div className="video-layout" data-trimming={trimming || undefined}
      style={{ "--trim-preview": `${previewHeight}%` } as CSSProperties}>
    <div className="image-stage video-stage" tabIndex={0} aria-label={file.name}>
      {!failed && <video
        ref={videoRef}
        key={sourceRevision}
        className="video-stage-player"
        src={videoSrc}
        poster={api.mediaUrl(file.id, "video-cover")}
        controls={!trimming}
        controlsList="nodownload noremoteplayback"
        playsInline
        preload="auto"
        disablePictureInPicture
        aria-label={file.name}
        onLoadedMetadata={event => {
          const el = event.currentTarget;
          if (resumeState.current) {
            const { time, play } = resumeState.current;
            resumeState.current = null;
            if (time > 0 && Number.isFinite(el.duration) && time < el.duration) {
              el.currentTime = time;
            }
            if (play) void el.play().catch(() => {});
          }
          setLoaded(true);
        }}
        onLoadedData={() => setLoaded(true)}
        onPlay={event => {
          if (!trimming && trimRange) {
            const el = event.currentTarget;
            if (el.currentTime < trimRange.startSec || el.currentTime >= trimRange.endSec - 0.02) {
              el.currentTime = trimRange.startSec;
            }
          }
        }}
        onTimeUpdate={event => {
          if (!trimming && trimRange) {
            const el = event.currentTarget;
            if (el.currentTime < trimRange.startSec - 0.05) {
              el.currentTime = trimRange.startSec;
            } else if (el.currentTime >= trimRange.endSec) {
              el.pause();
              el.currentTime = trimRange.startSec;
            }
          }
        }}
        onError={() => { setLoaded(false); setFailed(true); }}
      />}
      {!trimming && trimRange && (
        <div className="viewer-trim-badge" role="status">
          {t("Trimmed preview", "剪辑预览")} {formatTrimTime(trimRange.startSec)} – {formatTrimTime(trimRange.endSec)}
        </div>
      )}
      {!loaded && <div className="image-viewer-note">
        {failed ? <>
          <p role="alert">{t("Video preview unavailable.", "暂无法播放视频预览。")}</p>
          <div className="video-error-actions">
            <button type="button" className="button subtle small" onClick={() => {
              setFailed(false);
              setSourceRevision(value => value + 1);
            }}><RotateCw size={16} /> {t("Retry", "重试")}</button>
          </div>
        </> : <LoaderCircle className="preview-spinner" role="status" aria-label="Loading video" />}
      </div>}
      {trimming && loadingTrimAssets && (
        <div className="image-viewer-note" role="status">
          <LoaderCircle className="preview-spinner" aria-label={t("Preparing trimmer", "正在加载剪辑轨道")} />
          <p>{t("Loading timeline…", "正在加载剪辑时间轴…")}</p>
        </div>
      )}
      {!draft && !hasOriginal && !trimming && <div className="viewer-original-video-bar">
        {origDownloading ? <>
          <span className="viewer-original-video-pill" role="status">
            <LoaderCircle className="preview-spinner" size={14} aria-hidden />
            {origDownload?.status === "verifying"
              ? t("Verifying original…", "校验原视频中…")
              : `${t("Downloading original", "下载原视频")} ${origPct}%`}
          </span>
          <button type="button" className="viewer-original-video-cancel" aria-label={t("Cancel original download", "取消原视频下载")}
            onClick={() => void cancelOriginalDownload()}><X size={14} /></button>
        </> : <button type="button" className="viewer-original-video-btn" onClick={() => void downloadOriginal()}>
          {origDownload?.status === "failed"
            ? `${t("Retry original", "重试原视频")} ${decimalSize(video?.original_size ?? file.size)}`
            : `${t("View original video", "查看原视频")} ${decimalSize(video?.original_size ?? file.size)}`}
        </button>}
      </div>}
      {exportProgress && exportProgress.status !== "cancelled" && (
        <VideoExportBar
          progress={exportProgress}
          confirmingCancel={confirmingCancelExport}
          onRequestCancel={() => setConfirmingCancelExport(true)}
          onConfirmCancel={() => void handleCancelExport()}
          onContinueExport={() => setConfirmingCancelExport(false)}
        />
      )}
      {(notice || actionError || saveStatus || saveError || origDownload?.error) && <div className="image-action-status"
        role={actionError || saveError || origDownload?.error ? "alert" : "status"}>
        {actionError || saveError || origDownload?.error || notice || saveStatus}
      </div>}
    </div>
    {trimming && <div className="video-trim-divider" role="separator" tabIndex={0}
      aria-label={t("Resize video preview", "调整视频预览高度")} aria-orientation="horizontal"
      aria-valuemin={25} aria-valuemax={75} aria-valuenow={previewHeight}
      onKeyDown={event => {
        const step = event.key === "ArrowUp" ? -5 : event.key === "ArrowDown" ? 5 : 0;
        if (!step) return;
        event.preventDefault();
        setPreviewHeight(value => Math.max(25, Math.min(75, value + step)));
      }}
      onPointerDown={event => {
        if (event.button === 0) event.currentTarget.setPointerCapture(event.pointerId);
      }}
      onPointerMove={event => {
        if (!event.currentTarget.hasPointerCapture(event.pointerId)) return;
        const bounds = event.currentTarget.parentElement?.getBoundingClientRect();
        if (bounds?.height) setPreviewHeight(Math.round(Math.max(25, Math.min(75,
          (event.clientY - bounds.top) * 100 / bounds.height))));
      }} />}
      {trimming && !loadingTrimAssets && probe && (
        <div className="video-trim-pane">
        <VideoTrimmer
          attachmentId={file.id}
          videoRef={videoRef}
          initialRange={trimRange}
          probe={probe}
          thumbnails={thumbnails}
          onComplete={range => {
            const isFull = Math.abs(range.startSec) < 1e-4 && Math.abs(range.endSec - range.durationSec) < 1e-4;
            setTrimRange(isFull ? null : range);
            setTrimming(false);
            const el = videoRef.current;
            if (el && !isFull) {
              el.currentTime = range.startSec;
            }
          }}
          onRequestCancel={dirty => {
            if (!dirty) {
              setTrimming(false);
              return;
            }
            setPendingDiscardAction(() => () => setTrimming(false));
          }}
          onPendingEditChange={setPendingVideoEdit}
        />
        </div>
      )}
    </div>
    </div>
    <MediaForwardPicker
      open={forwardOpen}
      submitting={sendingMedia}
      onClose={() => setForwardOpen(false)}
      onConfirm={async (batchId, targets) => {
        if (trimRange !== null) {
          return await executeSendVideo(batchId, targets);
        }
        setSendingMedia(true);
        try {
          const res = await executeSendVideo(batchId, targets);
          if (!res.items.some(item => item.status === "failed")) {
            forceCloseWindow();
          }
          return res;
        } finally {
          setSendingMedia(false);
        }
      }}
      onRetryFailed={async (batchId, fullTargets) => {
        setSendingMedia(true);
        try {
          return await executeSendVideo(batchId, fullTargets);
        } finally {
          setSendingMedia(false);
        }
      }}
      onDoneAfterPartial={() => forceCloseWindow()}
    />
    {confirmOriginalDownload && (
      <Dialog title={t("Download original video?", "下载原视频以剪辑？")} close={() => setConfirmOriginalDownload(false)}>
        <p className="viewer-dialog-copy">
          {t(
            `Trimming requires the verified original video (${decimalSize(video?.original_size ?? file.size)}). Download it now?`,
            `剪辑需要已校验的原视频（${decimalSize(video?.original_size ?? file.size)}），是否立即下载？`,
          )}
        </p>
        <div className="viewer-dialog-actions">
          <button type="button" className="button subtle" onClick={() => setConfirmOriginalDownload(false)}>
            {t("Cancel", "取消")}
          </button>
          <button type="button" className="button primary" onClick={() => {
            setConfirmOriginalDownload(false);
            setPendingOpenTrimmer(true);
            void downloadOriginal();
          }}>
            {t("Download now", "立即下载")}
          </button>
        </div>
      </Dialog>
    )}
    {pendingDiscardAction && (
      <Dialog title={t("Discard edits?", "放弃编辑？")} close={() => setPendingDiscardAction(null)}>
        <p className="viewer-dialog-copy">
          {t("Your unsent video trim will be discarded and the original video stays unchanged.", "当前未发送的视频剪辑将被丢弃，原视频不会受影响。")}
        </p>
        <div className="viewer-dialog-actions">
          <button type="button" className="button subtle" onClick={() => setPendingDiscardAction(null)}>
            {t("Continue viewing", "继续查看")}
          </button>
          <button type="button" className="button danger" onClick={() => {
            const action = pendingDiscardAction;
            setPendingDiscardAction(null);
            action();
          }}>
            {t("Discard edits", "放弃编辑")}
          </button>
        </div>
      </Dialog>
    )}
  </div>;
}
