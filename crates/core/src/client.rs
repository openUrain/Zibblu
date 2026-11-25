mod ai_context;
mod ai_prompts;
mod chat_prefs;
mod device;
mod local_sync;
mod media_store;
mod outbox;
mod preview_cache;
mod profile_cache;
mod staging;
mod storage_stats;
mod transfer;

pub use ai_context::{AiContextMessage, AiScope, AuthorizedAiAttachment};
pub use ai_prompts::AiLocalPrompt;
pub use chat_prefs::{ConversationPrefState, sort_conversations};
pub use media_store::{
    CacheCleanupReport, ConsistencyReport, MediaDescriptor, MediaVariant, PublicResourceKind,
    TempFileGuard,
};
pub use profile_cache::CachedProfile;
pub use preview_cache::{IMAGE_THUMBNAIL, VideoStreamChunk, VideoStreamInfo};
pub use staging::{
    FileIdentity, FolderEntrySnapshot, SourceChangedError, SourceSnapshot,
    capture_source_snapshot, open_verified_local_file, verify_file_sample, verify_open_handle,
};
pub use storage_stats::{ConversationStorage, StorageStats};
pub use transfer::{Direction, OriginalDownloadStatus, Transfer, TransferStatus, ViewerForwardItem};

use crate::protocol::*;
use anyhow::{Context, Result, bail, ensure};
use reqwest::{Method, RequestBuilder, Response, StatusCode, Url};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use std::{
    collections::HashMap,
    error::Error as _,
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;
use transfer::{Active, JobKey};
use uuid::Uuid;

const LOCAL_PAGE: usize = 60;

#[derive(Debug, thiserror::Error)]
pub enum OnlineError {
    #[error("{}", network_error_message(.0))]
    Network(#[from] reqwest::Error),
    #[error("{message}")]
    Rejected { status: StatusCode, message: String },
    #[error("The service returned an invalid response.")]
    InvalidResponse,
}

fn network_error_message(error: &reqwest::Error) -> &'static str {
    let mut source = error.source();
    while let Some(cause) = source {
        if matches!(
            cause.downcast_ref::<rustls::Error>(),
            Some(rustls::Error::InvalidCertificate(_) | rustls::Error::NoCertificatesPresented)
        ) {
            return "The server's security certificate could not be verified. Check the address and your system clock, or contact the server administrator.";
        }
        source = match cause
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::get_ref)
        {
            Some(inner) => Some(inner),
            None => cause.source(),
        };
    }
    if error.is_timeout() {
        return "The Zibblu server took too long to respond. Try again in a moment.";
    }
    "Could not reach the Zibblu server. Check your connection and that the server is online."
}

pub struct Client {
    pub user: User,
    device_id: Uuid,
    api: Api,
    db: SqlitePool,
    staging: PathBuf,
    managed_files: PathBuf,
    media_root: PathBuf,
    active_tmp: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    media_locks: [Mutex<()>; 16],
    video_stream_locks: [Mutex<()>; 16],
    history_revision: AtomicI64,
    device_re_registered: bool,
    sync_gate: Mutex<()>,
    profile_gate: Mutex<()>,
    outbox_gate: Mutex<()>,
    prepare_gate: Mutex<()>,
    forward_batch_gate: Mutex<()>,
    workers: Mutex<HashMap<JobKey, Active>>,
    original_video_jobs: Mutex<HashMap<Uuid, transfer::OriginalVideoJob>>,
    closed: AtomicBool,
    server_clock_offset: AtomicI64,
    _profile_lock: File,
}

#[derive(Serialize)]
pub struct Page {
    pub items: Vec<Message>,
    pub has_more: bool,
}

#[derive(Serialize)]
pub struct SyncUpdate {
    pub directory: Directory,
    pub recalled: Vec<i64>,
    pub deleted: Vec<i64>,
    pub server_now: i64,
    pub last_seq: i64,
    pub revision: i64,
    pub latest_ai_seq: i64,
}

