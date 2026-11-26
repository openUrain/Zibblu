import type { RefObject } from "react";

// ============================================================================
// Frozen Stage 2 Shared Interfaces (#124 Image Annotator & #125 Video Trimmer)
// Subagents A and B treat this file as read-only.
// ============================================================================

export type AnnotationColor =
  | "#E53935"
  | "#FB8C00"
  | "#FBC02D"
  | "#2E7D32"
  | "#1E88E5"
  | "#634FB4"
  | "#1D1D1F";

export type StrokeWidthLevel = "thin" | "medium" | "thick";

export type AnnotationTool =
  | "select"
  | "pen"
  | "text"
  | "eraser"
  | "line"
  | "arrow"
  | "rect"
  | "ellipse";

export type ImagePoint = { x: number; y: number };

export type PenAnnotation = {
  id: string;
  kind: "pen";
  color: AnnotationColor;
  widthLevel: StrokeWidthLevel;
  points: ImagePoint[];
  erase?: true;
};

export type TextAnnotation = {
  id: string;
  kind: "text";
  color: AnnotationColor;
  fontSize: number; // in natural image pixels
  x: number;
  y: number;
  text: string;
  width?: number;
  height?: number;
  rotation?: number;
  family?: "system-ui" | "serif" | "monospace";
  bold?: boolean;
  italic?: boolean;
  underline?: boolean;
  align?: "left" | "center" | "right";
};

export type ShapeAnnotation = {
  id: string;
  kind: "line" | "arrow" | "rect" | "ellipse";
  color: AnnotationColor;
  widthLevel: StrokeWidthLevel;
  x1: number;
  y1: number;
  x2: number;
  y2: number;
};

export type ImageAnnotationItem = PenAnnotation | TextAnnotation | ShapeAnnotation;

export interface ImageAnnotationDoc {
  version: 1;
  naturalWidth: number;
  naturalHeight: number;
  items: ImageAnnotationItem[];
}

export interface ViewTransform {
  x: number;
  y: number;
  scale: number;
  turn: number; // 0 | 1 | 2 | 3 (90-deg clockwise steps)
}

export interface ImageAnnotatorProps {
  imageElement: HTMLImageElement | null;
  naturalWidth: number;
  naturalHeight: number;
  viewTransform: ViewTransform;
  stageRect: { width: number; height: number };
  editing: boolean;
  doc: ImageAnnotationDoc;
  onChange: (next: ImageAnnotationDoc) => void;
  onComplete: (doc: ImageAnnotationDoc) => void;
  onRequestExit: () => void;
  onPendingEditChange: (dirty: boolean) => void;
}

export interface VideoTrimRange {
  startSec: number;
  endSec: number;
  durationSec: number;
}

export interface VideoProbeInfo {
  duration_sec: number;
  width: number;
  height: number;
  fps: number;
  has_audio: boolean;
}

export interface VideoExportProgress {
  export_id: string;
  status: "running" | "complete" | "failed" | "cancelled";
  progress: number; // 0..1
  output_size?: number;
  error?: string;
}

export interface VideoTrimmerProps {
  attachmentId: string;
  videoRef: RefObject<HTMLVideoElement | null>;
  initialRange: VideoTrimRange | null;
  probe: VideoProbeInfo;
  thumbnails: string[]; // data URLs
  onComplete: (range: VideoTrimRange) => void;
  onRequestCancel: (dirty: boolean) => void;
  onPendingEditChange: (dirty: boolean) => void;
}

export interface VideoExportBarProps {
  progress: VideoExportProgress;
  confirmingCancel: boolean;
  onRequestCancel: () => void;
  onConfirmCancel: () => void;
  onContinueExport: () => void;
}
