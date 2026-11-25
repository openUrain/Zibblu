use super::{Client, OnlineError, SyncUpdate, media_store, unix_millis};
use crate::{
    Attachment, DeviceSession, Directory, Message, MessageRecall, SYNC_PAGE_SIZE, SyncAck,
    SyncEvent, SyncPage, SyncPosition,
};
use anyhow::{Result, ensure};
use reqwest::{Method, StatusCode};
use sqlx::{SqliteConnection, SqlitePool};
use std::sync::{Arc, atomic::Ordering};

impl Client {
    pub(super) async fn initialize_history(
        db: &SqlitePool,
        session: &DeviceSession,
        restore: bool,
    ) -> Result<(bool, Option<String>)> {
        let mut tx = db.begin().await?;
        let statements =
            "CREATE TABLE IF NOT EXISTS inbox (
                seq INTEGER PRIMARY KEY, conversation_id TEXT NOT NULL, payload TEXT NOT NULL,
                attachment_id TEXT UNIQUE
             );
             CREATE INDEX IF NOT EXISTS inbox_chat ON inbox (conversation_id, seq);
             CREATE INDEX IF NOT EXISTS inbox_chat_time ON inbox (conversation_id, json_extract(payload, '$.created_at'), seq);
             CREATE INDEX IF NOT EXISTS inbox_nonce ON inbox (json_extract(payload, '$.nonce'))
                WHERE json_extract(payload, '$.nonce') IS NOT NULL;
             CREATE INDEX IF NOT EXISTS inbox_ai_replies ON inbox (seq)
                WHERE json_type(payload, '$.ai_provider') = 'text';
             CREATE TABLE IF NOT EXISTS hidden_messages (
                seq INTEGER PRIMARY KEY REFERENCES inbox (seq), attachment_id TEXT UNIQUE
             );
             CREATE TABLE IF NOT EXISTS local_deleted_messages (
                seq INTEGER PRIMARY KEY, conversation_id TEXT NOT NULL, attachment_id TEXT UNIQUE
             );
             CREATE TABLE IF NOT EXISTS local_deleted_chats (conversation_id TEXT PRIMARY KEY);
             CREATE TABLE IF NOT EXISTS local_suppressed_sends (
                kind TEXT NOT NULL, id TEXT NOT NULL, conversation_id TEXT NOT NULL,
                PRIMARY KEY (kind, id)
             );
             CREATE TABLE IF NOT EXISTS cancelled_downloads (id TEXT PRIMARY KEY);
             CREATE TABLE IF NOT EXISTS discarded_uploads (id TEXT PRIMARY KEY);
             CREATE TABLE IF NOT EXISTS send_failure_notices (
                kind TEXT NOT NULL, id TEXT NOT NULL, PRIMARY KEY (kind, id)
             );
             CREATE TABLE IF NOT EXISTS media_reclaims (rel_path TEXT PRIMARY KEY);
             CREATE TABLE IF NOT EXISTS directory (
                id INTEGER PRIMARY KEY CHECK (id = 1), payload TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS cached_profiles (
                user_id TEXT PRIMARY KEY, payload TEXT NOT NULL, expires_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS cached_profile_images (
                user_id TEXT NOT NULL, version TEXT NOT NULL, data BLOB NOT NULL,
                expires_at INTEGER NOT NULL, PRIMARY KEY (user_id, version)
             );
             CREATE TABLE IF NOT EXISTS sync_checkpoint (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                device_id TEXT NOT NULL, cursor TEXT NOT NULL, pending_ack TEXT, reconcile_after INTEGER
             );
             CREATE TABLE IF NOT EXISTS recall_tombstones (
                message_seq INTEGER PRIMARY KEY, event_seq INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS local_read_positions (
                conversation_id TEXT PRIMARY KEY, read_through INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS outbox (
                id INTEGER PRIMARY KEY AUTOINCREMENT, nonce TEXT NOT NULL UNIQUE, conversation_id TEXT,
                work TEXT NOT NULL, draft TEXT, blocked INTEGER NOT NULL DEFAULT 0,
                submitted INTEGER NOT NULL DEFAULT 0, next_attempt INTEGER NOT NULL DEFAULT 0,
                attempts INTEGER NOT NULL DEFAULT 0
             );
             CREATE INDEX IF NOT EXISTS outbox_pending ON outbox (id) WHERE blocked = 0 AND submitted = 0;
             CREATE TABLE IF NOT EXISTS confirmed_sends (
                nonce TEXT PRIMARY KEY, request_hash TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS transfers (
                id TEXT NOT NULL, direction TEXT NOT NULL, payload TEXT NOT NULL,
                PRIMARY KEY (id, direction)
             );
             CREATE TABLE IF NOT EXISTS local_files (
                id TEXT PRIMARY KEY, message_seq INTEGER NOT NULL UNIQUE,
                sha256 TEXT NOT NULL, size INTEGER NOT NULL, ack_pending INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS media_previews (
                message_seq INTEGER NOT NULL, sha256 TEXT NOT NULL, variant INTEGER NOT NULL,
                mime TEXT NOT NULL, bytes BLOB NOT NULL, touched INTEGER NOT NULL,
                PRIMARY KEY (message_seq, sha256, variant)
             );
             CREATE TABLE IF NOT EXISTS media_files (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                device_id TEXT NOT NULL,
                conversation_id TEXT NOT NULL,
                variant TEXT NOT NULL,
                content_key TEXT NOT NULL,
                sha256 TEXT NOT NULL,
                size INTEGER NOT NULL,
                mime TEXT NOT NULL,
                rel_path TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'available',
                width INTEGER,
                height INTEGER,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                UNIQUE (device_id, conversation_id, variant, content_key)
             );
             CREATE INDEX IF NOT EXISTS media_files_conv_status
                ON media_files (device_id, conversation_id, variant, status);
             CREATE TABLE IF NOT EXISTS media_refs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                file_id INTEGER NOT NULL REFERENCES media_files(id) ON DELETE CASCADE,
                ref_kind TEXT NOT NULL,
                ref_id TEXT NOT NULL,
                conversation_id TEXT NOT NULL,
                attachment_id TEXT,
                variant TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                UNIQUE (ref_kind, ref_id, variant)
             );
             CREATE INDEX IF NOT EXISTS media_refs_file ON media_refs (file_id);
             CREATE INDEX IF NOT EXISTS media_refs_attachment ON media_refs (attachment_id, variant);
             CREATE INDEX IF NOT EXISTS media_refs_conversation ON media_refs (conversation_id);
             CREATE TABLE IF NOT EXISTS public_cache_files (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                device_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                version TEXT NOT NULL,
                sha256 TEXT NOT NULL,
                size INTEGER NOT NULL,
                mime TEXT NOT NULL,
                rel_path TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'available',
                created_at INTEGER NOT NULL,
                touched_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                UNIQUE (device_id, kind, version)
             );
             CREATE TABLE IF NOT EXISTS public_cache_refs (
                user_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                active_file_id INTEGER REFERENCES public_cache_files(id) ON DELETE SET NULL,
                active_version TEXT,
                desired_version TEXT,
                updated_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                PRIMARY KEY (user_id, kind)
             );
             CREATE INDEX IF NOT EXISTS public_cache_refs_file ON public_cache_refs (active_file_id);
             CREATE TABLE IF NOT EXISTS media_meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS video_preview_partials (
                device_id TEXT NOT NULL,
                conversation_id TEXT NOT NULL,
                content_key TEXT NOT NULL,
                preview_sha256 TEXT NOT NULL,
                preview_size INTEGER NOT NULL,
                mime TEXT NOT NULL,
                rel_path TEXT NOT NULL,
                chunks_json TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (device_id, conversation_id, content_key)
             );
             DROP VIEW IF EXISTS preview_eligible;
             CREATE VIEW preview_eligible AS
                SELECT i.seq, i.conversation_id, i.attachment_id,
                       json_extract(i.payload, '$.attachment.sha256') AS sha256,
                       json_extract(i.payload, '$.expires_at') AS expires_at
                FROM inbox i JOIN directory d ON d.id = 1
                JOIN json_each(d.payload, '$.conversations') c
                  ON json_extract(c.value, '$.id') = i.conversation_id
                 AND (json_extract(c.value, '$.joined_after') IS NULL
                      OR i.seq > json_extract(c.value, '$.joined_after'))
                WHERE i.attachment_id IS NOT NULL
                  AND json_extract(i.payload, '$.recalled') IS NOT 1
                  AND NOT EXISTS (SELECT 1 FROM hidden_messages h WHERE h.seq = i.seq);
             DROP VIEW IF EXISTS timeline;
             CREATE VIEW timeline AS
                SELECT seq, conversation_id, attachment_id,
                  CASE WHEN attachment_id IS NOT NULL AND EXISTS (
                         SELECT 1 FROM media_refs r
                         JOIN media_files f ON f.id = r.file_id
                         WHERE r.ref_kind = 'message'
                           AND r.ref_id = CAST(inbox.seq AS TEXT)
                           AND r.conversation_id = inbox.conversation_id
                           AND r.variant IN ('image_original', 'video_original', 'attachment')
                           AND f.status = 'available'
                       )
                       THEN json_set(payload, '$.local_file', json('true')) ELSE payload END AS payload
                FROM inbox
                UNION ALL SELECT -id, conversation_id, NULL, draft FROM outbox WHERE draft IS NOT NULL
                  AND NOT EXISTS (SELECT 1 FROM local_suppressed_sends s WHERE s.kind = 'text' AND s.id = outbox.nonce);
             UPDATE transfers SET payload = json_set(payload, '$.status', 'queued', '$.retry_at', 0)
                WHERE json_extract(payload, '$.status') IN ('running', 'queued');";
        for statement in statements.split_terminator(';') {
            sqlx::query(statement).execute(&mut *tx).await?;
        }
        let saved: Option<String> =
            sqlx::query_scalar("SELECT device_id FROM sync_checkpoint WHERE id = 1")
                .fetch_optional(&mut *tx)
                .await?;
        let device_id = session.device_id.to_string();
        let re_registered = saved
            .as_ref()
            .is_some_and(|previous| *previous != device_id);
        let previous_device_id = saved.clone();
        if saved.as_deref() != Some(&device_id) {
            ensure!(
                !restore,
                OnlineError::Rejected {
                    status: StatusCode::UNAUTHORIZED,
                    message:
                        "This local profile no longer matches the saved device. Sign in again."
                            .into(),
                }
            );
            sqlx::query(
                "INSERT INTO sync_checkpoint (id, device_id, cursor, reconcile_after) VALUES (1,?,?,?)
                 ON CONFLICT (id) DO UPDATE SET device_id = excluded.device_id, cursor = excluded.cursor,
                    reconcile_after = excluded.reconcile_after, pending_ack = NULL",
            ).bind(&device_id).bind(session.cursor.to_string()).bind(saved.map(|_| 0_i64))
                .execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok((re_registered, previous_device_id))
    }

    pub(super) async fn is_locally_deleted(&self, seq: i64) -> Result<bool> {
        Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_deleted_messages WHERE seq = ?)")
            .bind(seq).fetch_one(&self.db).await?)
    }

    pub(super) async fn is_suppressed_upload(&self, id: uuid::Uuid) -> Result<bool> {
        Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_suppressed_sends WHERE kind = 'upload' AND id = ?)")
            .bind(id.to_string()).fetch_one(&self.db).await?)
    }

    pub async fn take_send_failure_notices(&self) -> Result<usize> {
        let ids: Vec<String> = sqlx::query_scalar("DELETE FROM send_failure_notices RETURNING id")
            .fetch_all(&self.db).await?;
        Ok(ids.len())
    }

    pub(super) fn history_changed(&self) {
        self.history_revision.fetch_add(1, Ordering::Relaxed);
    }

    pub async fn sync(self: &Arc<Self>) -> Result<Directory> {
        Ok(self.sync_updates().await?.directory)
    }

    pub async fn sync_updates(self: &Arc<Self>) -> Result<SyncUpdate> {
        self.flush_pending_messages().await?;
        self.sync_inbox().await
    }

    pub async fn sync_inbox(self: &Arc<Self>) -> Result<SyncUpdate> {
        let _gate = self.sync_gate.lock().await;
        self.ensure_open()?;
        self.reconcile_history().await?;
        self.resume_queued().await?;
        self.flush_content_acks().await?;
        self.flush_sync_ack().await?;
        loop {
            self.ensure_open()?;
            let cursor: String =
                sqlx::query_scalar("SELECT cursor FROM sync_checkpoint WHERE id = 1")
                    .fetch_one(&self.db)
                    .await?;
            let started = unix_millis();
            let page: SyncPage = self
                .api
                .json(
                    self.api
                        .request(Method::GET, "/v2/sync")
                        .query(&[("cursor", cursor)]),
                )
                .await?;
            let elapsed = unix_millis().saturating_sub(started);
            self.server_clock_offset.store(
                page.server_time.saturating_sub(started + elapsed / 2),
                Ordering::Relaxed,
            );
            let more = page.has_more;
            self.commit_page(page).await?;
            self.flush_sync_ack().await?;
            if !more {
                return self.local_update().await;
            }
        }
    }

    async fn commit_page(&self, page: SyncPage) -> Result<()> {
        ensure!(
            page.events.len() <= SYNC_PAGE_SIZE,
            OnlineError::InvalidResponse
        );
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        let mut ack = SyncAck {
            page_id: page.page_id,
            metadata_received: Vec::new(),
        };
        let mut last = 0;
        let mut changed = false;
        let mut recalled_files = Vec::new();
        let mut recalled_media = Vec::new();
        for event in page.events {
            match event {
                SyncEvent::Published(mut message) => {
                    ensure!(
                        message.seq > last
                            && message.pending.is_none()
                            && message.attachment.as_ref().is_none_or(Attachment::is_valid),
                        OnlineError::InvalidResponse
                    );
                    last = message.seq;
                    if message.expired.is_none() && !message.recalled {
                        ack.metadata_received.push(message.seq);
                    }
                    if message.sender.id == self.user.id {
                        if let Some(id) = page.own_attachment_keys.get(&message.seq).copied()
                            .or_else(|| message.attachment.as_ref().map(|file| file.id))
                        {
                            let suppressed: bool = sqlx::query_scalar(
                                "SELECT EXISTS(SELECT 1 FROM local_suppressed_sends WHERE kind IN ('upload', 'forward') AND id = ? AND conversation_id = ?)",
                            ).bind(id.to_string()).bind(message.conversation_id.to_string()).fetch_one(&mut *tx).await?;
                            if suppressed {
                                sqlx::query("INSERT OR IGNORE INTO local_deleted_messages (seq, conversation_id, attachment_id) VALUES (?,?,?)")
                                    .bind(message.seq).bind(message.conversation_id.to_string()).bind(id.to_string())
                                    .execute(&mut *tx).await?;
                                changed = true;
                                continue;
                            }
                        }
                        if let Some(nonce) = message.nonce {
                            self.merge_outbox(&mut tx, nonce, &mut message).await?;
                        }
                    }
                    changed |= persist_message(&mut tx, message).await?;
                }
                SyncEvent::Recalled(recall) => {
                    ensure!(
                        recall.seq > last
                            && recall.seq > recall.message_seq
                            && recall.message_seq > 0,
                        OnlineError::InvalidResponse
                    );
                    last = recall.seq;
                    apply_recall(&mut tx, recall, &mut recalled_files, &mut recalled_media).await?;
                    changed = true;
                }
            }
        }
        let directory_changed = sqlx::query(
            "INSERT INTO directory (id, payload) VALUES (1,?)
             ON CONFLICT (id) DO UPDATE SET payload = excluded.payload
             WHERE directory.payload <> excluded.payload",
        )
        .bind(serde_json::to_string(&page.directory)?)
        .execute(&mut *tx)
        .await?
        .rows_affected() > 0;
        let removed_public = if directory_changed {
            self.prune_profile_cache(&mut tx).await?
        } else {
            Vec::new()
        };
        sqlx::query("UPDATE sync_checkpoint SET pending_ack = ? WHERE id = 1")
            .bind(serde_json::to_string(&ack)?)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        if changed || directory_changed {
            self.history_changed();
        }
        self.remove_rel_paths(recalled_media).await;
        self.remove_rel_paths(removed_public).await;
        for id in recalled_files {
            let _ = tokio::fs::remove_file(self.managed_files.join(id)).await;
        }
        Ok(())
    }

    async fn flush_sync_ack(&self) -> Result<()> {
        let pending: Option<String> =
            sqlx::query_scalar("SELECT pending_ack FROM sync_checkpoint WHERE id = 1")
                .fetch_one(&self.db)
                .await?;
        if let Some(pending) = pending {
            let ack: SyncAck = serde_json::from_str(&pending)?;
            let position: SyncPosition = self
                .api
                .json(self.api.request(Method::POST, "/v2/sync/ack").json(&ack))
                .await?;
            ensure!(position.cursor == ack.page_id, OnlineError::InvalidResponse);
            sqlx::query("UPDATE sync_checkpoint SET cursor = ?, pending_ack = NULL WHERE id = 1")
                .bind(position.cursor.to_string())
                .execute(&self.db)
                .await?;
        }
        Ok(())
    }

    pub(super) async fn reconcile_history(&self) -> Result<()> {
        while let Some(after) = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT reconcile_after FROM sync_checkpoint WHERE id = 1",
        )
        .fetch_one(&self.db)
        .await?
        {
            self.ensure_open()?;
            let known: Vec<i64> =
                sqlx::query_scalar("SELECT seq FROM inbox WHERE seq > ? ORDER BY seq LIMIT ?")
                    .bind(after)
                    .bind(SYNC_PAGE_SIZE as i64)
                    .fetch_all(&self.db)
                    .await?;
            let mut files = Vec::new();
            let mut recalled_media = Vec::new();
            let recalls: Vec<MessageRecall> = if known.is_empty() {
                Vec::new()
            } else {
                self.api
                    .json(
                        self.api
                            .request(Method::POST, "/v2/history/status")
                            .json(&known),
                    )
                    .await?
            };
            let mut tx = self.db.begin().await?;
            for recall in recalls {
                ensure!(
                    known.contains(&recall.message_seq) && recall.seq > recall.message_seq,
                    OnlineError::InvalidResponse
                );
                apply_recall(&mut tx, recall, &mut files, &mut recalled_media).await?;
            }
            sqlx::query("UPDATE sync_checkpoint SET reconcile_after = ? WHERE id = 1")
                .bind(known.last())
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            self.history_changed();
            self.remove_rel_paths(recalled_media).await;
            for id in files {
                let _ = tokio::fs::remove_file(self.managed_files.join(id)).await;
            }
        }
        Ok(())
    }

