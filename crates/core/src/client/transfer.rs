use super::{Client, OnlineError, ensure_new_destination, read_limited, staging};
use crate::{Attachment, CHUNK_SIZE, MAX_PASTED_IMAGE_BYTES, UploadProgress, User};
use anyhow::{Context, Result, bail, ensure};
use reqwest::Method;
use serde::{Deserialize, Serialize};
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
    Paused,
    Failed,
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
}

#[derive(Serialize, Deserialize)]
struct Job {
    #[serde(flatten)]
    transfer: Transfer,
    destination: Option<PathBuf>,
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
            },
            destination: None,
        }
    }

    fn key(&self) -> JobKey {
        (self.transfer.attachment.id, self.transfer.direction)
    }
}

impl Client {
    pub async fn transfers(&self) -> Result<Vec<Transfer>> {
        let rows: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT t.payload, json_extract(i.payload, '$.sender') FROM transfers t
             LEFT JOIN inbox i ON t.id = i.attachment_id
             WHERE NOT EXISTS (SELECT 1 FROM hidden_messages h WHERE h.attachment_id = t.id)
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
        if let Err(error) = self.store_job(&job).await {
            let _ = fs::remove_file(self.spool(job.key())).await;
            return Err(error);
        }
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
            Some(job) if job.transfer.status != TransferStatus::Complete => job,
            _ => {
                self.check_capacity().await?;
                Job::new(file, message.conversation_id, Direction::Download)
            }
        };
        job.destination = Some(destination);
        job.transfer.sharer = Some(message.sender);
        job.transfer.status = TransferStatus::Paused;
        job.transfer.error = None;
        self.store_job(&job).await?;
        self.launch(job, &mut workers).await
    }

    pub async fn pause(&self, id: Uuid, direction: Direction) -> Result<()> {
        let workers = self.workers.lock().await;
        if let Some(active) = workers.get(&(id, direction)) {
            active.stop.store(true, Ordering::Relaxed);
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
        if workers.contains_key(&key) || job.transfer.status == TransferStatus::Complete {
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
        if direction == Direction::Upload {
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
        Ok(())
    }

    pub async fn logout(&self) -> Result<()> {
        self.closed.store(true, Ordering::Relaxed);
        let active = std::mem::take(&mut *self.workers.lock().await);
        for worker in active.values() {
            worker.stop.store(true, Ordering::Relaxed);
        }
        for (_, worker) in active {
            let _ = worker.task.await;
        }
        let _prepare = self.prepare_gate.lock().await;
        self.api
            .send(self.api.request(Method::DELETE, "/v1/session"))
            .await?;
        Ok(())
    }

    fn ensure_open(&self) -> Result<()> {
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
                    job.transfer.status = TransferStatus::Failed;
                    job.transfer.error = Some(error.to_string());
                }
            }
            let mut workers = client.workers.lock().await;
            if client.store_job(&job).await.is_ok()
                && job.transfer.status == TransferStatus::Complete
            {
                let _ = fs::remove_file(client.spool(key)).await;
            }
            workers.remove(&key);
        });
        workers.insert(key, Active { stop, task });
        Ok(transfer)
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
        let (id, direction) = job.key();
        sqlx::query(
            "INSERT INTO transfers (id, direction, payload) VALUES (?,?,?)
             ON CONFLICT (id, direction) DO UPDATE SET payload = excluded.payload",
        )
        .bind(id.to_string())
        .bind(direction.as_str())
        .bind(serde_json::to_string(job)?)
        .execute(&self.db)
        .await
        .context("Could not save transfer progress")?;
        Ok(())
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
        if progress.complete {
            return Ok(TransferStatus::Complete);
        }
        self.store_job(job).await?;
        let mut source = fs::File::open(self.spool(job.key()))
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
        self.api
            .send(
                self.api
                    .request(Method::POST, &format!("/v1/files/{id}/complete")),
            )
            .await?;
        Ok(TransferStatus::Complete)
    }

    async fn download(&self, job: &mut Job, stop: &AtomicBool) -> Result<TransferStatus> {
        let attachment = job.transfer.attachment.clone();
        let id = attachment.id;
        let destination = job.destination.clone().context("Choose a save location")?;
        ensure_new_destination(&destination)?;
        let spool = self.spool(job.key());
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
            let bytes = self.read_attachment_chunk(id, offset, expected).await?;
            output
                .write_all(&bytes)
                .await
                .context("Could not write the partial download")?;
            output.flush().await?;
            job.transfer.received += expected as u64;
            self.store_job(job).await?;
        }
        drop(output);
        if stop.load(Ordering::Relaxed) {
            return Ok(TransferStatus::Paused);
        }
        let source = spool.clone();
        let verified = tokio::task::spawn_blocking(move || -> Result<bool> {
            let parent = destination.parent().context("Choose a save folder")?;
            let mut temp =
                NamedTempFile::new_in(parent).context("Could not write to the save folder")?;
            if staging::hash_copy(std::fs::File::open(source)?, temp.as_file_mut())?
                != attachment.sha256
            {
                return Ok(false);
            }
            temp.as_file().sync_all()?;
            temp.persist_noclobber(destination)
                .context("Could not publish the download. Choose an unused filename.")?;
            Ok(true)
        })
        .await??;
        if !verified {
            fs::remove_file(spool).await?;
            job.transfer.received = 0;
            bail!("File verification failed. Retry the download.");
        }
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
