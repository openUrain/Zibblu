import type { Draft } from "./api";
import { t } from "./locale";
import { conversationSummary } from "./rich-markdown";

export type DraftPreview = {
  prefix: string;
  body: string;
};

export function deriveDraftPreview(draft?: Draft): DraftPreview | undefined {
  if (!draft) return undefined;
  const text = draft.text.trim() ? conversationSummary(draft.text) : "";
  if (text) {
    return { prefix: t("[Draft]", "[草稿]"), body: text };
  }
  const attachments = draft.attachments;
  if (!attachments || attachments.length === 0) return undefined;
  const first = attachments[0].name;
  if (attachments.length === 1) {
    return { prefix: t("[Draft]", "[草稿]"), body: first };
  }
  return {
    prefix: t("[Draft]", "[草稿]"),
    body: t(`${first} (${attachments.length} files)`, `${first} 等${attachments.length}个文件`),
  };
}
