import type { ComponentPropsWithoutRef } from "react";
import "./selector.css";

export function Selector({ className = "", ...props }: ComponentPropsWithoutRef<"select">) {
  return <select {...props} className={`zibblu-selector ${className}`} />;
}
