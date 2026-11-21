import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ChevronLeft, ChevronRight, Copy, Download, LoaderCircle, Maximize, Minimize, RotateCw, Scan } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import type { KeyboardEvent, PointerEvent } from "react";
import { api, errorMessage } from "./api";
import type { Attachment, Transfer } from "./api";
import "./image-viewer.css";

type Selection = { file: Attachment; previous: boolean; next: boolean; download?: Transfer };
type Point = { x: number; y: number };
type View = Point & { scale?: number; turn: number };
type Size = { width: number; height: number };

const centered: View = { x: 0, y: 0, turn: 0 };
const empty: Size = { width: 0, height: 0 };
const window = getCurrentWindow();

export function ImageViewer() {
  const [selection, setSelection] = useState<Selection>();
  const [failure, setFailure] = useState("");
  const generation = useRef(0);
  const refresh = useCallback(async () => {
    const request = ++generation.current;
    try {
      const selected = await invoke<Selection>("image_viewer_selection");
      if (generation.current === request) {
        setSelection(selected);
        setFailure("");
      }
    } catch (error) {
      if (generation.current === request) {
        setSelection(undefined);
        setFailure(errorMessage(error));
      }
    }
  }, []);

  useEffect(() => {
    void refresh();
    const timer = globalThis.setInterval(refresh, 750);
    return () => {
      globalThis.clearInterval(timer);
      generation.current++;
    };
  }, [refresh]);

  return <main className="image-viewer" aria-label="Image viewer" tabIndex={-1} onKeyDown={event => {
    if (!selection && event.key === "Escape") void window.close().catch(error => setFailure(errorMessage(error)));
  }}>
    {selection ? <ImageCanvas key={selection.file.id} {...selection} refresh={refresh} />
      : <div className="image-viewer-note" role={failure ? "alert" : "status"}>
        {failure || <LoaderCircle className="preview-spinner" aria-label="Loading image" />}
      </div>}
  </main>;
}

