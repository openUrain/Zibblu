import type {
  AnnotationColor,
  ImageAnnotationDoc,
  ImageAnnotationItem,
  ImagePoint,
  PenAnnotation,
  ShapeAnnotation,
  StrokeWidthLevel,
  TextAnnotation,
  ViewTransform,
} from "./media-viewer-types";

export const ANNOTATION_COLORS: readonly AnnotationColor[] = [
  "#E53935",
  "#FB8C00",
  "#FBC02D",
  "#2E7D32",
  "#1E88E5",
  "#634FB4",
  "#1D1D1F",
] as const;

export const STROKE_WIDTH_LEVELS: readonly StrokeWidthLevel[] = ["thin", "medium", "thick"] as const;

export type TextSizeLevel = "small" | "medium" | "large";
export const TEXT_SIZE_LEVELS: readonly TextSizeLevel[] = ["small", "medium", "large"] as const;

export const MAX_EXPORT_BYTES = 16 * 1024 * 1024; // 16 MiB

export type StageRectLike = {
  width: number;
  height: number;
};

export type StageElementRect = StageRectLike & {
  left: number;
  top: number;
};

export type BoundsRect = {
  x: number;
  y: number;
  width: number;
  height: number;
};

export type SelectionHandleKind = "start" | "end" | "nw" | "ne" | "sw" | "se" | "n" | "e" | "s" | "w" | "rotate";

export type SelectionHandle = {
  kind: SelectionHandleKind;
  x: number;
  y: number;
};

export interface AnnotationHistoryState {
  past: ImageAnnotationDoc[];
  present: ImageAnnotationDoc;
  future: ImageAnnotationDoc[];
}

export function createEmptyAnnotationDoc(naturalWidth: number, naturalHeight: number): ImageAnnotationDoc {
  return {
    version: 1,
    naturalWidth,
    naturalHeight,
    items: [],
  };
}

export function createAnnotationId(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }
  return `ann_${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 9)}`;
}

// ---------------------------------------------------------------------------
// Immutable Undo / Redo History
// ---------------------------------------------------------------------------

export function createHistory(initial: ImageAnnotationDoc): AnnotationHistoryState {
  return {
    past: [],
    present: initial,
    future: [],
  };
}

export function pushHistory(
  state: AnnotationHistoryState,
  next: ImageAnnotationDoc,
  maxDepth = 100,
): AnnotationHistoryState {
  if (next === state.present) return state;
  const past = [...state.past, state.present];
  return {
    past: past.length > maxDepth ? past.slice(past.length - maxDepth) : past,
    present: next,
    future: [],
  };
}

export function undoHistory(state: AnnotationHistoryState): AnnotationHistoryState {
  if (state.past.length === 0) return state;
  const previous = state.past[state.past.length - 1];
  return {
    past: state.past.slice(0, -1),
    present: previous,
    future: [state.present, ...state.future],
  };
}

export function redoHistory(state: AnnotationHistoryState): AnnotationHistoryState {
  if (state.future.length === 0) return state;
  const next = state.future[0];
  return {
    past: [...state.past, state.present],
    present: next,
    future: state.future.slice(1),
  };
}

export function canUndoHistory(state: AnnotationHistoryState): boolean {
  return state.past.length > 0;
}

export function canRedoHistory(state: AnnotationHistoryState): boolean {
  return state.future.length > 0;
}

// ---------------------------------------------------------------------------
// Resolution-adaptive sizing
// ---------------------------------------------------------------------------

function imageScaleRef(naturalWidth: number, naturalHeight: number): number {
  const w = Math.max(1, naturalWidth);
  const h = Math.max(1, naturalHeight);
  return Math.sqrt(w * h);
}

export function resolveStrokeWidth(
  level: StrokeWidthLevel,
  naturalWidth: number,
  naturalHeight: number,
): number {
  const ref = imageScaleRef(naturalWidth, naturalHeight);
  switch (level) {
    case "thin":
      return Math.max(1.5, ref * 0.003);
    case "medium":
      return Math.max(3, ref * 0.006);
    case "thick":
      return Math.max(6, ref * 0.012);
  }
}

export function resolveTextFontSize(
  level: TextSizeLevel,
  naturalWidth: number,
  naturalHeight: number,
): number {
  const ref = imageScaleRef(naturalWidth, naturalHeight);
  switch (level) {
    case "small":
      return Math.max(14, Math.round(ref * 0.022));
    case "medium":
      return Math.max(20, Math.round(ref * 0.036));
    case "large":
      return Math.max(28, Math.round(ref * 0.056));
  }
}

export function inferTextSizeLevel(
  fontSize: number,
  naturalWidth: number,
  naturalHeight: number,
): TextSizeLevel {
  const s = resolveTextFontSize("small", naturalWidth, naturalHeight);
  const m = resolveTextFontSize("medium", naturalWidth, naturalHeight);
  const l = resolveTextFontSize("large", naturalWidth, naturalHeight);
  const ds = Math.abs(fontSize - s);
  const dm = Math.abs(fontSize - m);
  const dl = Math.abs(fontSize - l);
  if (ds <= dm && ds <= dl) return "small";
  if (dm <= dl) return "medium";
  return "large";
}

