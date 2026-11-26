import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import type { AiProvider, AiSource } from "./ai-api";
import type { VideoExportProgress, VideoProbeInfo } from "./media-viewer-types";

export type User = { id: string; handle: string };
export type Session = { user: User; server: string; notice?: string };
export type Device = { id: string; last_seen_at: number; current: boolean };
export type SavedAccount = { user: User; server: string; avatar: number[] | null };
export type CloseAction = "exit" | "tray";
export type Preferences = { server: string; official_server: string; private_quiet: boolean; close_action?: CloseAction; saved_account?: SavedAccount };
export type Conversation = {
  id: string; title: string | null; members: User[]; activity_at: number; preview: string | null;
  preview_seq: number | null; latest_seq: number; unread: number; read_through: number; joined_after?: number;
  pinned: boolean; manual_unread: boolean; muted: boolean;
};
export type ConversationStorage = {
  conversation_id: string;
  media_bytes: number;
  chat_data_bytes: number;
  chat_data_estimated: boolean;
  files_bytes: number;
  total_bytes: number;
};
export type StorageStats = {
  app_bytes: number;
  conversation_media_bytes: number;
  database_bytes: number;
  public_cache_and_temp_bytes: number;
  outbox_bytes: number;
  total_bytes: number;
  conversations: ConversationStorage[];
};
export type CacheCleanupReport = {
  removed_public_files: number;
  removed_temp_files: number;
  reclaimed_bytes: number;
};
export type MessageReceipt = { seq: number; status: "delivered" | "read" };
export type PendingContact = { user: User; incoming: boolean; request_id: string | null; greeting: string };
export type Directory = {
  contacts: User[]; conversations: Conversation[]; pending_contacts: PendingContact[];
  remarks: Record<string, string>; names: Record<string, string>; avatars: Record<string, string>; read_receipts: boolean;
};
export type ContactNotes = { remark: string; contact_info: string };
export type Profile = {
  user: User; relation: "own" | "contact" | "incoming" | "outgoing" | "stranger";
  display_name: string | null; avatar: string | null; cover: string | null; bio: string | null; notes: ContactNotes | null;
};
export type CachedProfile = { profile: Profile; expires_at: number };
export type Attachment = { id: string; name: string; kind: "file" | "folder"; size: number; sha256: string };
export type ImageSize = [number, number];
export type Message = {
  seq: number; conversation_id: string; sender: User; created_at: number;
  text: string | null; attachment: Attachment | null; ai_provider?: AiProvider; ai_sources?: AiSource[]; recalled: boolean;
  reply_to?: number;
  nonce?: string;
  expires_at?: number;
  expired?: "text" | "file";
  pending?: { error: string | null; blocked: boolean };
  local_file?: boolean;
  preview_size?: ImageSize;
};
export type DraftAttachment = {
  id: string;
  name: string;
  kind: "file" | "folder";
  size: number;
  media_kind?: "image" | "video";
  can_preview: boolean;
  error?: string;
};
export type AddDraftsResult = {
  added: DraftAttachment[];
  skipped_duplicates: number;
  errors: string[];
};
export type Draft = { text: string; quote?: Message; attachments?: DraftAttachment[]; nonce?: string };
export type Page = { items: Message[]; has_more: boolean };
export type Transfer = {
  attachment: Attachment; conversation_id: string; direction: "upload" | "download";
  status: "running" | "queued" | "paused" | "failed" | "expired" | "complete"; received: number; error: string | null;
  sharer: User | null;
  created_at?: number;
};
export type OriginalDownloadStatus = {
  attachment_id: string;
  status: "downloading" | "verifying" | "complete" | "failed" | "cancelled";
  received: number;
  total: number;
  error?: string;
};
export type ViewerVideoState = {
  has_local_original: boolean;
  has_local_preview: boolean;
  has_local_cover: boolean;
  original_size: number;
  original_download?: OriginalDownloadStatus;
};
export type ViewerSelection = {
  conversation_id: string;
  seq: number;
  file: Attachment;
  media_kind: "image" | "video";
  previous: boolean;
  next: boolean;
  download?: Transfer;
  video?: ViewerVideoState;
  draft?: boolean;
};
export type ForwardTarget = {
  conversation_id: string;
  attachment_id: string;
};
export type ViewerForwardItem = {
  target: ForwardTarget;
  status: "sent" | "queued" | "failed" | "cancelled";
  error?: string | null;
  cache_error?: string | null;
};
export type ViewerForwardTargets = {
  own_id: string;
  current_conversation_id: string;
  directory: Directory;
};
export type FileFilter = {
  conversation_id?: string; sender_id?: string; kind?: Attachment["kind"]; name?: string; oldest_first: boolean;
  include_transfer_rows?: boolean;
};
export type ConversationRequest =
  | { kind: "direct"; contact_id: string }
  | { kind: "group"; title: string; contact_ids: string[] };
