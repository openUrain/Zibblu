import {
  ArrowUpRight,
  Check,
  Circle,
  Eraser,
  Minus,
  MousePointer2,
  Pen,
  Redo2,
  Square,
  Type,
  Undo2,
  X,
} from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { KeyboardEvent, PointerEvent, ReactNode } from "react";
import {
  ANNOTATION_COLORS,
  STROKE_WIDTH_LEVELS,
  TEXT_SIZE_LEVELS,
  applyImageTransformToContext,
  canRedoHistory,
  canUndoHistory,
  clientToImagePoint,
  constrainBoxPoint,
  constrainLineEndpoint,
  createAnnotationId,
  createHistory,
  findItemsAlongSegment,
  hitTestAnnotation,
  hitTestSelectionHandle,
  imageToStagePoint,
  inferTextSizeLevel,
  moveAnnotationItem,
  pushHistory,
  redoHistory,
  renderAnnotationDoc,
  renderSelectionChrome,
  resizeShapeByHandle,
  resolveTextFontSize,
  undoHistory,
} from "./image-annotator";
import type { AnnotationHistoryState, SelectionHandleKind, TextSizeLevel } from "./image-annotator";
import { t } from "./locale";
import type {
  AnnotationColor,
  AnnotationTool,
  ImageAnnotationDoc,
  ImageAnnotationItem,
  ImageAnnotatorProps,
  ImagePoint,
  PenAnnotation,
  ShapeAnnotation,
  StrokeWidthLevel,
  TextAnnotation,
  ViewTransform,
} from "./media-viewer-types";
import "./image-annotator.css";

type OverlayProps = {
  naturalWidth: number;
  naturalHeight: number;
  viewTransform: ViewTransform;
  stageRect: { width: number; height: number };
  doc: ImageAnnotationDoc;
};

type DragSession =
  | {
      kind: "pen";
      item: PenAnnotation;
      pointerId: number;
    }
  | {
      kind: "shape";
      item: ShapeAnnotation;
      origin: ImagePoint;
      pointerId: number;
    }
  | {
      kind: "move";
      initialItem: ImageAnnotationItem;
      currentItem: ImageAnnotationItem;
      startPoint: ImagePoint;
      moved: boolean;
      pointerId: number;
    }
  | {
      kind: "resize";
      initialItem: ShapeAnnotation;
      currentItem: ShapeAnnotation;
      handle: SelectionHandleKind;
      pointerId: number;
    }
  | {
      kind: "eraser";
      lastPoint: ImagePoint;
      erasedIds: Set<string>;
      pointerId: number;
    };

type TextEditorState = {
  targetId: string | null; // null = new text item, string = editing existing text
  x: number;
  y: number;
  color: AnnotationColor;
  fontSize: number;
  value: string;
};

const COLOR_LABELS: Record<AnnotationColor, [string, string]> = {
  "#E53935": ["Red", "红色"],
  "#FB8C00": ["Orange", "橙色"],
  "#FBC02D": ["Yellow", "黄色"],
  "#2E7D32": ["Green", "绿色"],
  "#1E88E5": ["Blue", "蓝色"],
  "#634FB4": ["Lavender", "薰衣草紫"],
  "#1D1D1F": ["Ink", "炭黑"],
};

const STROKE_LABELS: Record<StrokeWidthLevel, [string, string]> = {
  thin: ["Thin stroke", "细线条"],
  medium: ["Medium stroke", "中线条"],
  thick: ["Thick stroke", "粗线条"],
};

const TEXT_SIZE_SHORT: Record<TextSizeLevel, [string, string]> = {
  small: ["S", "小"],
  medium: ["M", "中"],
  large: ["L", "大"],
};

const TEXT_SIZE_LABELS: Record<TextSizeLevel, [string, string]> = {
  small: ["Small text", "小号文字"],
  medium: ["Medium text", "中号文字"],
  large: ["Large text", "大号文字"],
};

function normalizeTurn(turn: number): number {
  return ((Math.trunc(turn) % 4) + 4) % 4;
}

function isEditableFocus(target: EventTarget | null): boolean {
  return (
    target instanceof HTMLInputElement ||
    target instanceof HTMLTextAreaElement ||
    (target instanceof HTMLElement && target.isContentEditable)
  );
}

/**
 * Read-only annotation overlay synced to the viewer's live transform & stage size.
 */