// ---------------------------------------------------------------------------
// Exact Coordinate Transforms matching image-viewer.tsx:
// translate(-50%, -50%) translate(x, y) rotate(turn * 90deg) scale(scale)
// ---------------------------------------------------------------------------

function normalizeTurn(turn: number): 0 | 1 | 2 | 3 {
  const normalized = ((Math.trunc(turn) % 4) + 4) % 4;
  return normalized as 0 | 1 | 2 | 3;
}

function safeScale(scale: number): number {
  return Number.isFinite(scale) && scale > 0 ? scale : 1;
}

export function stageToImagePoint(
  stagePoint: ImagePoint,
  stageRect: StageRectLike,
  viewTransform: ViewTransform,
  naturalWidth: number,
  naturalHeight: number,
): ImagePoint {
  const scale = safeScale(viewTransform.scale);
  const turn = normalizeTurn(viewTransform.turn);
  const cx = stageRect.width / 2 + viewTransform.x;
  const cy = stageRect.height / 2 + viewTransform.y;

  const sx = (stagePoint.x - cx) / scale;
  const sy = (stagePoint.y - cy) / scale;

  let dx: number;
  let dy: number;
  switch (turn) {
    case 0:
      dx = sx;
      dy = sy;
      break;
    case 1:
      dx = sy;
      dy = -sx;
      break;
    case 2:
      dx = -sx;
      dy = -sy;
      break;
    case 3:
      dx = -sy;
      dy = sx;
      break;
  }

  return {
    x: dx + naturalWidth / 2,
    y: dy + naturalHeight / 2,
  };
}

export function clientToImagePoint(
  clientX: number,
  clientY: number,
  stageElementRect: StageElementRect,
  viewTransform: ViewTransform,
  naturalWidth: number,
  naturalHeight: number,
): ImagePoint {
  return stageToImagePoint(
    {
      x: clientX - stageElementRect.left,
      y: clientY - stageElementRect.top,
    },
    stageElementRect,
    viewTransform,
    naturalWidth,
    naturalHeight,
  );
}

export function imageToStagePoint(
  imagePoint: ImagePoint,
  stageRect: StageRectLike,
  viewTransform: ViewTransform,
  naturalWidth: number,
  naturalHeight: number,
): ImagePoint {
  const scale = safeScale(viewTransform.scale);
  const turn = normalizeTurn(viewTransform.turn);
  const cx = stageRect.width / 2 + viewTransform.x;
  const cy = stageRect.height / 2 + viewTransform.y;

  const dx = imagePoint.x - naturalWidth / 2;
  const dy = imagePoint.y - naturalHeight / 2;

  let rx: number;
  let ry: number;
  switch (turn) {
    case 0:
      rx = dx;
      ry = dy;
      break;
    case 1:
      rx = -dy;
      ry = dx;
      break;
    case 2:
      rx = -dx;
      ry = -dy;
      break;
    case 3:
      rx = dy;
      ry = -dx;
      break;
  }

  return {
    x: cx + rx * scale,
    y: cy + ry * scale,
  };
}

export function applyImageTransformToContext(
  ctx: CanvasRenderingContext2D,
  stageRect: StageRectLike,
  viewTransform: ViewTransform,
  naturalWidth: number,
  naturalHeight: number,
  dpr = 1,
): void {
  const scale = safeScale(viewTransform.scale);
  const turn = normalizeTurn(viewTransform.turn);
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.scale(dpr, dpr);
  ctx.translate(stageRect.width / 2 + viewTransform.x, stageRect.height / 2 + viewTransform.y);
  ctx.rotate((turn * Math.PI) / 2);
  ctx.scale(scale, scale);
  ctx.translate(-naturalWidth / 2, -naturalHeight / 2);
}

// ---------------------------------------------------------------------------
// Text Measurement & Geometry / Bounds / Selection Handles
// ---------------------------------------------------------------------------

const TEXT_FONT_FAMILY =
  '-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", sans-serif';

export function buildCanvasFont(fontSize: number, style?: Partial<TextAnnotation>): string {
  return `${style?.italic ? "italic " : ""}${style?.bold ? 700 : style?.width ? 400 : 600} ${fontSize}px ${style?.family ?? TEXT_FONT_FAMILY}`;
}

function estimateTextLineWidth(line: string, fontSize: number): number {
  let units = 0;
  for (const ch of line) {
    const code = ch.codePointAt(0) ?? 0;
    units += code >= 0x2e80 ? 1.0 : 0.6;
  }
  return Math.max(fontSize * 0.5, units * fontSize);
}