export type DropOffer = { ticket: string; names: string[] };
export type ShareSource = { kind: "file" | "folder" } | { kind: "dropped"; ticket: string };
export type GroupInvite = { code: string; expires_at: number };
export type GroupPreview = { id: string; title: string; member_count: number };
export type PrivateSummary = {
  id: string; conversation: string | null; host: User; invited: User[]; participants: User[];
  phase: "discovering" | "waiting" | "invited" | "authenticating" | "connected" | "suspended" | "reconnecting" | "missed" | "failed" | "ended";
  open: boolean; hosted: boolean; unread: boolean;
};
export type PrivateList = { rooms: PrivateSummary[]; prompt: string | null };
export type NearbyDevice = { id: string; matched_account: User | null };
export type Discovery = { status: "starting" | "network_unavailable" | "permission_denied" | "discovery_failed" | "ready"; devices: NearbyDevice[] };
export type PrivateMessage = { seq: number; sender: User; text: string; created_at: number };
export type PrivateCode = { room: string; host: string; addresses: string[]; secret: string; expires_at: number };
type PrivateView = PrivateSummary & { connection: string; messages: PrivateMessage[] };
export type SyncUpdate = {
  directory: Directory; recalled: number[]; deleted: number[]; server_now: number; revision: number; latest_ai_seq: number;
};

