use super::{
    Client, MediaDescriptor, MediaVariant, OnlineError, ensure_new_destination,
    local_sync::persist_message, media_store, read_limited, staging, unix_millis,
};
use crate::{
    Attachment, CHUNK_SIZE, ContentAck, ForwardRequest, ForwardResult, ForwardUploadProgress,
    ForwardUploadRequest, MAX_PASTED_IMAGE_BYTES, Message, Publication, UploadProgress, User,
};
use anyhow::{Context, Result, bail, ensure};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use sqlx::SqliteConnection;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    overwrite: Option<OverwriteTarget>,
    #[serde(default)]
    publishing: bool,
    #[serde(default)]
    retry_at: i64,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OverwriteTarget {
    volume: u32,
    index: u64,
    size: u64,
    last_write: u64,
}

pub(super) type JobKey = (Uuid, Direction);

pub(super) struct Active {
    conversation: Uuid,
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
            overwrite: None,
            publishing: false,
            retry_at: 0,
        }
    }

    fn key(&self) -> JobKey {
        (self.transfer.attachment.id, self.transfer.direction)
    }
}

impl Client {
    pub async fn hide_pending_upload(&self, conversation: Uuid, id: Uuid) -> Result<()> {
        self.ensure_open()?;
        let prepare = self.prepare_gate.lock().await;
        let workers = self.workers.lock().await;
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        let chat = conversation.to_string();
        let published: Option<i64> = sqlx::query_scalar("SELECT seq FROM inbox WHERE attachment_id = ? AND conversation_id = ?")
            .bind(id.to_string()).bind(&chat).fetch_optional(&mut *tx).await?;
        if let Some(seq) = published {
            tx.commit().await?;
            drop(workers);
            drop(prepare);
            return self.hide_message(conversation, seq).await;
        }
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM transfers WHERE id = ? AND direction = 'upload'
            AND json_extract(payload, '$.conversation_id') = ? AND json_extract(payload, '$.status') <> 'complete')")
            .bind(id.to_string()).bind(&chat).fetch_one(&mut *tx).await?;
        ensure!(exists, "This upload is unavailable");
        sqlx::query("INSERT OR IGNORE INTO local_suppressed_sends (kind, id, conversation_id) VALUES ('upload', ?, ?)")
            .bind(id.to_string()).bind(&chat).execute(&mut *tx).await?;
        sqlx::query("UPDATE transfers SET payload = json_set(payload, '$.status', 'queued', '$.retry_at', 0)
            WHERE id = ? AND direction = 'upload' AND json_extract(payload, '$.status') IN ('paused','failed','expired')")
            .bind(id.to_string()).execute(&mut *tx).await?;
        let paths = release_detached_upload_media(&mut tx, &chat, Some(id)).await?;
        tx.commit().await?;
        self.remove_rel_paths(paths).await;
        self.history_changed();
        Ok(())
    }