export function measureTextBounds(
  item: TextAnnotation,
  ctx?: CanvasRenderingContext2D | null,
): BoundsRect {
  if (item.width && item.height) return { x: item.x, y: item.y, width: item.width, height: item.height };
  const lines = item.text.length > 0 ? item.text.split(/\r?\n/) : [" "];
  const lineHeight = item.fontSize * 1.25;
  let maxWidth = item.fontSize * 0.5;

  if (ctx) {
    ctx.save();
    ctx.font = buildCanvasFont(item.fontSize);
    for (const line of lines) {
      maxWidth = Math.max(maxWidth, ctx.measureText(line || " ").width);
    }
    ctx.restore();
  } else {
    for (const line of lines) {
      maxWidth = Math.max(maxWidth, estimateTextLineWidth(line || " ", item.fontSize));
    }
  }

  return {
    x: item.x,
    y: item.y,
    width: maxWidth,
    height: Math.max(item.fontSize, lines.length * lineHeight),
  };
}

export function getItemBounds(
  item: ImageAnnotationItem,
  naturalWidth: number,
  naturalHeight: number,
  ctx?: CanvasRenderingContext2D | null,
): BoundsRect {
  if (item.kind === "text") {
    return measureTextBounds(item, ctx);
  }
  if (item.kind === "pen") {
    if (item.points.length === 0) {
      return { x: 0, y: 0, width: 0, height: 0 };
    }
    const pad = resolveStrokeWidth(item.widthLevel, naturalWidth, naturalHeight) / 2;
    let minX = Infinity;
    let minY = Infinity;
    let maxX = -Infinity;
    let maxY = -Infinity;
    for (const pt of item.points) {
      if (pt.x < minX) minX = pt.x;
      if (pt.y < minY) minY = pt.y;
      if (pt.x > maxX) maxX = pt.x;
      if (pt.y > maxY) maxY = pt.y;
    }
    return {
      x: minX - pad,
      y: minY - pad,
      width: Math.max(1, maxX - minX + pad * 2),
      height: Math.max(1, maxY - minY + pad * 2),
    };
  }
  const pad = resolveStrokeWidth(item.widthLevel, naturalWidth, naturalHeight) / 2;
  const minX = Math.min(item.x1, item.x2);
  const minY = Math.min(item.y1, item.y2);
  const maxX = Math.max(item.x1, item.x2);
  const maxY = Math.max(item.y1, item.y2);
  return {
    x: minX - pad,
    y: minY - pad,
    width: Math.max(1, maxX - minX + pad * 2),
    height: Math.max(1, maxY - minY + pad * 2),
  };
}

function rotateTextPoint(item: TextAnnotation, point: ImagePoint, inverse = false): ImagePoint {
  const angle = (item.rotation ?? 0) * (inverse ? -1 : 1);
  const sin = Math.sin(angle);
  const cos = Math.cos(angle);
  const x = point.x - item.x;
  const y = point.y - item.y;
  return { x: item.x + x * cos - y * sin, y: item.y + x * sin + y * cos };
}

export function getSelectionHandles(item: ImageAnnotationItem, viewScale = 1): SelectionHandle[] {
  if (item.kind === "text") {
    const { x, y, width, height } = measureTextBounds(item);
    const handles: SelectionHandle[] = [
      { kind: "nw", x, y },
      { kind: "n", x: x + width / 2, y },
      { kind: "ne", x: x + width, y },
      { kind: "e", x: x + width, y: y + height / 2 },
      { kind: "se", x: x + width, y: y + height },
      { kind: "s", x: x + width / 2, y: y + height },
      { kind: "sw", x, y: y + height },
      { kind: "w", x, y: y + height / 2 },
      { kind: "rotate", x: x + width / 2, y: y - 24 / safeScale(viewScale) },
    ];
    return handles.map(handle => ({ ...rotateTextPoint(item, handle), kind: handle.kind }));
  }
  if (item.kind === "line" || item.kind === "arrow") {
    return [
      { kind: "start", x: item.x1, y: item.y1 },
      { kind: "end", x: item.x2, y: item.y2 },
    ];
  }
  if (item.kind === "rect" || item.kind === "ellipse") {
    const left = Math.min(item.x1, item.x2);
    const right = Math.max(item.x1, item.x2);
    const top = Math.min(item.y1, item.y2);
    const bottom = Math.max(item.y1, item.y2);
    return [
      { kind: "nw", x: left, y: top },
      { kind: "ne", x: right, y: top },
      { kind: "sw", x: left, y: bottom },
      { kind: "se", x: right, y: bottom },
    ];
  }
  return [];
}

export function hitTestSelectionHandle(
  item: ImageAnnotationItem,
  point: ImagePoint,
  viewScale: number,
  handleScreenRadius = 10,
): SelectionHandleKind | null {
  const handles = getSelectionHandles(item, viewScale);
  if (handles.length === 0) return null;
  const r = handleScreenRadius / safeScale(viewScale);
  for (const handle of handles) {
    if (Math.hypot(point.x - handle.x, point.y - handle.y) <= r) {
      return handle.kind;
    }
  }
  return null;
}

