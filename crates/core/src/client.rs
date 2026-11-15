mod ai_context;
mod staging;
mod transfer;

pub use ai_context::AiScope;
pub use transfer::{Direction, Transfer, TransferStatus};

use crate::protocol::*;
use anyhow::{Context, Result, bail, ensure};
use reqwest::{Method, RequestBuilder, Response, StatusCode, Url};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
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
    api: Api,
    db: SqlitePool,
    staging: PathBuf,
    sync_gate: Mutex<()>,
    prepare_gate: Mutex<()>,
    workers: Mutex<HashMap<JobKey, Active>>,
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
    pub server_now: i64,
    pub last_seq: i64,
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
}

impl Client {
    pub async fn login(
        data_dir: &Path,
        address: &str,
        credentials: Credentials,
        register: bool,
    ) -> Result<Arc<Self>> {
        let mut api = Api::new(address)?;
        let route = if register {
            "/v1/register"
        } else {
            "/v1/login"
        };
        let session: Session = api
            .json(api.request(Method::POST, route).json(&credentials))
            .await?;
        api.token = Some(session.token);
        Self::open(data_dir, api, session.user).await
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
        let profile = match api.profile(account_id).await {
            Ok(profile) => profile,
            Err(OnlineError::Rejected {
                status: StatusCode::UNAUTHORIZED | StatusCode::NOT_FOUND,
                ..
            }) => {
                return Err(invalid().into());
            }
            Err(error) => return Err(error.into()),
        };
        ensure!(
            matches!(profile.relation, ProfileRelation::Own) && profile.user.id == account_id,
            invalid()
        );
        Self::open(data_dir, api, profile.user).await
    }

    async fn open(data_dir: &Path, api: Api, user: User) -> Result<Arc<Self>> {
        let profile_id = format!("{:x}", Sha256::digest(format!("{}\n{}", api.base, user.id)));
        let profile = data_dir.join("profiles").join(profile_id);
        let staging = profile.join("transfers");
        std::fs::create_dir_all(&staging).context("Could not create the local profile")?;
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
                    .busy_timeout(Duration::from_secs(5)),
            )
            .await
            .context("Could not open the local chat history")?;
        sqlx::raw_sql(
            "CREATE TABLE IF NOT EXISTS inbox (
                seq INTEGER PRIMARY KEY, conversation_id TEXT NOT NULL, payload TEXT NOT NULL,
                attachment_id TEXT UNIQUE
             );
             CREATE INDEX IF NOT EXISTS inbox_chat ON inbox (conversation_id, seq);
             CREATE INDEX IF NOT EXISTS inbox_chat_time ON inbox (conversation_id, json_extract(payload, '$.created_at'), seq);
             CREATE INDEX IF NOT EXISTS inbox_recalled ON inbox (seq)
                WHERE json_extract(payload, '$.recalled') = 1;
             CREATE INDEX IF NOT EXISTS inbox_ai_replies ON inbox (seq)
                WHERE json_type(payload, '$.ai_provider') = 'text';
             CREATE TABLE IF NOT EXISTS hidden_messages (
                seq INTEGER PRIMARY KEY REFERENCES inbox (seq), attachment_id TEXT UNIQUE
             );
             CREATE TABLE IF NOT EXISTS directory (
                id INTEGER PRIMARY KEY CHECK (id = 1), payload TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS recall_cursor (
                id INTEGER PRIMARY KEY CHECK (id = 1), seq INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS pending_delivery_ack (
                batch TEXT PRIMARY KEY NOT NULL
             );
             CREATE TABLE IF NOT EXISTS transfers (
                id TEXT NOT NULL, direction TEXT NOT NULL, payload TEXT NOT NULL,
                PRIMARY KEY (id, direction)
             );
             UPDATE transfers SET payload = json_set(payload, '$.status', 'paused')
                WHERE json_extract(payload, '$.status') = 'running';",
        )
        .execute(&db)
        .await
        .context("Could not initialize the local chat history")?;
        Ok(Arc::new(Self {
            user,
            api,
            db,
            staging,
            _profile_lock: lock,
            sync_gate: Mutex::new(()),
            prepare_gate: Mutex::new(()),
            workers: Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
            server_clock_offset: AtomicI64::new(0),
        }))
    }