    pub(super) async fn delete_received_history(&self, conversation: Uuid, seq: Option<i64>, delete_chat: bool) -> Result<()> {
        self.ensure_open()?;
        ensure!(seq != Some(0), "This message is unavailable");
        let _prepare = self.prepare_gate.lock().await;
        let workers = self.workers.lock().await;
        let chat = conversation.to_string();
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(seq) = seq.filter(|seq| *seq < 0) {
            let id = seq.checked_neg().context("Invalid message ID")?;
            let nonce: Option<String> = sqlx::query_scalar(
                "SELECT nonce FROM outbox WHERE id = ? AND conversation_id = ?",
            ).bind(id).bind(&chat).fetch_optional(&mut *tx).await?;
            let nonce = nonce.context("This message is unavailable")?;
            sqlx::query("INSERT OR IGNORE INTO local_suppressed_sends (kind, id, conversation_id) VALUES ('text', ?, ?)")
                .bind(&nonce).bind(&chat).execute(&mut *tx).await?;
            sqlx::query("DELETE FROM outbox WHERE id = ? AND blocked = 1")
                .bind(id).execute(&mut *tx).await?;
            tx.commit().await?;
            self.history_changed();
            return Ok(());
        }
        let rows: Vec<(i64, Option<String>)> = sqlx::query_as(
            "SELECT i.seq, coalesce(i.attachment_id, h.attachment_id) FROM inbox i
                LEFT JOIN hidden_messages h ON h.seq = i.seq
                WHERE i.conversation_id = ?1 AND (?2 IS NULL OR i.seq = ?2)",
        )
        .bind(&chat)
        .bind(seq)
        .fetch_all(&mut *tx)
        .await?;
        if let Some(seq) = seq
            && rows.is_empty()
        {
            let deleted: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM local_deleted_messages WHERE seq = ? AND conversation_id = ?)",
            ).bind(seq).bind(&chat).fetch_one(&mut *tx).await?;
            ensure!(deleted, "This message is unavailable");
        }
        sqlx::query(
            "INSERT OR IGNORE INTO local_deleted_messages (seq, conversation_id, attachment_id)
                SELECT i.seq, i.conversation_id, coalesce(i.attachment_id, h.attachment_id)
                FROM inbox i LEFT JOIN hidden_messages h ON h.seq = i.seq
                WHERE i.conversation_id = ?1 AND (?2 IS NULL OR i.seq = ?2)",
        ).bind(&chat).bind(seq).execute(&mut *tx).await?;
        sqlx::query(
            "DELETE FROM hidden_messages WHERE seq IN (
                SELECT seq FROM inbox WHERE conversation_id = ?1 AND (?2 IS NULL OR seq = ?2))",
        ).bind(&chat).bind(seq).execute(&mut *tx).await?;
        // Remove the entire selected set before releasing refs so dedup cannot rebind a deleted sibling.
        sqlx::query("DELETE FROM inbox WHERE conversation_id = ?1 AND (?2 IS NULL OR seq = ?2)")
            .bind(&chat).bind(seq).execute(&mut *tx).await?;
        let mut media_paths = Vec::new();
        let mut legacy_files = Vec::new();
        let mut downloads = Vec::new();
        if seq.is_none() {
            sqlx::query("INSERT OR IGNORE INTO local_suppressed_sends (kind, id, conversation_id)
                SELECT 'text', nonce, conversation_id FROM outbox WHERE conversation_id = ?")
                .bind(&chat).execute(&mut *tx).await?;
            sqlx::query("INSERT OR IGNORE INTO local_suppressed_sends (kind, id, conversation_id)
                SELECT 'upload', id, ?1 FROM transfers WHERE direction = 'upload'
                    AND json_extract(payload, '$.conversation_id') = ?1
                    AND json_extract(payload, '$.status') <> 'complete'")
                .bind(&chat).execute(&mut *tx).await?;
            sqlx::query("DELETE FROM outbox WHERE conversation_id = ? AND blocked = 1")
                .bind(&chat).execute(&mut *tx).await?;
            sqlx::query("UPDATE transfers SET payload = json_set(payload, '$.status', 'queued', '$.retry_at', 0)
                WHERE direction = 'upload' AND json_extract(payload, '$.conversation_id') = ?
                    AND json_extract(payload, '$.status') IN ('paused', 'failed', 'expired')")
                .bind(&chat).execute(&mut *tx).await?;
            media_paths.extend(release_detached_upload_media(&mut tx, &chat, None).await?);
        }
        if delete_chat {
            sqlx::query("INSERT OR IGNORE INTO local_deleted_chats (conversation_id) VALUES (?)")
                .bind(&chat).execute(&mut *tx).await?;
            let ids: Vec<String> = sqlx::query_scalar("SELECT id FROM transfers WHERE direction = 'download'
                AND json_extract(payload, '$.conversation_id') = ?")
                .bind(&chat).fetch_all(&mut *tx).await?;
            for id in ids {
                sqlx::query("INSERT OR IGNORE INTO cancelled_downloads (id) VALUES (?)")
                    .bind(&id).execute(&mut *tx).await?;
                media_paths.extend(media_store::release_media_refs_in_tx(&mut tx, "download", &id).await?);
                downloads.push(Uuid::parse_str(&id)?);
            }
            sqlx::query("DELETE FROM transfers WHERE direction = 'download' AND json_extract(payload, '$.conversation_id') = ?")
                .bind(&chat).execute(&mut *tx).await?;
        }
        for (seq, attachment_id) in rows {
            if let Some(id) = attachment_id.as_ref() {
                sqlx::query("INSERT OR IGNORE INTO local_suppressed_sends (kind, id, conversation_id)
                    SELECT 'upload', id, ?2 FROM transfers WHERE id = ?1 AND direction = 'upload'
                    AND json_extract(payload, '$.status') <> 'complete'")
                    .bind(id).bind(&chat).execute(&mut *tx).await?;
                if !delete_chat {
                    // An already requested external save owns its original independently of the message.
                    sqlx::query("INSERT OR IGNORE INTO media_refs (file_id, ref_kind, ref_id, conversation_id, attachment_id, variant, created_at)
                        SELECT r.file_id, 'download', ?1, r.conversation_id, r.attachment_id, r.variant, ?3
                        FROM media_refs r JOIN transfers t ON t.id = ?1 AND t.direction = 'download'
                        WHERE r.ref_kind = 'message' AND r.ref_id = ?2 AND r.attachment_id = ?1
                        AND r.variant IN ('image_original', 'video_original', 'attachment')
                        AND json_extract(t.payload, '$.status') <> 'complete'")
                        .bind(id).bind(seq.to_string()).bind(unix_millis()).execute(&mut *tx).await?;
                }
            }
            media_paths.extend(media_store::release_media_refs_in_tx(
                &mut tx, "message", &seq.to_string(),
            ).await?);
            sqlx::query("DELETE FROM media_previews WHERE message_seq = ?")
                .bind(seq).execute(&mut *tx).await?;
            let legacy: Option<String> = sqlx::query_scalar(
                "DELETE FROM local_files WHERE message_seq = ? RETURNING id",
            ).bind(seq).fetch_optional(&mut *tx).await?;
            if let Some(id) = legacy {
                legacy_files.push(id);
            }
            if let Some(id) = attachment_id {
                sqlx::query(
                    "DELETE FROM transfers WHERE id = ? AND json_extract(payload, '$.status') = 'complete'",
                ).bind(&id).execute(&mut *tx).await?;
            }
        }
        if seq.is_none() {
            sqlx::query("DELETE FROM local_read_positions WHERE conversation_id = ?")
                .bind(&chat).execute(&mut *tx).await?;
        }
        // Signal active downloads before committing cancellation; external files already published remain user-owned.
        for id in &downloads {
            if let Some(active) = workers.get(&(*id, Direction::Download)) {
                active.stop.store(true, Ordering::Relaxed);
            }
        }
        tx.commit().await?;
        for id in downloads {
            let key = (id, Direction::Download);
            if let Some(active) = workers.get(&key) {
                active.stop.store(true, Ordering::Relaxed);
            } else {
                let _ = fs::remove_file(self.spool(key)).await;
            }
        }
        drop(workers);
        self.remove_rel_paths(media_paths).await;
        for id in legacy_files {
            if let Ok(id) = Uuid::parse_str(&id) {
                let _ = fs::remove_file(self.managed_files.join(id.to_string())).await;
            }
        }
        self.history_changed();
        Ok(())
    }

    pub(super) async fn migrate_hidden_history(&self) -> Result<()> {
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT i.conversation_id, i.seq FROM inbox i JOIN hidden_messages h ON h.seq = i.seq",
        ).fetch_all(&self.db).await?;
        for (conversation, seq) in rows {
            self.delete_received_history(Uuid::parse_str(&conversation)?, Some(seq), false).await?;
        }
        Ok(())
    }

    pub(super) async fn restore_upload_times(&self) -> Result<()> {
        let deleted_attachments: Vec<String> = sqlx::query_scalar(
            "SELECT attachment_id FROM local_deleted_messages WHERE attachment_id IS NOT NULL",
        ).fetch_all(&self.db).await?;
        for id in deleted_attachments {
            if let Ok(id) = Uuid::parse_str(&id) {
                let _ = fs::remove_file(self.managed_files.join(id.to_string())).await;
            }
        }
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        let stale_downloads: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT ref_id FROM media_refs r WHERE r.ref_kind = 'download'
                AND NOT EXISTS (SELECT 1 FROM transfers t WHERE t.id = r.ref_id AND t.direction = 'download'
                    AND json_extract(t.payload, '$.status') <> 'complete')",
        ).fetch_all(&mut *tx).await?;
        let mut paths = Vec::new();
        for id in stale_downloads {
            paths.extend(media_store::release_media_refs_in_tx(&mut tx, "download", &id).await?);
        }
        tx.commit().await?;
        self.remove_rel_paths(paths).await;
        let retired: Vec<(String, String)> = sqlx::query_as(
            "SELECT id, 'download' FROM cancelled_downloads
             UNION SELECT id, 'upload' FROM local_suppressed_sends s WHERE s.kind = 'upload'
                AND NOT EXISTS (SELECT 1 FROM transfers t WHERE t.id = s.id AND t.direction = 'upload')",
        ).fetch_all(&self.db).await?;
        for (id, direction) in retired {
            if let Ok(id) = Uuid::parse_str(&id) {
                let direction = if direction == "upload" { Direction::Upload } else { Direction::Download };
                let _ = fs::remove_file(self.spool((id, direction))).await;
            }
        }
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
        let verified = tokio::task::spawn_blocking(move || verify_file(&path, &file).ok().map(|()| (path, file)))
            .await?;
        self.ensure_open()?;
        let Some((path, file)) = verified else { return Ok(None) };
        ensure!(self.attachment(id).await?.attachment == Some(file), "This attachment is unavailable");
        Ok(Some(path))
    }

    pub async fn transfers(&self) -> Result<Vec<Transfer>> {
        let rows: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT t.payload, json_extract(i.payload, '$.sender') FROM transfers t
             LEFT JOIN inbox i ON t.id = i.attachment_id
             WHERE (json_extract(t.payload, '$.status') <> 'complete'
                OR NOT EXISTS (SELECT 1 FROM hidden_messages h WHERE h.attachment_id = t.id))
                AND NOT EXISTS (SELECT 1 FROM local_suppressed_sends s WHERE s.kind = 'upload' AND t.direction = 'upload' AND s.id = t.id)
                AND ((t.direction = 'download' AND json_extract(t.payload, '$.status') <> 'complete')
                    OR NOT EXISTS (SELECT 1 FROM local_deleted_messages d WHERE d.attachment_id = t.id))
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

    /// Keep the request and target UUIDs unchanged when retrying an uncertain result.
    pub async fn forward_attachment(
        self: &Arc<Self>,
        source_id: Uuid,
        request: &ForwardRequest,
    ) -> Result<Vec<ForwardResult>> {
        self.ensure_open()?;
        let route = format!("/v2/files/{source_id}/forward");
        let mut results = self.forward_results(&route, request).await?;
        let needs_upload = results.iter().any(|item| item.status == 410);
        if !needs_upload && !results.iter().any(|item| item.publication.is_some()) {
            return Ok(results);
        }
        // A direct forward never downloads the original. A verified cached original can
        // also restore target caches after an earlier publication response was lost.
        let Ok(message) = self.attachment(source_id).await else { return Ok(results); };
        let Some(original) = message.attachment else { return Ok(results); };
        if message.recalled || !message.local_file {
            return Ok(results);
        }
        // Use the media store's active-temp guard so cleanup skips the snapshot
        // during forwarding, but can reclaim abandoned snapshots after a crash.
        let prefix = format!("forward-{}-", Uuid::new_v4());
        let _snapshot_guard = media_store::ActiveTempGuard::track(&self.active_tmp, prefix.clone());
        let staged: Result<Option<tempfile::TempPath>> = async {
            if !self.ensure_message_authorized_for_cache(message.conversation_id, message.seq).await? {
                return Ok(None);
            }
            let cached = self.verified_media_path(
                message.conversation_id, message.seq, Some(source_id),
                MediaVariant::for_attachment(&original), &original.sha256, true,
            ).await?;
            let (_, path, _) = cached.filter(|(_, _, size)| *size == original.size)
                .context("The local original failed verification.")?;
            let expected = original.clone();
            let destination = media_store::fs_path(&self.media_root().join("tmp").join("snapshot.part"));
            let staged = tokio::task::spawn_blocking(move || {
                stage_publication(&path, &destination, &expected, &prefix)
            }).await??;
            Ok(Some(staged))
        }.await;
        let staged = match staged {
            Ok(Some(path)) => path,
            Ok(None) => return Ok(results),
            Err(error) => {
                for item in &mut results {
                    if item.status == 410 {
                        item.status = 422;
                        item.error = Some(error.to_string());
                    } else if item.publication.is_some() {
                        item.cache_error = Some(error.to_string());
                    }
                }
                return Ok(results);
            }
        };
        if needs_upload {
            self.ensure_open()?;
            let upload: ForwardUploadProgress = self.api.json(
                self.api.request(Method::POST, &format!("/v2/files/{source_id}/forward-upload"))
                    .json(&ForwardUploadRequest { request: request.clone(), original: original.clone() }),
            ).await?;
            let mut received = upload.progress.received;
            ensure!(received <= original.size
                && (received == original.size || received % CHUNK_SIZE == 0)
                && (!upload.progress.complete || received == original.size), OnlineError::InvalidResponse);
            if !upload.progress.complete {
                let mut file = fs::File::open(&staged).await?;
                file.seek(SeekFrom::Start(received)).await?;
                while received < original.size {
                    self.ensure_open()?;
                    let len = (original.size - received).min(CHUNK_SIZE);
                    let mut bytes = vec![0; len as usize];
                    file.read_exact(&mut bytes).await?;
                    let progress: UploadProgress = self.api.json(
                        self.api.request(Method::PUT, &format!("/v1/files/{}/chunks/{received}", upload.id))
                            .body(bytes),
                    ).await?;
                    ensure!(progress.received == received + len && !progress.complete,
                        OnlineError::InvalidResponse);
                    received = progress.received;
                }
            }
            results = self.forward_results(&route, request).await?;

        }
        // Publication and cache recovery have separate outcomes. Never turn a known
        // committed target into a failed send because its local cache could not converge.
        if let Err(error) = self.sync_inbox().await {
            for item in &mut results {
                if item.publication.is_some() { item.cache_error = Some(error.to_string()); }
            }
            return Ok(results);
        }
        for item in &mut results {
            if let Some(stamp) = item.publication.as_ref()
                && let Err(error) = self.cache_forwarded_original(
                    &item.target, stamp, &original, &staged,
                ).await
            {
                item.cache_error = Some(error.to_string());
            }
        }
        self.history_changed();
        Ok(results)
    }

    async fn cache_forwarded_original(
        &self, target: &crate::ForwardTarget, stamp: &Publication, original: &Attachment, source: &Path,
    ) -> Result<()> {
        if stamp.recalled || !self.ensure_message_authorized_for_cache(target.conversation_id, stamp.seq).await? {
            return Ok(());
        }
        let message = self.attachment(target.attachment_id).await?;
        let Some(file) = message.attachment else { return Ok(()); };
        if message.recalled || message.seq != stamp.seq || message.conversation_id != target.conversation_id
            || file.sha256 != original.sha256 || file.size != original.size {
            return Ok(());
        }
        let desc = MediaDescriptor {
            conversation_id: target.conversation_id,
            message_seq: stamp.seq,
            attachment_id: Some(file.id),
            variant: MediaVariant::for_attachment(&file),
            content_key: &file.sha256,
            expected_sha256: &file.sha256,
            expected_size: file.size,
            mime: file.preview_mime().unwrap_or("application/octet-stream"),
            dimensions: None,
        };
        self.commit_media_file(&desc, source).await?;
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        let visible: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM preview_eligible WHERE seq = ? AND attachment_id = ? AND conversation_id = ?)",
        ).bind(stamp.seq).bind(file.id.to_string()).bind(target.conversation_id.to_string())
            .fetch_one(&mut *tx).await?;
        let removed = if visible {
            record_local_file(&mut tx, &file, stamp.seq, true).await?;
            Vec::new()
        } else {
            media_store::release_media_refs_in_tx(&mut tx, "message", &stamp.seq.to_string()).await?
        };
        tx.commit().await?;
        self.remove_rel_paths(removed).await;
        Ok(())
    }

    async fn forward_results(&self, route: &str, request: &ForwardRequest) -> Result<Vec<ForwardResult>> {
        let results: Vec<ForwardResult> = self.api.json(
            self.api.request(Method::POST, route).json(request),
        ).await?;
        ensure!(results.len() == request.targets.len()
            && results.iter().zip(&request.targets).all(|(item, target)| {
                &item.target == target && ((item.status == 200) == item.publication.is_some())
                    && item.publication.as_ref().is_none_or(|stamp| stamp.seq > 0)
            }), OnlineError::InvalidResponse);
        Ok(results)
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

    pub async fn queue_upload_verified(
        self: &Arc<Self>,
        conversation: Uuid,
        upload_id: Uuid,
        source: PathBuf,
        expected: staging::SourceSnapshot,
    ) -> Result<Transfer> {
        let _prepare = self.prepare_gate.lock().await;
        self.ensure_open()?;
        ensure!(!self.is_suppressed_upload(upload_id).await?, "This upload was deleted locally");
        if let Some(existing) = self.job((upload_id, Direction::Upload)).await? {
            ensure!(
                existing.transfer.conversation_id == conversation,
                "This attachment belongs to another conversation"
            );
            return Ok(existing.transfer);
        }
        self.check_upload(conversation).await?;
        let target = self.spool((upload_id, Direction::Upload));
        let attachment = tokio::task::spawn_blocking(move || {
            staging::stage_verified(&source, &expected, &target, upload_id)
        })
        .await??;
        self.start_upload(conversation, attachment).await
    }

    pub async fn queue_pasted_image(
        self: &Arc<Self>,
        conversation: Uuid,
        bytes: Vec<u8>,
    ) -> Result<Transfer> {
        self.queue_pasted_image_with_id(conversation, Uuid::new_v4(), bytes)
            .await
    }

    pub async fn queue_pasted_image_with_id(
        self: &Arc<Self>,
        conversation: Uuid,
        upload_id: Uuid,
        bytes: Vec<u8>,
    ) -> Result<Transfer> {
        ensure!(
            bytes.len() <= MAX_PASTED_IMAGE_BYTES,
            "Pasted images can be up to 16 MiB"
        );
        let _prepare = self.prepare_gate.lock().await;
        self.ensure_open()?;
        ensure!(!self.is_suppressed_upload(upload_id).await?, "This upload was deleted locally");
        if let Some(existing) = self.job((upload_id, Direction::Upload)).await? {
            ensure!(
                existing.transfer.conversation_id == conversation,
                "This attachment belongs to another conversation"
            );
            return Ok(existing.transfer);
        }
        self.check_upload(conversation).await?;
        let spool = self.spool((upload_id, Direction::Upload));
        let attachment =
            tokio::task::spawn_blocking(move || staging::stage_pasted(&bytes, &spool, upload_id))
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
        self.queue_download_to(id, destination, None).await
    }

    #[cfg(windows)]
    pub async fn queue_windows_save(
        self: &Arc<Self>,
        id: Uuid,
        destination: PathBuf,
    ) -> Result<Transfer> {
        // Only pass a path returned by the native save dialog with overwrite confirmation.
        let path = destination.clone();
        let overwrite = tokio::task::spawn_blocking(move || overwrite_target(&path, 0)).await??;
        self.queue_download_to(id, destination, overwrite).await
    }

    async fn queue_download_to(
        self: &Arc<Self>,
        id: Uuid,
        destination: PathBuf,
        overwrite: Option<OverwriteTarget>,
    ) -> Result<Transfer> {
        let _prepare = self.prepare_gate.lock().await;
        self.ensure_open()?;
        if overwrite.is_none() {
            ensure_new_destination(&destination)?;
        }
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
        job.overwrite = overwrite;
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
        let mut job = self
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
        if direction == Direction::Upload {
            // Persist Queued before barrier/capacity checks so retries behind a running upload
            // or pending text are never dropped and will resume automatically once unblocked.
            job.transfer.status = TransferStatus::Queued;
            job.transfer.error = None;
            job.retry_at = 0;
            self.store_job(&job).await?;
            return self.launch(job, &mut workers).await;
        }
        ensure!(
            workers.len() < MAX_RUNNING,
            "Pause a running transfer or wait for it to finish"
        );
        self.launch(job, &mut workers).await
    }

    pub async fn discard(self: &Arc<Self>, id: Uuid, direction: Direction) -> Result<()> {
        let _prepare = self.prepare_gate.lock().await;
        self.ensure_open()?;
        let key = (id, direction);
        if direction == Direction::Upload {
            let active = {
                let mut workers = self.workers.lock().await;
                if let Some(active) = workers.remove(&key) {
                    active.stop.store(true, Ordering::Relaxed);
                    Some(active)
                } else {
                    None
                }
            };
            if let Some(active) = active {
                let _ = active.task.await;
            }
        }
        let workers = self.workers.lock().await;
        self.ensure_open()?;
        ensure!(
            !workers.contains_key(&key),
            "Pause the transfer before discarding it"
        );
        let job = self
            .job(key)
            .await?
            .context("This transfer is unavailable")?;
        let published_in_inbox: bool = if direction == Direction::Upload {
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM inbox WHERE attachment_id = ?)")
                .bind(id.to_string())
                .fetch_one(&self.db)
                .await?
        } else {
            false
        };
        ensure!(
            job.transfer.status != TransferStatus::Complete && !published_in_inbox,
            "This attachment was already sent."
        );
        if direction == Direction::Upload && job.transfer.status != TransferStatus::Expired {
            match self
                .api
                .send(self.api.request(Method::DELETE, &format!("/v1/files/{id}")))
                .await
            {
                Ok(_)
                | Err(OnlineError::Rejected {
                    status: StatusCode::NOT_FOUND | StatusCode::GONE,
                    ..
                }) => {}
                Err(OnlineError::Rejected {
                    status: StatusCode::CONFLICT,
                    ..
                }) => bail!("This attachment was already sent."),
                Err(error) => {
                    // If all chunks were uploaded, /complete may have raced or had an unknown ACK.
                    // Require server confirmation instead of falsely reporting cancellation.
                    if job.transfer.received >= job.transfer.attachment.size {
                        bail!(
                            "Could not verify whether this attachment was already sent. Reconnect and try again."
                        );
                    }
                    if !matches!(error, OnlineError::Network(_)) {
                        return Err(error.into());
                    }
                }
            }
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
        drop(workers);
        if direction == Direction::Download {
            self.release_task_media_ref("download", &id.to_string()).await?;
        }
        if direction == Direction::Upload {
            self.history_changed();
            if !self.closed.load(Ordering::Relaxed) {
                let _ = self.resume_queued().await;
            }
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

    async fn has_earlier_pending_in_conversation(
        &self,
        conversation: Uuid,
        upload_id: Uuid,
    ) -> Result<bool> {
        let chat = conversation.to_string();
        let pending_text: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM outbox
                WHERE conversation_id = ?
                  AND blocked = 0
                  AND submitted = 0
                  AND json_extract(work, '$.kind') = 'text'
            )",
        )
        .bind(&chat)
        .fetch_one(&self.db)
        .await?;
        if pending_text {
            return Ok(true);
        }
        let earlier_upload: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM transfers AS earlier
                JOIN transfers AS cur ON cur.id = ? AND cur.direction = 'upload'
                WHERE earlier.direction = 'upload'
                  AND earlier.rowid < cur.rowid
                  AND json_extract(earlier.payload, '$.conversation_id') = ?
                  AND json_extract(earlier.payload, '$.status') IN ('queued', 'running')
            )",
        )
        .bind(upload_id.to_string())
        .bind(&chat)
        .fetch_one(&self.db)
        .await?;
        Ok(earlier_upload)
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
        if key.1 == Direction::Upload {
            let conversation = job.transfer.conversation_id;
            if workers
                .iter()
                .any(|((_, dir), active)| *dir == Direction::Upload && active.conversation == conversation)
                || self
                    .has_earlier_pending_in_conversation(conversation, key.0)
                    .await?
            {
                return Ok(job.transfer);
            }
        }
        job.transfer.status = TransferStatus::Running;
        job.transfer.error = None;
        self.store_job(&job).await?;
        let transfer = job.transfer.clone();
        let conversation = job.transfer.conversation_id;
        let stop = Arc::new(AtomicBool::new(false));
        let signal = stop.clone();
        let client = self.clone();
        let task = tokio::spawn(async move {
            let result = match key.1 {
                Direction::Upload => client.upload(&mut job, &signal).await,
                Direction::Download => client.download(&mut job, &signal).await,
            };
            let mut definitive_failure = false;
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
                    definitive_failure = matches!(job.transfer.status, TransferStatus::Failed | TransferStatus::Expired)
                        && matches!(error.downcast_ref::<OnlineError>(), Some(OnlineError::Rejected { .. }));
                    job.retry_at = unix_millis() + 2_000;
                }
            }
            {
                let mut workers = client.workers.lock().await;
                let complete = job.transfer.status == TransferStatus::Complete;
                let deleted_download = key.1 == Direction::Download && sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS(SELECT 1 FROM cancelled_downloads WHERE id = ?)",
                ).bind(key.0.to_string()).fetch_one(&client.db).await.unwrap_or(false);
                let mut retired = false;
                if key.1 == Direction::Upload && client.is_suppressed_upload(key.0).await.unwrap_or(false) && !complete {
                    let confirmed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_deleted_messages WHERE attachment_id = ?)")
                        .bind(key.0.to_string()).fetch_one(&client.db).await.unwrap_or(false);
                    if definitive_failure || confirmed {
                        let finish = async {
                            let mut tx = client.db.begin_with("BEGIN IMMEDIATE").await?;
                            let confirmed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_deleted_messages WHERE attachment_id = ?)")
                                .bind(key.0.to_string()).fetch_one(&mut *tx).await?;
                            if !confirmed {
                                sqlx::query("INSERT OR IGNORE INTO send_failure_notices (kind, id) VALUES ('upload', ?)")
                                    .bind(key.0.to_string()).execute(&mut *tx).await?;
                            }
                            sqlx::query("DELETE FROM transfers WHERE id = ? AND direction = 'upload'")
                                .bind(key.0.to_string()).execute(&mut *tx).await?;
                            tx.commit().await?;
                            Ok::<_, anyhow::Error>(())
                        }.await;
                        retired = finish.is_ok();
                    }
                    if !retired {
                        // Unknown publication results and local errors need reconciliation with the stable upload ID.
                        job.transfer.status = TransferStatus::Queued;
                    }
                }
                if complete || deleted_download || retired
                    || (client.store_job(&job).await.is_ok()
                        && job.transfer.status == TransferStatus::Expired)
                {
                    let _ = fs::remove_file(client.spool(key)).await;
                }
                workers.remove(&key);
            }
            if !client.closed.load(Ordering::Relaxed) {
                tokio::spawn(async move {
                    let _ = client.resume_queued().await;
                });
            }
        });
        workers.insert(key, Active { conversation, stop, task });
        Ok(transfer)
    }

    pub fn resume_queued(
        self: &Arc<Self>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            let mut workers = self.workers.lock().await;
            let available = MAX_RUNNING.saturating_sub(workers.len());
            if available > 0 {
                let rows: Vec<String> = sqlx::query_scalar(
                    "SELECT payload FROM transfers WHERE json_extract(payload, '$.status') = 'queued'
                     AND coalesce(json_extract(payload, '$.retry_at'), 0) <= ? ORDER BY rowid LIMIT 16",
                )
                .bind(unix_millis())
                .fetch_all(&self.db)
                .await?;
                for row in rows {
                    if workers.len() >= MAX_RUNNING {
                        break;
                    }
                    let job: Job = serde_json::from_str(&row)?;
                    if !workers.contains_key(&job.key()) {
                        if job.transfer.direction == Direction::Upload && self.is_suppressed_upload(job.key().0).await? {
                            let confirmed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_deleted_messages WHERE attachment_id = ?)")
                                .bind(job.key().0.to_string()).fetch_one(&self.db).await?;
                            if confirmed {
                                sqlx::query("DELETE FROM transfers WHERE id = ? AND direction = 'upload'")
                                    .bind(job.key().0.to_string()).execute(&self.db).await?;
                                let _ = fs::remove_file(self.spool(job.key())).await;
                                continue;
                            }
                        }
                        self.launch(job, &mut workers).await?;
                    }
                }
            }
            Ok(())
        })
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

    pub async fn staged_image(&self, id: Uuid) -> Result<std::fs::File> {
        self.ensure_open()?;
        let key = (id, Direction::Upload);
        ensure!(!self.is_suppressed_upload(id).await?, "This upload was deleted locally");
        let job = self.job(key).await?.context("This upload is unavailable")?;
        let file = job.transfer.attachment;
        ensure!(file.size <= MAX_PASTED_IMAGE_BYTES as u64
            && file.preview_mime().is_some_and(|mime| mime.starts_with("image/")), "This upload is not an image");
        open_verified_input(&self.spool(key), file.size)
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
        if !progress.complete && progress.received < attachment.size {
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
        if self
            .has_earlier_pending_in_conversation(conversation, id)
            .await?
        {
            return Ok(TransferStatus::Queued);
        }
        let stamp: Publication = self
            .api
            .json(
                self.api
                    .request(Method::POST, &format!("/v1/files/{id}/complete")),
            )
            .await?;
        ensure!(stamp.seq > 0, OnlineError::InvalidResponse);
        if !stamp.recalled && !self.is_locally_deleted(stamp.seq).await? && !self.is_suppressed_upload(id).await? {
            let desc = MediaDescriptor {
                conversation_id: conversation,
                message_seq: stamp.seq,
                attachment_id: Some(attachment.id),
                variant: MediaVariant::for_attachment(&attachment),
                content_key: &attachment.sha256,
                expected_sha256: &attachment.sha256,
                expected_size: attachment.size,
                mime: attachment
                    .preview_mime()
                    .unwrap_or("application/octet-stream"),
                dimensions: None,
            };
            if let Err(error) = self.commit_media_file(&desc, &spool).await
                && !self.is_locally_deleted(stamp.seq).await?
                && !self.is_suppressed_upload(id).await?
            {
                return Err(error);
            }
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
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        persist_message(&mut tx, message).await?;
        let visible: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM inbox WHERE seq = ? AND attachment_id = ?)",
        )
        .bind(stamp.seq)
        .bind(id.to_string())
        .fetch_one(&mut *tx)
        .await?;
        let removed_paths = if visible {
            record_local_file(&mut tx, &job.transfer.attachment, stamp.seq, false).await?;
            Vec::new()
        } else {
            media_store::release_media_refs_in_tx(&mut tx, "message", &stamp.seq.to_string())
                .await?
        };
        job.transfer.status = TransferStatus::Complete;
        let deleted: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM local_deleted_messages WHERE seq = ?)",
        ).bind(stamp.seq).fetch_one(&mut *tx).await?;
        if deleted {
            sqlx::query("DELETE FROM transfers WHERE id = ? AND direction = 'upload'")
                .bind(id.to_string()).execute(&mut *tx).await?;
        } else {
            store_job(&mut tx, job).await?;
        }
        tx.commit().await?;
        if !removed_paths.is_empty() {
            self.remove_rel_paths(removed_paths).await;
        }
        self.history_changed();
        Ok(TransferStatus::Complete)
    }

    pub(super) async fn flush_content_acks(&self) -> Result<()> {
        let pending: Vec<(String, i64, String, i64)> = sqlx::query_as(
            "SELECT id, message_seq, sha256, size FROM local_files WHERE ack_pending = 1 LIMIT 8",
        )
        .fetch_all(&self.db)
        .await?;
        for (id, seq, sha256, _size) in pending {
            let inbox_row: Option<(String, String)> = sqlx::query_as(
                "SELECT conversation_id, payload FROM inbox
                 WHERE seq = ?1 AND attachment_id = ?2
                   AND json_extract(payload, '$.recalled') IS NOT 1
                   AND seq NOT IN (SELECT seq FROM hidden_messages)",
            )
            .bind(seq)
            .bind(&id)
            .fetch_optional(&self.db)
            .await?;
            let verified = if let Some((conv_str, payload)) = inbox_row
                && let (Ok(conv_id), Ok(att_id), Ok(msg)) = (
                    Uuid::parse_str(&conv_str),
                    Uuid::parse_str(&id),
                    serde_json::from_str::<Message>(&payload),
                )
                && let Some(att) = msg.attachment
            {
                let variant = MediaVariant::for_attachment(&att);
                self.verified_media_path(conv_id, seq, Some(att_id), variant, &sha256, true)
                    .await
                    .ok()
                    .flatten()
                    .is_some()
            } else {
                false
            };
            if !verified {
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

    async fn download(&self, job: &mut Job, stop: &Arc<AtomicBool>) -> Result<TransferStatus> {
        let attachment = job.transfer.attachment.clone();
        let id = attachment.id;
        let destination = job.destination.clone().context("Choose a save location")?;
        let (seq, conv_str): (i64, String) =
            sqlx::query_as("SELECT seq, conversation_id FROM inbox WHERE attachment_id = ?1
                UNION ALL SELECT seq, conversation_id FROM local_deleted_messages WHERE attachment_id = ?1")
                .bind(id.to_string())
                .fetch_optional(&self.db)
                .await?
                .context("This attachment is no longer available")?;
        let conversation_id = Uuid::parse_str(&conv_str)?;
        let is_hidden: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM hidden_messages WHERE seq = ?1)
                OR EXISTS(SELECT 1 FROM local_deleted_messages WHERE seq = ?1)")
                .bind(seq)
                .fetch_one(&self.db)
                .await?;
        let revoked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM recall_tombstones WHERE message_seq = ?1)
            OR EXISTS(SELECT 1 FROM cancelled_downloads WHERE id = ?2)")
            .bind(seq).bind(id.to_string()).fetch_one(&self.db).await?;
        ensure!(!revoked && conversation_id == job.transfer.conversation_id, "This attachment is no longer available");
        let variant = MediaVariant::for_attachment(&attachment);
        let spool = self.spool(job.key());
        let has_local_ack: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_files WHERE id = ?)")
                .bind(id.to_string())
                .fetch_one(&self.db)
                .await?;
        if !job.publishing && job.overwrite.is_none() {
            ensure_new_destination(&destination)?;
        }
        let existing_cached = if is_hidden {
            let row: Option<(i64, String)> = sqlx::query_as(
                "SELECT f.id, f.rel_path FROM media_refs r JOIN media_files f ON f.id = r.file_id
                 WHERE r.ref_kind = 'download' AND r.ref_id = ?1 AND r.conversation_id = ?2
                 AND r.attachment_id = ?1 AND r.variant = ?3 AND f.status = 'available'
                 AND f.sha256 = ?4 AND f.size = ?5 AND f.device_id = ?6
                 AND EXISTS (SELECT 1 FROM directory d, json_each(d.payload, '$.conversations') c
                    WHERE d.id = 1 AND json_extract(c.value, '$.id') = ?2
                    AND json_extract(c.value, '$.joined_after') < ?7)",
            ).bind(id.to_string()).bind(&conv_str).bind(variant.as_str())
                .bind(&attachment.sha256).bind(attachment.size as i64)
                .bind(self.device_id.to_string()).bind(seq).fetch_optional(&self.db).await?;
            if let Some((file_id, rel)) = row {
                let path = self.resolve_conversation_path(conversation_id, &rel)?;
                let check = path.clone();
                let file = attachment.clone();
                if tokio::task::spawn_blocking(move || verify_file(&check, &file)).await?.is_ok() {
                    Some((file_id, path, attachment.size))
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            self.verified_media_path(
                conversation_id,
                seq,
                Some(id),
                variant,
                &attachment.sha256,
                true,
            )
            .await?
        };
        let (cached_file_id, cached, need_ack) = if let Some((file_id, path, _)) = existing_cached {
            let need_ack = if has_local_ack || is_hidden {
                false
            } else {
                // Best-effort download registration for content ACK if cached earlier via preview;
                // offline or expired server copies still export cleanly from verified local media.
                self.api
                    .send(
                        self.api
                            .request(Method::POST, &format!("/v2/files/{id}/download")),
                    )
                    .await
                    .is_ok()
            };
            (Some(file_id), path, need_ack)
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
                .map_err(save_io_error)?;
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
                    .map_err(save_io_error)?;
                output.flush().await.map_err(save_io_error)?;
                job.transfer.received += expected as u64;
                self.store_job(job).await?;
            }
            output.sync_all().await.map_err(save_io_error)?;
            drop(output);
            if is_hidden || self.is_locally_deleted(seq).await? {
                let check_spool = spool.clone();
                let check_att = attachment.clone();
                if let Err(error) =
                    tokio::task::spawn_blocking(move || verify_file(&check_spool, &check_att))
                        .await?
                {
                    let _ = fs::remove_file(&spool).await;
                    job.transfer.received = 0;
                    return Err(error);
                }
                (None, spool.clone(), false)
            } else {
                let desc = MediaDescriptor {
                    conversation_id,
                    message_seq: seq,
                    attachment_id: Some(attachment.id),
                    variant,
                    content_key: &attachment.sha256,
                    expected_sha256: &attachment.sha256,
                    expected_size: attachment.size,
                    mime: attachment
                        .preview_mime()
                        .unwrap_or("application/octet-stream"),
                    dimensions: None,
                };
                match self.commit_media_file(&desc, &spool).await {
                    Ok(path) => {
                        let file_id = self
                            .verified_media_path(
                                conversation_id,
                                seq,
                                Some(id),
                                variant,
                                &attachment.sha256,
                                false,
                            )
                            .await?
                            .context("The downloaded file is unavailable")?
                            .0;
                        (Some(file_id), path, !has_local_ack)
                    }
                    Err(error) => {
                        if self.is_locally_deleted(seq).await? {
                            // A single-message deletion may race the cache commit; retry the independent export.
                            return Ok(TransferStatus::Queued);
                        }
                        let _ = fs::remove_file(&spool).await;
                        job.transfer.received = 0;
                        return Err(error);
                    }
                }
            }
        };
        // Pin an active download ref so hiding the message cannot delete the backing file mid-export.
        let download_ref_id = id.to_string();
        if let Some(file_id) = cached_file_id {
            self.pin_task_media_ref(
                file_id,
                "download",
                &download_ref_id,
                conversation_id,
                Some(id),
                variant,
            )
            .await?;
        }
        let result = async {
            if stop.load(Ordering::Relaxed) {
                return Ok(TransferStatus::Paused);
            }
            let revoked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM recall_tombstones WHERE message_seq = ?1)
                OR EXISTS(SELECT 1 FROM cancelled_downloads WHERE id = ?2)")
                .bind(seq).bind(id.to_string()).fetch_one(&self.db).await?;
            ensure!(!revoked, "This attachment is no longer available");
            let recovering = job.publishing;
            job.publishing = true;
            self.store_job(job).await?;
            let file = attachment.clone();
            let overwrite = job.overwrite.clone();
            let stop = stop.clone();
            let saved = tokio::task::spawn_blocking(move || {
                publish_download(
                    &cached,
                    &destination,
                    &file,
                    overwrite.as_ref(),
                    recovering,
                    &stop,
                )
            })
            .await?;
            let published = match saved {
                Ok(published) => published,
                Err(error) => {
                    job.publishing = false;
                    return Err(error);
                }
            };
            if !published {
                job.publishing = false;
                return Ok(TransferStatus::Paused);
            }
            let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
            let visible: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM inbox WHERE seq = ? AND attachment_id = ?)",
            )
            .bind(seq)
            .bind(id.to_string())
            .fetch_one(&mut *tx)
            .await?;
            let deleted: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_deleted_messages WHERE seq = ?)")
                .bind(seq).fetch_one(&mut *tx).await?;
            if !visible && !deleted {
                let removed =
                    media_store::release_media_refs_in_tx(&mut tx, "message", &seq.to_string())
                        .await?;
                tx.commit().await?;
                self.remove_rel_paths(removed).await;
                bail!("This attachment was recalled");
            }
            if deleted {
                sqlx::query("DELETE FROM transfers WHERE id = ? AND direction = 'download'")
                    .bind(id.to_string()).execute(&mut *tx).await?;
                tx.commit().await?;
                self.history_changed();
                return Ok(TransferStatus::Complete);
            }
            if !has_local_ack && !is_hidden {
                record_local_file(&mut tx, &attachment, seq, need_ack).await?;
            }
            job.publishing = false;
            job.overwrite = None;
            job.transfer.received = attachment.size;
            job.transfer.status = TransferStatus::Complete;
            store_job(&mut tx, job).await?;
            tx.commit().await?;
            self.history_changed();
            Ok(TransferStatus::Complete)
        }
        .await;
        if cached_file_id.is_some()
            && (matches!(&result, Ok(TransferStatus::Complete))
                || !sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM local_deleted_messages WHERE seq = ?)")
                    .bind(seq).fetch_one(&self.db).await?)
        {
            self.release_task_media_ref("download", &download_ref_id).await?;
        }
        result
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
        let row: Option<(i64, String, String)> = sqlx::query_as(
            "SELECT i.seq, i.conversation_id, i.payload FROM inbox i
             WHERE i.attachment_id = ?1
               AND json_extract(i.payload, '$.recalled') IS NOT 1
               AND NOT EXISTS (SELECT 1 FROM hidden_messages h WHERE h.seq = i.seq)",
        )
        .bind(id.to_string())
        .fetch_optional(&self.db)
        .await?;
        if let Some((seq, conv_str, payload)) = row
            && let (Ok(conv_id), Ok(message)) = (
                Uuid::parse_str(&conv_str),
                serde_json::from_str::<Message>(&payload),
            )
            && let Some(attachment) = message.attachment
        {
            let variant = MediaVariant::for_attachment(&attachment);
            if let Some((file_id, path, size)) = self
                .verified_media_path(conv_id, seq, Some(id), variant, &attachment.sha256, false)
                .await?
            {
                ensure!(
                    offset <= size && expected as u64 <= size - offset,
                    OnlineError::InvalidResponse
                );
                let read = async {
                    let mut file = fs::File::open(&path).await?;
                    ensure!(
                        file.metadata().await?.len() == size,
                        "The local file is incomplete"
                    );
                    file.seek(SeekFrom::Start(offset)).await?;
                    let mut bytes = vec![0; expected];
                    file.read_exact(&mut bytes).await?;
                    Ok::<_, anyhow::Error>(bytes)
                }
                .await;
                match read {
                    Ok(bytes) => return Ok(bytes),
                    Err(_) => {
                        self.mark_media_status(file_id, "corrupted").await;
                    }
                }
            }
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

async fn release_detached_upload_media(db: &mut SqliteConnection, chat: &str, id: Option<Uuid>) -> Result<Vec<String>> {
    let refs: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT r.ref_id FROM media_refs r JOIN local_suppressed_sends s
            ON s.kind = 'upload' AND s.id = r.attachment_id AND s.conversation_id = r.conversation_id
            WHERE r.ref_kind = 'message' AND r.conversation_id = ?1 AND (?2 IS NULL OR s.id = ?2)",
    ).bind(chat).bind(id.map(|id| id.to_string())).fetch_all(&mut *db).await?;
    let mut paths = Vec::new();
    for reference in refs {
        paths.extend(media_store::release_media_refs_in_tx(&mut *db, "message", &reference).await?);
    }
    Ok(paths)
}

