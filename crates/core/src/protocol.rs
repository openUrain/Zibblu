use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

pub const CHUNK_SIZE: u64 = 1024 * 1024;
pub const MAX_ATTACHMENT_SIZE: u64 = 256 * CHUNK_SIZE;
pub const MAX_PASTED_IMAGE_BYTES: usize = 16 * CHUNK_SIZE as usize;
pub const SYNC_PAGE_SIZE: usize = 100;
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;
pub const MAX_AI_PROMPT_BYTES: usize = 8 * 1024;
pub const MAX_AI_REPLY_BYTES: usize = 24 * 1024;
pub const RECALL_WINDOW_MS: i64 = 5 * 60 * 1000;
pub const MAX_GROUP_MEMBERS: usize = 16;
pub const MAX_PROFILE_IMAGE_BYTES: usize = 2 * CHUNK_SIZE as usize;
pub const MAX_RECEIPT_QUERY: usize = 200;
pub const LAN_NEARBY_NAME: &str = "nearby.zibblu.invalid";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: Uuid,
    pub handle: String,
}

#[derive(Serialize, Deserialize)]
pub struct Credentials {
    pub handle: String,
    pub password: String,
}

#[derive(Serialize, Deserialize)]
pub struct DeviceLogin {
    #[serde(flatten)]
    pub credentials: Credentials,
    pub device_key: String,
}

#[derive(Serialize, Deserialize)]
pub struct Session {
    pub user: User,
    pub token: String,
    pub device_id: Uuid,
    pub cursor: Uuid,
}

#[derive(Serialize, Deserialize)]
pub struct DeviceSession {
    pub user: User,
    pub device_id: Uuid,
    pub cursor: Uuid,
}

#[derive(Serialize, Deserialize)]
pub struct Device {
    pub id: Uuid,
    pub last_seen_at: i64,
    pub current: bool,
}

