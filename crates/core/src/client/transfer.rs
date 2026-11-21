use super::{
    Client, OnlineError, ensure_new_destination, local_sync::persist_message, read_limited,
    staging, unix_millis,
};
use crate::{
    Attachment, CHUNK_SIZE, ContentAck, MAX_PASTED_IMAGE_BYTES, Message, Publication,
    UploadProgress, User,
};
use anyhow::{Context, Result, bail, ensure};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use sqlx::SqliteConnection;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tempfile::NamedTempFile;
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom},
    task::JoinHandle,
};
use uuid::Uuid;

const MAX_PENDING: i64 = 8;
const MAX_RUNNING: usize = 2;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Upload,
    Download,
}

impl Direction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Upload => "upload",
            Self::Download => "download",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferStatus {
    Running,
    Queued,
    Paused,
    Failed,
    Expired,
    Complete,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Transfer {
    pub attachment: Attachment,
    pub conversation_id: Uuid,
    pub direction: Direction,
    pub status: TransferStatus,
    pub received: u64,
    pub error: Option<String>,
    #[serde(default)]
    pub sharer: Option<User>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
}

#[derive(Serialize, Deserialize)]
struct Job {
    #[serde(flatten)]
    transfer: Transfer,
    destination: Option<PathBuf>,
    #[serde(default)]
    publishing: bool,
    #[serde(default)]
    retry_at: i64,
}

pub(super) type JobKey = (Uuid, Direction);

pub(super) struct Active {
    stop: Arc<AtomicBool>,
    task: JoinHandle<()>,
}

impl Job {
    fn new(attachment: Attachment, conversation_id: Uuid, direction: Direction) -> Self {
        Self {
            transfer: Transfer {
                attachment,
                conversation_id,
                direction,
                status: TransferStatus::Paused,
                received: 0,
                error: None,
                sharer: None,
                created_at: None,
            },
            destination: None,
            publishing: false,
            retry_at: 0,
        }
    }

    fn key(&self) -> JobKey {
        (self.transfer.attachment.id, self.transfer.direction)
    }
}

impl Client {
    pub(super) async fn restore_upload_times(&self) -> Result<()> {
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT payload FROM transfers WHERE direction = 'upload'
             AND json_extract(payload, '$.status') <> 'complete'
             AND json_extract(payload, '$.created_at') IS NULL",
        ).fetch_all(&self.db).await?;
        for row in rows {
            let mut job: Job = serde_json::from_str(&row)?;
            let time = fs::metadata(self.spool(job.key())).await.ok()
                .and_then(|metadata| metadata.created().or_else(|_| metadata.modified()).ok())
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok());
            if let Some(time) = time {
                job.transfer.created_at = Some(time.as_millis() as i64);
                self.store_job(&job).await?;
            }
        }
        Ok(())
    }

    pub async fn saved_destination(&self, id: Uuid) -> Result<Option<PathBuf>> {
        self.ensure_open()?;
        let file = self.attachment(id).await?.attachment.context("This attachment is unavailable")?;
        let job = self.job((id, Direction::Download)).await?
            .filter(|job| job.transfer.status == TransferStatus::Complete && job.transfer.attachment == file)
            .context("Save this attachment before opening it")?;
        let path = job.destination.context("The save location is unavailable")?;
        let existing = tokio::task::spawn_blocking(move || -> Result<Option<PathBuf>> {
            if !path.try_exists()? {
                return Ok(None);
            }
            verify_file(&path, &file).context("The saved file no longer matches this attachment")?;
            Ok(Some(path))
        }).await??;
        self.ensure_open()?;
        self.attachment(id).await?;
        Ok(existing)
    }

    pub async fn transfers(&self) -> Result<Vec<Transfer>> {
        let rows: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT t.payload, json_extract(i.payload, '$.sender') FROM transfers t
             LEFT JOIN inbox i ON t.id = i.attachment_id
             WHERE json_extract(t.payload, '$.status') <> 'complete'
                OR NOT EXISTS (SELECT 1 FROM hidden_messages h WHERE h.attachment_id = t.id)
             ORDER BY t.rowid DESC",
        )
        .fetch_all(&self.db)
        .await?;
        rows.into_iter()
            .map(|(json, sender)| {
                let mut transfer = serde_json::from_str::<Job>(&json)?.transfer;
                if transfer.sharer.is_none() {
                    transfer.sharer = if transfer.direction == Direction::Upload {
                        Some(self.user.clone())
                    } else {
                        sender
                            .map(|value| serde_json::from_str(&value))
                            .transpose()?
                    };
                }
                Ok(transfer)
            })
            .collect()
    }

    pub async fn queue_upload(
        self: &Arc<Self>,
        conversation: Uuid,
        source: PathBuf,
    ) -> Result<Transfer> {
        let _prepare = self.prepare_gate.lock().await;
        self.check_upload(conversation).await?;
        let id = Uuid::new_v4();
        let target = self.spool((id, Direction::Upload));
        let attachment =
            tokio::task::spawn_blocking(move || staging::stage(&source, &target, id)).await??;
        self.start_upload(conversation, attachment).await
    }

    pub async fn queue_pasted_image(
        self: &Arc<Self>,
        conversation: Uuid,
        bytes: Vec<u8>,
    ) -> Result<Transfer> {
        ensure!(
            bytes.len() <= MAX_PASTED_IMAGE_BYTES,
            "Pasted images can be up to 16 MiB"
        );
        let _prepare = self.prepare_gate.lock().await;
        self.check_upload(conversation).await?;
        let id = Uuid::new_v4();
        let spool = self.spool((id, Direction::Upload));
        let attachment =
            tokio::task::spawn_blocking(move || staging::stage_pasted(&bytes, &spool, id))
                .await??;
        self.start_upload(conversation, attachment).await
    }

    async fn check_upload(&self, conversation: Uuid) -> Result<()> {
        self.ensure_open()?;
        ensure!(
            self.directory()
                .await?
                .conversations
                .iter()
                .any(|chat| chat.id == conversation),
            "Select an available conversation before sharing"
        );
        self.check_capacity().await
    }

    async fn start_upload(
        self: &Arc<Self>,
        conversation: Uuid,
        attachment: Attachment,
    ) -> Result<Transfer> {
        let mut job = Job::new(attachment, conversation, Direction::Upload);
        job.transfer.sharer = Some(self.user.clone());
        job.transfer.created_at = Some(self.server_now());
        job.transfer.status = TransferStatus::Queued;
        if let Err(error) = self.store_job(&job).await {
            let _ = fs::remove_file(self.spool(job.key())).await;
            return Err(error);
        }
        self.history_changed();
        let mut workers = self.workers.lock().await;
        self.launch(job, &mut workers).await
    }

    pub async fn queue_download(
        self: &Arc<Self>,
        id: Uuid,
        destination: PathBuf,
    ) -> Result<Transfer> {
        let _prepare = self.prepare_gate.lock().await;
        self.ensure_open()?;
        ensure_new_destination(&destination)?;
        let message = self.attachment(id).await?;
        let file = message
            .attachment
            .filter(Attachment::is_valid)
            .context("Invalid attachment")?;
        let key = (id, Direction::Download);
        let mut workers = self.workers.lock().await;
        ensure!(
            !workers.contains_key(&key),
            "Pause this transfer before choosing another filename"
        );
        let mut job = match self.job(key).await? {
            Some(job)
                if !matches!(
                    job.transfer.status,
                    TransferStatus::Complete | TransferStatus::Expired
                ) =>
            {
                job
            }
            _ => {
                self.check_capacity().await?;
                Job::new(file, message.conversation_id, Direction::Download)
            }
        };
        job.destination = Some(destination);
        job.publishing = false;
        job.retry_at = 0;
        job.transfer.sharer = Some(message.sender);
        job.transfer.status = TransferStatus::Queued;
        job.transfer.error = None;
        self.store_job(&job).await?;
        self.launch(job, &mut workers).await
    }

    pub async fn pause(&self, id: Uuid, direction: Direction) -> Result<()> {
        let workers = self.workers.lock().await;
        if let Some(active) = workers.get(&(id, direction)) {
            active.stop.store(true, Ordering::Relaxed);
        } else if let Some(mut job) = self.job((id, direction)).await?
            && job.transfer.status == TransferStatus::Queued
        {
            job.transfer.status = TransferStatus::Paused;
            self.store_job(&job).await?;
        }
        Ok(())
    }

    pub async fn resume(self: &Arc<Self>, id: Uuid, direction: Direction) -> Result<Transfer> {
        let mut workers = self.workers.lock().await;
        self.ensure_open()?;
        let key = (id, direction);
        let job = self
            .job(key)
            .await?
            .context("This transfer is unavailable")?;
        if workers.contains_key(&key)
            || matches!(
                job.transfer.status,
                TransferStatus::Complete | TransferStatus::Expired
            )
        {
            return Ok(job.transfer);
        }
        ensure!(
            workers.len() < MAX_RUNNING,
            "Pause a running transfer or wait for it to finish"
        );
        self.launch(job, &mut workers).await
    }

    pub async fn discard(&self, id: Uuid, direction: Direction) -> Result<()> {
        let _prepare = self.prepare_gate.lock().await;
        let workers = self.workers.lock().await;
        self.ensure_open()?;
        let key = (id, direction);
        ensure!(
            !workers.contains_key(&key),
            "Pause the transfer before discarding it"
        );
        let job = self
            .job(key)
            .await?
            .context("This transfer is unavailable")?;
        ensure!(
            job.transfer.status != TransferStatus::Complete,
            "This transfer is already complete"
        );
        if direction == Direction::Upload && job.transfer.status != TransferStatus::Expired {
            self.api
                .send(self.api.request(Method::DELETE, &format!("/v1/files/{id}")))
                .await?;
        }
        match fs::remove_file(self.spool(key)).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("Could not remove the partial transfer"),
        }
        sqlx::query("DELETE FROM transfers WHERE id = ? AND direction = ?")
            .bind(id.to_string())
            .bind(direction.as_str())
            .execute(&self.db)
            .await?;
        if direction == Direction::Upload {
            self.history_changed();
        }
        Ok(())
    }

    pub async fn logout(&self) -> Result<()> {
        self.close().await;
        self.api
            .send(self.api.request(Method::DELETE, "/v2/session"))
            .await?;
        Ok(())
    }

    pub async fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
        let _sync = self.sync_gate.lock().await;
        let _outbox = self.outbox_gate.lock().await;
        let active = std::mem::take(&mut *self.workers.lock().await);
        for worker in active.values() {
            worker.stop.store(true, Ordering::Relaxed);
        }
        for (_, worker) in active {
            let _ = worker.task.await;
        }
        let _prepare = self.prepare_gate.lock().await;
    }

    pub(super) fn ensure_open(&self) -> Result<()> {
        ensure!(
            !self.closed.load(Ordering::Relaxed),
            "The session has ended"
        );
        Ok(())
    }

    async fn check_capacity(&self) -> Result<()> {
        let pending: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM transfers WHERE json_extract(payload, '$.status') <> 'complete'",
        )
        .fetch_one(&self.db)
        .await?;
        ensure!(
            pending < MAX_PENDING,
            "Finish or discard a transfer first (8 pending maximum)"
        );
        Ok(())
    }

    async fn launch(
        self: &Arc<Self>,
        mut job: Job,
        workers: &mut HashMap<JobKey, Active>,
    ) -> Result<Transfer> {
        if self.closed.load(Ordering::Relaxed) || workers.len() >= MAX_RUNNING {
            return Ok(job.transfer);
        }
        let key = job.key();
        job.transfer.status = TransferStatus::Running;
        job.transfer.error = None;
        self.store_job(&job).await?;
        let transfer = job.transfer.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let signal = stop.clone();
        let client = self.clone();
        let task = tokio::spawn(async move {
            let result = match key.1 {
                Direction::Upload => client.upload(&mut job, &signal).await,
                Direction::Download => client.download(&mut job, &signal).await,
            };
            match result {
                Ok(status) => job.transfer.status = status,
                Err(error) => {
                    job.transfer.error = Some(error.to_string());
                    job.transfer.status = if signal.load(Ordering::Relaxed) {
                        TransferStatus::Paused
                    } else {
                        match error.downcast_ref::<OnlineError>() {
                            Some(OnlineError::Rejected {
                                status: StatusCode::GONE,
                                ..
                            }) => TransferStatus::Expired,
                            Some(OnlineError::Network(_)) => TransferStatus::Queued,
                            Some(OnlineError::Rejected { status, .. })
                                if matches!(
                                    *status,
                                    StatusCode::UNAUTHORIZED
                                        | StatusCode::TOO_MANY_REQUESTS
                                        | StatusCode::REQUEST_TIMEOUT
                                ) || (status.is_server_error()
                                    && *status != StatusCode::INSUFFICIENT_STORAGE) =>
                            {
                                TransferStatus::Queued
                            }
                            _ => TransferStatus::Failed,
                        }
                    };
                    job.retry_at = unix_millis() + 2_000;
                }
            }
            let mut workers = client.workers.lock().await;
            let complete = job.transfer.status == TransferStatus::Complete;
            if complete
                || (client.store_job(&job).await.is_ok()
                    && job.transfer.status == TransferStatus::Expired)
            {
                let _ = fs::remove_file(client.spool(key)).await;
            }
            workers.remove(&key);
        });
        workers.insert(key, Active { stop, task });
        Ok(transfer)
    }

    pub(super) async fn resume_queued(self: &Arc<Self>) -> Result<()> {
        let mut workers = self.workers.lock().await;
        let available = MAX_RUNNING.saturating_sub(workers.len());
        if available > 0 {
            let rows: Vec<String> = sqlx::query_scalar(
                "SELECT payload FROM transfers WHERE json_extract(payload, '$.status') = 'queued'
                 AND coalesce(json_extract(payload, '$.retry_at'), 0) <= ? ORDER BY rowid LIMIT ?",
            )
            .bind(unix_millis())
            .bind(available as i64)
            .fetch_all(&self.db)
            .await?;
            for row in rows {
                let job: Job = serde_json::from_str(&row)?;
                if !workers.contains_key(&job.key()) {
                    self.launch(job, &mut workers).await?;
                }
            }
        }
        Ok(())
    }

    fn spool(&self, (id, direction): JobKey) -> PathBuf {
        self.staging
            .join(format!("{}-{id}.part", direction.as_str()))
    }

    async fn job(&self, (id, direction): JobKey) -> Result<Option<Job>> {
        let json: Option<String> =
            sqlx::query_scalar("SELECT payload FROM transfers WHERE id = ? AND direction = ?")
                .bind(id.to_string())
                .bind(direction.as_str())
                .fetch_optional(&self.db)
                .await?;
        Ok(json.map(|json| serde_json::from_str(&json)).transpose()?)
    }

    async fn store_job(&self, job: &Job) -> Result<()> {
        let mut db = self.db.acquire().await?;
        store_job(&mut db, job).await
    }

    async fn upload(&self, job: &mut Job, stop: &AtomicBool) -> Result<TransferStatus> {
        let attachment = job.transfer.attachment.clone();
        let conversation = job.transfer.conversation_id;
        let id = attachment.id;
        let progress: UploadProgress = self
            .api
            .json(
                self.api
                    .request(
                        Method::POST,
                        &format!("/v1/conversations/{conversation}/uploads"),
                    )
                    .json(&attachment),
            )
            .await?;
        ensure!(
            progress.received <= attachment.size
                && (progress.received == attachment.size || progress.received % CHUNK_SIZE == 0)
                && (!progress.complete || progress.received == attachment.size),
            OnlineError::InvalidResponse
        );
        job.transfer.received = progress.received;
        self.store_job(job).await?;
        let spool = self.spool(job.key());
        if !progress.complete {
            let mut source = fs::File::open(&spool)
                .await
                .context("The staged file is unavailable. Share it again.")?;
            source.seek(SeekFrom::Start(progress.received)).await?;
            while job.transfer.received < attachment.size {
                if stop.load(Ordering::Relaxed) {
                    return Ok(TransferStatus::Paused);
                }
                let offset = job.transfer.received;
                let len = (attachment.size - offset).min(CHUNK_SIZE);
                let mut bytes = vec![0; len as usize];
                source
                    .read_exact(&mut bytes)
                    .await
                    .context("Could not read the staged attachment")?;
                let progress: UploadProgress = self
                    .api
                    .json(
                        self.api
                            .request(Method::PUT, &format!("/v1/files/{id}/chunks/{offset}"))
                            .body(bytes),
                    )
                    .await?;
                ensure!(
                    progress.received == offset + len && !progress.complete,
                    OnlineError::InvalidResponse
                );
                job.transfer.received = progress.received;
                self.store_job(job).await?;
            }
            if stop.load(Ordering::Relaxed) {
                return Ok(TransferStatus::Paused);
            }
        }
        let stamp: Publication = self
            .api
            .json(
                self.api
                    .request(Method::POST, &format!("/v1/files/{id}/complete")),
            )
            .await?;
        ensure!(stamp.seq > 0, OnlineError::InvalidResponse);
        if !stamp.recalled {
            self.stage_verified(&attachment, spool).await?;
        }
        let message = Message {
            seq: stamp.seq,
            conversation_id: conversation,
            sender: self.user.clone(),
            created_at: stamp.created_at,
            text: None,
            attachment: Some(attachment),
            ai_provider: None,
            recalled: stamp.recalled,
            reply_to: None,
            nonce: None,
            expires_at: stamp.expires_at,
            expired: None,
            pending: None,
            local_file: false,
        };
        let mut tx = self.db.begin().await?;
        persist_message(&mut tx, message).await?;
        let visible: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM inbox WHERE seq = ? AND attachment_id = ?)",
        )
        .bind(stamp.seq)
        .bind(id.to_string())
        .fetch_one(&mut *tx)
        .await?;
        if visible {
            record_local_file(&mut tx, &job.transfer.attachment, stamp.seq, false).await?;
        }
        job.transfer.status = TransferStatus::Complete;
        store_job(&mut tx, job).await?;
        tx.commit().await?;
        if !visible {
            let _ = fs::remove_file(self.managed_path(id)).await;
        }
        self.history_changed();
        Ok(TransferStatus::Complete)
    }

    fn managed_path(&self, id: Uuid) -> PathBuf {
        self.managed_files.join(id.to_string())
    }

    async fn stage_verified(&self, attachment: &Attachment, source: PathBuf) -> Result<PathBuf> {
        let path = self.managed_path(attachment.id);
        let target = path.clone();
        let attachment = attachment.clone();
        tokio::task::spawn_blocking(move || {
            if target.try_exists()? {
                verify_file(&target, &attachment)
            } else {
                publish_verified(&source, &target, &attachment)
            }
        })
        .await??;
        Ok(path)
    }

    pub(super) async fn flush_content_acks(&self) -> Result<()> {
        let pending: Vec<(String, String, i64)> = sqlx::query_as(
            "SELECT id, sha256, size FROM local_files WHERE ack_pending = 1 LIMIT 8",
        )
        .fetch_all(&self.db)
        .await?;
        for (id, sha256, size) in pending {
            let path = self.managed_path(Uuid::parse_str(&id)?);
            let digest = sha256.clone();
            if tokio::task::spawn_blocking(move || check_hash(&path, size as u64, &digest))
                .await?
                .is_err()
            {
                sqlx::query("DELETE FROM local_files WHERE id = ?")
                    .bind(&id)
                    .execute(&self.db)
                    .await?;
                self.history_changed();
                continue;
            }
            let request = self
                .api
                .request(Method::POST, &format!("/v2/files/{id}/ack"))
                .timeout(std::time::Duration::from_secs(3))
                .json(&ContentAck { sha256 });
            match self.api.send(request).await {
                Ok(_)
                | Err(OnlineError::Rejected {
                    status: StatusCode::NOT_FOUND | StatusCode::GONE,
                    ..
                }) => {
                    sqlx::query("UPDATE local_files SET ack_pending = 0 WHERE id = ?")
                        .bind(id)
                        .execute(&self.db)
                        .await?;
                }
                Err(
                    error @ OnlineError::Rejected {
                        status: StatusCode::UNAUTHORIZED,
                        ..
                    },
                ) => return Err(error.into()),
                Err(_) => break,
            }
        }
        Ok(())
    }

    async fn download(&self, job: &mut Job, stop: &AtomicBool) -> Result<TransferStatus> {
        let attachment = job.transfer.attachment.clone();
        let id = attachment.id;
        let destination = job.destination.clone().context("Choose a save location")?;
        let seq: i64 = sqlx::query_scalar("SELECT seq FROM inbox WHERE attachment_id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.db)
            .await?
            .context("This attachment is no longer available")?;
        let spool = self.spool(job.key());
        let path = self.managed_path(id);
        let has_cache: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_files WHERE id = ?)")
                .bind(id.to_string())
                .fetch_one(&self.db)
                .await?;
        if !job.publishing {
            ensure_new_destination(&destination)?;
        }
        let cached = if has_cache || path.is_file() {
            self.stage_verified(&attachment, spool.clone()).await?
        } else {
            self.api
                .send(
                    self.api
                        .request(Method::POST, &format!("/v2/files/{id}/download")),
                )
                .await?;
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .open(&spool)
                .await
                .context("Could not open the partial download")?;
            let length = output.metadata().await?.len();
            let offset = if length == attachment.size {
                length
            } else if length > attachment.size {
                0
            } else {
                length - length % CHUNK_SIZE
            };
            output.set_len(offset).await?;
            output.seek(SeekFrom::Start(offset)).await?;
            job.transfer.received = offset;
            self.store_job(job).await?;
            while job.transfer.received < attachment.size {
                if stop.load(Ordering::Relaxed) {
                    return Ok(TransferStatus::Paused);
                }
                let offset = job.transfer.received;
                let expected = (attachment.size - offset).min(CHUNK_SIZE) as usize;
                let bytes = self.remote_chunk(id, offset, expected).await?;
                output
                    .write_all(&bytes)
                    .await
                    .context("Could not write the partial download")?;
                output.flush().await?;
                job.transfer.received += expected as u64;
                self.store_job(job).await?;
            }
            output.sync_all().await?;
            drop(output);
            match self.stage_verified(&attachment, spool.clone()).await {
                Ok(path) => path,
                Err(error) => {
                    let _ = fs::remove_file(&spool).await;
                    job.transfer.received = 0;
                    return Err(error);
                }
            }
        };
        if stop.load(Ordering::Relaxed) {
            return Ok(TransferStatus::Paused);
        }
        let recovering = job.publishing;
        job.publishing = true;
        self.store_job(job).await?;
        let file = attachment.clone();
        let saved = tokio::task::spawn_blocking(move || {
            if recovering && destination.try_exists()? {
                verify_file(&destination, &file)
            } else {
                publish_verified(&cached, &destination, &file)
            }
        })
        .await?;
        if let Err(error) = saved {
            job.publishing = false;
            return Err(error);
        }
        let mut tx = self.db.begin().await?;
        let visible: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM inbox WHERE seq = ? AND attachment_id = ?)",
        )
        .bind(seq)
        .bind(id.to_string())
        .fetch_one(&mut *tx)
        .await?;
        if !visible {
            let _ = fs::remove_file(path).await;
            bail!("This attachment was recalled");
        }
        record_local_file(&mut tx, &attachment, seq, !has_cache).await?;
        job.publishing = false;
        job.transfer.received = attachment.size;
        job.transfer.status = TransferStatus::Complete;
        store_job(&mut tx, job).await?;
        tx.commit().await?;
        self.history_changed();
        Ok(TransferStatus::Complete)
    }

    pub async fn read_attachment_chunk(
        &self,
        id: Uuid,
        offset: u64,
        expected: usize,
    ) -> Result<Vec<u8>> {
        self.ensure_open()?;
        ensure!(
            expected <= CHUNK_SIZE as usize,
            OnlineError::InvalidResponse
        );
        let size: Option<i64> = sqlx::query_scalar(
            "SELECT size FROM local_files f JOIN inbox i ON i.attachment_id = f.id
             WHERE f.id = ? AND NOT EXISTS (SELECT 1 FROM hidden_messages h WHERE h.seq = i.seq)",
        )
        .bind(id.to_string())
        .fetch_optional(&self.db)
        .await?;
        if let Some(size) = size {
            ensure!(
                offset <= size as u64 && expected as u64 <= size as u64 - offset,
                OnlineError::InvalidResponse
            );
            let mut file = fs::File::open(self.managed_path(id)).await?;
            ensure!(
                file.metadata().await?.len() == size as u64,
                "The local file is incomplete"
            );
            file.seek(SeekFrom::Start(offset)).await?;
            let mut bytes = vec![0; expected];
            file.read_exact(&mut bytes).await?;
            return Ok(bytes);
        }
        self.remote_chunk(id, offset, expected).await
    }

    async fn remote_chunk(&self, id: Uuid, offset: u64, expected: usize) -> Result<Vec<u8>> {
        self.ensure_open()?;
        let response = self
            .api
            .send(
                self.api
                    .request(Method::GET, &format!("/v1/files/{id}/chunks/{offset}")),
            )
            .await?;
        let bytes = read_limited(response, expected).await?;
        ensure!(bytes.len() == expected, OnlineError::InvalidResponse);
        Ok(bytes)
    }
}