async fn store_job(db: &mut SqliteConnection, job: &Job) -> Result<()> {
    let (id, direction) = job.key();
    sqlx::query(
        "INSERT INTO transfers (id, direction, payload)
             SELECT ?1, ?2, ?3 WHERE ?2 <> 'download' OR NOT EXISTS (
                 SELECT 1 FROM cancelled_downloads WHERE id = ?1)
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

fn stage_publication(source: &Path, destination: &Path, attachment: &Attachment, prefix: &str) -> Result<tempfile::TempPath> {
    let parent = destination.parent().context("Choose a save folder")?;
    let input = open_verified_input(source, attachment.size)?;
    let mut temp = tempfile::Builder::new().prefix(prefix).tempfile_in(parent).map_err(save_io_error)?;
    ensure!(
        staging::hash_copy(input, temp.as_file_mut()).map_err(|error| {
            error.downcast::<std::io::Error>().map_or_else(|error| error, save_io_error)
        })? == attachment.sha256,
        "File verification failed. Retry the download."
    );
    temp.as_file().sync_all().map_err(save_io_error)?;
    Ok(temp.into_temp_path())
}

fn publish_download(
    source: &Path,
    destination: &Path,
    attachment: &Attachment,
    overwrite: Option<&OverwriteTarget>,
    recovering: bool,
    stop: &AtomicBool,
) -> Result<bool> {
    if recovering && verify_file(destination, attachment).is_ok() {
        return Ok(true);
    }
    let temp = stage_publication(source, destination, attachment, ".tmp")?;
    if stop.load(Ordering::Relaxed) {
        return Ok(false);
    }
    if let Some(expected) = overwrite {
        #[cfg(windows)]
        {
            use windows_sys::Win32::Storage::FileSystem::DELETE;

            // MoveFileEx may report a sharing conflict as access denied. Check delete access first.
            ensure!(overwrite_target(destination, DELETE)?.as_ref() == Some(expected),
                "The destination changed. Choose the save location and confirm replacement again.");
            // The temp file is already verified, closed and on the destination volume.
            temp.persist(destination).map_err(|error| save_io_error(error.error))?;
        }
        #[cfg(not(windows))]
        {
            let _ = expected;
            bail!("Choose a new save location on this device");
        }
    } else {
        temp.persist_noclobber(destination).map_err(|error| save_io_error(error.error))?;
    }
    Ok(true)
}

#[cfg(windows)]
fn overwrite_target(path: &Path, access: u32) -> Result<Option<OverwriteTarget>> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_FLAG_OPEN_REPARSE_POINT, GetFileInformationByHandle,
    };

    ensure!(path.is_absolute() && path.file_name().is_some(), "Choose an absolute save path");
    let file = match std::fs::OpenOptions::new()
        .access_mode(access)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(save_io_error(error)),
    };
    let metadata = file.metadata().map_err(save_io_error)?;
    ensure!(metadata.is_file() && !staging::is_link(&metadata), "Choose a regular file to replace");
    ensure!(!metadata.permissions().readonly(), "The save destination is read-only. Choose another location or change its permissions.");
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(save_io_error(std::io::Error::last_os_error()));
    }
    Ok(Some(OverwriteTarget {
        volume: info.dwVolumeSerialNumber,
        index: u64::from(info.nFileIndexHigh) << 32 | u64::from(info.nFileIndexLow),
        size: metadata.len(),
        last_write: u64::from(info.ftLastWriteTime.dwHighDateTime) << 32
            | u64::from(info.ftLastWriteTime.dwLowDateTime),
    }))
}