// ---------------------------------------------------------------------------
// Hit Testing
// ---------------------------------------------------------------------------

function distanceToSegment(p: ImagePoint, a: ImagePoint, b: ImagePoint): number {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const lenSq = dx * dx + dy * dy;
  if (lenSq <= 1e-8) return Math.hypot(p.x - a.x, p.y - a.y);
  const t = Math.max(0, Math.min(1, ((p.x - a.x) * dx + (p.y - a.y) * dy) / lenSq));
  return Math.hypot(p.x - (a.x + t * dx), p.y - (a.y + t * dy));
}

function segmentToSegmentDistance(a: ImagePoint, b: ImagePoint, c: ImagePoint, d: ImagePoint): number {
  // Check endpoint-to-segment distances; also check intersection for crossing strokes.
  const ccw = (p1: ImagePoint, p2: ImagePoint, p3: ImagePoint) =>
    (p3.y - p1.y) * (p2.x - p1.x) > (p2.y - p1.y) * (p3.x - p1.x);
  if (ccw(a, c, d) !== ccw(b, c, d) && ccw(a, b, c) !== ccw(a, b, d)) {
    return 0;
  }
  return Math.min(
    distanceToSegment(a, c, d),
    distanceToSegment(b, c, d),
    distanceToSegment(c, a, b),
    distanceToSegment(d, a, b),
  );
}

function hitTestPen(
  item: PenAnnotation,
  point: ImagePoint,
  tolerance: number,
  prevPoint?: ImagePoint,
): boolean {
  const pts = item.points;
  if (pts.length === 0) return false;
  if (pts.length === 1) {
    return prevPoint
      ? distanceToSegment(pts[0], prevPoint, point) <= tolerance
      : Math.hypot(point.x - pts[0].x, point.y - pts[0].y) <= tolerance;
  }
  for (let i = 0; i < pts.length - 1; i++) {
    const dist = prevPoint
      ? segmentToSegmentDistance(prevPoint, point, pts[i], pts[i + 1])
      : distanceToSegment(point, pts[i], pts[i + 1]);
    if (dist <= tolerance) return true;
  }
  return false;
}

function hitTestShape(
  item: ShapeAnnotation,
  point: ImagePoint,
  tolerance: number,
  prevPoint?: ImagePoint,
): boolean {
  const a = { x: item.x1, y: item.y1 };
  const b = { x: item.x2, y: item.y2 };
  if (item.kind === "line" || item.kind === "arrow") {
    const dist = prevPoint
      ? segmentToSegmentDistance(prevPoint, point, a, b)
      : distanceToSegment(point, a, b);
    return dist <= tolerance;
  }
  const left = Math.min(item.x1, item.x2);
  const right = Math.max(item.x1, item.x2);
  const top = Math.min(item.y1, item.y2);
  const bottom = Math.max(item.y1, item.y2);

  if (item.kind === "rect") {
    const edges: [ImagePoint, ImagePoint][] = [
      [{ x: left, y: top }, { x: right, y: top }],
      [{ x: right, y: top }, { x: right, y: bottom }],
      [{ x: right, y: bottom }, { x: left, y: bottom }],
      [{ x: left, y: bottom }, { x: left, y: top }],
    ];
    for (const [e1, e2] of edges) {
      const dist = prevPoint
        ? segmentToSegmentDistance(prevPoint, point, e1, e2)
        : distanceToSegment(point, e1, e2);
      if (dist <= tolerance) return true;
    }
    return false;
  }

  // Ellipse: approximate perimeter with 32 segments for exact aspect-ratio-independent hit testing.
  const cx = (left + right) / 2;
  const cy = (top + bottom) / 2;
  const rx = Math.max(0.5, (right - left) / 2);
  const ry = Math.max(0.5, (bottom - top) / 2);
  const steps = 32;
  let prev = { x: cx + rx, y: cy };
  for (let i = 1; i <= steps; i++) {
    const angle = (i / steps) * Math.PI * 2;
    const next = { x: cx + Math.cos(angle) * rx, y: cy + Math.sin(angle) * ry };
    const dist = prevPoint
      ? segmentToSegmentDistance(prevPoint, point, prev, next)
      : distanceToSegment(point, prev, next);
    if (dist <= tolerance) return true;
    prev = next;
  }
  return false;
}