async fn store_job(db: &mut SqliteConnection, job: &Job) -> Result<()> {
    let (id, direction) = job.key();
    sqlx::query(
        "INSERT INTO transfers (id, direction, payload) VALUES (?,?,?)
             ON CONFLICT (id, direction) DO UPDATE SET payload = excluded.payload",
    )
    .bind(id.to_string())
    .bind(direction.as_str())
    .bind(serde_json::to_string(job)?)
    .execute(db)
    .await
    .context("Could not save transfer progress")?;
    Ok(())
}

async fn record_local_file(
    db: &mut SqliteConnection,
    file: &Attachment,
    seq: i64,
    ack_pending: bool,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO local_files (id, message_seq, sha256, size, ack_pending) VALUES (?,?,?,?,?)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(file.id.to_string())
    .bind(seq)
    .bind(&file.sha256)
    .bind(file.size as i64)
    .bind(ack_pending)
    .execute(db)
    .await?;
    Ok(())
}

fn open_verified_input(path: &std::path::Path, size: u64) -> Result<std::fs::File> {
    let metadata = std::fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && !staging::is_link(&metadata) && metadata.len() == size,
        "File length or type verification failed. Retry the download."
    );
    Ok(std::fs::File::open(path)?)
}

fn check_hash(path: &std::path::Path, size: u64, sha256: &str) -> Result<()> {
    let input = open_verified_input(path, size)?;
    ensure!(
        staging::hash_copy(input, std::io::sink())? == sha256,
        "File verification failed. Retry the download."
    );
    Ok(())
}