    pub async fn local_update(&self) -> Result<SyncUpdate> {
        let recalled = sqlx::query_scalar(
            "SELECT message_seq FROM recall_tombstones
             UNION SELECT seq FROM inbox WHERE json_extract(payload, '$.recalled') = 1 ORDER BY 1",
        )
        .fetch_all(&self.db)
        .await?;
        let last_seq = sqlx::query_scalar("SELECT coalesce(max(seq), 0) FROM inbox")
            .fetch_one(&self.db)
            .await?;
        let latest_ai_seq = sqlx::query_scalar(
            "SELECT coalesce(max(seq), 0) FROM inbox WHERE json_type(payload, '$.ai_provider') = 'text'",
        ).fetch_one(&self.db).await?;
        // Read the revision before loading the directory so a concurrent write is visible on the next tick.
        let revision = self.history_revision.load(Ordering::Relaxed);
        Ok(SyncUpdate {
            directory: self.directory().await?,
            recalled,
            deleted: sqlx::query_scalar("SELECT seq FROM local_deleted_messages
                UNION SELECT -o.id FROM outbox o JOIN local_suppressed_sends s
                  ON s.kind = 'text' AND s.id = o.nonce ORDER BY 1").fetch_all(&self.db).await?,
            server_now: self.server_now(),
            last_seq,
            revision,
            latest_ai_seq,
        })
    }

    pub async fn membership_directory(&self) -> Result<Directory> {
        let json: Option<String> = sqlx::query_scalar("SELECT payload FROM directory WHERE id = 1")
            .fetch_optional(&self.db)
            .await?;
        Ok(json.map(|value| serde_json::from_str(&value)).transpose()?.unwrap_or_default())
    }

    pub async fn directory(&self) -> Result<Directory> {
        let mut directory = self.membership_directory().await?;
        let deleted_chats: Vec<String> = sqlx::query_scalar("SELECT conversation_id FROM local_deleted_chats")
            .fetch_all(&self.db).await?;
        directory.conversations.retain(|chat| !deleted_chats.contains(&chat.id.to_string()));
        for chat in &mut directory.conversations {
            let id = chat.id.to_string();
            let latest: Option<(i64, String, i64)> = sqlx::query_as(
                "SELECT seq,
                    CASE WHEN json_extract(payload, '$.recalled') = 1 THEN 'Message recalled'
                         WHEN json_extract(payload, '$.expired') = 'file' THEN 'File expired'
                         WHEN json_extract(payload, '$.expired') = 'text' THEN 'Message expired'
                         ELSE coalesce(substr(json_extract(payload, '$.text'), 1, 100),
                                       json_extract(payload, '$.attachment.name'), '') END,
                    json_extract(payload, '$.created_at')
                 FROM timeline WHERE conversation_id = ? AND seq NOT IN (SELECT seq FROM hidden_messages)
                 ORDER BY json_extract(payload, '$.created_at') DESC, seq DESC LIMIT 1",
            ).bind(&id).fetch_optional(&self.db).await?;
            if let Some((seq, preview, time)) = latest {
                chat.preview_seq = Some(seq);
                chat.preview = Some(preview);
                chat.activity_at = time;
            } else {
                chat.preview_seq = None;
                chat.preview = None;
            }
            chat.read_through = sqlx::query_scalar::<_, i64>(
                "SELECT read_through FROM local_read_positions WHERE conversation_id = ?",
            )
            .bind(&id)
            .fetch_optional(&self.db)
            .await?
            .unwrap_or_default();
            (chat.latest_seq, chat.unread) = sqlx::query_as(
                "SELECT coalesce(max(seq), 0), count(*) FILTER (
                     WHERE seq > ? AND json_extract(payload, '$.recalled') IS NOT 1
                       AND json_extract(payload, '$.expired') IS NULL)
                 FROM inbox WHERE json_extract(payload, '$.sender.id') <> ?
                   AND conversation_id = ? AND seq NOT IN (SELECT seq FROM hidden_messages)",
            )
            .bind(chat.read_through)
            .bind(self.user.id.to_string())
            .bind(id)
            .fetch_one(&self.db)
            .await?;
        }
        directory
            .conversations
            .sort_by_key(|chat| (std::cmp::Reverse(chat.activity_at), chat.id));
        Ok(directory)
    }
}