export function hitTestItem(
  item: ImageAnnotationItem,
  point: ImagePoint,
  naturalWidth: number,
  naturalHeight: number,
  viewScale: number,
  ctx?: CanvasRenderingContext2D | null,
  prevPoint?: ImagePoint,
): boolean {
  if (item.kind === "pen" && item.erase) return false;
  const screenPad = 10 / safeScale(viewScale);
  if (item.kind === "text") {
    point = rotateTextPoint(item, point, true);
    const previous = prevPoint && rotateTextPoint(item, prevPoint, true);
    const bounds = measureTextBounds(item, ctx);
    const pad = Math.min(screenPad, item.fontSize * 0.35);
    const minX = bounds.x - pad;
    const maxX = bounds.x + bounds.width + pad;
    const minY = bounds.y - pad;
    const maxY = bounds.y + bounds.height + pad;
    if (
      point.x >= minX &&
      point.x <= maxX &&
      point.y >= minY &&
      point.y <= maxY
    ) {
      return true;
    }
    if (previous) {
      const edges: [ImagePoint, ImagePoint][] = [
        [{ x: minX, y: minY }, { x: maxX, y: minY }],
        [{ x: maxX, y: minY }, { x: maxX, y: maxY }],
        [{ x: maxX, y: maxY }, { x: minX, y: maxY }],
        [{ x: minX, y: maxY }, { x: minX, y: minY }],
      ];
      return edges.some(([e1, e2]) => segmentToSegmentDistance(previous, point, e1, e2) === 0);
    }
    return false;
  }

  const strokeHalf = resolveStrokeWidth(item.widthLevel, naturalWidth, naturalHeight) / 2;
  const tolerance = strokeHalf + screenPad;
  if (item.kind === "pen") {
    return hitTestPen(item, point, tolerance, prevPoint);
  }
  return hitTestShape(item, point, tolerance, prevPoint);
}

export function resizeTextByHandle(
  item: TextAnnotation, handle: SelectionHandleKind, point: ImagePoint, start: ImagePoint,
): TextAnnotation {
  const { x, y, width, height } = measureTextBounds(item);
  if (handle === "rotate") {
    const angle = Math.atan2(point.y - y, point.x - x) - Math.atan2(start.y - y, start.x - x);
    return { ...item, rotation: (item.rotation ?? 0) + angle };
  }
  const local = rotateTextPoint(item, point, true);
  const origin = rotateTextPoint(item, start, true);
  const dx = local.x - origin.x;
  const dy = local.y - origin.y;
  const west = handle.includes("w");
  const north = handle.includes("n");
  const dw = west ? -dx : handle.includes("e") ? dx : 0;
  const dh = north ? -dy : handle.includes("s") ? dy : 0;
  let nextWidth = Math.max(item.fontSize, width + dw);
  let nextHeight = Math.max(item.fontSize * 1.25, height + dh);
  let fontSize = item.fontSize;
  if (handle.length === 2) {
    const ratio = Math.max(8 / fontSize, 1 + (dw * width + dh * height) / (width ** 2 + height ** 2));
    nextWidth = width * ratio;
    nextHeight = height * ratio;
    fontSize *= ratio;
  }
  return {
    ...item,
    ...rotateTextPoint(item, { x: west ? x + width - nextWidth : x, y: north ? y + height - nextHeight : y }),
    width: nextWidth,
    height: nextHeight,
    fontSize,
  };
}

export function hitTestAnnotation(
  doc: ImageAnnotationDoc,
  point: ImagePoint,
  viewScale: number,
  ctx?: CanvasRenderingContext2D | null,
): ImageAnnotationItem | null {
  for (let i = doc.items.length - 1; i >= 0; i--) {
    const item = doc.items[i];
    if (hitTestItem(item, point, doc.naturalWidth, doc.naturalHeight, viewScale, ctx)) {
      return item;
    }
  }
  return null;
}

export function findItemsAlongSegment(
  doc: ImageAnnotationDoc,
  from: ImagePoint,
  to: ImagePoint,
  viewScale: number,
  excludeIds?: ReadonlySet<string>,
  ctx?: CanvasRenderingContext2D | null,
): string[] {
  const hits: string[] = [];
  for (let i = doc.items.length - 1; i >= 0; i--) {
    const item = doc.items[i];
    if (excludeIds?.has(item.id)) continue;
    if (hitTestItem(item, to, doc.naturalWidth, doc.naturalHeight, viewScale, ctx, from)) {
      hits.push(item.id);
    }
  }
  return hits;
}

// ---------------------------------------------------------------------------
// Item Mutation Helpers (Move / Resize)
// ---------------------------------------------------------------------------

export function moveAnnotationItem(
  item: ImageAnnotationItem,
  dx: number,
  dy: number,
): ImageAnnotationItem {
  if (item.kind === "pen") {
    return {
      ...item,
      points: item.points.map(pt => ({ x: pt.x + dx, y: pt.y + dy })),
    };
  }
  if (item.kind === "text") {
    return {
      ...item,
      x: item.x + dx,
      y: item.y + dy,
    };
  }
  return {
    ...item,
    x1: item.x1 + dx,
    y1: item.y1 + dy,
    x2: item.x2 + dx,
    y2: item.y2 + dy,
  };
}

