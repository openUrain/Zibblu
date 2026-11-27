import { ChevronDown } from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";
import type { ReactNode } from "react";

type Choice = { label: string; icon?: ReactNode; run: () => void };

export function MediaSplitButton({ label, menuLabel, children, choices, run, disabled, pressed, className = "" }: {
  label: string; menuLabel: string; children: ReactNode; choices: Choice[]; run: () => void;
  disabled?: boolean; pressed?: boolean; className?: string;
}) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const toggle = useRef<HTMLButtonElement>(null);
  const options = useRef<HTMLDivElement>(null);
  const id = useId();

  useEffect(() => {
    if (!open || disabled) return;
    options.current?.querySelector("button")?.focus();
    function outside(event: globalThis.PointerEvent) {
      if (event.target instanceof Node && !root.current?.contains(event.target)) setOpen(false);
    }
    document.addEventListener("pointerdown", outside);
    return () => document.removeEventListener("pointerdown", outside);
  }, [disabled, open]);

  function dismiss() {
    setOpen(false);
    toggle.current?.focus();
  }

  return <div ref={root} className={`media-split-button ${className}`}
    onBlur={event => {
      if (!event.currentTarget.contains(event.relatedTarget)) setOpen(false);
    }}
    onKeyDown={event => {
      if (event.key === "Escape" && open) {
        event.preventDefault();
        event.stopPropagation();
        dismiss();
      }
      if (event.key === "ArrowDown" && event.target === toggle.current) {
        event.preventDefault();
        if (open) options.current?.querySelector("button")?.focus();
        else setOpen(true);
      }
    }}>
    <button type="button" aria-label={label} title={label} aria-pressed={pressed}
      disabled={disabled} onClick={() => { setOpen(false); run(); }}>{children}</button>
    <button ref={toggle} type="button" className="media-split-toggle" aria-label={menuLabel} title={menuLabel}
      aria-controls={id} aria-expanded={open && !disabled} disabled={disabled || choices.length === 0}
      onClick={() => setOpen(value => !value)}><ChevronDown size={14} aria-hidden /></button>
    <div id={id} ref={options} className="media-split-options" hidden={!open || disabled}>
      {choices.map(choice => <button key={choice.label} type="button" onClick={() => {
        dismiss();
        choice.run();
      }}>{choice.icon}{choice.label}</button>)}
    </div>
  </div>;
}