    pub fn server(&self) -> &str {
        self.api.base.as_str()
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

    pub async fn sync(&self) -> Result<Directory> {
        Ok(self.sync_updates().await?.directory)
    }

    pub async fn sync_updates(&self) -> Result<SyncUpdate> {
        let _sync = self.sync_gate.lock().await;
        let mut can_ack = self.flush_delivery_acks().await?;
        let mut after: i64 = sqlx::query_scalar("SELECT coalesce(max(seq), 0) FROM inbox")
            .fetch_one(&self.db)
            .await?;
        let mut recall_after: i64 =
            sqlx::query_scalar("SELECT seq FROM recall_cursor WHERE id = 1")
                .fetch_optional(&self.db)
                .await?
                .unwrap_or_default();
        loop {
            let started = unix_millis();
            let page: SyncPage = self
                .api
                .json(
                    self.api
                        .request(Method::GET, "/v1/sync")
                        .query(&[("after", after), ("recall_after", recall_after)])
                        .query(&[("delivery_acks", true)]),
                )
                .await?;
            if let Some(server_time) = page.server_time {
                let elapsed = unix_millis().saturating_sub(started);
                self.server_clock_offset.store(
                    server_time.saturating_sub(started + elapsed / 2),
                    Ordering::Relaxed,
                );
            }
            let mut tx = self.db.begin().await?;
            let count = page.messages.len();
            let recall_count = page.recalls.len();
            for message in page.messages {
                ensure!(
                    message.seq > after
                        && message.attachment.as_ref().is_none_or(Attachment::is_valid),
                    OnlineError::InvalidResponse
                );
                after = message.seq;
                sqlx::query("INSERT INTO inbox (seq, conversation_id, payload, attachment_id) VALUES (?,?,?,?)")
                    .bind(message.seq).bind(message.conversation_id.to_string())
                    .bind(serde_json::to_string(&message)?)
                    .bind(message.attachment.as_ref().map(|attachment| attachment.id.to_string()))
                    .execute(&mut *tx).await?;
            }
            for recall in page.recalls {
                ensure!(
                    recall.seq > recall_after && recall.message_seq < recall.seq,
                    OnlineError::InvalidResponse
                );
                recall_after = recall.seq;
                sqlx::query(
                    "UPDATE inbox SET attachment_id = NULL,
                         payload = json_set(payload, '$.text', NULL, '$.attachment', NULL, '$.recalled', json('true'))
                     WHERE seq = ?",
                )
                .bind(recall.message_seq).execute(&mut *tx).await?;
            }
            if recall_count > 0 {
                sqlx::query(
                    "INSERT INTO recall_cursor (id, seq) VALUES (1,?)
                     ON CONFLICT (id) DO UPDATE SET seq = excluded.seq",
                )
                .bind(recall_after)
                .execute(&mut *tx)
                .await?;
            }
            sqlx::query(
                "INSERT INTO directory (id, payload) VALUES (1,?)
                 ON CONFLICT (id) DO UPDATE SET payload = excluded.payload",
            )
            .bind(serde_json::to_string(&page.directory)?)
            .execute(&mut *tx)
            .await?;
            if let Some(batch) = page.delivery_batch {
                sqlx::query("INSERT OR IGNORE INTO pending_delivery_ack (batch) VALUES (?)")
                    .bind(batch.to_string())
                    .execute(&mut *tx)
                    .await?;
                sqlx::query(
                    "DELETE FROM pending_delivery_ack WHERE rowid IN (
                    SELECT rowid FROM pending_delivery_ack ORDER BY rowid DESC LIMIT -1 OFFSET ?)",
                )
                .bind(MAX_PENDING_DELIVERY_BATCHES as i64)
                .execute(&mut *tx)
                .await?;
            }
            tx.commit().await?;
            if can_ack && page.delivery_batch.is_some() {
                can_ack = self.flush_delivery_acks().await?;
            }
            if count < SYNC_PAGE_SIZE && recall_count < SYNC_PAGE_SIZE {
                let recalled = sqlx::query_scalar(
                    "SELECT seq FROM inbox WHERE json_extract(payload, '$.recalled') = 1 ORDER BY seq",
                )
                .fetch_all(&self.db).await?;
                let latest_ai_seq = sqlx::query_scalar(
                    "SELECT coalesce(max(seq), 0) FROM inbox WHERE json_type(payload, '$.ai_provider') = 'text'",
                )
                .fetch_one(&self.db).await?;
                return Ok(SyncUpdate {
                    directory: self.visible_directory(page.directory).await?,
                    recalled,
                    server_now: self.server_now(),
                    last_seq: after,
                    latest_ai_seq,
                });
            }
        }
    }

