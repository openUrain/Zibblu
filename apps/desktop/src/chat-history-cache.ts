import type { Page } from "./api";

const maxConversations = 8;
const pageSize = 60;

export class ChatHistoryCache {
  private readonly pages = new Map<string, Page>();

  get(conversation: string) {
    return this.pages.get(conversation);
  }

  save(conversation: string, page: Page) {
    this.pages.delete(conversation);
    this.pages.set(conversation, {
      items: page.items.slice(0, pageSize),
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
