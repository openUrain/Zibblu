import { invoke } from "@tauri-apps/api/core";

export type Theme = "system" | "light" | "dark";
export type User = { id: string; handle: string };
export type Session = { user: User; server: string };
export type Preferences = { theme: Theme; server: string };
export type Conversation = {
  id: string; title: string | null; members: User[]; activity_at: number; preview: string | null;
};
export type Directory = { contacts: User[]; conversations: Conversation[] };
export type Attachment = { id: string; name: string; kind: "file" | "folder"; size: number; sha256: string };
export type Message = {
  seq: number; conversation_id: string; sender: User; created_at: number;
  text: string | null; attachment: Attachment | null;
};
export type Page = { items: Message[]; has_more: boolean };
export type Transfer = {
  attachment: Attachment; conversation_id: string; direction: "upload" | "download";
  status: "running" | "paused" | "failed" | "complete"; received: number; error: string | null;
};
export type FileFilter = {
  conversation_id?: string; sender_id?: string; kind?: Attachment["kind"]; start_at?: number; end_before?: number;
};
export type ConversationRequest =
  | { kind: "direct"; contact_id: string }
  | { kind: "group"; title: string; contact_ids: string[] };
export type DropOffer = { ticket: string; name: string };
export type ShareSource = { kind: "file" | "folder" } | { kind: "dropped"; ticket: string };
export type PrivateStatus =
  | { state: "discovering" | "connecting" | "connected" | "ended" }
  | { state: "verify" | "waiting_for_peer"; code: string }
  | { state: "failed"; reason: string };
export type PrivateMessage = { seq: number; mine: boolean; text: string };
type PrivateView = PrivateStatus & { messages: PrivateMessage[] };

export const api = {
  preferences: () => invoke<Preferences>("preferences"),
  setTheme: (theme: Theme) => invoke<void>("set_theme", { theme }),
  login: (server: string, handle: string, password: string, register: boolean) =>
    invoke<Session>("login", { server, handle, password, register }),
  logout: () => invoke<void>("logout"),
  directory: () => invoke<Directory>("directory"),
  sync: () => invoke<Directory>("sync"),
  messages: (conversation: string, before?: number) => invoke<Page>("messages", { conversation, before }),
  files: (filter: FileFilter, before?: number) => invoke<Page>("files", { filter, before }),
  findUser: (handle: string) => invoke<User>("find_user", { handle }),
  addContact: (userId: string) => invoke<void>("add_contact", { userId }),
  removeContact: (userId: string) => invoke<void>("remove_contact", { userId }),
  create: (request: ConversationRequest) => invoke<string>("create_conversation", { request }),
  send: (conversation: string, text: string, nonce: string) =>
    invoke<void>("send_message", { conversation, message: { text, nonce } }),
  startPrivate: (conversation: string) => invoke<string>("private_start", { conversation }),
  viewPrivate: (id: string, after: number) => invoke<PrivateView>("private_view", { id, after }),
  verifyPrivate: (id: string, matches: boolean) => invoke<void>("private_verify", { id, matches }),
  sendPrivate: (id: string, text: string) => invoke<void>("private_send", { id, text }),
  endPrivate: (id: string) => invoke<void>("private_end", { id }),
  share: (conversation: string, source: ShareSource) => invoke<Transfer | null>("share", { conversation, source }),
  save: (id: string) => invoke<Transfer | null>("save_attachment", { id }),
  transfers: () => invoke<Transfer[]>("transfers"),
  transferAction: (action: "pause" | "resume" | "discard", transfer: Transfer) =>
    invoke<void>(action + "_transfer", { id: transfer.attachment.id, direction: transfer.direction }),
};

export function isExpired(error: unknown): boolean {
  return typeof error === "object" && error !== null && "expired" in error && error.expired === true;
}

export function errorMessage(error: unknown): string {
  if (typeof error === "object" && error !== null && "message" in error && typeof error.message === "string") {
    return error.message;
  }
  return typeof error === "string" ? error : "Something went wrong. Try again.";
}