    async fn flush_delivery_acks(&self) -> Result<bool> {
        let pending: Vec<String> = sqlx::query_scalar("SELECT batch FROM pending_delivery_ack")
            .fetch_all(&self.db)
            .await?;
        if pending.is_empty() {
            return Ok(true);
        }
        let batches = pending
            .iter()
            .map(|id| Uuid::parse_str(id))
            .collect::<std::result::Result<_, _>>()?;
        let request = self
            .api
            .request(Method::POST, "/v1/sync/ack")
            .timeout(Duration::from_secs(3))
            .json(&DeliveryAck { batches });
        match self.api.send(request).await {
            Ok(_)
            | Err(OnlineError::Rejected {
                status: StatusCode::NOT_FOUND,
                ..
            }) => {
                sqlx::query("DELETE FROM pending_delivery_ack")
                    .execute(&self.db)
                    .await?;
                Ok(true)
            }
            // Keep the proof for the next sync even if the optional receipt path is down.
            Err(_) => Ok(false),
        }
    }

    pub async fn directory(&self) -> Result<Directory> {
        let json: Option<String> = sqlx::query_scalar("SELECT payload FROM directory WHERE id = 1")
            .fetch_optional(&self.db)
            .await?;
        let directory = json
            .map(|value| serde_json::from_str(&value))
            .transpose()?
            .unwrap_or_default();
        self.visible_directory(directory).await
    }

    async fn visible_directory(&self, mut directory: Directory) -> Result<Directory> {
        let hidden: Vec<(i64, String, bool, bool)> = sqlx::query_as(
            "SELECT i.seq, i.conversation_id, json_extract(i.payload, '$.sender.id') <> ?,
                    coalesce(json_extract(i.payload, '$.recalled'), 0)
             FROM hidden_messages h JOIN inbox i ON i.seq = h.seq",
        )
        .bind(self.user.id.to_string())
        .fetch_all(&self.db)
        .await?;
        let mut chats: HashMap<_, _> = directory
            .conversations
            .iter_mut()
            .map(|chat| (chat.id.to_string(), chat))
            .collect();
        for (seq, id, incoming, recalled) in hidden {
            let Some(chat) = chats.get_mut(&id) else {
                continue;
            };
            if incoming && !recalled && seq > chat.read_through {
                chat.unread = (chat.unread - 1).max(0);
            }
            if chat.preview_seq == Some(seq) {
                chat.preview = sqlx::query_scalar(
                    "SELECT CASE WHEN json_extract(payload, '$.recalled') = 1 THEN 'Message recalled'
                                 ELSE coalesce(substr(json_extract(payload, '$.text'), 1, 100),
                                               json_extract(payload, '$.attachment.name')) END
                     FROM inbox WHERE conversation_id = ? AND seq NOT IN (SELECT seq FROM hidden_messages)
                     ORDER BY json_extract(payload, '$.created_at') DESC, seq DESC LIMIT 1",
                ).bind(&id).fetch_optional(&self.db).await?;
            }
        }
        Ok(directory)
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
            "SELECT payload FROM inbox WHERE seq NOT IN (SELECT seq FROM hidden_messages) AND conversation_id = ",
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
        let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
            "SELECT payload FROM inbox WHERE attachment_id IS NOT NULL AND seq NOT IN (SELECT seq FROM hidden_messages)",
        );
        if let Some(before) = before {
            push_before(&mut query, before, filter.oldest_first);
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
            .push(if filter.oldest_first {
                " ORDER BY json_extract(payload, '$.created_at'), seq LIMIT "
            } else {
                " ORDER BY json_extract(payload, '$.created_at') DESC, seq DESC LIMIT "
            })
            .push_bind(LOCAL_PAGE as i64 + 1);
        read_page(query.build_query_scalar().fetch_all(&self.db).await?)
    }

