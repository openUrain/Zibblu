mod staging;
mod transfer;

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
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};
use tokio::sync::Mutex;
use transfer::{Active, JobKey};
use uuid::Uuid;

const LOCAL_PAGE: usize = 60;

#[derive(Debug, thiserror::Error)]
pub enum OnlineError {
    #[error("Could not reach the Zibblu service.")]
    Network(#[from] reqwest::Error),
    #[error("{message}")]
    Rejected { status: StatusCode, message: String },
    #[error("The service returned an invalid response.")]
    InvalidResponse,
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
    _profile_lock: File,
}

#[derive(Serialize)]
pub struct Page {
    pub items: Vec<Message>,
    pub has_more: bool,
}

#[derive(Default, Deserialize)]
pub struct FileFilter {
    pub conversation_id: Option<Uuid>,
    pub sender_id: Option<Uuid>,
    pub kind: Option<AttachmentKind>,
    pub start_at: Option<i64>,
    pub end_before: Option<i64>,
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
        let profile_id = format!(
            "{:x}",
            Sha256::digest(format!("{}\n{}", api.base, session.user.id))
        );
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
             CREATE TABLE IF NOT EXISTS directory (
                id INTEGER PRIMARY KEY CHECK (id = 1), payload TEXT NOT NULL
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
            user: session.user,
            api,
            db,
            staging,
            _profile_lock: lock,
            sync_gate: Mutex::new(()),
            prepare_gate: Mutex::new(()),
            workers: Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
        }))
    }

    pub fn server(&self) -> &str {
        self.api.base.as_str()
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
        let _sync = self.sync_gate.lock().await;
        let mut after: i64 = sqlx::query_scalar("SELECT coalesce(max(seq), 0) FROM inbox")
            .fetch_one(&self.db)
            .await?;
        loop {
            let page: SyncPage = self
                .api
                .json(
                    self.api
                        .request(Method::GET, "/v1/sync")
                        .query(&[("after", after)]),
                )
                .await?;
            let mut tx = self.db.begin().await?;
            let count = page.messages.len();
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
            sqlx::query(
                "INSERT INTO directory (id, payload) VALUES (1,?)
                 ON CONFLICT (id) DO UPDATE SET payload = excluded.payload",
            )
            .bind(serde_json::to_string(&page.directory)?)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            if count < SYNC_PAGE_SIZE {
                return Ok(page.directory);
            }
        }
    }

    pub async fn directory(&self) -> Result<Directory> {
        let json: Option<String> = sqlx::query_scalar("SELECT payload FROM directory WHERE id = 1")
            .fetch_optional(&self.db)
            .await?;
        json.map(|value| serde_json::from_str(&value))
            .transpose()
            .map(|value| value.unwrap_or_default())
            .map_err(Into::into)
    }

    pub async fn messages(&self, conversation: Uuid, before: Option<i64>) -> Result<Page> {
        let rows = sqlx::query_scalar(
            "SELECT payload FROM inbox WHERE conversation_id = ? AND seq < ? ORDER BY seq DESC LIMIT ?",
        ).bind(conversation.to_string()).bind(before.unwrap_or(i64::MAX))
            .bind(LOCAL_PAGE as i64 + 1).fetch_all(&self.db).await?;
        read_page(rows)
    }

    pub async fn files(&self, filter: FileFilter, before: Option<i64>) -> Result<Page> {
        let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
            "SELECT payload FROM inbox WHERE attachment_id IS NOT NULL AND seq < ",
        );
        query.push_bind(before.unwrap_or(i64::MAX));
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
        query
            .push(" ORDER BY seq DESC LIMIT ")
            .push_bind(LOCAL_PAGE as i64 + 1);
        read_page(query.build_query_scalar().fetch_all(&self.db).await?)
    }

    pub async fn attachment(&self, id: Uuid) -> Result<Message> {
        let json: String = sqlx::query_scalar("SELECT payload FROM inbox WHERE attachment_id = ?")
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
        self.api
            .send(
                self.api
                    .request(Method::POST, "/v1/contacts")
                    .json(&ContactRequest { user_id }),
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
            .unwrap_or_else(|| format!("The service rejected the request ({status})."));
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
