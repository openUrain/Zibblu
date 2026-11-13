import { Camera, ImageUp } from "lucide-react";
import { useContext, useEffect, useRef, useState } from "react";
import jsQR from "jsqr";
import QRCode from "qrcode";
import { api, errorMessage } from "./api";
import type { GroupPreview, PrivateCode, Session, User } from "./api";
import { Avatar, Dialog, People, PersonAvatar, dateTimeLabel, displayName, messageTimeLabel } from "./common";

export type SharedCode = {
  kind: "contact" | "group" | "private";
  value: string;
  name: string;
  expiresAt?: number;
};

export function ShareQr({ code, server, close }: { code: SharedCode; server: string; close: () => void }) {
  const [image, setImage] = useState("");
  const [error, setError] = useState("");
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    let current = true;
    const url = new URL("zibblu://" + code.kind);
    url.searchParams.set("server", server);
    url.searchParams.set("v", code.value);
    QRCode.toDataURL(url.toString(), { width: 320, margin: 2, errorCorrectionLevel: "M" })
      .then(value => { if (current) setImage(value); })
      .catch(() => { if (current) setError("Could not generate this QR code."); });
    const tick = code.expiresAt ? window.setTimeout(() => setNow(Date.now()), Math.max(0, code.expiresAt - Date.now())) : undefined;
    return () => { current = false; window.clearInterval(tick); };
  }, [code, server]);
  const expired = code.expiresAt !== undefined && now >= code.expiresAt;
  return <Dialog title={code.name} close={close}>
    <div className="qr-content">
      {error && <p className="inline-error" role="alert">{error}</p>}
      {expired
        ? <p role="status">This QR code expired.</p>
        : image && <img className="qr-image" src={image} alt="Zibblu QR code" />}
      {code.kind === "private" && !expired &&
        <p>Anyone with this code on the allowed LAN can join. They receive messages sent after they join.</p>}
      {code.expiresAt && !expired && <p className="muted">
        Expires {code.kind === "private" ? messageTimeLabel(code.expiresAt) : dateTimeLabel(code.expiresAt)}
      </p>}
    </div>
  </Dialog>;
}

type ScanResult =
  | { kind: "contact"; user: User }
  | { kind: "group"; group: GroupPreview; code: string }
  | { kind: "private"; code: PrivateCode };