function ImageCanvas({ file, previous, next, download, refresh }: Selection & { refresh: () => Promise<void> }) {
  const stage = useRef<HTMLDivElement>(null);
  const drag = useRef<(Point & { id: number }) | undefined>(undefined);
  const [viewport, setViewport] = useState<Size>(empty);
  const [image, setImage] = useState<Size>(empty);
  const [view, setView] = useState<View>(centered);
  const [attempt, setAttempt] = useState(0);
  const [loaded, setLoaded] = useState(false);
  const [failed, setFailed] = useState(false);
  const [fullscreen, setFullscreen] = useState(false);
  const [notice, setNotice] = useState("");
  const [actionError, setActionError] = useState("");
  const [requestedSave, setRequestedSave] = useState(false);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    const element = stage.current!;
    const observer = new ResizeObserver(() => {
      setViewport({ width: element.clientWidth, height: element.clientHeight });
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  useEffect(() => { void window.isFullscreen().then(setFullscreen).catch(() => {}); }, []);

  const dimensions = useCallback((turn: number) => turn % 2
    ? { width: image.height, height: image.width } : image, [image]);
  const fit = useCallback((turn: number) => {
    const { width, height } = dimensions(turn);
    return width && height && viewport.width && viewport.height
      ? Math.min(1, viewport.width / width, viewport.height / height) : 1;
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
      const previous = current.scale ?? fit(current.turn);
      const scale = Math.max(Math.min(fit(current.turn), 0.25), Math.min(8, previous * factor));
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

  async function operate(action: () => Promise<void>) {
    setActionError("");
    setNotice("");
    try { await action(); }
    catch (error) { setActionError(errorMessage(error)); }
  }
  async function step(forward: boolean) {
    if (busy || !(forward ? next : previous)) return;
    setBusy(true);
    await operate(async () => {
      await invoke("image_viewer_step", { id: file.id, forward });
      await refresh();
    });
    setBusy(false);
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
      const transfer = await api.save(file.id);
      if (transfer) {
        setRequestedSave(true);
        await refresh();
      }
    });
    setBusy(false);
  }
  const full = () => operate(async () => {
    await window.setFullscreen(!fullscreen);
    setFullscreen(!fullscreen);
  });
  function keys(event: KeyboardEvent) {
    if (event.key === "Escape") {
      void (fullscreen ? full() : operate(() => window.close()));
    } else if (event.ctrlKey && event.key.toLowerCase() === "c") {
      event.preventDefault();
      void copy();
    } else if (event.target === stage.current && event.shiftKey && pannable && event.key.startsWith("Arrow")) {
      const x = event.key === "ArrowLeft" ? 32 : event.key === "ArrowRight" ? -32 : 0;
      const y = event.key === "ArrowUp" ? 32 : event.key === "ArrowDown" ? -32 : 0;
      setView(current => clamp({ ...current, x: current.x + x, y: current.y + y }));
      event.preventDefault();
    } else if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
      event.preventDefault();
      void step(event.key === "ArrowRight");
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
  const tool = (label: string, icon: React.ReactNode, action: () => void, disabled = false, pressed?: boolean) =>
    <button type="button" className="icon-button" title={label} aria-label={label} aria-pressed={pressed}
      onClick={action} disabled={disabled}>{icon}</button>;

  return <div className="image-canvas" onKeyDown={keys}>
    <div className="image-tools" role="group" aria-label="Image controls">
      <div>
        {tool("Previous image", <ChevronLeft size={18} />, () => void step(false), busy || !previous)}
        {tool("Next image", <ChevronRight size={18} />, () => void step(true), busy || !next)}
      </div>
      <div>
        {tool("Fit to window", <Scan size={18} />, () => setScale(), !loaded, view.scale === undefined)}
        {tool("Original size", <span aria-hidden>1:1</span>, () => setScale(1), !loaded, view.scale === 1)}
        {tool("Rotate clockwise", <RotateCw size={18} />, () => {
          setView(current => ({ ...current, x: 0, y: 0, turn: (current.turn + 1) % 4 }));
        }, !loaded)}
      </div>
      <div>
        {tool("Copy image", <Copy size={18} />, () => void copy(), !loaded || busy)}
        {tool("Save image", <Download size={18} />, () => void save(), busy || saving)}
        {tool(fullscreen ? "Exit full screen" : "Full screen", fullscreen ? <Minimize size={18} /> : <Maximize size={18} />, () => void full())}
      </div>
    </div>
    <div ref={stage} className="image-stage" tabIndex={0} aria-label={file.name}
      data-ready={loaded || undefined} data-pannable={pannable || undefined}
      onPointerDown={pointerDown} onPointerMove={pointerMove} onLostPointerCapture={() => { drag.current = undefined; }}
      onDoubleClick={event => {
        if (loaded && event.target instanceof Element && !event.target.closest("button")) setScale(view.scale === 1 ? undefined : 1);
      }}>
      {!loaded && !failed && <img className="image-stage-thumbnail" aria-hidden alt="" draggable={false}
        src={api.mediaUrl(file.id, "cached-thumbnail")} onError={event => { event.currentTarget.hidden = true; }} />}
      {!failed && <img key={attempt} className="image-stage-original" draggable={false} alt={file.name}
        style={{
          visibility: loaded ? "visible" : "hidden",
          transform: `translate(-50%, -50%) translate(${transform.x}px, ${transform.y}px) rotate(${view.turn * 90}deg) scale(${scale})`,
        }}
        src={api.mediaUrl(file.id) + (attempt ? "?original&retry=" + attempt : "")}
        onLoad={event => {
          const { naturalWidth: width, naturalHeight: height } = event.currentTarget;
          setImage({ width, height });
          setLoaded(true);
        }}
        onError={() => { setLoaded(false); setFailed(true); }} />}
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
  </div>;
}