export const api = {
  preferences: () => invoke<Preferences>("preferences"),
  setCloseAction: (action: CloseAction) => invoke<void>("set_close_action", { action }),
  login: (server: string, handle: string, password: string, register: boolean) =>
    invoke<Session>("login", { server, handle, password, register }),
  restoreSession: (account: SavedAccount) => invoke<Session>("restore_session", { accountId: account.user.id, server: account.server }),
  logout: () => invoke<void>("logout"),
  directory: () => invoke<Directory>("directory"),
  membershipConversations: () => invoke<Conversation[]>("membership_conversations"),
  sync: () => invoke<SyncUpdate>("sync"),
  localUpdate: () => invoke<SyncUpdate>("local_update"),
  devices: () => invoke<Device[]>("devices"),
  retireDevice: (id: string) => invoke<void>("retire_device", { id }),
  messages: (conversation: string, before?: number, search?: string) => invoke<Page>("messages", { conversation, before, search }),
  files: (filter: FileFilter, before?: number) => invoke<Page>("files", { filter, before }),
  findUser: (handle: string) => invoke<User>("find_user", { handle }),
  addContact: (userId: string, greeting: string) => invoke<void>("add_contact", { userId, greeting }),
  respondContact: (userId: string, accept: boolean, requestId?: string | null) => invoke<void>("respond_contact", { userId, accept, requestId }),
  removeContact: (userId: string) => invoke<void>("remove_contact", { userId }),
  profile: (id: string) => invoke<CachedProfile>("profile", { id }),
  cachedProfile: (id: string) => invoke<CachedProfile | null>("cached_profile", { id }),
  setDisplayName: (name: string) => invoke<void>("set_display_name", { name }),
  setBio: (bio: string) => invoke<void>("set_bio", { bio }),
  setContactNotes: (id: string, notes: ContactNotes) => invoke<void>("set_contact_notes", { id, notes }),
  pickImage: (forProfile: boolean) => invoke<ArrayBuffer | number[]>("pick_image", { forProfile })
    .then(bytes => Array.isArray(bytes) ? new Uint8Array(bytes).buffer : bytes),
  setProfileImage: (kind: "avatar" | "cover", bytes: ArrayBuffer) =>
    invoke<string>("set_profile_image", bytes, { headers: { "zibblu-profile-image": kind } }),
  copyText: (text: string) => invoke<void>("copy_text", { text }),
  pasteText: () => invoke<string | null>("paste_text"),
  scanIosQr: () => invoke<string | null>("ios_scan_qr"),
  profileImageUrl: (id: string, version: string) => convertFileSrc(`profile_${id}_${version}`, "zibblu-media"),
  previewImageBytes: (id: string, profileVersion?: string) => invoke<ArrayBuffer>("preview_image_bytes", { id, profileVersion }),
  previewVideoCoverBytes: (id: string) => invoke<ArrayBuffer>("preview_video_cover_bytes", { id }),
  uploadVideoCover: (id: string) => invoke<ArrayBuffer>("upload_video_cover", { id }),
  privatePreview: (enabled: boolean) => invoke<void>("private_preview", { enabled }),
  uploadImageSize: (id: string) => invoke<ImageSize>("upload_image_size", { id }),
  readUpdate: (conversation: string, through?: number, sent?: number, viewed?: number) =>
    invoke<MessageReceipt | null>("read_update", { conversation, update: { through, sent: sent === undefined ? [] : [sent], viewed } }),
  setReadReceipts: (enabled: boolean) => invoke<void>("set_read_receipts", { enabled }),
  hideMessage: (conversation: string, seq: number) => invoke<void>("hide_message", { conversation, seq }),
  hidePendingUpload: (conversation: string, id: string) => invoke<void>("hide_pending_upload", { conversation, id }),
  clearHistory: (conversation: string) => invoke<void>("clear_history", { conversation }),
  deleteConversation: (conversation: string) => invoke<void>("delete_conversation", { conversation }),
  setConversationPinned: (conversation: string, pinned: boolean) =>
    invoke<void>("set_conversation_pinned", { conversation, pinned }),
  setConversationManualUnread: (conversation: string, unread: boolean) =>
    invoke<void>("set_conversation_manual_unread", { conversation, unread }),
  setConversationMuted: (conversation: string, muted: boolean) =>
    invoke<void>("set_conversation_muted", { conversation, muted }),
  hideConversation: (conversation: string) => invoke<void>("hide_conversation", { conversation }),
  unhideConversation: (conversation: string) => invoke<void>("unhide_conversation", { conversation }),
  hiddenGroupConversations: () => invoke<Conversation[]>("hidden_group_conversations"),
  markConversationReadLocal: (conversation: string) =>
    invoke<void>("mark_conversation_read_local", { conversation }),
  openChatWindow: (conversation: string) => invoke<void>("open_chat_window", { conversation }),
  storageStats: () => invoke<StorageStats>("storage_stats"),
  clearCache: () => invoke<CacheCleanupReport>("clear_cache"),
  takeSendFailureNotices: () => invoke<number>("take_send_failure_notices"),
  quotedMessage: (conversation: string, seq: number) => invoke<Message | null>("quoted_message", { conversation, seq }),
  create: (request: ConversationRequest) => invoke<string>("create_conversation", { request }),
  send: (conversation: string, text: string, nonce: string, replyTo?: number) =>
    invoke<void>("send_message", { conversation, message: { text, nonce, reply_to: replyTo } }),
  retryMessage: (nonce: string) => invoke<void>("retry_message", { nonce }),
  recall: (conversation: string, seq: number) => invoke<void>("recall_message", { conversation, seq }),
  publishAiReply: (conversation: string, reply: { nonce: string; provider: AiProvider; text: string; created_at: number; readers: string[] }) =>
    invoke<void>("publish_ai_reply", { conversation, reply }),
  groupInvite: (conversation: string) => invoke<GroupInvite>("group_invite", { conversation }),
  groupPreview: (code: string) => invoke<GroupPreview>("group_preview", { code }),
  groupJoin: (code: string) => invoke<string>("group_join", { code }),
  leaveGroup: (conversation: string) => invoke<void>("leave_group", { conversation }),
  startPrivate: (conversation: string | null, invited: string[], open = false) =>
    invoke<string>("private_start", { conversation, invited, open }),
  nearbyDevices: () => invoke<Discovery>("private_nearby"),
  inviteNearby: (device: string, account?: string) => invoke<string>("private_invite_nearby", { device, account }),
  privateRooms: () => invoke<PrivateList>("private_list"),
  viewPrivate: (id: string, after: number) => invoke<PrivateView>("private_view", { id, after }),
  markPrivateRead: (id: string, connection: string, through: number) =>
    invoke<boolean>("private_mark_read", { id, connection, through }),
  respondPrivate: (id: string, accept: boolean) => invoke<void>("private_respond", { id, accept }),
  sendPrivate: (id: string, text: string) => invoke<void>("private_send", { id, text }),
  endPrivate: (id: string) => invoke<void>("private_end", { id }),
  privateCode: (id: string) => invoke<PrivateCode>("private_qr", { id }),
  scanPrivate: (code: PrivateCode) => invoke<string>("private_scan", { code }),
  setPrivateQuiet: (quiet: boolean) => invoke<void>("set_private_quiet", { quiet }),
  offerClipboardFiles: () => invoke<DropOffer | null>("offer_clipboard_files"),
  cancelAttachmentOffer: (ticket: string) => invoke<void>("cancel_attachment_offer", { ticket }),
  share: (conversation: string, source: ShareSource) => invoke<void>("share", { conversation, source }),
  addClipboardFiles: (conversation: string) => invoke<AddDraftsResult | null>("add_clipboard_files", { conversation }),
  addDroppedFiles: (conversation: string, ticket: string) => invoke<AddDraftsResult>("add_dropped_files", { conversation, ticket }),
  pickDraftAttachments: (conversation: string, kind: "file" | "folder") =>
    invoke<AddDraftsResult | null>("pick_draft_attachments", { conversation, kind }),
  addPastedImageDraft: (conversation: string, bytes: ArrayBuffer) =>
    invoke<DraftAttachment>("add_pasted_image_draft", bytes, { headers: { "zibblu-conversation": conversation } }),
  removeDraftAttachment: (id: string) => invoke<void>("remove_draft_attachment", { id }),
  enqueueDraftAttachment: (conversation: string, id: string) =>
    invoke<Transfer>("enqueue_draft_attachment", { conversation, id }),
  draftMediaUrl: (id: string, variant?: "thumbnail") =>
    convertFileSrc("draft_" + id, "zibblu-media") + (variant ? "?" + variant : ""),
  checkPastedImage: (bytes: ArrayBuffer) => invoke<string>("check_pasted_image", bytes),
  readClipboardImage: () => invoke<ArrayBuffer | number[]>("read_clipboard_image")
    .then(bytes => Array.isArray(bytes) ? new Uint8Array(bytes).buffer : bytes),
  pasteImage: (conversation: string, bytes: ArrayBuffer) =>
    invoke<Transfer>("paste_image", bytes, { headers: { "zibblu-conversation": conversation } }),
  mediaUrl: (id: string, variant?: "thumbnail" | "cached-thumbnail" | "video-cover" | "video-stream" | "video-original") =>
    convertFileSrc(id, "zibblu-media") + (variant ? "?" + variant : ""),
  openImage: (id: string) => invoke<void>("open_image_viewer", { id }),
  openDraftImage: (id: string) => invoke<void>("open_draft_image_viewer", { id }),
  viewerDownloadOriginalVideo: (id: string) => invoke<OriginalDownloadStatus>("viewer_download_original_video", { id }),
  viewerCancelOriginalVideo: (id: string) => invoke<void>("viewer_cancel_original_video", { id }),
  viewerVideoProbe: (id: string) => invoke<VideoProbeInfo>("viewer_video_probe", { id }),
  viewerVideoThumbnails: (id: string, count?: number) => invoke<string[]>("viewer_video_thumbnails", { id, count }),
  viewerStartVideoExport: (id: string, startSec: number, endSec: number, durationSec: number) =>
    invoke<VideoExportProgress>("viewer_start_video_export", { id, startSec, endSec, durationSec }),
  viewerVideoExportStatus: (exportId: string) =>
    invoke<VideoExportProgress | null>("viewer_video_export_status", { exportId }),
  viewerCancelVideoExport: (exportId: string) =>
    invoke<VideoExportProgress | null>("viewer_cancel_video_export", { exportId }),
  viewerForwardTargets: () => invoke<ViewerForwardTargets>("viewer_forward_targets"),
  viewerResolveDirectTarget: (contactId: string) =>
    invoke<string>("viewer_resolve_direct_target", { contactId }),
  viewerSendUnedited: (sourceId: string, batchId: string, targets: ForwardTarget[]) =>
    invoke<ViewerForwardItem[]>("viewer_send_unedited", { sourceId, batchId, targets }),
  viewerSendAnnotatedImage: (
    sourceId: string,
    batchId: string,
    targets: ForwardTarget[],
    fileName: string,
    bytes: Uint8Array,
  ) => {
    const metaBytes = new TextEncoder().encode(JSON.stringify({ sourceId, batchId, targets, fileName }));
    let binary = "";
    for (let i = 0; i < metaBytes.length; i += 0x8000) {
      binary += String.fromCharCode(...metaBytes.subarray(i, i + 0x8000));
    }
    return invoke<ViewerForwardItem[]>("viewer_send_annotated_image", new Uint8Array(bytes), {
      headers: { "zibblu-forward-meta": btoa(binary) },
    });
  },
  viewerSendTrimmedVideo: (
    sourceId: string,
    exportId: string,
    batchId: string,
    targets: ForwardTarget[],
    fileName: string,
  ) =>
    invoke<ViewerForwardItem[]>("viewer_send_trimmed_video", {
      sourceId,
      exportId,
      batchId,
      targets,
      fileName,
    }),
  viewerSaveEditedImage: (id: string, bytes: Uint8Array) =>
    invoke<boolean>("viewer_save_edited_image", bytes, { headers: { "zibblu-source-id": id } }),
  save: (id: string) => invoke<Transfer | null>("save_attachment", { id }),
  openSaved: (id: string) => invoke<boolean>("open_saved_attachment", { id }),
  transfers: () => invoke<Transfer[]>("transfers"),
  transferAction: (action: "pause" | "resume" | "discard", transfer: Transfer) =>
    invoke<void>(action + "_transfer", { id: transfer.attachment.id, direction: transfer.direction }),
};

export function fileExpired(message: Message, now = Date.now()): boolean {
  return !message.local_file && (message.expired === "file"
    || (message.attachment !== null && message.expires_at !== undefined && message.expires_at <= now));
}

export function isExpired(error: unknown): boolean {
  return typeof error === "object" && error !== null && "expired" in error && error.expired === true;
}

export function isSourceChanged(error: unknown): boolean {
  return typeof error === "object" && error !== null && "code" in error && error.code === "source_changed";
}

export function errorMessage(error: unknown): string {
  if (typeof error === "object" && error !== null && "message" in error && typeof error.message === "string") {
    return error.message;
  }
  return typeof error === "string" ? error : "Something went wrong. Try again.";
}