pub(super) async fn persist_message(
    db: &mut SqliteConnection,
    mut message: Message,
) -> Result<bool> {
    let deleted: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_deleted_messages WHERE seq = ?)")
            .bind(message.seq)
            .fetch_one(&mut *db)
            .await?;
    if deleted {
        return Ok(false);
    }
    let suppressed: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM local_suppressed_sends WHERE conversation_id = ?1
            AND ((kind = 'text' AND id = ?2) OR (kind IN ('upload', 'forward') AND id = ?3)))",
    ).bind(message.conversation_id.to_string())
        .bind(message.nonce.map(|id| id.to_string()))
        .bind(message.attachment.as_ref().map(|file| file.id.to_string()))
        .fetch_one(&mut *db).await?;
    if suppressed {
        sqlx::query("INSERT OR IGNORE INTO local_deleted_messages (seq, conversation_id, attachment_id) VALUES (?,?,?)")
            .bind(message.seq).bind(message.conversation_id.to_string())
            .bind(message.attachment.as_ref().map(|file| file.id.to_string()))
            .execute(&mut *db).await?;
        return Ok(false);
    }
    let recalled: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM recall_tombstones WHERE message_seq = ?)")
            .bind(message.seq)
            .fetch_one(&mut *db)
            .await?;
    if recalled || message.recalled {
        message.recalled = true;
        message.expired = None;
        message.text = None;
        message.attachment = None;
        message.reply_to = None;
    }
    let changed = sqlx::query(
        "INSERT INTO inbox (seq, conversation_id, payload, attachment_id) VALUES (?,?,?,?)
         ON CONFLICT (seq) DO UPDATE SET payload = excluded.payload, attachment_id = excluded.attachment_id
         WHERE inbox.payload <> excluded.payload AND (
            json_extract(excluded.payload, '$.recalled') = 1 OR (
                json_extract(inbox.payload, '$.recalled') IS NOT 1 AND
                (json_extract(excluded.payload, '$.expired') IS NULL OR json_extract(inbox.payload, '$.expired') IS NOT NULL)
            )
         )",
    ).bind(message.seq).bind(message.conversation_id.to_string())
        .bind(serde_json::to_string(&message)?)
        .bind(message.attachment.as_ref().map(|f| f.id.to_string()))
        .execute(&mut *db).await?;
    if !message.recalled
        && let Some(ref attachment) = message.attachment
    {
        media_store::bind_message_to_existing_media_in_tx(
            &mut *db,
            message.conversation_id,
            message.seq,
            attachment,
        )
        .await?;
    }
    if changed.rows_affected() != 0 {
        sqlx::query("DELETE FROM local_deleted_chats WHERE conversation_id = ?")
            .bind(message.conversation_id.to_string()).execute(&mut *db).await?;
    }
    Ok(changed.rows_affected() != 0)
}