pub(super) fn save_io_error(error: std::io::Error) -> anyhow::Error {
    use std::io::ErrorKind;
    let message = if cfg!(windows) && matches!(error.raw_os_error(), Some(32 | 33)) {
        "The save destination is in use. Close the file and retry."
    } else {
        match error.kind() {
            ErrorKind::AlreadyExists => "The save destination already exists. Choose the location again to confirm replacement.",
            ErrorKind::PermissionDenied => "Permission denied at the save destination. Check its permissions or choose another location.",
            ErrorKind::StorageFull => "There is not enough disk space to save the file. Free up space and retry.",
            _ => "Could not save the attachment",
        }
    };
    anyhow::Error::new(error).context(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AttachmentKind, DeviceSession, FileFilter};
    use super::super::Api;

    #[cfg(windows)]
    #[test]
    fn windows_replaces_only_the_confirmed_file_with_verified_bytes() -> Result<()> {
        use sha2::{Digest, Sha256};
        use std::{fs, os::windows::fs::OpenOptionsExt};
        use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

        let root = tempfile::tempdir()?;
        let source = root.path().join("cache");
        let destination = root.path().join("saved.txt");
        let old = b"edited copy";
        let new = b"correct copy";
        let file = Attachment {
            id: Uuid::new_v4(),
            kind: AttachmentKind::File,
            name: "saved.txt".into(),
            size: new.len() as u64,
            sha256: format!("{:x}", Sha256::digest(new)),
        };
        fs::write(&destination, old)?;
        let target = overwrite_target(&destination, 0)?;
        let stop = AtomicBool::new(false);
        let publish = |target, recovering| {
            publish_download(&source, &destination, &file, target, recovering, &stop)
        };
        fs::write(&source, b"corrupt copy")?;
        assert!(publish(target.as_ref(), false).unwrap_err().to_string().contains("verification failed"));
        assert_eq!(fs::read(&destination)?, old);

        fs::write(&source, new)?;
        assert!(publish(None, false).is_err());
        stop.store(true, Ordering::Relaxed);
        assert!(!publish(target.as_ref(), false)?);
        stop.store(false, Ordering::Relaxed);
        let locked = fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(&destination)?;
        assert!(publish(target.as_ref(), false).unwrap_err().to_string().contains("in use"));
        assert_eq!(fs::read(&destination)?, old);
        drop(locked);

        fs::write(&destination, b"another edit after choosing the destination")?;
        assert!(publish(target.as_ref(), false).unwrap_err().to_string().contains("destination changed"));
        assert_eq!(fs::read(&destination)?, b"another edit after choosing the destination");
        let reconfirmed = overwrite_target(&destination, 0)?;
        assert!(publish(reconfirmed.as_ref(), true)?);
        assert!(publish(reconfirmed.as_ref(), true)?);
        verify_file(&destination, &file)?;
        assert_eq!(fs::read_dir(root.path())?.count(), 2);
        Ok(())
    }

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

    #[tokio::test]
    async fn verified_upload_blocks_changed_source_and_deduplicates_by_upload_id() -> Result<()> {
        let root = tempfile::tempdir()?;
        let files = tempfile::tempdir()?;
        let source = files.path().join("doc.txt");
        std::fs::write(&source, b"version-1")?;
        let snapshot = staging::capture_source_snapshot(&source)?;

        let session = DeviceSession {
            user: User { id: Uuid::nil(), handle: "draft_tester".into() },
            device_id: Uuid::nil(),
            cursor: Uuid::nil(),
        };
        let client = Client::open(root.path(), Api::new("http://127.0.0.1:7841").unwrap(), session, false).await?;
        let conversation = Uuid::new_v4();
        let directory = crate::Directory {
            contacts: Vec::new(),
            conversations: vec![crate::Conversation {
                id: conversation,
                title: None,
                members: vec![client.user.clone()],
                activity_at: 1,
                preview: None,
                preview_seq: None,
                latest_seq: 0,
                unread: 0,
                read_through: 0,
                joined_after: None,
            }],
            pending_contacts: Vec::new(),
            remarks: HashMap::new(),
            names: HashMap::new(),
            avatars: HashMap::new(),
            read_receipts: false,
        };
        sqlx::query("INSERT INTO directory (id, payload) VALUES (1, ?) ON CONFLICT(id) DO UPDATE SET payload = excluded.payload")
            .bind(serde_json::to_string(&directory)?)
            .execute(&client.db)
            .await?;

        // Modify source before enqueue -> blocked with SourceChangedError and nothing queued.
        std::fs::write(&source, b"version-2")?;
        let blocked = client
            .queue_upload_verified(conversation, Uuid::new_v4(), source.clone(), snapshot)
            .await
            .unwrap_err();
        assert!(blocked.downcast_ref::<staging::SourceChangedError>().is_some());
        assert!(client.transfers().await?.is_empty());

        // Re-capture snapshot after modification -> queues once; second call with same upload_id is idempotent.
        let fresh = staging::capture_source_snapshot(&source)?;
        let upload_id = Uuid::new_v4();
        let first = client
            .queue_upload_verified(conversation, upload_id, source.clone(), fresh.clone())
            .await?;
        let second = client
            .queue_upload_verified(conversation, upload_id, source, fresh)
            .await?;
        assert_eq!(first.attachment.id, upload_id);
        assert_eq!(second.attachment.id, upload_id);
        assert_eq!(client.transfers().await?.len(), 1);
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn mixed_text_and_varying_size_uploads_publish_in_queue_order_across_recovery_and_failure() -> Result<()> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        use std::sync::Mutex;

        #[derive(Default)]
        struct MockState {
            seq: i64,
            published: Vec<String>,
            text_attempts: usize,
            uploads: HashMap<String, (String, u64, u64)>,
            fail_name: Option<String>,
        }

        let state = Arc::new(Mutex::new(MockState {
            fail_name: Some("bad.txt".into()),
            ..Default::default()
        }));
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let server_state = state.clone();
        let server = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else { break };
                let state = server_state.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 4096];
                    let header_end;
                    loop {
                        let Ok(n) = stream.read(&mut tmp).await else { return };
                        if n == 0 { return; }
                        buf.extend_from_slice(&tmp[..n]);
                        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            header_end = pos + 4;
                            break;
                        }
                    }
                    let headers = String::from_utf8_lossy(&buf[..header_end]).to_string();
                    let content_len: usize = headers
                        .lines()
                        .find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            k.eq_ignore_ascii_case("content-length").then(|| v.trim().parse().ok()).flatten()
                        })
                        .unwrap_or(0);
                    while buf.len() < header_end + content_len {
                        let Ok(n) = stream.read(&mut tmp).await else { return };
                        if n == 0 { break; }
                        buf.extend_from_slice(&tmp[..n]);
                    }
                    let body = &buf[header_end..header_end + content_len];
                    let req_line = headers.lines().next().unwrap_or("");
                    let mut parts = req_line.split_whitespace();
                    let method = parts.next().unwrap_or("");
                    let path = parts.next().unwrap_or("");

                    let (status, resp_body) = if method == "POST" && path.ends_with("/messages") {
                        let mut s = state.lock().unwrap();
                        s.text_attempts += 1;
                        if s.text_attempts == 1 {
                            (503, r#"{"error":"temporary outage"}"#.to_string())
                        } else {
                            let val: serde_json::Value = serde_json::from_slice(body).unwrap();
                            s.seq += 1;
                            let seq = s.seq;
                            s.published.push(format!("text:{}", val["text"].as_str().unwrap()));
                            (200, serde_json::json!({ "seq": seq, "created_at": 1000 + seq, "recalled": false }).to_string())
                        }
                    } else if method == "POST" && path.ends_with("/uploads") {
                        let att: Attachment = serde_json::from_slice(body).unwrap();
                        let mut s = state.lock().unwrap();
                        if s.fail_name.as_deref() == Some(att.name.as_str()) {
                            (400, r#"{"error":"rejected attachment"}"#.to_string())
                        } else {
                            let entry = s.uploads.entry(att.id.to_string()).or_insert((att.name.clone(), att.size, 0));
                            (200, serde_json::json!({ "received": entry.2, "complete": false }).to_string())
                        }
                    } else if method == "PUT" && path.contains("/chunks/") {
                        let segs: Vec<&str> = path.split('/').collect();
                        let id = segs[3].to_string();
                        let offset: u64 = segs[5].parse().unwrap();
                        let slow = {
                            let s = state.lock().unwrap();
                            s.uploads.get(&id).is_some_and(|(name, _, _)| name == "large.bin") && offset == 0
                        };
                        if slow {
                            tokio::time::sleep(std::time::Duration::from_millis(80)).await;
                        }
                        let mut s = state.lock().unwrap();
                        let entry = s.uploads.get_mut(&id).unwrap();
                        entry.2 = offset + body.len() as u64;
                        (200, serde_json::json!({ "received": entry.2, "complete": false }).to_string())
                    } else if method == "POST" && path.ends_with("/complete") {
                        let segs: Vec<&str> = path.split('/').collect();
                        let id = segs[3].to_string();
                        let mut s = state.lock().unwrap();
                        let name = s.uploads.get(&id).unwrap().0.clone();
                        s.seq += 1;
                        let seq = s.seq;
                        s.published.push(format!("file:{name}"));
                        (200, serde_json::json!({ "seq": seq, "created_at": 1000 + seq, "recalled": false }).to_string())
                    } else {
                        (404, "{}".to_string())
                    };
                    let resp = format!(
                        "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{resp_body}",
                        resp_body.len()
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                });
            }
        });

        let root = tempfile::tempdir()?;
        let files = tempfile::tempdir()?;
        let large = files.path().join("large.bin");
        let bad = files.path().join("bad.txt");
        let small = files.path().join("small.txt");
        std::fs::write(&large, vec![b'A'; 600 * 1024])?; // 2 chunks, slower
        std::fs::write(&bad, b"will-fail")?;
        std::fs::write(&small, b"tiny")?; // 1 chunk, fast

        let session = DeviceSession {
            user: User { id: Uuid::nil(), handle: "order_tester".into() },
            device_id: Uuid::nil(),
            cursor: Uuid::nil(),
        };
        let client = Client::open(root.path(), Api::new(&format!("http://{addr}")).unwrap(), session, false).await?;
        let conversation = Uuid::new_v4();
        let directory = crate::Directory {
            contacts: Vec::new(),
            conversations: vec![crate::Conversation {
                id: conversation,
                title: None,
                members: vec![client.user.clone()],
                activity_at: 1,
                preview: None,
                preview_seq: None,
                latest_seq: 0,
                unread: 0,
                read_through: 0,
                joined_after: None,
            }],
            pending_contacts: Vec::new(),
            remarks: HashMap::new(),
            names: HashMap::new(),
            avatars: HashMap::new(),
            read_receipts: false,
        };
        sqlx::query("INSERT INTO directory (id, payload) VALUES (1, ?) ON CONFLICT(id) DO UPDATE SET payload = excluded.payload")
            .bind(serde_json::to_string(&directory)?)
            .execute(&client.db)
            .await?;

        // Queue text, then large.bin, then bad.txt (400 rejected), then small.txt
        client
            .queue_message(
                conversation,
                crate::SendMessage {
                    text: "hello first".into(),
                    nonce: Uuid::new_v4(),
                    reply_to: None,
                },
            )
            .await?;
        let large_snap = staging::capture_source_snapshot(&large)?;
        let bad_snap = staging::capture_source_snapshot(&bad)?;
        let small_snap = staging::capture_source_snapshot(&small)?;
        client.queue_upload_verified(conversation, Uuid::new_v4(), large, large_snap).await?;
        client.queue_upload_verified(conversation, Uuid::new_v4(), bad, bad_snap).await?;
        client.queue_upload_verified(conversation, Uuid::new_v4(), small, small_snap).await?;

        // First flush hits 503 on text; force next_attempt=0 to simulate reconnect recovery.
        let _ = client.flush_pending_messages().await?;
        sqlx::query("UPDATE outbox SET next_attempt = 0").execute(&client.db).await?;
        let _ = client.flush_pending_messages().await?;
        client.resume_queued().await?;

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let published_len = state.lock().unwrap().published.len();
            if published_len >= 3 {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline, "timed out waiting for ordered publications");
            let _ = client.resume_queued().await;
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        let published = state.lock().unwrap().published.clone();
        assert_eq!(published, vec!["text:hello first", "file:large.bin", "file:small.txt"]);
        let transfers = client.transfers().await?;
        let bad_job = transfers.iter().find(|t| t.attachment.name == "bad.txt").unwrap();
        assert_eq!(bad_job.status, TransferStatus::Failed);

        client.close().await;
        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn upload_retry_persists_queued_behind_running_upload_and_pending_text() -> Result<()> {
        let root = tempfile::tempdir()?;
        let session = DeviceSession {
            user: User { id: Uuid::nil(), handle: "retry_tester".into() },
            device_id: Uuid::nil(),
            cursor: Uuid::nil(),
        };
        let client = Client::open(root.path(), Api::new("http://127.0.0.1:1").unwrap(), session, false).await?;
        let conversation = Uuid::new_v4();
        let make_att = |name: &str| Attachment {
            id: Uuid::new_v4(),
            kind: AttachmentKind::File,
            name: name.into(),
            size: 4,
            sha256: "0".repeat(64),
        };

        // Case 1: Retry paused upload A while upload B is actively running in the same conversation.
        let att_a = make_att("a.txt");
        let att_b = make_att("b.txt");
        let mut job_a = Job::new(att_a.clone(), conversation, Direction::Upload);
        job_a.transfer.status = TransferStatus::Paused;
        job_a.transfer.error = Some("old pause".into());
        job_a.retry_at = unix_millis() + 60_000;
        client.store_job(&job_a).await?;
        {
            let mut workers = client.workers.lock().await;
            workers.insert(
                (att_b.id, Direction::Upload),
                Active {
                    conversation,
                    stop: Arc::new(AtomicBool::new(false)),
                    task: tokio::spawn(async {}),
                },
            );
        }
        let resumed_a = client.resume(att_a.id, Direction::Upload).await?;
        assert_eq!(resumed_a.status, TransferStatus::Queued);
        assert_eq!(resumed_a.error, None);
        let stored_a = client.job((att_a.id, Direction::Upload)).await?.unwrap();
        assert_eq!(stored_a.transfer.status, TransferStatus::Queued);
        assert_eq!(stored_a.retry_at, 0);
        client.workers.lock().await.clear();

        // Case 2: Retry failed upload C while pending text exists in outbox for the same conversation.
        client
            .queue_message(
                conversation,
                crate::SendMessage {
                    text: "pending text".into(),
                    nonce: Uuid::new_v4(),
                    reply_to: None,
                },
            )
            .await?;
        let att_c = make_att("c.txt");
        let mut job_c = Job::new(att_c.clone(), conversation, Direction::Upload);
        job_c.transfer.status = TransferStatus::Failed;
        job_c.transfer.error = Some("network failed".into());
        client.store_job(&job_c).await?;
        let resumed_c = client.resume(att_c.id, Direction::Upload).await?;
        assert_eq!(resumed_c.status, TransferStatus::Queued);
        assert_eq!(resumed_c.error, None);
        let stored_c = client.job((att_c.id, Direction::Upload)).await?.unwrap();
        assert_eq!(stored_c.transfer.status, TransferStatus::Queued);

        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn upload_discard_handles_in_flight_published_race_and_unknown_ack() -> Result<()> {
        let root = tempfile::tempdir()?;
        let src_dir = tempfile::tempdir()?;
        let original = src_dir.path().join("keep_me.txt");
        std::fs::write(&original, b"user-original-file")?;

        let session = DeviceSession {
            user: User { id: Uuid::nil(), handle: "discard_tester".into() },
            device_id: Uuid::nil(),
            cursor: Uuid::nil(),
        };
        let client = Client::open(root.path(), Api::new("http://127.0.0.1:1").unwrap(), session, false).await?;
        let conversation = Uuid::new_v4();

        // Case 1: Discard upload with partial received < size while offline succeeds and keeps original file.
        let att_partial = Attachment {
            id: Uuid::new_v4(),
            kind: AttachmentKind::File,
            name: "keep_me.txt".into(),
            size: 100,
            sha256: "0".repeat(64),
        };
        let mut job_partial = Job::new(att_partial.clone(), conversation, Direction::Upload);
        job_partial.transfer.status = TransferStatus::Queued;
        job_partial.transfer.received = 0;
        std::fs::write(client.spool(job_partial.key()), b"staged-copy")?;
        client.store_job(&job_partial).await?;
        client.discard(att_partial.id, Direction::Upload).await?;
        assert!(client.job(job_partial.key()).await?.is_none());
        assert!(!client.spool(job_partial.key()).exists());
        assert!(original.exists(), "discard must never delete the user's original file");

        // Case 2: Discard upload when all chunks were uploaded (received == size) and server unreachable
        // fails closed so we never falsely claim cancellation on an unknown /complete ACK.
        let att_unknown = Attachment {
            id: Uuid::new_v4(),
            kind: AttachmentKind::File,
            name: "unknown_ack.txt".into(),
            size: 10,
            sha256: "0".repeat(64),
        };
        let mut job_unknown = Job::new(att_unknown.clone(), conversation, Direction::Upload);
        job_unknown.transfer.status = TransferStatus::Failed;
        job_unknown.transfer.received = 10;
        client.store_job(&job_unknown).await?;
        let err = client.discard(att_unknown.id, Direction::Upload).await.unwrap_err();
        assert!(err.to_string().contains("Could not verify whether this attachment was already sent"));
        assert!(client.job(job_unknown.key()).await?.is_some());

        // Case 3: Discard upload already recorded in inbox (published race) is rejected.
        let att_sent = Attachment {
            id: Uuid::new_v4(),
            kind: AttachmentKind::File,
            name: "sent.txt".into(),
            size: 5,
            sha256: "0".repeat(64),
        };
        let mut job_sent = Job::new(att_sent.clone(), conversation, Direction::Upload);
        job_sent.transfer.status = TransferStatus::Queued;
        client.store_job(&job_sent).await?;
        let mut tx = client.db.begin().await?;
        persist_message(
            &mut tx,
            Message {
                seq: 99,
                conversation_id: conversation,
                sender: client.user.clone(),
                created_at: 1000,
                text: None,
                attachment: Some(att_sent.clone()),
                ai_provider: None,
                recalled: false,
                reply_to: None,
                nonce: None,
                expires_at: None,
                expired: None,
                pending: None,
                local_file: false,
            },
        )
        .await?;
        tx.commit().await?;
        let err = client.discard(att_sent.id, Direction::Upload).await.unwrap_err();
        assert!(err.to_string().contains("already sent"));

        client.close().await;
        Ok(())
    }
}
