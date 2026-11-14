import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import type { AiProvider } from "./ai-api";

export type User = { id: string; handle: string };
export type Session = { user: User; server: string };
export type Preferences = { server: string; official_server: string; private_quiet: boolean };
export type Conversation = {
  id: string; title: string | null; members: User[]; activity_at: number; preview: string | null;
  latest_seq: number; unread: number; read_through: number;
};
export type PendingContact = { user: User; incoming: boolean; request_id: string | null; greeting: string };
export type Directory = {
  contacts: User[]; conversations: Conversation[]; pending_contacts: PendingContact[];
  remarks: Record<string, string>; avatars: Record<string, string>; read_receipts: boolean;
};
export type ContactNotes = { remark: string; contact_info: string };
export type Profile = {
  user: User; relation: "own" | "contact" | "incoming" | "outgoing" | "stranger";
  avatar: string | null; cover: string | null; bio: string | null; notes: ContactNotes | null;
};
export type Attachment = { id: string; name: string; kind: "file" | "folder"; size: number; sha256: string };
export type Message = {
  seq: number; conversation_id: string; sender: User; created_at: number;
  text: string | null; attachment: Attachment | null; ai_provider?: AiProvider; recalled: boolean;
  reply_to?: number;
};
export type Draft = { text: string; quote?: Message; image?: File; nonce?: string };
export type Page = { items: Message[]; has_more: boolean };
export type Transfer = {
  attachment: Attachment; conversation_id: string; direction: "upload" | "download";
  status: "running" | "paused" | "failed" | "complete"; received: number; error: string | null;
  sharer: User | null;
};
export type FileFilter = {
  conversation_id?: string; sender_id?: string; kind?: Attachment["kind"]; name?: string; oldest_first: boolean;
};
export type ConversationRequest =
  | { kind: "direct"; contact_id: string }
  | { kind: "group"; title: string; contact_ids: string[] };
export type DropOffer = { ticket: string; name: string };
export type ShareSource = { kind: "file" | "folder" } | { kind: "dropped"; ticket: string };
export type GroupInvite = { code: string; expires_at: number };
export type GroupPreview = { id: string; title: string; member_count: number };
export type PrivateSummary = {
  id: string; conversation: string | null; host: User; invited: User[]; participants: User[];
  phase: "discovering" | "waiting" | "invited" | "authenticating" | "connected" | "missed" | "failed" | "ended";
  open: boolean; hosted: boolean;
};
export type PrivateList = { rooms: PrivateSummary[]; prompt: string | null };
export type NearbyDevice = { id: string; matched_account: User | null };
export type Discovery = { status: "starting" | "network_unavailable" | "discovery_failed" | "ready"; devices: NearbyDevice[] };
export type PrivateMessage = { seq: number; sender: User; text: string; created_at: number };
export type PrivateCode = { room: string; host: string; addresses: string[]; secret: string; expires_at: number };
type PrivateView = PrivateSummary & { connection: string; messages: PrivateMessage[] };

export const api = {
  preferences: () => invoke<Preferences>("preferences"),
  login: (server: string, handle: string, password: string, register: boolean) =>
    invoke<Session>("login", { server, handle, password, register }),
  logout: () => invoke<void>("logout"),
  directory: () => invoke<Directory>("directory"),
  sync: () => invoke<{
    directory: Directory; recalled: number[]; server_now: number; last_seq: number; latest_ai_seq: number;
  }>("sync"),
  messages: (conversation: string, before?: number, search?: string) => invoke<Page>("messages", { conversation, before, search }),
  files: (filter: FileFilter, before?: number) => invoke<Page>("files", { filter, before }),
  findUser: (handle: string) => invoke<User>("find_user", { handle }),
  addContact: (userId: string, greeting: string) => invoke<void>("add_contact", { userId, greeting }),
  respondContact: (userId: string, accept: boolean, requestId?: string | null) => invoke<void>("respond_contact", { userId, accept, requestId }),
  removeContact: (userId: string) => invoke<void>("remove_contact", { userId }),
  profile: (id: string) => invoke<Profile>("profile", { id }),
  setBio: (bio: string) => invoke<void>("set_bio", { bio }),
  setContactNotes: (id: string, notes: ContactNotes) => invoke<void>("set_contact_notes", { id, notes }),
  setProfileImage: (kind: "avatar" | "cover", bytes: ArrayBuffer) =>
    invoke<void>("set_profile_image", bytes, { headers: { "zibblu-profile-image": kind } }),
  profileImageUrl: (id: string, version: string) => convertFileSrc(`profile_${id}_${version}`, "zibblu-media"),
  readUpdate: (conversation: string, through: number | undefined, sent: number[]) =>
    invoke<number[]>("read_update", { conversation, update: { through, sent } }),
  setReadReceipts: (enabled: boolean) => invoke<void>("set_read_receipts", { enabled }),
  hideMessage: (conversation: string, seq: number) => invoke<void>("hide_message", { conversation, seq }),
  quotedMessage: (conversation: string, seq: number) => invoke<Message | null>("quoted_message", { conversation, seq }),
  create: (request: ConversationRequest) => invoke<string>("create_conversation", { request }),
  send: (conversation: string, text: string, nonce: string, replyTo?: number) =>
    invoke<void>("send_message", { conversation, message: { text, nonce, reply_to: replyTo } }),
  recall: (conversation: string, seq: number) => invoke<void>("recall_message", { conversation, seq }),
  publishAiReply: (conversation: string, reply: { nonce: string; provider: AiProvider; text: string; created_at: number; readers: string[] }) =>
    invoke<void>("publish_ai_reply", { conversation, reply }),
  groupInvite: (conversation: string) => invoke<GroupInvite>("group_invite", { conversation }),
  groupPreview: (code: string) => invoke<GroupPreview>("group_preview", { code }),
  groupJoin: (code: string) => invoke<string>("group_join", { code }),
  startPrivate: (conversation: string | null, invited: string[], open = false) =>
    invoke<string>("private_start", { conversation, invited, open }),
  nearbyDevices: () => invoke<Discovery>("private_nearby"),
  inviteNearby: (device: string, account?: string) => invoke<string>("private_invite_nearby", { device, account }),
  privateRooms: () => invoke<PrivateList>("private_list"),
  viewPrivate: (id: string, after: number) => invoke<PrivateView>("private_view", { id, after }),
  respondPrivate: (id: string, accept: boolean) => invoke<void>("private_respond", { id, accept }),
  sendPrivate: (id: string, text: string) => invoke<void>("private_send", { id, text }),
  endPrivate: (id: string) => invoke<void>("private_end", { id }),
  privateCode: (id: string) => invoke<PrivateCode>("private_qr", { id }),
  scanPrivate: (code: PrivateCode) => invoke<string>("private_scan", { code }),
  setPrivateQuiet: (quiet: boolean) => invoke<void>("set_private_quiet", { quiet }),
  share: (conversation: string, source: ShareSource) => invoke<Transfer | null>("share", { conversation, source }),
  pasteImage: (conversation: string, bytes: ArrayBuffer) =>
    invoke<Transfer>("paste_image", bytes, { headers: { "zibblu-conversation": conversation } }),
  mediaUrl: (id: string) => convertFileSrc(id, "zibblu-media"),
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