export function normalizeRectShape(item: ShapeAnnotation): ShapeAnnotation {
  if (item.kind !== "rect" && item.kind !== "ellipse") return item;
  return {
    ...item,
    x1: Math.min(item.x1, item.x2),
    y1: Math.min(item.y1, item.y2),
    x2: Math.max(item.x1, item.x2),
    y2: Math.max(item.y1, item.y2),
  };
}

export function resizeShapeByHandle(
  initial: ShapeAnnotation,
  handle: SelectionHandleKind,
  point: ImagePoint,
  constrain = false,
): ShapeAnnotation {
  if (initial.kind === "line" || initial.kind === "arrow") {
    if (handle === "start") {
      const target = constrain ? constrainLineEndpoint({ x: initial.x2, y: initial.y2 }, point) : point;
      return { ...initial, x1: target.x, y1: target.y };
    }
    const target = constrain ? constrainLineEndpoint({ x: initial.x1, y: initial.y1 }, point) : point;
    return { ...initial, x2: target.x, y2: target.y };
  }

  const norm = normalizeRectShape(initial);
  let anchorX: number;
  let anchorY: number;
  switch (handle) {
    case "nw":
      anchorX = norm.x2;
      anchorY = norm.y2;
      break;
    case "ne":
      anchorX = norm.x1;
      anchorY = norm.y2;
      break;
    case "sw":
      anchorX = norm.x2;
      anchorY = norm.y1;
      break;
    case "se":
    default:
      anchorX = norm.x1;
      anchorY = norm.y1;
      break;
  }

  const target = constrain ? constrainBoxPoint({ x: anchorX, y: anchorY }, point) : point;
  return {
    ...norm,
    x1: Math.min(anchorX, target.x),
    y1: Math.min(anchorY, target.y),
    x2: Math.max(anchorX, target.x),
    y2: Math.max(anchorY, target.y),
  };
}

export function constrainLineEndpoint(origin: ImagePoint, point: ImagePoint): ImagePoint {
  const dx = point.x - origin.x;
  const dy = point.y - origin.y;
  const len = Math.hypot(dx, dy);
  if (len < 1e-3) return point;
  const snappedAngle = Math.round(Math.atan2(dy, dx) / (Math.PI / 4)) * (Math.PI / 4);
  return {
    x: origin.x + Math.cos(snappedAngle) * len,
    y: origin.y + Math.sin(snappedAngle) * len,
  };
}

export function constrainBoxPoint(origin: ImagePoint, point: ImagePoint): ImagePoint {
  const dx = point.x - origin.x;
  const dy = point.y - origin.y;
  const size = Math.max(Math.abs(dx), Math.abs(dy));
  return {
    x: origin.x + (dx >= 0 ? size : -size),
    y: origin.y + (dy >= 0 ? size : -size),
  };
}

// ---------------------------------------------------------------------------
// Shared Canvas Rendering (Preview Overlay & Offscreen Export)
// ---------------------------------------------------------------------------

function drawPen(ctx: CanvasRenderingContext2D, item: PenAnnotation, lineWidth: number): void {
  const pts = item.points;
  if (pts.length === 0) return;

  ctx.save();
  ctx.strokeStyle = item.color;
  ctx.fillStyle = item.color;
  ctx.lineWidth = lineWidth;
  ctx.lineCap = "round";
  ctx.lineJoin = "round";

  if (pts.length === 1) {
    ctx.beginPath();
    ctx.arc(pts[0].x, pts[0].y, lineWidth / 2, 0, Math.PI * 2);
    ctx.fill();
    ctx.restore();
    return;
  }

  ctx.beginPath();
  ctx.moveTo(pts[0].x, pts[0].y);
  if (pts.length === 2) {
    ctx.lineTo(pts[1].x, pts[1].y);
  } else {
    for (let i = 1; i < pts.length - 1; i++) {
      const midX = (pts[i].x + pts[i + 1].x) / 2;
      const midY = (pts[i].y + pts[i + 1].y) / 2;
      ctx.quadraticCurveTo(pts[i].x, pts[i].y, midX, midY);
    }
    const last = pts[pts.length - 1];
    ctx.lineTo(last.x, last.y);
  }
  ctx.stroke();
  ctx.restore();
}