async fn apply_recall(
    db: &mut SqliteConnection,
    recall: MessageRecall,
    files: &mut Vec<String>,
    media_paths: &mut Vec<String>,
) -> Result<()> {
    let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT attachment_id, conversation_id FROM inbox WHERE seq = ?1 AND attachment_id IS NOT NULL
         UNION SELECT attachment_id, conversation_id FROM local_deleted_messages WHERE seq = ?1 AND attachment_id IS NOT NULL",
    ).bind(recall.message_seq).fetch_optional(&mut *db).await?;
    let (attachment_id, conv_id) = row.unwrap_or((None, None));
    if let Some(id) = attachment_id {
        media_paths.extend(media_store::release_media_refs_in_tx(&mut *db, "download", &id).await?);
    }
    media_paths.extend(
        media_store::release_media_refs_in_tx(&mut *db, "message", &recall.message_seq.to_string())
            .await?,
    );
    sqlx::query("DELETE FROM media_previews WHERE message_seq = ?")
        .bind(recall.message_seq)
        .execute(&mut *db)
        .await?;
    sqlx::query("INSERT OR IGNORE INTO recall_tombstones (message_seq, event_seq) VALUES (?,?)")
        .bind(recall.message_seq)
        .bind(recall.seq)
        .execute(&mut *db)
        .await?;
    let file: Option<String> =
        sqlx::query_scalar("DELETE FROM local_files WHERE message_seq = ? RETURNING id")
            .bind(recall.message_seq)
            .fetch_optional(&mut *db)
            .await?;
    if let Some(id) = file {
        sqlx::query(
            "DELETE FROM transfers WHERE id = ? AND json_extract(payload, '$.status') = 'complete'",
        )
        .bind(&id)
        .execute(&mut *db)
        .await?;
        files.push(id);
    }
    sqlx::query(
        "UPDATE inbox SET attachment_id = NULL,
            payload = json_remove(json_set(payload, '$.text', NULL, '$.attachment', NULL, '$.recalled', json('true')),
                                 '$.reply_to', '$.expired')
         WHERE seq = ?",
    ).bind(recall.message_seq).execute(&mut *db).await?;
    if let Some(conv) = conv_id {
        media_store::cleanup_unreferenced_video_partials_in_tx(&mut *db, &conv, media_paths).await?;
    }
    Ok(())
}