fn verify_file(path: &std::path::Path, attachment: &Attachment) -> Result<()> {
    check_hash(path, attachment.size, &attachment.sha256)
}

fn publish_verified(
    source: &std::path::Path,
    destination: &std::path::Path,
    attachment: &Attachment,
) -> Result<()> {
    let parent = destination.parent().context("Choose a save folder")?;
    let input = open_verified_input(source, attachment.size)?;
    let mut temp = NamedTempFile::new_in(parent).context("Could not write to the save folder")?;
    ensure!(
        staging::hash_copy(input, temp.as_file_mut())? == attachment.sha256,
        "File verification failed. Retry the download."
    );
    temp.as_file().sync_all()?;
    temp.persist_noclobber(destination)
        .context("Could not publish the download. Choose an unused filename.")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AttachmentKind, DeviceSession, FileFilter};
    use super::super::Api;

    #[tokio::test]
    async fn shared_files_keep_the_upload_position_and_cursor_after_publication() -> Result<()> {
        let root = tempfile::tempdir()?;
        let session = || DeviceSession {
            user: User { id: Uuid::nil(), handle: "file_fixture".into() },
            device_id: Uuid::nil(),
            cursor: Uuid::nil(),
        };
        let open = || Client::open(root.path(), Api::new("http://127.0.0.1:7841").unwrap(), session(), false);
        let client = open().await?;
        let conversation = Uuid::new_v4();
        let attachment = |name: &str| Attachment {
            id: Uuid::new_v4(), kind: AttachmentKind::File, name: name.into(), size: 1, sha256: "0".repeat(64),
        };
        let message = |seq, file: Attachment, created_at| Message {
            seq, conversation_id: conversation, sender: session().user, created_at, text: None,
            attachment: Some(file), ai_provider: None, recalled: false, reply_to: None, nonce: None,
            expires_at: None, expired: None, pending: None, local_file: false,
        };
        let pending = attachment("pending.txt");
        let mut upload = Job::new(pending.clone(), conversation, Direction::Upload);
        upload.transfer.created_at = Some(200);
        upload.transfer.status = TransferStatus::Failed;
        client.store_job(&upload).await?;
        let mut tx = client.db.begin().await?;
        persist_message(&mut tx, message(10, attachment("old.txt"), 100)).await?;
        persist_message(&mut tx, message(11, attachment("new.txt"), 300)).await?;
        tx.commit().await?;
        drop(client);

        let client = open().await?;
        let filter = || FileFilter { conversation_id: Some(conversation), include_transfer_rows: true, ..Default::default() };
        let before = client.files(filter(), None).await?.items;
        assert_eq!(before.iter().map(|message| message.created_at).collect::<Vec<_>>(), [300, 200, 100]);
        assert_eq!(before[1].sender.handle, "file_fixture");
        let pending_cursor = before[1].seq;
        assert!(pending_cursor < 0);
        assert_eq!(client.transfers().await?[0].status, TransferStatus::Failed);
        assert_eq!(client.files(FileFilter::default(), None).await?.items.len(), 2);

        upload.transfer.status = TransferStatus::Complete;
        let mut tx = client.db.begin().await?;
        persist_message(&mut tx, message(12, pending.clone(), 400)).await?;
        store_job(&mut tx, &upload).await?;
        tx.commit().await?;
        let after = client.files(filter(), None).await?.items;
        assert_eq!(after.iter().map(|message| message.seq).collect::<Vec<_>>(), [11, 12, 10]);
        assert_eq!(after[1].attachment.as_ref(), Some(&pending));
        assert_eq!(after[1].created_at, 400);
        assert_eq!(client.files(filter(), Some(pending_cursor)).await?.items[0].seq, 10);
        assert_eq!(client.files(FileFilter { oldest_first: true, ..filter() }, Some(pending_cursor)).await?.items[0].seq, 11);
        Ok(())
    }
}
