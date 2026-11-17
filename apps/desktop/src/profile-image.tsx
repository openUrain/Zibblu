import { useEffect, useRef, useState } from "react";
import { actions, t } from "./locale";

type Point = { x: number; y: number };
const center: Point = { x: 0, y: 0 };

export function ProfileImageEditor({ source, kind, disabled, saveLabel, cancel, save }: {
  source: ArrayBuffer; kind: "avatar" | "cover"; disabled: boolean; saveLabel: string; cancel: () => void;
  save: (crop: () => Promise<ArrayBuffer>) => void;
}) {
  const image = useRef<HTMLImageElement>(null);
  const drag = useRef<{ id: number; start: Point; origin: Point }>(undefined);
  const [url, setUrl] = useState("");
  const [imageAspect, setImageAspect] = useState(0);
  const [zoom, setZoom] = useState(1);
  const [position, setPosition] = useState(center);
  const aspect = kind === "avatar" ? 1 : 3;
  const width = Math.max(1, imageAspect / aspect) * zoom;
  const height = Math.max(1, aspect / (imageAspect || aspect)) * zoom;
  const clamp = (value: number, size: number) => Math.max((1 - size) / 2, Math.min((size - 1) / 2, value));
  const x = clamp(position.x, width);
  const y = clamp(position.y, height);

  useEffect(() => {
    const objectUrl = URL.createObjectURL(new Blob([source], { type: "image/png" }));
    setUrl(objectUrl);
    return () => URL.revokeObjectURL(objectUrl);
  }, [source]);

  async function crop() {
    const picture = image.current;
    const canvas = document.createElement("canvas");
    const context = canvas.getContext("2d");
    if (!picture?.naturalWidth || !context) throw new Error(t("Could not crop the image.", "无法裁剪此图片。"));
    canvas.width = Math.min(1024, Math.max(1, Math.round(picture.naturalWidth / width)));
    canvas.height = Math.max(1, Math.round(canvas.width / aspect));
    context.drawImage(picture, (0.5 + x - width / 2) * canvas.width, (0.5 + y - height / 2) * canvas.height,
      width * canvas.width, height * canvas.height);
    const result = await new Promise<Blob | null>(resolve => canvas.toBlob(resolve, "image/png"));
    if (!result) throw new Error(t("Could not crop the image.", "无法裁剪此图片。"));
    return result.arrayBuffer();
  }

  return <form className="profile-image-editor" onSubmit={event => { event.preventDefault(); save(crop); }}>
    <div className="profile-crop" style={{ aspectRatio: aspect }} tabIndex={0}
      aria-label={t("Crop position. Drag the image or use arrow keys.", "裁剪位置。拖动图片或使用方向键。")}
      onKeyDown={event => {
        const step = { ArrowLeft: [-0.05, 0], ArrowRight: [0.05, 0], ArrowUp: [0, -0.05], ArrowDown: [0, 0.05] }[event.key];
        if (disabled || !step) return;
        event.preventDefault();
        setPosition({ x: clamp(x + step[0], width), y: clamp(y + step[1], height) });
      }}
      onPointerDown={event => {
        if (disabled || event.button !== 0) return;
        event.currentTarget.setPointerCapture(event.pointerId);
        drag.current = { id: event.pointerId, start: { x: event.clientX, y: event.clientY }, origin: { x, y } };
      }}
      onPointerMove={event => {
        const active = drag.current;
        if (!active || active.id !== event.pointerId || disabled) return;
        const rect = event.currentTarget.getBoundingClientRect();
        setPosition({
          x: clamp(active.origin.x + (event.clientX - active.start.x) / rect.width, width),
          y: clamp(active.origin.y + (event.clientY - active.start.y) / rect.height, height),
        });
      }}
      onLostPointerCapture={() => { drag.current = undefined; }}>
      {url && <img src={url} alt={t("Crop preview", "裁剪预览")} ref={image} draggable={false}
        onLoad={event => setImageAspect(event.currentTarget.naturalWidth / event.currentTarget.naturalHeight)}
        style={{ width: width * 100 + "%", height: height * 100 + "%", left: (x + 0.5) * 100 + "%", top: (y + 0.5) * 100 + "%" }} />}
    </div>
    <label className="profile-zoom"><span>{t("Zoom", "缩放")}</span>
      <input type="range" min={1} max={3} step={0.05} value={zoom} disabled={disabled || !imageAspect}
        aria-valuetext={Math.round(zoom * 100) + "%"} onChange={event => {
          setPosition({ x, y });
          setZoom(event.currentTarget.valueAsNumber);
        }} />
      <output>{Math.round(zoom * 100)}%</output>
    </label>
    <div className="dialog-actions">
      <button type="button" className="button subtle" onClick={cancel} disabled={disabled}>{actions.cancel}</button>
      <button className="button primary" disabled={disabled || !imageAspect}>{saveLabel}</button>
    </div>
  </form>;
}
