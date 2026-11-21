import { Search } from "lucide-react";
import type { ComponentPropsWithoutRef } from "react";
import { FloatingField } from "./floating-field";
import { windows } from "./window-chrome";

type WorkspaceSearchProps = Pick<ComponentPropsWithoutRef<"input">, "value" | "onChange"> & {
  label: string;
  otherIconSize?: number;
};

export function WorkspaceSearchField({ otherIconSize = 16, ...field }: WorkspaceSearchProps) {
  return <FloatingField {...field} fieldClass="search workspace-search" type="search"
    prefixIcon={<Search size={windows ? undefined : otherIconSize} strokeWidth={2} />} />;
}
