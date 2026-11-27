import type { Attachment, Page } from "./api";

export const MAX_IMAGE_BYTES = 16 * 1024 * 1024;

const inlineImages = /\.(a?png|jpe?g|gif|webp|bmp)$/i;
const inlineVideos = /\.(mp4|m4v|webm|mov)$/i;
const otherImages = /\.(avif|heic|heif|tiff?|svg|ico)$/i;
const otherVideos = /\.(avi|mkv|wmv|flv|mpg|mpeg|3gp|ogv)$/i;

export function mediaKind(file: Pick<Attachment, "kind" | "name">) {
  if (file.kind !== "file") return undefined;
  if (inlineImages.test(file.name) || otherImages.test(file.name)) return "image";
  if (inlineVideos.test(file.name) || otherVideos.test(file.name)) return "video";
  return undefined;
}

export function canPreview(file: Attachment) {
  return file.kind === "file" && file.size > 0 && (inlineVideos.test(file.name)
    || (inlineImages.test(file.name) && file.size <= MAX_IMAGE_BYTES));
}

export async function readAttachmentPage(read: (before?: number) => Promise<Page>,
  media: boolean, recalled: ReadonlySet<number>, hidden: ReadonlySet<number>, before?: number, limit = 24): Promise<Page> {
  let cursor = before;
  const items: Page["items"] = [];
  for (;;) {
    const page = await read(cursor);
    items.push(...page.items.filter(item => item.attachment && !item.recalled
      && !recalled.has(item.seq) && !hidden.has(item.seq) && !!mediaKind(item.attachment) === media));
    if (items.length > limit) return { items: items.slice(0, limit), has_more: true };
    const last = page.items.at(-1)?.seq;
    if (!page.has_more || last === undefined || last === cursor) return { items, has_more: false };
    // The cursor belongs to the unfiltered page, even when the entire page was skipped.
    cursor = last;
  }
}
