import { Check, LoaderCircle, Pause, Play, Redo2, Undo2, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import type { KeyboardEvent, PointerEvent } from "react";
import { decimalSize } from "./file-labels";
import { t } from "./locale";
import type { VideoExportBarProps, VideoTrimRange, VideoTrimmerProps } from "./media-viewer-types";
import {
  canRedoTrim,
  canUndoTrim,
  clampTrimRange,
  commitTrimHistory,
  createTrimHistory,
  formatTrimTime,
  redoTrimHistory,
  stepFrame,
  trimRangesEqual,
  undoTrimHistory,
} from "./video-trimmer";
import "./video-trimmer.css";

type DragTarget = "start" | "end" | "playhead";

export function VideoTrimmer({
  attachmentId,
  videoRef,
  initialRange,
  probe,
  thumbnails,
  onComplete,
  onRequestCancel,
  onPendingEditChange,
}: VideoTrimmerProps) {
  const durationSec = probe.duration_sec > 0 ? probe.duration_sec : 0;
  const computeInitial = () => clampTrimRange(
    initialRange ?? { startSec: 0, endSec: durationSec, durationSec },
  );
  const baselineRef = useRef<{ attachmentId: string; range: VideoTrimRange }>({
    attachmentId,
    range: computeInitial(),
  });
  if (baselineRef.current.attachmentId !== attachmentId) {
    baselineRef.current = { attachmentId, range: computeInitial() };
  }
  const baselineRange = baselineRef.current.range;

  const [history, setHistory] = useState(() => createTrimHistory(baselineRange));
  const [liveRange, setLiveRange] = useState<VideoTrimRange | null>(null);
  const [playheadSec, setPlayheadSec] = useState(baselineRange.startSec);
  const [playing, setPlaying] = useState(false);

  const trackRef = useRef<HTMLDivElement>(null);
  const dragRef = useRef<{ pointerId: number; target: DragTarget } | null>(null);
  const liveRangeRef = useRef<VideoTrimRange | null>(null);

  const range = liveRange ?? history.present;
  const rangeRef = useRef(range);
  rangeRef.current = range;

  const dirty = !trimRangesEqual(baselineRange, history.present);
  useEffect(() => {
    onPendingEditChange(!trimRangesEqual(baselineRange, range));
    return () => onPendingEditChange(false);
  }, [baselineRange, onPendingEditChange, range]);

  useEffect(() => {
    const initial = baselineRef.current.range;
    setHistory(createTrimHistory(initial));
    setLiveRange(null);
    liveRangeRef.current = null;
    setPlayheadSec(initial.startSec);
    const video = videoRef.current;
    if (video) {
      video.pause();
      if (Number.isFinite(video.duration) && video.duration > 0) {
        video.currentTime = Math.min(initial.startSec, video.duration);
      }
    }
    setPlaying(false);
  }, [attachmentId, videoRef]);

  const seekVideo = useCallback((sec: number) => {
    setPlayheadSec(sec);
    const video = videoRef.current;
    if (video && Number.isFinite(video.duration) && video.duration > 0) {
      video.currentTime = Math.max(0, Math.min(video.duration, sec));
    }
  }, [videoRef]);

  // Keep video playback strictly bounded within [startSec, endSec].
  useEffect(() => {
    const video = videoRef.current;
    if (!video) return;

    const enforceBounds = () => {
      const { startSec, endSec } = rangeRef.current;
      const tNow = video.currentTime;
      if (tNow < startSec - 0.03) {
        video.currentTime = startSec;
        setPlayheadSec(startSec);
      } else if (tNow >= endSec - 0.015) {
        if (!video.paused) {
          video.pause();
        }
        if (Math.abs(tNow - endSec) > 0.03) {
          video.currentTime = endSec;
        }
        setPlayheadSec(endSec);
      } else {
        setPlayheadSec(tNow);
      }
    };

    const onPlay = () => {
      const { startSec, endSec } = rangeRef.current;
      if (video.currentTime < startSec - 0.03 || video.currentTime >= endSec - 0.03) {
        video.currentTime = startSec;
        setPlayheadSec(startSec);
      }
      setPlaying(true);
    };
    const onPause = () => setPlaying(false);

    video.addEventListener("play", onPlay);
    video.addEventListener("pause", onPause);
    video.addEventListener("ended", onPause);
    video.addEventListener("timeupdate", enforceBounds);
    video.addEventListener("seeked", enforceBounds);
    setPlaying(!video.paused && !video.ended);

    return () => {
      video.removeEventListener("play", onPlay);
      video.removeEventListener("pause", onPause);
      video.removeEventListener("ended", onPause);
      video.removeEventListener("timeupdate", enforceBounds);
      video.removeEventListener("seeked", enforceBounds);
    };
  }, [videoRef]);

  useEffect(() => {
    if (!playing) return;
    let rafId = 0;
    const tick = () => {
      const video = videoRef.current;
      if (!video || video.paused) return;
      const { startSec, endSec } = rangeRef.current;
      if (video.currentTime < startSec - 0.03) {
        video.currentTime = startSec;
        setPlayheadSec(startSec);
      } else if (video.currentTime >= endSec - 0.015) {
        video.pause();
        video.currentTime = endSec;
        setPlayheadSec(endSec);
        return;
      } else {
        setPlayheadSec(video.currentTime);
      }
      rafId = globalThis.requestAnimationFrame(tick);
    };
    rafId = globalThis.requestAnimationFrame(tick);
    return () => globalThis.cancelAnimationFrame(rafId);
  }, [playing, videoRef]);

  const togglePlayback = useCallback(() => {
    const video = videoRef.current;
    if (!video) return;
    if (!video.paused && !video.ended) {
      video.pause();
      return;
    }
    const { startSec, endSec } = rangeRef.current;
    if (video.currentTime < startSec - 0.03 || video.currentTime >= endSec - 0.03) {
      video.currentTime = startSec;
      setPlayheadSec(startSec);
    }
    void video.play().catch(() => {});
  }, [videoRef]);

  const applyUndo = useCallback(() => {
    setHistory(prev => {
      const next = undoTrimHistory(prev);
      if (next !== prev) {
        const clampedPlayhead = Math.max(
          next.present.startSec,
          Math.min(next.present.endSec, playheadSec),
        );
        seekVideo(clampedPlayhead);
      }
      return next;
    });
  }, [playheadSec, seekVideo]);

  const applyRedo = useCallback(() => {
    setHistory(prev => {
      const next = redoTrimHistory(prev);
      if (next !== prev) {
        const clampedPlayhead = Math.max(
          next.present.startSec,
          Math.min(next.present.endSec, playheadSec),
        );
        seekVideo(clampedPlayhead);
      }
      return next;
    });
  }, [playheadSec, seekVideo]);

  const timeFromClientX = useCallback((clientX: number): number => {
    const el = trackRef.current;
    if (!el || durationSec <= 0) return 0;
    const rect = el.getBoundingClientRect();
    if (rect.width <= 0) return 0;
    const ratio = Math.max(0, Math.min(1, (clientX - rect.left) / rect.width));
    return ratio * durationSec;
  }, [durationSec]);

  const updateDragAt = useCallback((target: DragTarget, clientX: number) => {
    const time = timeFromClientX(clientX);
    if (target === "playhead") {
      const current = liveRangeRef.current ?? rangeRef.current;
      const clamped = Math.max(current.startSec, Math.min(current.endSec, time));
      seekVideo(clamped);
      return;
    }
    const base = liveRangeRef.current ?? rangeRef.current;
    const next = target === "start"
      ? clampTrimRange({ ...base, startSec: time }, "start")
      : clampTrimRange({ ...base, endSec: time }, "end");
    liveRangeRef.current = next;
    setLiveRange(next);
    seekVideo(target === "start" ? next.startSec : next.endSec);
  }, [seekVideo, timeFromClientX]);

  const finishDrag = useCallback(() => {
    const drag = dragRef.current;
    dragRef.current = null;
    if (!drag) return;
    if (drag.target !== "playhead" && liveRangeRef.current) {
      const finalRange = liveRangeRef.current;
      liveRangeRef.current = null;
      setLiveRange(null);
      setHistory(prev => commitTrimHistory(prev, finalRange));
    }
  }, []);

  const handleTrackPointerDown = (event: PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0 || durationSec <= 0) return;
    const targetEl = event.target instanceof Element
      ? event.target.closest<HTMLElement>("[data-trim-handle]")
      : null;
    const handle = targetEl?.dataset.trimHandle as DragTarget | undefined;
    const dragTarget: DragTarget = handle ?? "playhead";

    videoRef.current?.pause();
    event.currentTarget.setPointerCapture(event.pointerId);
    dragRef.current = { pointerId: event.pointerId, target: dragTarget };
    updateDragAt(dragTarget, event.clientX);
  };

  const handleTrackPointerMove = (event: PointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    updateDragAt(drag.target, event.clientX);
  };

  const handleHandleKeyDown = (side: "start" | "end", event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    event.preventDefault();
    event.stopPropagation();
    videoRef.current?.pause();
    const dir: -1 | 1 = event.key === "ArrowRight" ? 1 : -1;
    const stepCount = event.shiftKey ? 10 : 1;
    let nextSec = side === "start" ? range.startSec : range.endSec;
    for (let i = 0; i < stepCount; i++) {
      nextSec = stepFrame(nextSec, probe.fps, dir, 0, durationSec);
    }
    const nextRange = side === "start"
      ? clampTrimRange({ ...range, startSec: nextSec }, "start")
      : clampTrimRange({ ...range, endSec: nextSec }, "end");
    setHistory(prev => commitTrimHistory(prev, nextRange));
    seekVideo(side === "start" ? nextRange.startSec : nextRange.endSec);
  };

  const handleTrimmerKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      onRequestCancel(dirty);
      return;
    }
    const mod = event.ctrlKey || event.metaKey;
    if (mod && event.key.toLowerCase() === "z") {
      event.preventDefault();
      event.stopPropagation();
      if (event.shiftKey) {
        applyRedo();
      } else {
        applyUndo();
      }
      return;
    }
    if (mod && event.key.toLowerCase() === "y") {
      event.preventDefault();
      event.stopPropagation();
      applyRedo();
      return;
    }
    if (event.key === " " && !(event.target instanceof HTMLButtonElement)) {
      event.preventDefault();
      event.stopPropagation();
      togglePlayback();
    }
  };

  const startPct = durationSec > 0 ? (range.startSec / durationSec) * 100 : 0;
  const endPct = durationSec > 0 ? (range.endSec / durationSec) * 100 : 100;
  const playheadPct = durationSec > 0
    ? Math.max(startPct, Math.min(endPct, (playheadSec / durationSec) * 100))
    : 0;
  const selectedDuration = Math.max(0, range.endSec - range.startSec);

  return (
    <section
      className="video-trimmer"
      aria-label={t("Video trimmer", "视频剪辑")}
      onKeyDown={handleTrimmerKeyDown}
    >
      <div className="video-trimmer-top">
        <div className="video-trimmer-controls">
          <button
            type="button"
            className="icon-button video-trimmer-icon-btn"
            aria-label={playing ? t("Pause", "暂停") : t("Play", "播放")}
            title={playing ? t("Pause", "暂停") : t("Play", "播放")}
            onClick={togglePlayback}
          >
            {playing ? <Pause size={16} /> : <Play size={16} />}
          </button>
          <button
            type="button"
            className="icon-button video-trimmer-icon-btn"
            aria-label={t("Undo", "撤销")}
            title={t("Undo (Ctrl+Z)", "撤销 (Ctrl+Z)")}
            disabled={!canUndoTrim(history)}
            onClick={applyUndo}
          >
            <Undo2 size={16} />
          </button>
          <button
            type="button"
            className="icon-button video-trimmer-icon-btn"
            aria-label={t("Redo", "重做")}
            title={t("Redo (Ctrl+Y)", "重做 (Ctrl+Y)")}
            disabled={!canRedoTrim(history)}
            onClick={applyRedo}
          >
            <Redo2 size={16} />
          </button>
        </div>

        <div className="video-trimmer-times" aria-label={t("Trim timestamps", "裁剪时间")}>
          <span className="video-trimmer-time-item">
            <span className="video-trimmer-time-label">{t("Start", "开始")}</span>
            <output className="video-trimmer-time-value">{formatTrimTime(range.startSec)}</output>
          </span>
          <span className="video-trimmer-time-item">
            <span className="video-trimmer-time-label">{t("End", "结束")}</span>
            <output className="video-trimmer-time-value">{formatTrimTime(range.endSec)}</output>
          </span>
          <span className="video-trimmer-time-item video-trimmer-time-duration">
            <span className="video-trimmer-time-label">{t("Selected", "时长")}</span>
            <output className="video-trimmer-time-value">{formatTrimTime(selectedDuration)}</output>
          </span>
        </div>

        <div className="video-trimmer-actions">
          <button
            type="button"
            className="button small video-trimmer-cancel-btn"
            onClick={() => onRequestCancel(dirty)}
          >
            {t("Cancel", "取消")}
          </button>
          <button
            type="button"
            className="button primary small"
            onClick={() => onComplete(range)}
          >
            {t("Done", "完成")}
          </button>
        </div>
      </div>

      <div
        className="video-trimmer-track-shell"
        onPointerDown={handleTrackPointerDown}
        onPointerMove={handleTrackPointerMove}
        onPointerUp={finishDrag}
        onPointerCancel={finishDrag}
        onLostPointerCapture={finishDrag}
      >
        <div ref={trackRef} className="video-trimmer-track">
          <div className="video-trimmer-thumbs" aria-hidden="true">
            {thumbnails.length > 0 ? (
              thumbnails.map((src, index) => (
                <img key={index} src={src} alt="" draggable={false} />
              ))
            ) : (
              <div className="video-trimmer-track-empty" />
            )}
          </div>

          <div
            className="video-trimmer-mask video-trimmer-mask-left"
            style={{ width: `${startPct}%` }}
          />
          <div
            className="video-trimmer-mask video-trimmer-mask-right"
            style={{ width: `${Math.max(0, 100 - endPct)}%` }}
          />

          <div
            className="video-trimmer-selection"
            style={{ left: `${startPct}%`, width: `${Math.max(0, endPct - startPct)}%` }}
          >
            <div
              role="slider"
              tabIndex={0}
              data-trim-handle="start"
              className="video-trimmer-handle video-trimmer-handle-start"
              aria-label={t("Trim start", "裁剪起点")}
              aria-valuemin={0}
              aria-valuemax={Number(range.endSec.toFixed(2))}
              aria-valuenow={Number(range.startSec.toFixed(2))}
              aria-valuetext={formatTrimTime(range.startSec)}
              onKeyDown={event => handleHandleKeyDown("start", event)}
            >
              <span className="video-trimmer-handle-grip" aria-hidden="true" />
            </div>

            <div
              role="slider"
              tabIndex={0}
              data-trim-handle="end"
              className="video-trimmer-handle video-trimmer-handle-end"
              aria-label={t("Trim end", "裁剪终点")}
              aria-valuemin={Number(range.startSec.toFixed(2))}
              aria-valuemax={Number(durationSec.toFixed(2))}
              aria-valuenow={Number(range.endSec.toFixed(2))}
              aria-valuetext={formatTrimTime(range.endSec)}
              onKeyDown={event => handleHandleKeyDown("end", event)}
            >
              <span className="video-trimmer-handle-grip" aria-hidden="true" />
            </div>
          </div>

          <div
            data-trim-handle="playhead"
            className="video-trimmer-playhead"
            style={{ left: `${playheadPct}%` }}
            aria-hidden="true"
          >
            <span className="video-trimmer-playhead-head" />
          </div>
        </div>
      </div>
    </section>
  );
}