export function ScanQr({ session, close, contactFound, groupJoined, privateJoined }: {
  session: Session; close: () => void; contactFound: (user: User) => void;
  groupJoined: (id: string) => void; privateJoined: (id: string) => void;
}) {
  const { remarks } = useContext(People);
  const [camera, setCamera] = useState(false);
  const [failure, setFailure] = useState("");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<ScanResult>();
  const video = useRef<HTMLVideoElement>(null);
  const mounted = useRef(true);

  useEffect(() => () => { mounted.current = false; }, []);

  async function decode(text: string) {
    setCamera(false);
    setBusy(true);
    setFailure("");
    try {
      if (text.length > 4096) throw new Error("This is not a Zibblu QR code.");
      const url = new URL(text);
      const value = url.searchParams.get("v");
      if (url.protocol !== "zibblu:" || !value) throw new Error("This is not a Zibblu QR code.");
      if (url.searchParams.get("server") !== session.server) throw new Error("This code belongs to a different Zibblu server.");
      if (url.hostname === "contact") {
        const user = await api.findUser(value);
        if (user.id === session.user.id) throw new Error("This is your own contact code.");
        if (mounted.current) setResult({ kind: "contact", user });
      } else if (url.hostname === "group") {
        const group = await api.groupPreview(value);
        if (mounted.current) setResult({ kind: "group", group, code: value });
      } else if (url.hostname === "private") {
        const code = JSON.parse(value) as PrivateCode;
        if (!code || typeof code.room !== "string" || typeof code.expires_at !== "number") {
          throw new Error("This private QR code is invalid.");
        }
        if (code.expires_at <= Date.now()) throw new Error("This private QR code expired.");
        setResult({ kind: "private", code });
      } else {
        throw new Error("This is not a Zibblu QR code.");
      }
    } catch (error) {
      if (mounted.current) setFailure(error instanceof Error ? error.message : errorMessage(error));
    } finally {
      if (mounted.current) setBusy(false);
    }
  }

  useEffect(() => {
    if (!camera) return;
    let stopped = false;
    let stream: MediaStream | undefined;
    let timer: number | undefined;
    const player = video.current;
    const canvas = document.createElement("canvas");
    function scan() {
      if (stopped || !player) return;
      const result = readImage(canvas, player, player.videoWidth, player.videoHeight);
      if (result) void decode(result);
      else timer = window.setTimeout(scan, 160);
    }
    async function start() {
      try {
        stream = await navigator.mediaDevices.getUserMedia({ video: { facingMode: "environment" }, audio: false });
        if (stopped || !player) { stream.getTracks().forEach(track => track.stop()); return; }
        player.srcObject = stream;
        await player.play();
        scan();
      } catch {
        if (!stopped) {
          setCamera(false);
          setFailure("Camera unavailable. Choose a QR image or allow camera access.");
        }
      }
    }
    void start();
    return () => {
      stopped = true;
      window.clearTimeout(timer);
      stream?.getTracks().forEach(track => track.stop());
      if (player) player.srcObject = null;
    };
  }, [camera]);

  async function image(file: File | undefined) {
    if (!file) return;
    setCamera(false);
    setFailure("");
    try {
      const bitmap = await createImageBitmap(file);
      try {
        const text = readImage(document.createElement("canvas"), bitmap, bitmap.width, bitmap.height);
        if (!text) throw new Error("No QR code was found in this image.");
        if (mounted.current) await decode(text);
      } finally { bitmap.close(); }
    } catch (error) { if (mounted.current) setFailure(error instanceof Error ? error.message : errorMessage(error)); }
  }

  async function confirm() {
    if (!result || busy) return;
    setBusy(true);
    setFailure("");
    try {
      if (result.kind === "contact") {
        if (mounted.current) contactFound(result.user);
      } else if (result.kind === "group") {
        const id = await api.groupJoin(result.code);
        if (mounted.current) groupJoined(id);
      } else {
        const id = await api.scanPrivate(result.code);
        if (mounted.current) privateJoined(id);
      }
    } catch (error) {
      if (mounted.current) setFailure(errorMessage(error));
    } finally { if (mounted.current) setBusy(false); }
  }

  return <Dialog title="Scan a QR code" close={() => { if (!busy) close(); }}>
    <div className="qr-content">
      {camera && <video className="qr-camera" ref={video} muted playsInline />}
      {result ? <>
        {result.kind === "contact" && <><PersonAvatar user={result.user} large /><strong>{displayName(result.user, remarks)}</strong></>}
        {result.kind === "group" && <><Avatar name={result.group.title} group large />
          <strong>{result.group.title}</strong><p>{result.group.member_count} members · New messages only</p></>}
        {result.kind === "private" && <p>Join this temporary private chat over the local network.</p>}
        <button className="button primary" disabled={busy} onClick={() => void confirm()}>
          {result.kind === "contact" ? "View profile" : "Join chat"}
        </button>
      </> : <div className="qr-actions">
        <button className="button subtle" disabled={busy} onClick={() => setCamera(!camera)}>
          <Camera size={17} /> {camera ? "Stop camera" : "Use camera"}
        </button>
        <label className="button subtle"><ImageUp size={17} /> Choose QR image
          <input className="qr-file" type="file" accept="image/*" disabled={busy}
            onChange={event => { const file = event.currentTarget.files?.[0]; event.currentTarget.value = ""; void image(file); }} />
        </label>
      </div>}
      {failure && <p className="inline-error" role="alert">{failure}</p>}
    </div>
  </Dialog>;
}

function readImage(canvas: HTMLCanvasElement, source: CanvasImageSource, width: number, height: number) {
  if (!width || !height) return undefined;
  const scale = Math.min(1, 1024 / Math.max(width, height));
  canvas.width = Math.max(1, Math.round(width * scale));
  canvas.height = Math.max(1, Math.round(height * scale));
  const context = canvas.getContext("2d", { willReadFrequently: true });
  if (!context) return undefined;
  context.drawImage(source, 0, 0, canvas.width, canvas.height);
  const pixels = context.getImageData(0, 0, canvas.width, canvas.height);
  return jsQR(pixels.data, canvas.width, canvas.height)?.data;
}
