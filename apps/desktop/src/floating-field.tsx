import { useId } from "react";
import type { ComponentPropsWithRef, ReactNode } from "react";

type FieldProps = {
  label: string;
  prefixIcon?: ReactNode;
  trailingAction?: ReactNode;
  fieldClass?: string;
} & (
  | ({ multiline: true } & ComponentPropsWithRef<"textarea">)
  | ({ multiline?: false } & ComponentPropsWithRef<"input">)
);

export function FloatingField({ label, prefixIcon, trailingAction, fieldClass = "", ...control }: FieldProps) {
  const generatedId = useId();
  const id = control.id ?? generatedId;
  const placeholder = control.placeholder || " ";
  let editor;
  if (control.multiline) {
    const { multiline, ...props } = control;
    editor = <textarea {...props} id={id} placeholder={placeholder} />;
  } else {
    const { multiline, ...props } = control;
    editor = <input {...props} id={id} placeholder={placeholder} />;
  }
  return <div className={`floating-field ${fieldClass}`}>
    {prefixIcon && <span className="field-icon" aria-hidden="true">{prefixIcon}</span>}
    {editor}
    <label className="field-label" htmlFor={id}>{label}</label>
    {trailingAction}
  </div>;
}
