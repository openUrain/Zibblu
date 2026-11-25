import type { VideoTrimRange } from "./media-viewer-types";

export const MIN_TRIM_DURATION_SEC = 0.2;
const RANGE_EPSILON = 1e-4;
const MAX_HISTORY_ENTRIES = 100;

export interface TrimHistoryState {
  past: VideoTrimRange[];
  present: VideoTrimRange;
  future: VideoTrimRange[];
}

export function clampTrimRange(
  range: Pick<VideoTrimRange, "startSec" | "endSec" | "durationSec">,
  anchor?: "start" | "end",
): VideoTrimRange {
  const durationSec = Number.isFinite(range.durationSec) && range.durationSec > 0
    ? range.durationSec
    : 0;

  if (durationSec <= 0) {
    return { startSec: 0, endSec: 0, durationSec: 0 };
  }
  if (durationSec <= MIN_TRIM_DURATION_SEC) {
    return { startSec: 0, endSec: durationSec, durationSec };
  }

  let startSec = Number.isFinite(range.startSec) ? range.startSec : 0;
  let endSec = Number.isFinite(range.endSec) ? range.endSec : durationSec;

  startSec = Math.max(0, Math.min(durationSec - MIN_TRIM_DURATION_SEC, startSec));
  endSec = Math.max(MIN_TRIM_DURATION_SEC, Math.min(durationSec, endSec));

  if (endSec - startSec < MIN_TRIM_DURATION_SEC) {
    if (anchor === "start") {
      startSec = Math.max(0, endSec - MIN_TRIM_DURATION_SEC);
    } else if (anchor === "end") {
      endSec = Math.min(durationSec, startSec + MIN_TRIM_DURATION_SEC);
    } else {
      endSec = Math.min(durationSec, startSec + MIN_TRIM_DURATION_SEC);
      if (endSec - startSec < MIN_TRIM_DURATION_SEC) {
        startSec = Math.max(0, endSec - MIN_TRIM_DURATION_SEC);
      }
    }
  }

  return { startSec, endSec, durationSec };
}

export function stepFrame(
  sec: number,
  fps: number,
  dir: -1 | 1,
  minSec = 0,
  maxSec = Number.POSITIVE_INFINITY,
): number {
  const safeFps = Number.isFinite(fps) && fps > 0 ? fps : 30;
  const current = Number.isFinite(sec) ? sec : 0;
  const stepped = current + dir / safeFps;
  return Math.max(minSec, Math.min(maxSec, stepped));
}

export function formatTrimTime(sec: number): string {
  if (!Number.isFinite(sec) || sec <= 0) {
    return "00:00.0";
  }
  const totalTenths = Math.round(sec * 10);
  const tenths = totalTenths % 10;
  const totalSeconds = Math.floor(totalTenths / 10);
  const seconds = totalSeconds % 60;
  const minutes = Math.floor(totalSeconds / 60);
  return `${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}.${tenths}`;
}

export function trimRangesEqual(a: VideoTrimRange, b: VideoTrimRange): boolean {
  return Math.abs(a.startSec - b.startSec) < RANGE_EPSILON
    && Math.abs(a.endSec - b.endSec) < RANGE_EPSILON
    && Math.abs(a.durationSec - b.durationSec) < RANGE_EPSILON;
}

export function createTrimHistory(initial: VideoTrimRange): TrimHistoryState {
  return {
    past: [],
    present: clampTrimRange(initial),
    future: [],
  };
}

export function commitTrimHistory(
  state: TrimHistoryState,
  next: VideoTrimRange,
): TrimHistoryState {
  const normalized = clampTrimRange(next);
  if (trimRangesEqual(state.present, normalized)) {
    return state;
  }
  const nextPast = [...state.past, state.present];
  if (nextPast.length > MAX_HISTORY_ENTRIES) {
    nextPast.shift();
  }
  return {
    past: nextPast,
    present: normalized,
    future: [],
  };
}

export function undoTrimHistory(state: TrimHistoryState): TrimHistoryState {
  if (state.past.length === 0) return state;
  const previous = state.past[state.past.length - 1];
  return {
    past: state.past.slice(0, -1),
    present: previous,
    future: [state.present, ...state.future],
  };
}

export function redoTrimHistory(state: TrimHistoryState): TrimHistoryState {
  if (state.future.length === 0) return state;
  const [next, ...rest] = state.future;
  return {
    past: [...state.past, state.present],
    present: next,
    future: rest,
  };
}

export function canUndoTrim(state: TrimHistoryState): boolean {
  return state.past.length > 0;
}

export function canRedoTrim(state: TrimHistoryState): boolean {
  return state.future.length > 0;
}