export function ImageAnnotationOverlay({
  naturalWidth,
  naturalHeight,
  viewTransform,
  stageRect,
  doc,
}: OverlayProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useLayoutEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || stageRect.width <= 0 || stageRect.height <= 0) return;
    if (naturalWidth <= 0 || naturalHeight <= 0) return;

    const dpr = Math.max(1, globalThis.devicePixelRatio || 1);
    const targetW = Math.round(stageRect.width * dpr);
    const targetH = Math.round(stageRect.height * dpr);
    if (canvas.width !== targetW) canvas.width = targetW;
    if (canvas.height !== targetH) canvas.height = targetH;

    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, canvas.width, canvas.height);

    if (doc.items.length === 0) return;

    applyImageTransformToContext(ctx, stageRect, viewTransform, naturalWidth, naturalHeight, dpr);
    renderAnnotationDoc(ctx, { ...doc, naturalWidth, naturalHeight }, { clipToImage: true });
  }, [doc, naturalHeight, naturalWidth, stageRect, viewTransform]);

  return <canvas ref={canvasRef} className="image-annotation-overlay" aria-hidden />;
}

/**
 * Non-destructive image annotation editor with floating toolbar & history.
 */
export function ImageAnnotator({
  imageElement,
  naturalWidth,
  naturalHeight,
  viewTransform,
  stageRect,
  editing,
  doc,
  onChange,
  onComplete,
  onRequestExit,
}: ImageAnnotatorProps) {
  void imageElement;
  const rootRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const textInputRef = useRef<HTMLTextAreaElement>(null);

  const [tool, setTool] = useState<AnnotationTool>("pen");
  const [color, setColor] = useState<AnnotationColor>("#E53935");
  const [widthLevel, setWidthLevel] = useState<StrokeWidthLevel>("medium");
  const [textSize, setTextSize] = useState<TextSizeLevel>("medium");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [spacePressed, setSpacePressed] = useState(false);
  const [hoverHandle, setHoverHandle] = useState<SelectionHandleKind | null>(null);
  const [textEditor, setTextEditor] = useState<TextEditorState | null>(null);
  const [dragTick, setDragTick] = useState(0);

  const dragRef = useRef<DragSession | null>(null);
  const historyRef = useRef<AnnotationHistoryState>(createHistory(doc));
  const lastEmittedDocRef = useRef<ImageAnnotationDoc>(doc);

  // Sync external doc resets without breaking internal undo history.
  if (doc !== lastEmittedDocRef.current && doc !== historyRef.current.present) {
    historyRef.current = createHistory(doc);
    lastEmittedDocRef.current = doc;
  }

  const [, setHistoryVersion] = useState(0);

  const commitDoc = useCallback(
    (nextDoc: ImageAnnotationDoc) => {
      historyRef.current = pushHistory(historyRef.current, nextDoc);
      lastEmittedDocRef.current = nextDoc;
      setHistoryVersion(v => v + 1);
      onChange(nextDoc);
    },
    [onChange],
  );

  const performUndo = useCallback(() => {
    const nextHistory = undoHistory(historyRef.current);
    if (nextHistory === historyRef.current) return;
    historyRef.current = nextHistory;
    lastEmittedDocRef.current = nextHistory.present;
    setHistoryVersion(v => v + 1);
    setSelectedId(prev =>
      prev && nextHistory.present.items.some(item => item.id === prev) ? prev : null,
    );
    onChange(nextHistory.present);
  }, [onChange]);

  const performRedo = useCallback(() => {
    const nextHistory = redoHistory(historyRef.current);
    if (nextHistory === historyRef.current) return;
    historyRef.current = nextHistory;
    lastEmittedDocRef.current = nextHistory.present;
    setHistoryVersion(v => v + 1);
    setSelectedId(prev =>
      prev && nextHistory.present.items.some(item => item.id === prev) ? prev : null,
    );
    onChange(nextHistory.present);
  }, [onChange]);

  const deleteSelected = useCallback(() => {
    if (!selectedId) return;
    const nextItems = doc.items.filter(item => item.id !== selectedId);
    if (nextItems.length === doc.items.length) return;
    setSelectedId(null);
    commitDoc({ ...doc, items: nextItems });
  }, [commitDoc, doc, selectedId]);

  const commitTextEditor = useCallback(() => {
    if (!textEditor) return;
    const trimmed = textEditor.value.replace(/\s+$/g, "");
    if (textEditor.targetId) {
      const existing = doc.items.find(
        (item): item is TextAnnotation => item.id === textEditor.targetId && item.kind === "text",
      );
      setTextEditor(null);
      if (!existing) return;
      if (!trimmed) {
        setSelectedId(null);
        commitDoc({
          ...doc,
          items: doc.items.filter(item => item.id !== existing.id),
        });
        return;
      }
      if (
        existing.text === trimmed &&
        existing.color === textEditor.color &&
        existing.fontSize === textEditor.fontSize
      ) {
        return;
      }
      commitDoc({
        ...doc,
        items: doc.items.map(item =>
          item.id === existing.id
            ? {
                ...existing,
                text: trimmed,
                color: textEditor.color,
                fontSize: textEditor.fontSize,
              }
            : item,
        ),
      });
      return;
    }

    setTextEditor(null);
    if (!trimmed) return;
    const newItem: TextAnnotation = {
      id: createAnnotationId(),
      kind: "text",
      color: textEditor.color,
      fontSize: textEditor.fontSize,
      x: textEditor.x,
      y: textEditor.y,
      text: trimmed,
    };
    setSelectedId(newItem.id);
    commitDoc({
      ...doc,
      items: [...doc.items, newItem],
    });
  }, [commitDoc, doc, textEditor]);

  // Reset transient editing states when leaving edit mode.
  useEffect(() => {
    if (!editing) {
      dragRef.current = null;
      setTextEditor(null);
      setSelectedId(null);
      setSpacePressed(false);
    }
  }, [editing]);

  // Auto-focus inline text input when opened.
  useEffect(() => {
    if (textEditor && textInputRef.current) {
      const el = textInputRef.current;
      el.focus({ preventScroll: true });
      const end = el.value.length;
      el.setSelectionRange(end, end);
    }
  }, [textEditor?.targetId, textEditor?.x, textEditor?.y]);

  // Global keyboard support while editing (Space panning, Undo/Redo, Delete, Escape).
  useEffect(() => {
    if (!editing) return;

    function onKeyDown(event: globalThis.KeyboardEvent) {
      if (event.code === "Space" && !isEditableFocus(event.target)) {
        event.preventDefault();
        setSpacePressed(true);
        return;
      }

      if (isEditableFocus(event.target)) return;

      const mod = event.ctrlKey || event.metaKey;
      const key = event.key.toLowerCase();

      if (mod && key === "z" && !event.shiftKey) {
        event.preventDefault();
        event.stopPropagation();
        performUndo();
        return;
      }
      if ((mod && key === "y") || (mod && key === "z" && event.shiftKey)) {
        event.preventDefault();
        event.stopPropagation();
        performRedo();
        return;
      }
      if ((event.key === "Delete" || event.key === "Backspace") && selectedId) {
        event.preventDefault();
        event.stopPropagation();
        deleteSelected();
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        if (selectedId) {
          setSelectedId(null);
        } else {
          onRequestExit();
        }
      }
    }

    function onKeyUp(event: globalThis.KeyboardEvent) {
      if (event.code === "Space") {
        setSpacePressed(false);
      }
    }

    function onBlur() {
      setSpacePressed(false);
    }

    document.addEventListener("keydown", onKeyDown, true);
    document.addEventListener("keyup", onKeyUp);
    window.addEventListener("blur", onBlur);
    return () => {
      document.removeEventListener("keydown", onKeyDown, true);
      document.removeEventListener("keyup", onKeyUp);
      window.removeEventListener("blur", onBlur);
    };
  }, [deleteSelected, editing, onRequestExit, performRedo, performUndo, selectedId]);

  const selectedItem = useMemo(
    () => (selectedId ? doc.items.find(item => item.id === selectedId) ?? null : null),
    [doc.items, selectedId],
  );

  // Active style readout reflects selected item or inline text editor when applicable.
  const activeColor: AnnotationColor = textEditor
    ? textEditor.color
    : selectedItem
      ? selectedItem.color
      : color;

  const isTextStyleMode =
    Boolean(textEditor) ||
    (tool === "text" && (!selectedItem || selectedItem.kind === "text")) ||
    selectedItem?.kind === "text";

  const activeWidthLevel: StrokeWidthLevel =
    selectedItem && selectedItem.kind !== "text" ? selectedItem.widthLevel : widthLevel;

  const activeTextSize: TextSizeLevel = textEditor
    ? inferTextSizeLevel(textEditor.fontSize, naturalWidth, naturalHeight)
    : selectedItem?.kind === "text"
      ? inferTextSizeLevel(selectedItem.fontSize, naturalWidth, naturalHeight)
      : textSize;

  // Render live editing canvas (committed doc + in-flight draft + selection chrome).
  useLayoutEffect(() => {
    if (!editing) return;
    const canvas = canvasRef.current;
    if (!canvas || stageRect.width <= 0 || stageRect.height <= 0) return;
    if (naturalWidth <= 0 || naturalHeight <= 0) return;

    const dpr = Math.max(1, globalThis.devicePixelRatio || 1);
    const targetW = Math.round(stageRect.width * dpr);
    const targetH = Math.round(stageRect.height * dpr);
    if (canvas.width !== targetW) canvas.width = targetW;
    if (canvas.height !== targetH) canvas.height = targetH;

    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, canvas.width, canvas.height);

    const drag = dragRef.current;
    const hiddenIds = new Set<string>();
    if (textEditor?.targetId) hiddenIds.add(textEditor.targetId);
    if (drag?.kind === "move" || drag?.kind === "resize") hiddenIds.add(drag.initialItem.id);
    if (drag?.kind === "eraser") {
      for (const id of drag.erasedIds) hiddenIds.add(id);
    }

    const extraItems: ImageAnnotationItem[] = [];
    if (drag?.kind === "pen" || drag?.kind === "shape") {
      extraItems.push(drag.item);
    } else if (drag?.kind === "move" || drag?.kind === "resize") {
      extraItems.push(drag.currentItem);
    }

    applyImageTransformToContext(ctx, stageRect, viewTransform, naturalWidth, naturalHeight, dpr);

    const renderDoc: ImageAnnotationDoc = {
      version: 1,
      naturalWidth,
      naturalHeight,
      items: extraItems.length > 0 ? [...doc.items, ...extraItems] : doc.items,
    };
    renderAnnotationDoc(ctx, renderDoc, { clipToImage: true, hiddenIds });

    // Selection outline & resize handles sit outside the image clip so edge handles stay crisp.
    const activeSelection =
      drag?.kind === "move" || drag?.kind === "resize"
        ? drag.currentItem
        : selectedItem && !hiddenIds.has(selectedItem.id)
          ? selectedItem
          : null;

    if (activeSelection) {
      renderSelectionChrome(ctx, activeSelection, naturalWidth, naturalHeight, viewTransform.scale);
    }
  }, [
    doc,
    dragTick,
    editing,
    naturalHeight,
    naturalWidth,
    selectedItem,
    stageRect,
    textEditor?.targetId,
    viewTransform,
  ]);

  if (!editing) {
    return (
      <ImageAnnotationOverlay
        naturalWidth={naturalWidth}
        naturalHeight={naturalHeight}
        viewTransform={viewTransform}
        stageRect={stageRect}
        doc={doc}
      />
    );
  }

  function pointFromPointer(event: PointerEvent<HTMLCanvasElement>): ImagePoint | null {
    const canvas = canvasRef.current;
    if (!canvas) return null;
    const rect = canvas.getBoundingClientRect();
    return clientToImagePoint(
      event.clientX,
      event.clientY,
      {
        left: rect.left,
        top: rect.top,
        width: stageRect.width || rect.width,
        height: stageRect.height || rect.height,
      },
      viewTransform,
      naturalWidth,
      naturalHeight,
    );
  }

  function handlePointerDown(event: PointerEvent<HTMLCanvasElement>) {
    // Allow Space+left drag or middle-button drag to fall through to viewer panning.
    if (spacePressed || event.button === 1) return;
    if (event.button !== 0) return;

    event.stopPropagation();

    if (textEditor) {
      commitTextEditor();
      return;
    }

    const pt = pointFromPointer(event);
    if (!pt) return;

    const ctx = canvasRef.current?.getContext("2d");

    if (tool === "select") {
      if (selectedItem) {
        const handle = hitTestSelectionHandle(selectedItem, pt, viewTransform.scale);
        if (handle && selectedItem.kind !== "pen" && selectedItem.kind !== "text") {
          event.currentTarget.setPointerCapture(event.pointerId);
          dragRef.current = {
            kind: "resize",
            initialItem: selectedItem,
            currentItem: selectedItem,
            handle,
            pointerId: event.pointerId,
          };
          setDragTick(v => v + 1);
          return;
        }
      }

      const hit = hitTestAnnotation(doc, pt, viewTransform.scale, ctx);
      if (hit) {
        setSelectedId(hit.id);
        event.currentTarget.setPointerCapture(event.pointerId);
        dragRef.current = {
          kind: "move",
          initialItem: hit,
          currentItem: hit,
          startPoint: pt,
          moved: false,
          pointerId: event.pointerId,
        };
        setDragTick(v => v + 1);
      } else {
        setSelectedId(null);
      }
      return;
    }

    if (tool === "text") {
      const hit = hitTestAnnotation(doc, pt, viewTransform.scale, ctx);
      if (hit && hit.kind === "text") {
        setSelectedId(hit.id);
        setTextEditor({
          targetId: hit.id,
          x: hit.x,
          y: hit.y,
          color: hit.color,
          fontSize: hit.fontSize,
          value: hit.text,
        });
        return;
      }
      setSelectedId(null);
      setTextEditor({
        targetId: null,
        x: pt.x,
        y: pt.y,
        color,
        fontSize: resolveTextFontSize(textSize, naturalWidth, naturalHeight),
        value: "",
      });
      return;
    }

    if (tool === "eraser") {
      setSelectedId(null);
      const hits = findItemsAlongSegment(doc, pt, pt, viewTransform.scale, undefined, ctx);
      const erasedIds = new Set(hits);
      event.currentTarget.setPointerCapture(event.pointerId);
      dragRef.current = {
        kind: "eraser",
        lastPoint: pt,
        erasedIds,
        pointerId: event.pointerId,
      };
      setDragTick(v => v + 1);
      return;
    }

    if (tool === "pen") {
      setSelectedId(null);
      const item: PenAnnotation = {
        id: createAnnotationId(),
        kind: "pen",
        color,
        widthLevel,
        points: [pt],
      };
      event.currentTarget.setPointerCapture(event.pointerId);
      dragRef.current = { kind: "pen", item, pointerId: event.pointerId };
      setDragTick(v => v + 1);
      return;
    }

    // Shape tools: line, arrow, rect, ellipse
    setSelectedId(null);
    const shape: ShapeAnnotation = {
      id: createAnnotationId(),
      kind: tool,
      color,
      widthLevel,
      x1: pt.x,
      y1: pt.y,
      x2: pt.x,
      y2: pt.y,
    };
    event.currentTarget.setPointerCapture(event.pointerId);
    dragRef.current = { kind: "shape", item: shape, origin: pt, pointerId: event.pointerId };
    setDragTick(v => v + 1);
  }

  function handlePointerMove(event: PointerEvent<HTMLCanvasElement>) {
    const pt = pointFromPointer(event);
    if (!pt) return;

    const drag = dragRef.current;
    if (!drag) {
      if (tool === "select" && selectedItem && !spacePressed) {
        const handle = hitTestSelectionHandle(selectedItem, pt, viewTransform.scale);
        setHoverHandle(handle);
      } else if (hoverHandle !== null) {
        setHoverHandle(null);
      }
      return;
    }

    if (drag.pointerId !== event.pointerId) return;
    event.stopPropagation();

    if (drag.kind === "pen") {
      const last = drag.item.points[drag.item.points.length - 1];
      const minStep = 1.5 / (viewTransform.scale > 0 ? viewTransform.scale : 1);
      if (last && Math.hypot(pt.x - last.x, pt.y - last.y) < minStep) return;
      drag.item = {
        ...drag.item,
        points: [...drag.item.points, pt],
      };
      setDragTick(v => v + 1);
      return;
    }

    if (drag.kind === "shape") {
      let target = pt;
      if (event.shiftKey) {
        target =
          drag.item.kind === "line" || drag.item.kind === "arrow"
            ? constrainLineEndpoint(drag.origin, pt)
            : constrainBoxPoint(drag.origin, pt);
      }
      drag.item = { ...drag.item, x2: target.x, y2: target.y };
      setDragTick(v => v + 1);
      return;
    }

    if (drag.kind === "move") {
      const dx = pt.x - drag.startPoint.x;
      const dy = pt.y - drag.startPoint.y;
      const threshold = 1 / (viewTransform.scale > 0 ? viewTransform.scale : 1);
      if (!drag.moved && Math.hypot(dx, dy) < threshold) return;
      drag.moved = true;
      drag.currentItem = moveAnnotationItem(drag.initialItem, dx, dy);
      setDragTick(v => v + 1);
      return;
    }

    if (drag.kind === "resize") {
      drag.currentItem = resizeShapeByHandle(drag.initialItem, drag.handle, pt, event.shiftKey);
      setDragTick(v => v + 1);
      return;
    }

    if (drag.kind === "eraser") {
      const ctx = canvasRef.current?.getContext("2d");
      const newHits = findItemsAlongSegment(
        doc,
        drag.lastPoint,
        pt,
        viewTransform.scale,
        drag.erasedIds,
        ctx,
      );
      drag.lastPoint = pt;
      if (newHits.length > 0) {
        for (const id of newHits) drag.erasedIds.add(id);
        setDragTick(v => v + 1);
      }
    }
  }

  function finishDrag(pointerId?: number, cancelled = false) {
    const drag = dragRef.current;
    if (!drag) return;
    if (pointerId !== undefined && drag.pointerId !== pointerId) return;
    dragRef.current = null;
    setDragTick(v => v + 1);

    if (cancelled) return;

    if (drag.kind === "pen") {
      commitDoc({ ...doc, items: [...doc.items, drag.item] });
      return;
    }

    if (drag.kind === "shape") {
      const minLen = 2 / (viewTransform.scale > 0 ? viewTransform.scale : 1);
      if (Math.hypot(drag.item.x2 - drag.item.x1, drag.item.y2 - drag.item.y1) < minLen) {
        return;
      }
      const finalShape =
        drag.item.kind === "rect" || drag.item.kind === "ellipse"
          ? {
              ...drag.item,
              x1: Math.min(drag.item.x1, drag.item.x2),
              y1: Math.min(drag.item.y1, drag.item.y2),
              x2: Math.max(drag.item.x1, drag.item.x2),
              y2: Math.max(drag.item.y1, drag.item.y2),
            }
          : drag.item;
      commitDoc({ ...doc, items: [...doc.items, finalShape] });
      return;
    }

    if (drag.kind === "move") {
      if (!drag.moved) return;
      commitDoc({
        ...doc,
        items: doc.items.map(item => (item.id === drag.initialItem.id ? drag.currentItem : item)),
      });
      return;
    }

    if (drag.kind === "resize") {
      const a = drag.initialItem;
      const b = drag.currentItem;
      if (a.x1 === b.x1 && a.y1 === b.y1 && a.x2 === b.x2 && a.y2 === b.y2) return;
      commitDoc({
        ...doc,
        items: doc.items.map(item => (item.id === a.id ? b : item)),
      });
      return;
    }

    if (drag.kind === "eraser") {
      if (drag.erasedIds.size === 0) return;
      commitDoc({
        ...doc,
        items: doc.items.filter(item => !drag.erasedIds.has(item.id)),
      });
    }
  }

  function handleDoubleClick(event: PointerEvent<HTMLCanvasElement> | React.MouseEvent<HTMLCanvasElement>) {
    if (spacePressed) return;
    event.stopPropagation();

    const canvas = canvasRef.current;
    if (!canvas) return;
    const rect = canvas.getBoundingClientRect();
    const pt = clientToImagePoint(
      event.clientX,
      event.clientY,
      {
        left: rect.left,
        top: rect.top,
        width: stageRect.width || rect.width,
        height: stageRect.height || rect.height,
      },
      viewTransform,
      naturalWidth,
      naturalHeight,
    );
    const hit = hitTestAnnotation(doc, pt, viewTransform.scale, canvas.getContext("2d"));
    if (hit && hit.kind === "text") {
      setSelectedId(hit.id);
      setTextEditor({
        targetId: hit.id,
        x: hit.x,
        y: hit.y,
        color: hit.color,
        fontSize: hit.fontSize,
        value: hit.text,
      });
    }
  }

  function selectTool(nextTool: AnnotationTool) {
    if (textEditor) commitTextEditor();
    setTool(nextTool);
    if (nextTool !== "select") {
      setSelectedId(null);
    }
  }

  function handleColorPick(nextColor: AnnotationColor) {
    setColor(nextColor);
    if (textEditor) {
      setTextEditor({ ...textEditor, color: nextColor });
      return;
    }
    if (selectedItem && selectedItem.color !== nextColor) {
      commitDoc({
        ...doc,
        items: doc.items.map(item =>
          item.id === selectedItem.id ? { ...item, color: nextColor } : item,
        ),
      });
    }
  }

  function handleStrokeWidthPick(nextLevel: StrokeWidthLevel) {
    setWidthLevel(nextLevel);
    if (selectedItem && selectedItem.kind !== "text" && selectedItem.widthLevel !== nextLevel) {
      commitDoc({
        ...doc,
        items: doc.items.map(item =>
          item.id === selectedItem.id && item.kind !== "text"
            ? { ...item, widthLevel: nextLevel }
            : item,
        ),
      });
    }
  }

  function handleTextSizePick(nextSize: TextSizeLevel) {
    setTextSize(nextSize);
    const px = resolveTextFontSize(nextSize, naturalWidth, naturalHeight);
    if (textEditor) {
      setTextEditor({ ...textEditor, fontSize: px });
      return;
    }
    if (selectedItem?.kind === "text" && selectedItem.fontSize !== px) {
      commitDoc({
        ...doc,
        items: doc.items.map(item =>
          item.id === selectedItem.id && item.kind === "text" ? { ...item, fontSize: px } : item,
        ),
      });
    }
  }

  function handleDone() {
    if (textEditor) {
      // Compute latest doc synchronously if text editor is still open.
      const trimmed = textEditor.value.replace(/\s+$/g, "");
      let finalDoc = doc;
      if (textEditor.targetId) {
        finalDoc = trimmed
          ? {
              ...doc,
              items: doc.items.map(item =>
                item.id === textEditor.targetId && item.kind === "text"
                  ? {
                      ...item,
                      text: trimmed,
                      color: textEditor.color,
                      fontSize: textEditor.fontSize,
                    }
                  : item,
              ),
            }
          : { ...doc, items: doc.items.filter(item => item.id !== textEditor.targetId) };
      } else if (trimmed) {
        finalDoc = {
          ...doc,
          items: [
            ...doc.items,
            {
              id: createAnnotationId(),
              kind: "text",
              color: textEditor.color,
              fontSize: textEditor.fontSize,
              x: textEditor.x,
              y: textEditor.y,
              text: trimmed,
            },
          ],
        };
      }
      setTextEditor(null);
      if (finalDoc !== doc) commitDoc(finalDoc);
      onComplete(finalDoc);
      return;
    }
    onComplete(doc);
  }

  function handleTextKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.nativeEvent.isComposing) return;
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      setTextEditor(null);
      return;
    }
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      event.stopPropagation();
      commitTextEditor();
    }
  }

  const tools: { id: AnnotationTool; label: string; icon: ReactNode }[] = [
    { id: "select", label: t("Select / Move", "选择 / 移动"), icon: <MousePointer2 size={16} /> },
    { id: "pen", label: t("Pen", "画笔"), icon: <Pen size={16} /> },
    { id: "line", label: t("Line", "直线"), icon: <Minus size={16} /> },
    { id: "arrow", label: t("Arrow", "箭头"), icon: <ArrowUpRight size={16} /> },
    { id: "rect", label: t("Rectangle", "矩形"), icon: <Square size={16} /> },
    { id: "ellipse", label: t("Ellipse", "椭圆"), icon: <Circle size={16} /> },
    { id: "text", label: t("Text", "文字"), icon: <Type size={16} /> },
    { id: "eraser", label: t("Eraser", "橡皮擦"), icon: <Eraser size={16} /> },
  ];

  const undoEnabled = canUndoHistory(historyRef.current);
  const redoEnabled = canRedoHistory(historyRef.current);

  const textOverlayStyle = useMemo(() => {
    if (!textEditor) return undefined;
    const stagePt = imageToStagePoint(
      { x: textEditor.x, y: textEditor.y },
      stageRect,
      viewTransform,
      naturalWidth,
      naturalHeight,
    );
    const scale = viewTransform.scale > 0 ? viewTransform.scale : 1;
    const turn = normalizeTurn(viewTransform.turn);
    const scaledFont = textEditor.fontSize * scale;
    const lines = textEditor.value.split(/\r?\n/);
    return {
      left: `${stagePt.x}px`,
      top: `${stagePt.y}px`,
      color: textEditor.color,
      fontSize: `${scaledFont}px`,
      lineHeight: 1.25,
      minWidth: `${Math.max(96, scaledFont * 3)}px`,
      rows: Math.max(1, lines.length),
      transform: turn ? `rotate(${turn * 90}deg)` : undefined,
    };
  }, [naturalHeight, naturalWidth, stageRect, textEditor, viewTransform]);

  const cursorMode = spacePressed
    ? "pan"
    : hoverHandle
      ? "handle"
      : tool;

  return (
    <div ref={rootRef} className="image-annotator">
      <canvas
        ref={canvasRef}
        className="image-annotator-canvas"
        data-tool={cursorMode}
        data-panning={spacePressed || undefined}
        aria-label={t("Image annotation canvas", "图片标注画布")}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={event => {
          if (dragRef.current?.pointerId === event.pointerId) {
            event.stopPropagation();
            finishDrag(event.pointerId, false);
          }
        }}
        onPointerCancel={event => finishDrag(event.pointerId, true)}
        onLostPointerCapture={event => finishDrag(event.pointerId, false)}
        onDoubleClick={handleDoubleClick}
      />

      {textEditor && textOverlayStyle && (
        <textarea
          ref={textInputRef}
          className="image-annotator-text-input"
          aria-label={t("Annotation text", "标注文字")}
          placeholder={t("Type text…", "输入文字…")}
          rows={textOverlayStyle.rows}
          value={textEditor.value}
          style={{
            left: textOverlayStyle.left,
            top: textOverlayStyle.top,
            color: textOverlayStyle.color,
            fontSize: textOverlayStyle.fontSize,
            lineHeight: textOverlayStyle.lineHeight,
            minWidth: textOverlayStyle.minWidth,
            transform: textOverlayStyle.transform,
          }}
          onPointerDown={event => event.stopPropagation()}
          onDoubleClick={event => event.stopPropagation()}
          onChange={event => setTextEditor({ ...textEditor, value: event.target.value })}
          onKeyDown={handleTextKeyDown}
          onBlur={commitTextEditor}
        />
      )}

      <div
        className="image-annotator-toolbar"
        role="toolbar"
        aria-label={t("Annotation tools", "标注工具栏")}
        onMouseDown={event => event.preventDefault()}
        onPointerDown={event => event.stopPropagation()}
        onDoubleClick={event => event.stopPropagation()}
      >
        <div className="annotator-group" role="group" aria-label={t("Drawing tools", "绘图工具")}>
          {tools.map(item => (
            <button
              key={item.id}
              type="button"
              className="annotator-tool-btn"
              title={item.label}
              aria-label={item.label}
              aria-pressed={tool === item.id}
              onClick={() => selectTool(item.id)}
            >
              {item.icon}
            </button>
          ))}
        </div>

        <span className="annotator-divider" aria-hidden />

        <div className="annotator-group" role="group" aria-label={t("Colors", "颜色")}>
          {ANNOTATION_COLORS.map(swatch => {
            const [en, zh] = COLOR_LABELS[swatch];
            const label = t(en, zh);
            return (
              <button
                key={swatch}
                type="button"
                className="annotator-swatch-btn"
                title={label}
                aria-label={label}
                aria-pressed={activeColor === swatch}
                onClick={() => handleColorPick(swatch)}
              >
                <span className="annotator-swatch-dot" style={{ backgroundColor: swatch }} />
              </button>
            );
          })}
        </div>

        <span className="annotator-divider" aria-hidden />

        <div
          className="annotator-group annotator-size-group"
          role="group"
          aria-label={isTextStyleMode ? t("Text size", "文字字号") : t("Stroke width", "线条粗细")}
        >
          {isTextStyleMode
            ? TEXT_SIZE_LEVELS.map(level => {
                const [shortEn, shortZh] = TEXT_SIZE_SHORT[level];
                const [labelEn, labelZh] = TEXT_SIZE_LABELS[level];
                return (
                  <button
                    key={level}
                    type="button"
                    className="annotator-size-btn"
                    title={t(labelEn, labelZh)}
                    aria-label={t(labelEn, labelZh)}
                    aria-pressed={activeTextSize === level}
                    onClick={() => handleTextSizePick(level)}
                  >
                    {t(shortEn, shortZh)}
                  </button>
                );
              })
            : STROKE_WIDTH_LEVELS.map(level => {
                const [labelEn, labelZh] = STROKE_LABELS[level];
                return (
                  <button
                    key={level}
                    type="button"
                    className="annotator-size-btn"
                    title={t(labelEn, labelZh)}
                    aria-label={t(labelEn, labelZh)}
                    aria-pressed={activeWidthLevel === level}
                    disabled={tool === "eraser"}
                    onClick={() => handleStrokeWidthPick(level)}
                  >
                    <span className={`annotator-stroke-dot annotator-stroke-${level}`} />
                  </button>
                );
              })}
        </div>

        <span className="annotator-divider" aria-hidden />

        <div className="annotator-group" role="group" aria-label={t("History", "撤销与重做")}>
          <button
            type="button"
            className="annotator-tool-btn"
            title={t("Undo (Ctrl+Z)", "撤销 (Ctrl+Z)")}
            aria-label={t("Undo", "撤销")}
            disabled={!undoEnabled}
            onClick={performUndo}
          >
            <Undo2 size={16} />
          </button>
          <button
            type="button"
            className="annotator-tool-btn"
            title={t("Redo (Ctrl+Y)", "重做 (Ctrl+Y)")}
            aria-label={t("Redo", "重做")}
            disabled={!redoEnabled}
            onClick={performRedo}
          >
            <Redo2 size={16} />
          </button>
        </div>

        <span className="annotator-divider" aria-hidden />

        <div className="annotator-group annotator-actions">
          <button
            type="button"
            className="button subtle small annotator-exit-btn"
            onClick={onRequestExit}
          >
            <X size={14} />
            {t("Cancel", "退出")}
          </button>
          <button
            type="button"
            className="button primary small annotator-done-btn"
            onClick={handleDone}
          >
            <Check size={14} />
            {t("Done", "完成")}
          </button>
        </div>
      </div>
    </div>
  );
}