export function VideoExportBar({
  progress,
  confirmingCancel,
  onRequestCancel,
  onConfirmCancel,
  onContinueExport,
}: VideoExportBarProps) {
  const clamped = Math.max(0, Math.min(1, Number.isFinite(progress.progress) ? progress.progress : 0));
  const pct = progress.status === "complete" ? 100 : Math.round(clamped * 100);
  const isRunning = progress.status === "running";
  const isFailed = progress.status === "failed";
  const isComplete = progress.status === "complete";
  const isCancelled = progress.status === "cancelled";

  let statusText = t("Exporting trimmed video…", "正在导出剪辑视频…");
  if (isComplete) {
    statusText = progress.output_size != null
      ? `${t("Export complete", "导出完成")} · ${decimalSize(progress.output_size)}`
      : t("Export complete", "导出完成");
  } else if (isFailed) {
    statusText = progress.error || t("Export failed", "导出失败");
  } else if (isCancelled) {
    statusText = t("Export cancelled", "已取消导出");
  }

  return (
    <div
      className="video-export-bar"
      data-status={progress.status}
      role={isFailed ? "alert" : "status"}
      aria-live="polite"
    >
      <div className="video-export-summary">
        <span className="video-export-icon" aria-hidden="true">
          {isRunning && <LoaderCircle className="preview-spinner" size={14} />}
          {isComplete && <Check size={14} />}
          {(isFailed || isCancelled) && <X size={14} />}
        </span>
        <span className="video-export-status-text" title={statusText}>{statusText}</span>
        {isRunning && (
          <span className="video-export-percent">{pct}%</span>
        )}
      </div>

      {isRunning && (
        <div
          className="video-export-track"
          role="progressbar"
          aria-label={t("Video export progress", "视频导出进度")}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={pct}
        >
          <div className="video-export-fill" style={{ width: `${pct}%` }} />
        </div>
      )}

      {isRunning && (
        <div className="video-export-actions">
          {confirmingCancel ? (
            <>
              <button
                type="button"
                className="button small video-export-confirm-cancel"
                onClick={onConfirmCancel}
              >
                {t("Confirm cancel", "确认取消导出")}
              </button>
              <button
                type="button"
                className="button subtle small video-export-continue"
                onClick={onContinueExport}
              >
                {t("Continue export", "继续导出")}
              </button>
            </>
          ) : (
            <button
              type="button"
              className="button subtle small video-export-cancel"
              onClick={onRequestCancel}
            >
              {t("Cancel", "取消")}
            </button>
          )}
        </div>
      )}
    </div>
  );
}
