export function splitFileName(name: string): [string, string] {
  const dot = name.match(/\.tar\.(gz|bz2|xz|zst)$/i)?.index ?? name.lastIndexOf(".");
  return dot > 0 && dot < name.length - 1 ? [name.slice(0, dot), name.slice(dot)] : [name, ""];
}

export function decimalSize(value: number): string {
  if (value < 1000) return value + " B";
  if (value < 1_000_000) return (value / 1000).toFixed(1) + " KB";
  if (value < 1_000_000_000) return (value / 1_000_000).toFixed(1) + " MB";
  return (value / 1_000_000_000).toFixed(1) + " GB";
}