    pub async fn attachment(&self, id: Uuid) -> Result<Message> {
        let json: String = sqlx::query_scalar("SELECT payload FROM inbox WHERE attachment_id = ? AND seq NOT IN (SELECT seq FROM hidden_messages)")
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

    pub async fn request_contact(&self, user_id: Uuid, greeting: String) -> Result<()> {
        self.api
            .send(
                self.api
                    .request(Method::POST, "/v1/contacts")
                    .json(&ContactRequest { user_id, greeting }),
            )
            .await?;
        Ok(())
    }

    pub async fn remove_contact(&self, user_id: Uuid) -> Result<()> {
        self.api
            .send(
                self.api
                    .request(Method::DELETE, &format!("/v1/contacts/{user_id}")),
            )
            .await?;
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
        Ok(self.api.profile(id).await?)
    }

    pub async fn set_bio(&self, bio: String) -> Result<()> {
        self.api
            .send(
                self.api
                    .request(Method::PUT, "/v1/profiles/me/bio")
                    .json(&ProfileBio { bio }),
            )
            .await?;
        Ok(())
    }

    pub async fn set_contact_notes(&self, id: Uuid, notes: ContactNotes) -> Result<()> {
        self.api
            .send(
                self.api
                    .request(Method::PUT, &format!("/v1/contacts/{id}"))
                    .json(&notes),
            )
            .await?;
        Ok(())
    }

    pub async fn set_profile_image(&self, kind: ProfileImage, bytes: Vec<u8>) -> Result<()> {
        ensure!(
            bytes.len() <= MAX_PROFILE_IMAGE_BYTES,
            "Profile images can be up to 2 MiB"
        );
        self.api
            .send(
                self.api
                    .request(
                        Method::PUT,
                        &format!("/v1/profiles/me/images/{}", kind.as_str()),
                    )
                    .header(reqwest::header::CONTENT_TYPE, "image/png")
                    .body(bytes),
            )
            .await?;
        Ok(())
    }

    pub async fn profile_image(&self, id: Uuid, version: Uuid) -> Result<Vec<u8>> {
        let response = self
            .api
            .send(
                self.api
                    .request(Method::GET, &format!("/v1/profiles/{id}/images/{version}")),
            )
            .await?;
        ensure!(
            response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .is_some_and(|value| value == "image/png"),
            OnlineError::InvalidResponse
        );
        Ok(read_limited(response, MAX_PROFILE_IMAGE_BYTES).await?)
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
        if let Some(through) = update.through {
            let arrived: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM inbox WHERE seq = ? AND conversation_id = ?)",
            )
            .bind(through)
            .bind(conversation.to_string())
            .fetch_one(&self.db)
            .await?;
            ensure!(arrived, "Wait for the conversation to finish syncing");
        }
        if let Some(viewed) = update.viewed {
            let visible: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM inbox i WHERE i.seq = ? AND i.conversation_id = ?
                 AND NOT EXISTS (SELECT 1 FROM hidden_messages h WHERE h.seq = i.seq)
                 AND coalesce(json_extract(i.payload, '$.recalled'), 0) = 0)",
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
        let found = sqlx::query(
            "INSERT OR IGNORE INTO hidden_messages (seq, attachment_id)
             SELECT seq, attachment_id FROM inbox WHERE seq = ? AND conversation_id = ?",
        )
        .bind(seq)
        .bind(conversation.to_string())
        .execute(&self.db)
        .await?;
        if found.rows_affected() == 0 {
            let hidden: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM hidden_messages h JOIN inbox i USING(seq) WHERE h.seq = ? AND i.conversation_id = ?)")
                .bind(seq).bind(conversation.to_string()).fetch_one(&self.db).await?;
            ensure!(hidden, "This message is unavailable");
        }
        Ok(())
    }

    pub async fn quoted_message(&self, conversation: Uuid, seq: i64) -> Result<Option<Message>> {
        let payload: Option<String> = sqlx::query_scalar(
            "SELECT payload FROM inbox WHERE seq = ? AND conversation_id = ? AND seq NOT IN (SELECT seq FROM hidden_messages)",
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

    pub async fn publish_ai_reply(&self, conversation: Uuid, reply: PublishAiReply) -> Result<()> {
        self.api
            .send(
                self.api
                    .request(
                        Method::POST,
                        &format!("/v1/conversations/{conversation}/ai-replies"),
                    )
                    .json(&reply),
            )
            .await?;
        Ok(())
    }

    pub async fn create_conversation(&self, request: ConversationRequest) -> Result<Uuid> {
        let result: ConversationCreated = self
            .api
            .json(
                self.api
                    .request(Method::POST, "/v1/conversations")
                    .json(&request),
            )
            .await?;
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

    pub async fn send_message(&self, conversation: Uuid, message: SendMessage) -> Result<()> {
        if let Some(seq) = message.reply_to {
            ensure!(
                self.quoted_message(conversation, seq).await?.is_some(),
                "This quoted message is unavailable"
            );
        }
        self.api
            .send(
                self.api
                    .request(
                        Method::POST,
                        &format!("/v1/conversations/{conversation}/messages"),
                    )
                    .json(&message),
            )
            .await?;
        Ok(())
    }
}

fn push_before(query: &mut sqlx::QueryBuilder<'_, sqlx::Sqlite>, before: i64, oldest_first: bool) {
    query
        .push(" AND (json_extract(payload, '$.created_at'), seq) ")
        .push(if oldest_first { ">" } else { "<" })
        .push(" (SELECT json_extract(payload, '$.created_at'), seq FROM inbox WHERE seq = ")
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