function drawShape(ctx: CanvasRenderingContext2D, item: ShapeAnnotation, lineWidth: number): void {
  ctx.save();
  ctx.strokeStyle = item.color;
  ctx.fillStyle = item.color;
  ctx.lineWidth = lineWidth;
  ctx.lineCap = "round";
  ctx.lineJoin = "round";

  if (item.kind === "line") {
    ctx.beginPath();
    ctx.moveTo(item.x1, item.y1);
    ctx.lineTo(item.x2, item.y2);
    ctx.stroke();
    ctx.restore();
    return;
  }

  if (item.kind === "arrow") {
    const dx = item.x2 - item.x1;
    const dy = item.y2 - item.y1;
    const len = Math.hypot(dx, dy);
    if (len < 1e-3) {
      ctx.beginPath();
      ctx.arc(item.x1, item.y1, lineWidth / 2, 0, Math.PI * 2);
      ctx.fill();
      ctx.restore();
      return;
    }
    const angle = Math.atan2(dy, dx);
    const headLen = Math.min(len * 0.45, Math.max(lineWidth * 3.6, 12));
    const spread = Math.PI / 7;
    const shaftEndX = item.x2 - Math.cos(angle) * headLen * 0.7;
    const shaftEndY = item.y2 - Math.sin(angle) * headLen * 0.7;

    ctx.beginPath();
    ctx.moveTo(item.x1, item.y1);
    ctx.lineTo(shaftEndX, shaftEndY);
    ctx.stroke();

    ctx.beginPath();
    ctx.moveTo(item.x2, item.y2);
    ctx.lineTo(
      item.x2 - headLen * Math.cos(angle - spread),
      item.y2 - headLen * Math.sin(angle - spread),
    );
    ctx.lineTo(
      item.x2 - headLen * Math.cos(angle + spread),
      item.y2 - headLen * Math.sin(angle + spread),
    );
    ctx.closePath();
    ctx.fill();
    ctx.restore();
    return;
  }

  const left = Math.min(item.x1, item.x2);
  const top = Math.min(item.y1, item.y2);
  const width = Math.abs(item.x2 - item.x1);
  const height = Math.abs(item.y2 - item.y1);

  if (item.kind === "rect") {
    ctx.strokeRect(left, top, width, height);
    ctx.restore();
    return;
  }

  const cx = left + width / 2;
  const cy = top + height / 2;
  ctx.beginPath();
  ctx.ellipse(cx, cy, Math.max(0.5, width / 2), Math.max(0.5, height / 2), 0, 0, Math.PI * 2);
  ctx.stroke();
  ctx.restore();
}

function drawText(ctx: CanvasRenderingContext2D, item: TextAnnotation): void {
  if (!item.text) return;
  ctx.save();
  ctx.fillStyle = item.color;
  ctx.font = buildCanvasFont(item.fontSize, item);
  ctx.textAlign = item.align ?? "left";
  ctx.textBaseline = "top";
  ctx.translate(item.x, item.y);
  ctx.rotate(item.rotation ?? 0);
  if (item.width && item.height) {
    ctx.beginPath();
    ctx.rect(0, 0, item.width, item.height);
    ctx.clip();
  }
  const lines = item.text.split(/\r?\n/).flatMap(line => {
    if (!item.width || ctx.measureText(line).width <= item.width) return [line];
    const wrapped: string[] = [];
    let segment = "";
    for (const word of line.match(/\S+|\s+/gu) ?? []) {
      if (segment && ctx.measureText(segment + word).width > item.width) {
        wrapped.push(segment.trimEnd());
        segment = "";
        if (!word.trim()) continue;
      }
      if (ctx.measureText(word).width <= item.width) {
        segment += word;
      } else {
        for (const char of word) {
          if (segment && ctx.measureText(segment + char).width > item.width) {
            wrapped.push(segment);
            segment = "";
          }
          segment += char;
        }
      }
    }
    wrapped.push(segment);
    return wrapped;
  });
  const lineHeight = item.fontSize * 1.25;
  const x = item.align === "right" ? item.width ?? 0 : item.align === "center" ? (item.width ?? 0) / 2 : 0;
  for (let i = 0; i < lines.length; i++) {
    const y = i * lineHeight;
    if (item.height && y >= item.height) break;
    ctx.fillText(lines[i], x, y);
    if (item.underline) {
      const width = ctx.measureText(lines[i]).width;
      const left = item.align === "right" ? x - width : item.align === "center" ? x - width / 2 : x;
      ctx.fillRect(left, y + item.fontSize, width, Math.max(1, item.fontSize / 16));
    }
  }
  ctx.restore();
}

export function renderAnnotationDoc(
  ctx: CanvasRenderingContext2D,
  doc: ImageAnnotationDoc,
  options?: {
    clipToImage?: boolean;
    hiddenIds?: ReadonlySet<string>;
  },
): void {
  const { naturalWidth, naturalHeight } = doc;
  ctx.save();
  if (options?.clipToImage) {
    ctx.beginPath();
    ctx.rect(0, 0, naturalWidth, naturalHeight);
    ctx.clip();
  }
  for (const item of doc.items) {
    if (options?.hiddenIds?.has(item.id)) continue;
    if (item.kind === "pen") {
      const width = resolveStrokeWidth(item.widthLevel, naturalWidth, naturalHeight);
      if (item.erase) {
        ctx.save();
        ctx.globalCompositeOperation = "destination-out";
        drawPen(ctx, item, width * 3);
        ctx.restore();
      } else {
        drawPen(ctx, item, width);
      }
    } else if (item.kind === "text") {
      drawText(ctx, item);
    } else {
      drawShape(ctx, item, resolveStrokeWidth(item.widthLevel, naturalWidth, naturalHeight));
    }
  }
  ctx.restore();
}