#[derive(Default, Deserialize)]
pub struct FileFilter {
    pub conversation_id: Option<Uuid>,
    pub sender_id: Option<Uuid>,
    pub kind: Option<AttachmentKind>,
    pub start_at: Option<i64>,
    pub end_before: Option<i64>,
    pub name: Option<String>,
    #[serde(default)]
    pub oldest_first: bool,
    #[serde(default)]
    pub include_transfer_rows: bool,
}

impl Client {
    pub async fn login(
        data_dir: &Path,
        address: &str,
        credentials: Credentials,
        register: bool,
    ) -> Result<Arc<Self>> {
        Self::sign_in(data_dir, Api::new(address)?, credentials, register).await
    }

    pub async fn restore(
        data_dir: &Path,
        address: &str,
        account_id: Uuid,
        token: &str,
    ) -> Result<Arc<Self>> {
        let invalid = || {
            OnlineError::Rejected {
            status: StatusCode::UNAUTHORIZED,
            message: "The saved sign-in has expired or is no longer valid. Enter your password to sign in.".into(),
        }
        };
        ensure!(
            token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit()),
            invalid()
        );
        let mut api = Api::new(address)?;
        api.token = Some(token.to_owned());
        let session: DeviceSession = match api.json(api.request(Method::GET, "/v2/session")).await {
            Ok(session) => session,
            Err(OnlineError::Rejected {
                status:
                    StatusCode::UNAUTHORIZED | StatusCode::NOT_FOUND | StatusCode::UPGRADE_REQUIRED,
                ..
            }) => return Err(invalid().into()),
            Err(error) => return Err(error.into()),
        };
        ensure!(session.user.id == account_id, invalid());
        let profile = Self::profile_at(data_dir, &api, account_id);
        ensure!(profile.join("chat.sqlite3").is_file(), invalid());
        let client = Self::open(data_dir, api, session, true).await?;
        client.reconcile_history().await?;
        Ok(client)
    }

    async fn open(
        data_dir: &Path,
        api: Api,
        session: DeviceSession,
        restore: bool,
    ) -> Result<Arc<Self>> {
        let profile = Self::profile_at(data_dir, &api, session.user.id);
        let staging = profile.join("transfers");
        let managed_files = profile.join("files");
        let media_root = profile.join("media").join(session.device_id.to_string());
        std::fs::create_dir_all(media_store::fs_path(&staging))
            .context("Could not create the local profile")?;
        std::fs::create_dir_all(media_store::fs_path(&managed_files))
            .context("Could not create the local file directory")?;
        std::fs::create_dir_all(media_store::fs_path(&media_root.join("conversations")))
            .context("Could not create the conversation media directory")?;
        std::fs::create_dir_all(media_store::fs_path(&media_root.join("public")))
            .context("Could not create the public cache directory")?;
        std::fs::create_dir_all(media_store::fs_path(&media_root.join("tmp")))
            .context("Could not create the media temp directory")?;
        let lock = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(profile.join("profile.lock"))?;
        lock.try_lock()
            .context("This account profile is already open in another Zibblu window")?;
        let db = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(profile.join("chat.sqlite3"))
                    .create_if_missing(true)
                    .journal_mode(SqliteJournalMode::Wal)
                    .synchronous(SqliteSynchronous::Full)
                    .busy_timeout(Duration::from_secs(5)),
            )
            .await
            .context("Could not open the local chat history")?;
        let (device_re_registered, previous_device_id) =
            Self::initialize_history(&db, &session, restore).await?;
        let client = Arc::new(Self {
            user: session.user,
            device_id: session.device_id,
            api,
            db,
            staging,
            managed_files,
            media_root,
            active_tmp: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            media_locks: std::array::from_fn(|_| Mutex::new(())),
            video_stream_locks: std::array::from_fn(|_| Mutex::new(())),
            history_revision: AtomicI64::new(1),
            device_re_registered,
            _profile_lock: lock,
            sync_gate: Mutex::new(()),
            profile_gate: Mutex::new(()),
            outbox_gate: Mutex::new(()),
            prepare_gate: Mutex::new(()),
            forward_batch_gate: Mutex::new(()),
            workers: Mutex::new(HashMap::new()),
            original_video_jobs: Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
            server_clock_offset: AtomicI64::new(0),
        });
        client
            .migrate_legacy_media_if_needed(previous_device_id)
            .await?;
        client.recover_media_reclaims().await?;
        client.migrate_hidden_history().await?;
        client.restore_upload_times().await?;
        Ok(client)
    }

    pub fn server(&self) -> &str {
        self.api.base.as_str()
    }

    pub fn device_re_registered(&self) -> bool {
        self.device_re_registered
    }

    pub fn session_token(&self) -> &str {
        self.api.token.as_deref().expect("authenticated client")
    }

    pub async fn lan_credential(&self, public_key: Vec<u8>) -> Result<LanCredential> {
        Ok(self
            .api
            .json(
                self.api
                    .request(Method::POST, "/v1/lan/credential")
                    .json(&LanCredentialRequest { public_key }),
            )
            .await?)
    }

    pub fn server_now(&self) -> i64 {
        unix_millis().saturating_add(self.server_clock_offset.load(Ordering::Relaxed))
    }

    pub async fn messages(&self, conversation: Uuid, before: Option<i64>) -> Result<Page> {
        self.search_messages(conversation, before, None).await
    }

    pub async fn search_messages(
        &self,
        conversation: Uuid,
        before: Option<i64>,
        search: Option<&str>,
    ) -> Result<Page> {
        let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
            "SELECT payload FROM timeline WHERE seq NOT IN (SELECT seq FROM hidden_messages) AND conversation_id = ",
        );
        query.push_bind(conversation.to_string());
        if let Some(before) = before {
            push_before(&mut query, before, false);
        }
        if let Some(search) = search.filter(|value| !value.trim().is_empty()) {
            query
                .push(" AND instr(lower(json_extract(payload, '$.text')), lower(")
                .push_bind(search.trim())
                .push(")) > 0");
        }
        query
            .push(" ORDER BY json_extract(payload, '$.created_at') DESC, seq DESC LIMIT ")
            .push_bind(LOCAL_PAGE as i64 + 1);
        read_page(query.build_query_scalar().fetch_all(&self.db).await?)
    }

    pub async fn files(&self, filter: FileFilter, before: Option<i64>) -> Result<Page> {
        let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new("");
        if filter.include_transfer_rows {
            // The local upload rowid keeps the pagination key stable when publication assigns a message sequence.
            query.push(
                "WITH file_rows AS (
                   SELECT i.seq, i.conversation_id, i.attachment_id, i.payload,
                          coalesce(json_extract(t.payload, '$.created_at'), json_extract(i.payload, '$.created_at')) AS sort_at,
                          coalesce(-t.rowid, i.seq) AS sort_key
                   FROM timeline i LEFT JOIN transfers t ON t.id = i.attachment_id AND t.direction = 'upload'
                   WHERE i.attachment_id IS NOT NULL AND i.seq NOT IN (SELECT seq FROM hidden_messages)
                   UNION ALL
                   SELECT -t.rowid, json_extract(t.payload, '$.conversation_id'), t.id,
                          json_object('seq', -t.rowid, 'conversation_id', json_extract(t.payload, '$.conversation_id'),
                              'sender', coalesce(json_extract(t.payload, '$.sharer'), json(",
            ).push_bind(serde_json::to_string(&self.user)?).push(
                ")), 'created_at', coalesce(json_extract(t.payload, '$.created_at'), 0),
                          'attachment', json_extract(t.payload, '$.attachment')),
                          coalesce(json_extract(t.payload, '$.created_at'), 0), -t.rowid
                   FROM transfers t WHERE json_extract(t.payload, '$.status') <> 'complete'
                     AND NOT EXISTS (SELECT 1 FROM inbox i WHERE i.attachment_id = t.id)
                     AND NOT EXISTS (SELECT 1 FROM hidden_messages h WHERE h.attachment_id = t.id)
                     AND ((t.direction = 'upload'
                        AND NOT EXISTS (SELECT 1 FROM local_suppressed_sends s WHERE s.kind = 'upload' AND s.id = t.id)
                        AND NOT EXISTS (SELECT 1 FROM local_deleted_messages d WHERE d.attachment_id = t.id))
                       OR (t.direction = 'download' AND EXISTS (SELECT 1 FROM local_deleted_messages d WHERE d.attachment_id = t.id)))
                   UNION ALL
                   SELECT -t.rowid, i.conversation_id, i.attachment_id, json_set(i.payload, '$.seq', -t.rowid),
                          json_extract(i.payload, '$.created_at'), -t.rowid
                   FROM transfers t JOIN inbox i ON i.attachment_id = t.id
                   JOIN hidden_messages h ON h.seq = i.seq
                   WHERE t.direction = 'download' AND json_extract(t.payload, '$.status') <> 'complete'
                 ) SELECT payload FROM file_rows WHERE 1",
            );
        } else {
            query.push("SELECT payload FROM timeline WHERE attachment_id IS NOT NULL AND seq NOT IN (SELECT seq FROM hidden_messages)");
        }
        if let Some(before) = before {
            if filter.include_transfer_rows {
                query.push(" AND (sort_at, sort_key) ")
                    .push(if filter.oldest_first { ">" } else { "<" })
                    .push(" (SELECT sort_at, sort_key FROM file_rows WHERE seq = ")
                    .push_bind(before).push(" OR sort_key = ").push_bind(before).push(" LIMIT 1)");
            } else {
                push_before(&mut query, before, filter.oldest_first);
            }
        }
        if let Some(id) = filter.conversation_id {
            query
                .push(" AND conversation_id = ")
                .push_bind(id.to_string());
        }
        if let Some(id) = filter.sender_id {
            query
                .push(" AND json_extract(payload, '$.sender.id') = ")
                .push_bind(id.to_string());
        }
        if let Some(kind) = filter.kind {
            query
                .push(" AND json_extract(payload, '$.attachment.kind') = ")
                .push_bind(kind.as_str());
        }
        if let Some(start) = filter.start_at {
            query
                .push(" AND json_extract(payload, '$.created_at') >= ")
                .push_bind(start);
        }
        if let Some(end) = filter.end_before {
            query
                .push(" AND json_extract(payload, '$.created_at') < ")
                .push_bind(end);
        }
        if let Some(name) = filter.name.filter(|name| !name.trim().is_empty()) {
            query
                .push(" AND instr(lower(json_extract(payload, '$.attachment.name')), lower(")
                .push_bind(name.trim().to_owned())
                .push(")) > 0");
        }
        query
            .push(if filter.include_transfer_rows {
                if filter.oldest_first { " ORDER BY sort_at, sort_key LIMIT " } else { " ORDER BY sort_at DESC, sort_key DESC LIMIT " }
            } else if filter.oldest_first {
                " ORDER BY json_extract(payload, '$.created_at'), seq LIMIT "
            } else {
                " ORDER BY json_extract(payload, '$.created_at') DESC, seq DESC LIMIT "
            })
            .push_bind(LOCAL_PAGE as i64 + 1);
        read_page(query.build_query_scalar().fetch_all(&self.db).await?)
    }

    pub async fn attachment(&self, id: Uuid) -> Result<Message> {
        let json: String = sqlx::query_scalar("SELECT payload FROM timeline WHERE attachment_id = ? AND seq NOT IN (SELECT seq FROM hidden_messages)")
            .bind(id.to_string())
            .fetch_optional(&self.db)
            .await?
            .context("This attachment is unavailable")?;
        Ok(serde_json::from_str(&json)?)
    }

    pub async fn find_user(&self, handle: &str) -> Result<User> {
        let handle = handle.trim().to_ascii_lowercase();
        ensure!(valid_handle(&handle), "Enter a valid account handle");
        Ok(self
            .api
            .json(
                self.api
                    .request(Method::GET, &format!("/v1/users/{handle}")),
            )
            .await?)
    }

    pub async fn add_contact(&self, user_id: Uuid) -> Result<()> {
        self.request_contact(user_id, String::new()).await
    }

    pub async fn remove_contact(&self, user_id: Uuid) -> Result<()> {
        let _profile = self.profile_gate.lock().await;
        self.api
            .send(
                self.api
                    .request(Method::DELETE, &format!("/v1/contacts/{user_id}")),
            )
            .await?;
        self.forget_profile(user_id).await;
        Ok(())
    }

    pub async fn respond_contact(&self, user_id: Uuid, accept: bool) -> Result<()> {
        self.decide_contact(user_id, accept, None).await
    }

    pub async fn decide_contact(
        &self,
        user_id: Uuid,
        accept: bool,
        request_id: Option<Uuid>,
    ) -> Result<()> {
        self.api
            .send(
                self.api
                    .request(Method::POST, &format!("/v1/contacts/{user_id}/decision"))
                    .json(&ContactDecision { accept, request_id }),
            )
            .await?;
        Ok(())
    }

    pub async fn profile(&self, id: Uuid) -> Result<Profile> {
        self.ensure_open()?;
        let _profile = self.profile_gate.lock().await;
        match self.api.profile(id).await {
            Ok(profile) => {
                ensure!(
                    profile.user.id == id
                        && matches!(profile.relation, ProfileRelation::Own) == (id == self.user.id),
                    OnlineError::InvalidResponse
                );
                self.remember_profile(&profile).await;
                Ok(profile)
            }
            Err(error) => {
                if matches!(&error, OnlineError::Rejected { status, .. }
                    if matches!(*status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::NOT_FOUND))
                {
                    self.forget_profile(id).await;
                }
                Err(error.into())
            }
        }
    }

    pub async fn set_display_name(&self, name: String) -> Result<()> {
        let _profile = self.profile_gate.lock().await;
        let name = name.trim();
        self.api
            .send(self.api.request(Method::PUT, "/v1/profiles/me/name").json(&name))
            .await?;
        self.remember_profile_edit(
            self.user.id, "display_name", (!name.is_empty()).then_some(name), Some("names"),
        ).await;
        Ok(())
    }

    pub async fn set_bio(&self, bio: String) -> Result<()> {
        let _profile = self.profile_gate.lock().await;
        self.api
            .send(
                self.api
                    .request(Method::PUT, "/v1/profiles/me/bio")
                    .json(&ProfileBio { bio: bio.trim().to_owned() }),
            )
            .await?;
        self.remember_profile_edit(self.user.id, "bio", bio.trim(), None).await;
        Ok(())
    }

    pub async fn set_contact_notes(&self, id: Uuid, notes: ContactNotes) -> Result<()> {
        let _profile = self.profile_gate.lock().await;
        let notes = ContactNotes {
            remark: notes.remark.trim().to_owned(),
            contact_info: notes.contact_info.trim().to_owned(),
        };
        self.api
            .send(
                self.api
                    .request(Method::PUT, &format!("/v1/contacts/{id}"))
                    .json(&notes),
            )
            .await?;
        self.remember_profile_edit(id, "notes", &notes, None).await;
        Ok(())
    }

    pub async fn set_profile_image(&self, kind: ProfileImage, bytes: Vec<u8>) -> Result<Uuid> {
        let _profile = self.profile_gate.lock().await;
        ensure!(
            bytes.len() <= MAX_PROFILE_IMAGE_BYTES,
            "Profile images can be up to 2 MiB"
        );
        let version: Uuid = self.api
            .json(
                self.api
                    .request(
                        Method::PUT,
                        &format!("/v1/profiles/me/images/{}", kind.as_str()),
                    )
                    .header(reqwest::header::CONTENT_TYPE, "image/png")
                    .body(bytes.clone()),
            )
            .await?;
        self.remember_profile_edit(
            self.user.id, kind.as_str(), version,
            matches!(kind, ProfileImage::Avatar).then_some("avatars"),
        ).await;
        self.remember_profile_image(self.user.id, version, &bytes).await;
        Ok(version)
    }

    pub async fn profile_image(&self, id: Uuid, version: Uuid) -> Result<Vec<u8>> {
        self.profile_image_verified(id, version, |bytes| {
            ensure!(
                bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
                OnlineError::InvalidResponse
            );
            Ok(())
        })
        .await
    }

    pub async fn profile_image_verified<F>(
        &self,
        id: Uuid,
        version: Uuid,
        validate: F,
    ) -> Result<Vec<u8>>
    where
        F: FnOnce(&[u8]) -> Result<()>,
    {
        self.ensure_open()?;
        let response = match self
            .api
            .send(
                self.api
                    .request(Method::GET, &format!("/v1/profiles/{id}/images/{version}")),
            )
            .await
        {
            Ok(response) => response,
            Err(error) => {
                if matches!(&error, OnlineError::Rejected { status, .. }
                    if matches!(*status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN))
                {
                    self.forget_profile_image(id, version).await;
                }
                return Err(error.into());
            }
        };
        ensure!(
            response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .is_some_and(|value| value == "image/png"),
            OnlineError::InvalidResponse
        );
        let bytes = read_limited(response, MAX_PROFILE_IMAGE_BYTES).await?;
        validate(&bytes)?;
        self.remember_profile_image(id, version, &bytes).await;
        Ok(bytes)
    }

    pub async fn set_read_receipts(&self, enabled: bool) -> Result<()> {
        self.api
            .send(
                self.api
                    .request(Method::PUT, "/v1/read-receipts")
                    .json(&ReadPreference { enabled }),
            )
            .await?;
        Ok(())
    }

    pub async fn read_update(&self, conversation: Uuid, update: ReadUpdate) -> Result<Vec<i64>> {
        match self.update_read(conversation, update, false).await? {
            ReadResult::Legacy(seen) => Ok(seen),
            ReadResult::Latest { .. } => Err(OnlineError::InvalidResponse.into()),
        }
    }

    pub async fn update_receipt(
        &self,
        conversation: Uuid,
        update: ReadUpdate,
    ) -> Result<Option<MessageReceipt>> {
        match self.update_read(conversation, update, true).await? {
            ReadResult::Latest { receipt } => Ok(receipt),
            ReadResult::Legacy(_) => Ok(None),
        }
    }

    async fn update_read(
        &self,
        conversation: Uuid,
        update: ReadUpdate,
        latest: bool,
    ) -> Result<ReadResult> {
        self.ensure_open()?;
        if let Some(through) = update.through {
            let arrived: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM inbox WHERE seq = ? AND conversation_id = ?)",
            )
            .bind(through)
            .bind(conversation.to_string())
            .fetch_one(&self.db)
            .await?;
            ensure!(arrived, "Wait for the conversation to finish syncing");
            sqlx::query(
                "INSERT INTO local_read_positions (conversation_id, read_through) VALUES (?,?)
                 ON CONFLICT (conversation_id) DO UPDATE SET read_through = max(read_through, excluded.read_through)",
            ).bind(conversation.to_string()).bind(through).execute(&self.db).await?;
        }
        if update.sent.is_empty() && update.viewed.is_none() {
            return Ok(if latest {
                ReadResult::Latest { receipt: None }
            } else {
                ReadResult::Legacy(vec![])
            });
        }
        if let Some(viewed) = update.viewed {
            let visible: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM inbox i WHERE i.seq = ? AND i.conversation_id = ?
                 AND NOT EXISTS (SELECT 1 FROM hidden_messages h WHERE h.seq = i.seq)
                 AND coalesce(json_extract(i.payload, '$.recalled'), 0) = 0
                 AND json_extract(i.payload, '$.expired') IS NULL)",
            )
            .bind(viewed)
            .bind(conversation.to_string())
            .fetch_one(&self.db)
            .await?;
            ensure!(visible, "This message is unavailable");
        }
        Ok(self
            .api
            .json(
                self.api
                    .request(
                        Method::POST,
                        &format!("/v1/conversations/{conversation}/read"),
                    )
                    .query(&[("latest", latest)])
                    .json(&update),
            )
            .await?)
    }

    pub async fn hide_message(&self, conversation: Uuid, seq: i64) -> Result<()> {
        self.delete_received_history(conversation, Some(seq), false).await
    }

    pub async fn clear_received_history(&self, conversation: Uuid) -> Result<()> {
        self.delete_received_history(conversation, None, false).await
    }

    pub async fn delete_conversation(&self, conversation: Uuid) -> Result<()> {
        self.delete_received_history(conversation, None, true).await
    }

    pub async fn quoted_message(&self, conversation: Uuid, seq: i64) -> Result<Option<Message>> {
        let payload: Option<String> = sqlx::query_scalar(
            "SELECT payload FROM timeline WHERE seq = ? AND seq > 0 AND conversation_id = ? AND seq NOT IN (SELECT seq FROM hidden_messages)
             AND json_extract(payload, '$.expired') IS NULL AND coalesce(json_extract(payload, '$.recalled'), 0) = 0",
        ).bind(seq).bind(conversation.to_string()).fetch_optional(&self.db).await?;
        Ok(payload
            .map(|json| serde_json::from_str(&json))
            .transpose()?)
    }

    pub async fn recall_message(&self, conversation: Uuid, seq: i64) -> Result<()> {
        self.api
            .send(self.api.request(
                Method::DELETE,
                &format!("/v1/conversations/{conversation}/messages/{seq}"),
            ))
            .await?;
        Ok(())
    }

    pub async fn create_conversation(&self, request: ConversationRequest) -> Result<Uuid> {
        self.ensure_open()?;
        if let ConversationRequest::Direct { contact_id } = &request {
            let directory = self.membership_directory().await?;
            if directory.contacts.iter().any(|user| user.id == *contact_id)
                && let Some(chat) = directory.conversations.iter().find(|chat| chat.title.is_none()
                    && chat.members.iter().any(|user| user.id == *contact_id)
                    && chat.members.iter().any(|user| user.id == self.user.id))
            {
                let reopened = sqlx::query("DELETE FROM local_deleted_chats WHERE conversation_id = ?")
                    .bind(chat.id.to_string()).execute(&self.db).await?.rows_affected();
                let unhidden = sqlx::query(
                    "UPDATE conversation_prefs SET hidden_at_seq = NULL
                     WHERE conversation_id = ? AND hidden_at_seq IS NOT NULL",
                )
                .bind(chat.id.to_string())
                .execute(&self.db)
                .await?
                .rows_affected();
                if reopened != 0 || unhidden != 0 {
                    self.history_changed();
                    return Ok(chat.id);
                }
            }
        }
        let result: ConversationCreated = self
            .api
            .json(
                self.api
                    .request(Method::POST, "/v1/conversations")
                    .json(&request),
            )
            .await?;
        sqlx::query("DELETE FROM local_deleted_chats WHERE conversation_id = ?")
            .bind(result.id.to_string()).execute(&self.db).await?;
        sqlx::query(
            "UPDATE conversation_prefs SET hidden_at_seq = NULL
             WHERE conversation_id = ? AND hidden_at_seq IS NOT NULL",
        )
        .bind(result.id.to_string())
        .execute(&self.db)
        .await?;
        self.history_changed();
        Ok(result.id)
    }

    pub async fn issue_group_invite(&self, conversation: Uuid) -> Result<GroupInvite> {
        Ok(self
            .api
            .json(self.api.request(
                Method::POST,
                &format!("/v1/conversations/{conversation}/invite"),
            ))
            .await?)
    }

    pub async fn preview_group_invite(&self, code: Uuid) -> Result<GroupInvitePreview> {
        Ok(self
            .api
            .json(
                self.api
                    .request(Method::POST, "/v1/group-invites/preview")
                    .json(&GroupInviteRequest { code }),
            )
            .await?)
    }

    pub async fn join_group_invite(&self, code: Uuid) -> Result<Uuid> {
        let group: ConversationCreated = self
            .api
            .json(
                self.api
                    .request(Method::POST, "/v1/group-invites/join")
                    .json(&GroupInviteRequest { code }),
            )
            .await?;
        Ok(group.id)
    }

    pub async fn leave_group(&self, id: Uuid) -> Result<()> {
        self.ensure_open()?;
        self.api
            .send(self.api.request(
                Method::DELETE,
                &format!("/v1/conversations/{id}/membership"),
            ))
            .await?;
        Ok(())
    }
}

