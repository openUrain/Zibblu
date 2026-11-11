use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const CHUNK_SIZE: u64 = 1024 * 1024;
pub const MAX_ATTACHMENT_SIZE: u64 = 256 * CHUNK_SIZE;
pub const SYNC_PAGE_SIZE: usize = 100;
pub const MAX_MESSAGE_BYTES: usize = 8_192;
pub const MAX_GROUP_MEMBERS: usize = 16;

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
pub struct Session {
    pub user: User,
    pub token: String,
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
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Conversation {
    pub id: Uuid,
    pub title: Option<String>,
    pub members: Vec<User>,
    pub activity_at: i64,
    pub preview: Option<String>,
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
}

#[derive(Serialize, Deserialize)]
pub struct SendMessage {
    pub nonce: Uuid,
    pub text: String,
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
    pub contacts: Vec<User>,
    pub conversations: Vec<Conversation>,
}

#[derive(Serialize, Deserialize)]
pub struct SyncPage {
    pub directory: Directory,
    pub messages: Vec<Message>,
}

pub fn valid_handle(handle: &str) -> bool {
    (3..=32).contains(&handle.len())
        && handle.as_bytes()[0].is_ascii_lowercase()
        && handle
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}