export function renderSelectionChrome(
  ctx: CanvasRenderingContext2D,
  item: ImageAnnotationItem,
  naturalWidth: number,
  naturalHeight: number,
  viewScale: number,
): void {
  const scale = safeScale(viewScale);
  const strokePx = 1.5 / scale;
  const handleRadius = 5 / scale;
  const bounds = getItemBounds(item, naturalWidth, naturalHeight, ctx);

  ctx.save();
  if (item.kind !== "line" && item.kind !== "arrow") {
    ctx.strokeStyle = "#634FB4";
    ctx.lineWidth = strokePx;
    ctx.setLineDash([5 / scale, 4 / scale]);
    ctx.save();
    if (item.kind === "text") {
      ctx.translate(bounds.x, bounds.y);
      ctx.rotate(item.rotation ?? 0);
      ctx.strokeRect(0, 0, bounds.width, bounds.height);
    } else {
      ctx.strokeRect(bounds.x, bounds.y, bounds.width, bounds.height);
    }
    ctx.restore();
    ctx.setLineDash([]);
  }

  const handles = getSelectionHandles(item, viewScale);
  for (const handle of handles) {
    ctx.beginPath();
    ctx.arc(handle.x, handle.y, handleRadius, 0, Math.PI * 2);
    ctx.fillStyle = "#FFFFFF";
    ctx.fill();
    ctx.lineWidth = 2 / scale;
    ctx.strokeStyle = "#634FB4";
    ctx.stroke();
  }
  ctx.restore();
}

// ---------------------------------------------------------------------------
// Offscreen Full-Resolution Export
// ---------------------------------------------------------------------------

function canvasToBytes(
  canvas: HTMLCanvasElement,
  mime: string,
  quality?: number,
): Promise<Uint8Array> {
  return new Promise((resolve, reject) => {
    canvas.toBlob(
      blob => {
        if (!blob) {
          reject(new Error(`无法编码导出图片 (${mime}) / Failed to encode annotated image (${mime})`));
          return;
        }
        blob.arrayBuffer().then(
          buffer => resolve(new Uint8Array(buffer)),
          error => reject(error),
        );
      },
      mime,
      quality,
    );
  });
}

export async function exportAnnotatedImage(
  img: HTMLImageElement,
  doc: ImageAnnotationDoc,
  originalMime?: string,
): Promise<{ bytes: Uint8Array; mime: string; ext: string }> {
  void originalMime;
  const naturalWidth = doc.naturalWidth > 0 ? doc.naturalWidth : img.naturalWidth;
  const naturalHeight = doc.naturalHeight > 0 ? doc.naturalHeight : img.naturalHeight;
  if (naturalWidth <= 0 || naturalHeight <= 0) {
    throw new Error("原图尺寸无效，无法导出标注图片 / Invalid image dimensions for export");
  }

  const canvas = document.createElement("canvas");
  canvas.width = naturalWidth;
  canvas.height = naturalHeight;
  const ctx = canvas.getContext("2d");
  if (!ctx) {
    throw new Error("无法创建离屏画布上下文 / Failed to acquire 2D canvas context");
  }

  ctx.drawImage(img, 0, 0, naturalWidth, naturalHeight);
  if (doc.items.some(item => item.kind === "pen" && item.erase)) {
    const overlay = document.createElement("canvas");
    overlay.width = naturalWidth;
    overlay.height = naturalHeight;
    const overlayContext = overlay.getContext("2d");
    if (!overlayContext) throw new Error("无法创建标注图层 / Failed to create annotation layer");
    renderAnnotationDoc(overlayContext, { ...doc, naturalWidth, naturalHeight });
    ctx.drawImage(overlay, 0, 0);
  } else {
    renderAnnotationDoc(ctx, { ...doc, naturalWidth, naturalHeight });
  }

  const pngBytes = await canvasToBytes(canvas, "image/png");
  if (pngBytes.byteLength <= MAX_EXPORT_BYTES) {
    return { bytes: pngBytes, mime: "image/png", ext: "png" };
  }

  // JPEG has no alpha channel; fill white underneath transparent pixels before encoding.
  ctx.save();
  ctx.globalCompositeOperation = "destination-over";
  ctx.fillStyle = "#FFFFFF";
  ctx.fillRect(0, 0, naturalWidth, naturalHeight);
  ctx.restore();

  const qualities = [0.92, 0.85, 0.78, 0.7, 0.6];
  for (const quality of qualities) {
    const jpegBytes = await canvasToBytes(canvas, "image/jpeg", quality);
    if (jpegBytes.byteLength <= MAX_EXPORT_BYTES) {
      return { bytes: jpegBytes, mime: "image/jpeg", ext: "jpg" };
    }
  }

  throw new Error(
    "导出标注图片超过 16 MiB 上限，请减少标注或裁剪分辨率 / Annotated image exceeds 16 MiB limit",
  );
}