fn push_before(query: &mut sqlx::QueryBuilder<'_, sqlx::Sqlite>, before: i64, oldest_first: bool) {
    query
        .push(" AND (json_extract(payload, '$.created_at'), seq) ")
        .push(if oldest_first { ">" } else { "<" })
        .push(" (SELECT json_extract(payload, '$.created_at'), seq FROM timeline WHERE seq = ")
        .push_bind(before)
        .push(")");
}

fn unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn read_page(rows: Vec<String>) -> Result<Page> {
    let has_more = rows.len() > LOCAL_PAGE;
    let items = rows
        .iter()
        .take(LOCAL_PAGE)
        .map(|json| serde_json::from_str(json))
        .collect::<serde_json::Result<_>>()?;
    Ok(Page { items, has_more })
}

struct Api {
    base: Url,
    http: reqwest::Client,
    token: Option<String>,
}

impl Drop for Api {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.token.zeroize();
    }
}

impl Api {
    fn new(address: &str) -> Result<Self> {
        let base = Url::parse(address.trim())
            .context("Enter a server address such as https://chat.example.com")?;
        let loopback = base
            .host_str()
            .is_some_and(|host| host == "127.0.0.1" || host == "[::1]");
        ensure!(
            base.has_host()
                && base.username().is_empty()
                && base.password().is_none()
                && base.path() == "/"
                && base.query().is_none()
                && base.fragment().is_none()
                && (base.scheme() == "https" || (loopback && base.scheme() == "http")),
            "Use an HTTPS server address without a path. Local development can use HTTP on loopback."
        );
        let mut http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(20));
        if loopback {
            http = http.no_proxy();
        }
        Ok(Self {
            base,
            http: http.build()?,
            token: None,
        })
    }

    fn request(&self, method: Method, route: &str) -> RequestBuilder {
        let request = self
            .http
            .request(method, self.base.join(route).expect("internal API route"));
        match &self.token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }

    async fn profile(&self, id: Uuid) -> std::result::Result<Profile, OnlineError> {
        self.json(
            self.request(Method::GET, &format!("/v1/profiles/{id}"))
                .header(reqwest::header::CACHE_CONTROL, "no-cache"),
        )
        .await
    }

    async fn send(&self, request: RequestBuilder) -> std::result::Result<Response, OnlineError> {
        let response = request.send().await?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let message = read_limited(response, 8 * 1024)
            .await
            .ok()
            .and_then(|body| serde_json::from_slice::<serde_json::Value>(&body).ok())
            .and_then(|value| value.get("error")?.as_str().map(str::to_string))
            .unwrap_or_else(|| match status {
                StatusCode::TOO_MANY_REQUESTS => {
                    "Too many requests. Wait a moment and try again.".into()
                }
                status if status.is_server_error() => {
                    "The Zibblu server is temporarily unavailable. Try again later.".into()
                }
                _ => format!("The service rejected the request ({status})."),
            });
        Err(OnlineError::Rejected { status, message })
    }

    async fn json<T: DeserializeOwned>(
        &self,
        request: RequestBuilder,
    ) -> std::result::Result<T, OnlineError> {
        let body = read_limited(self.send(request).await?, 16 * 1024 * 1024).await?;
        serde_json::from_slice(&body).map_err(|_| OnlineError::InvalidResponse)
    }
}

async fn read_limited(
    mut response: Response,
    limit: usize,
) -> std::result::Result<Vec<u8>, OnlineError> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > limit - body.len() {
            return Err(OnlineError::InvalidResponse);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn ensure_new_destination(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute() && path.file_name().is_some(),
        "Choose an absolute save path"
    );
    match path.symlink_metadata() {
        Ok(_) => bail!("That file already exists. Choose another filename."),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("Could not check the save destination"),
    }
}
