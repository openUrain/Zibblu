import { Minus, Square, Copy, X } from "lucide-react";
import { useEffect, useId, useState } from "react";
import type { ReactNode } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { errorMessage } from "./api";
import type { CloseAction } from "./api";
import { Dialog } from "./common";

import { actions, t } from "./locale";

export const windows = navigator.userAgent.includes("Windows");
const appWindow = getCurrentWindow();

export function WindowDrag({ className = "", children }: { className?: string; children?: ReactNode }) {
  return <div className={`window-drag ${className}`} data-tauri-drag-region={windows ? "" : undefined}
    onPointerDown={event => {
      if (!windows || event.pointerType !== "mouse" || event.button !== 0 || event.target !== event.currentTarget) return;
      const field = document.activeElement;
      // Tauri cancels the default mousedown behavior when starting a native window drag.
      if ((field instanceof HTMLInputElement || field instanceof HTMLTextAreaElement)
          && field.parentElement?.matches(".floating-field")) field.blur();
    }}>{children}</div>;
}

export function WindowControls() {
  const [maximized, setMaximized] = useState(false);
  const [failure, setFailure] = useState("");

  useEffect(() => {
    const update = () => void appWindow.isMaximized().then(setMaximized).catch(() => {});
    const resized = appWindow.onResized(update);
    update();
    return () => { void resized.then(unlisten => unlisten()); };
  }, []);

  function run(action: () => Promise<void>) {
    setFailure("");
    void action().catch(error => setFailure(errorMessage(error)));
  }

  const maximizeLabel = maximized ? "Restore window" : "Maximize window";
  return <div className="window-controls" role="group" aria-label="Window controls">
    <button type="button" aria-label="Minimize window" title="Minimize window" onClick={() => run(() => appWindow.minimize())}><Minus size={16} /></button>
    <button type="button" aria-label={maximizeLabel} title={maximizeLabel} onClick={() => run(() => appWindow.toggleMaximize())}>
      {maximized ? <Copy size={14} /> : <Square size={13} />}
    </button>
    <button type="button" className="window-close" aria-label="Close window" title="Close window" onClick={() => run(() => appWindow.close())}><X size={17} /></button>
    {failure && <p className="window-error inline-error" role="alert">{failure}</p>}
  </div>;
}

export function CloseActionChoices({ action, choose, disabled }: {
  action?: CloseAction; choose: (action: CloseAction) => void; disabled?: boolean;
}) {
  const group = useId();
  return <fieldset className="close-action-options" disabled={disabled}>
    <legend>{t("When closing the window", "关闭主窗口时")}</legend>
    <label><input type="radio" name={group} checked={action === "exit"} onChange={() => choose("exit")} /> {t("Exit Zibblu", "退出 Zibblu")}</label>
    <label><input type="radio" name={group} checked={action === "tray"} onChange={() => choose("tray")} /> {t("Minimize to the system tray", "最小化到系统托盘")}</label>
  </fieldset>;
}

export function WindowClosePrompt({ save }: { save: (action: CloseAction) => Promise<void> }) {
  const [open, setOpen] = useState(false);
  const [action, setAction] = useState<CloseAction>();
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState("");

  useEffect(() => {
    const requested = getCurrentWindow().listen("choose-close-action", () => setOpen(true));
    return () => { void requested.then(unlisten => unlisten()); };
  }, []);

  function cancel() {
    setOpen(false);
    setAction(undefined);
    setFailure("");
  }

  async function confirm() {
    if (!action || busy) return;
    setBusy(true);
    setFailure("");
    try {
      await save(action);
      await getCurrentWindow().close();
      cancel();
    } catch (error) { setFailure(errorMessage(error)); }
    finally { setBusy(false); }
  }

  return open && <Dialog title={t("Close Zibblu", "关闭 Zibblu")} className="close-action-dialog" close={cancel} closeDisabled={busy}>
    <CloseActionChoices action={action} choose={setAction} disabled={busy} />
    {failure && <p className="inline-error" role="alert">{failure}</p>}
    <div className="dialog-actions">
      <button className="button subtle" onClick={cancel} disabled={busy}>{actions.cancel}</button>
      <button className="button primary" onClick={() => void confirm()} disabled={busy || !action}>{t("Confirm", "确定")}</button>
    </div>
  </Dialog>;
}
