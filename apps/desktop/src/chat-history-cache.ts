import type { Page } from "./api";
import { reuseMessageRendering } from "./rich-markdown";

const maxConversations = 8;
const pageSize = 60;

export class ChatHistoryCache {
  private readonly pages = new Map<string, Page>();

  get(conversation: string) {
    return this.pages.get(conversation);
  }

  save(conversation: string, page: Page) {
    const previous = this.pages.get(conversation);
    const items = page.items.slice(0, pageSize);
    if (previous) {
      const bySequence = new Map(previous.items.map(message => [message.seq, message]));
      for (const message of items) {
        const old = bySequence.get(message.seq);
        if (old) reuseMessageRendering(old, message, message.text);
      }
    }
    this.pages.delete(conversation);
    this.pages.set(conversation, {
      items,
      has_more: page.has_more || page.items.length > pageSize,
    });
    if (this.pages.size > maxConversations) this.pages.delete(this.pages.keys().next().value!);
  }

  retain(conversations: ReadonlySet<string>) {
    for (const id of this.pages.keys()) {
      if (!conversations.has(id)) this.pages.delete(id);
    }
  }
}