#[derive(Serialize, Deserialize)]
pub struct LanCredentialRequest {
    pub public_key: Vec<u8>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct LanCredential {
    pub certificate: Vec<u8>,
    pub authority: Vec<u8>,
    pub expires_at: i64,
}

pub fn lan_peer_name(account: Uuid) -> String {
    format!("{}.zibblu.invalid", account.simple())
}

#[derive(Serialize, Deserialize)]
pub struct ContactRequest {
    pub user_id: Uuid,
    pub request_id: Uuid,
    #[serde(default)]
    pub greeting: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingContact {
    pub user: User,
    pub incoming: bool,
    #[serde(default)]
    pub request_id: Option<Uuid>,
    #[serde(default)]
    pub greeting: String,
}

#[derive(Serialize, Deserialize)]
pub struct ContactDecision {
    pub accept: bool,
    #[serde(default)]
    pub request_id: Option<Uuid>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileRelation {
    Own,
    Contact,
    Incoming,
    Outgoing,
    Stranger,
}

#[derive(Serialize, Deserialize)]
pub struct Profile {
    pub user: User,
    #[serde(default)]
    pub display_name: Option<String>,
    pub relation: ProfileRelation,
    pub avatar: Option<Uuid>,
    pub cover: Option<Uuid>,
    pub bio: Option<String>,
    pub notes: Option<ContactNotes>,
}

#[derive(Serialize, Deserialize)]
pub struct ContactNotes {
    pub remark: String,
    pub contact_info: String,
}

#[derive(Serialize, Deserialize)]
pub struct ProfileBio {
    pub bio: String,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileImage {
    Avatar,
    Cover,
}

impl ProfileImage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Avatar => "avatar",
            Self::Cover => "cover",
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct ReadPreference {
    pub enabled: bool,
}

#[derive(Serialize, Deserialize)]
pub struct ReadUpdate {
    pub through: Option<i64>,
    pub sent: Vec<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewed: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptStatus {
    Delivered,
    Read,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageReceipt {
    pub seq: i64,
    pub status: ReceiptStatus,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub enum ReadResult {
    Latest { receipt: Option<MessageReceipt> },
    Legacy(Vec<i64>),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Conversation {
    pub id: Uuid,
    pub title: Option<String>,
    pub members: Vec<User>,
    pub activity_at: i64,
    pub preview: Option<String>,
    #[serde(default)]
    pub preview_seq: Option<i64>,
    #[serde(default)]
    pub latest_seq: i64,
    #[serde(default)]
    pub unread: i64,
    #[serde(default)]
    pub read_through: i64,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConversationRequest {
    Direct {
        contact_id: Uuid,
    },
    Group {
        title: String,
        contact_ids: Vec<Uuid>,
    },
}

#[derive(Serialize, Deserialize)]
pub struct ConversationCreated {
    pub id: Uuid,
}

#[derive(Serialize, Deserialize)]
pub struct GroupInvite {
    pub code: Uuid,
    pub expires_at: i64,
}

#[derive(Serialize, Deserialize)]
pub struct GroupInviteRequest {
    pub code: Uuid,
}

#[derive(Serialize, Deserialize)]
pub struct GroupInvitePreview {
    pub id: Uuid,
    pub title: String,
    pub member_count: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub seq: i64,
    pub conversation_id: Uuid,
    pub sender: User,
    pub created_at: i64,
    pub text: Option<String>,
    pub attachment: Option<Attachment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_provider: Option<AiProvider>,
    #[serde(default)]
    pub recalled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nonce: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expired: Option<ExpiredPayload>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<PendingSend>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub local_file: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpiredPayload {
    Text,
    File,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingSend {
    pub error: Option<String>,
    pub blocked: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Publication {
    pub seq: i64,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub recalled: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AiProvider {
    OpenAi,
    DeepSeek,
    Grok,
}

impl AiProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "open_ai",
            Self::DeepSeek => "deep_seek",
            Self::Grok => "grok",
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct PublishAiReply {
    pub nonce: Uuid,
    pub provider: AiProvider,
    pub text: String,
    pub created_at: i64,
    pub readers: Vec<Uuid>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MessageRecall {
    pub seq: i64,
    pub message_seq: i64,
}

#[derive(Serialize, Deserialize)]
pub struct SendMessage {
    pub nonce: Uuid,
    pub text: String,
    #[serde(default)]
    pub reply_to: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    File,
    Folder,
}

impl AttachmentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Folder => "folder",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub id: Uuid,
    pub name: String,
    pub kind: AttachmentKind,
    pub size: u64,
    pub sha256: String,
}

impl Attachment {
    pub fn is_valid(&self) -> bool {
        self.size <= MAX_ATTACHMENT_SIZE
            && self.sha256.len() == 64
            && self
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            && safe_filename(&self.name)
    }

    pub fn preview_mime(&self) -> Option<&'static str> {
        if self.kind != AttachmentKind::File || self.size == 0 {
            return None;
        }
        match self.name.rsplit_once('.')?.1.to_ascii_lowercase().as_str() {
            "png" | "apng" => Some("image/png"),
            "jpg" | "jpeg" => Some("image/jpeg"),
            "gif" => Some("image/gif"),
            "webp" => Some("image/webp"),
            "bmp" => Some("image/bmp"),
            "mp4" | "m4v" => Some("video/mp4"),
            "webm" => Some("video/webm"),
            "mov" => Some("video/quicktime"),
            _ => None,
        }
    }
}

pub fn safe_filename(name: &str) -> bool {
    if name.is_empty()
        || name.encode_utf16().count() > 255
        || name.ends_with(['.', ' '])
        || name
            .chars()
            .any(|c| c.is_control() || "/\\:*?\"<>|".contains(c))
    {
        return false;
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_ascii_uppercase();
    !matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) && !stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"))
        .is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct UploadProgress {
    pub received: u64,
    pub complete: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Directory {
    #[serde(default)]
    pub names: std::collections::HashMap<Uuid, String>,
    pub contacts: Vec<User>,
    pub conversations: Vec<Conversation>,
    #[serde(default)]
    pub pending_contacts: Vec<PendingContact>,
    #[serde(default)]
    pub remarks: HashMap<Uuid, String>,
    #[serde(default)]
    pub avatars: HashMap<Uuid, Uuid>,
    #[serde(default)]
    pub read_receipts: bool,
}

#[derive(Serialize, Deserialize)]
pub struct SyncPage {
    pub directory: Directory,
    pub events: Vec<SyncEvent>,
    pub server_time: i64,
    pub page_id: Uuid,
    pub has_more: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum SyncEvent {
    Published(Message),
    Recalled(MessageRecall),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncAck {
    pub page_id: Uuid,
    pub metadata_received: Vec<i64>,
}

#[derive(Serialize, Deserialize)]
pub struct SyncPosition {
    pub cursor: Uuid,
}

#[derive(Serialize, Deserialize)]
pub struct ContentAck {
    pub sha256: String,
}

pub fn valid_handle(handle: &str) -> bool {
    (3..=32).contains(&handle.len())
        && handle.as_bytes()[0].is_ascii_alphabetic()
        && handle
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
