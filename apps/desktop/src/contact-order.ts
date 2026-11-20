import { pinyin } from "pinyin-pro";
import type { User } from "./api";

const alphabet = new Intl.Collator("en", { sensitivity: "base", numeric: true });
const groupOrder = "ABCDEFGHIJKLMNOPQRSTUVWXYZ#";

function compareIds(a: string, b: string) {
  if (a === b) return 0;
  return a < b ? -1 : 1;
}

export function orderContacts(contacts: User[], names: Record<string, string>) {
  const counts = new Map<string, number>();
  const entries = contacts.map(user => {
    const name = names[user.id]?.trim() || user.handle;
    const normalized = name.normalize("NFKC");
    const nameKey = normalized.toLowerCase();
    counts.set(nameKey, (counts.get(nameKey) ?? 0) + 1);
    const sortKey = pinyin(normalized, { toneType: "none", nonZh: "consecutive", type: "array" })
      .join("").normalize("NFD").replace(/\p{M}/gu, "");
    const letter = /^[a-z]/i.test(sortKey) ? sortKey[0].toUpperCase() : "#";
    return { user, name, nameKey, letter, sortKey };
  });

  entries.sort((a, b) => groupOrder.indexOf(a.letter) - groupOrder.indexOf(b.letter)
    || alphabet.compare(a.sortKey, b.sortKey)
    || alphabet.compare(a.user.handle, b.user.handle)
    || compareIds(a.user.id, b.user.id));

  return entries.map(({ user, name, nameKey, letter }) => ({
    user, name, letter, showHandle: (counts.get(nameKey) ?? 0) > 1,
  }));
}
