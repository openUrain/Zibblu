use super::*;
use crate::assistant::AiMessage;
pub use crate::assistant::AiContextMessage;
pub use crate::assistant_extract::AuthorizedAiAttachment;
use sha2::{Digest, Sha256};
use sqlx::SqliteConnection;
use std::collections::HashSet;
use tokio::{io::AsyncWriteExt, sync::Notify};

const MAX_MESSAGES: usize = 60;

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AiScope {
    Prompt,
    Selected { sequences: Vec<i64> },
    Recent { limit: usize },
}

struct TempCleanupGuard {
    path: Option<PathBuf>,
}

impl TempCleanupGuard {
    fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    fn disarm(&mut self) -> Option<PathBuf> {
        self.path.take()
    }
}

impl Drop for TempCleanupGuard {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl Client {
    pub fn profile_dir(&self) -> &Path {
        self.staging.parent().expect("profile staging directory")
    }

    pub async fn ai_messages(&self, conversation: Uuid, scope: AiScope) -> Result<Vec<AiMessage>> {
        let context = self.ai_context_messages(conversation, scope).await?;
        Ok(context
            .into_iter()
            .filter_map(|message| {
                let text = match (message.text, message.attachment) {
                    (Some(text), _) if !text.is_empty() => text,
                    (_, Some(att)) => format!("[Attachment: {}]", att.name),
                    _ => return None,
                };
                Some(AiMessage {
                    sender: message.sender,
                    text,
                })
            })
            .collect())
    }

    pub async fn ai_context_messages(
        &self,
        conversation: Uuid,
        scope: AiScope,
    ) -> Result<Vec<AiContextMessage>> {
        self.ensure_open()?;
        let recent = matches!(scope, AiScope::Recent { .. });
        let mut tx = self.db.begin().await?;
        let joined_after = ensure_normal_chat(&mut tx, self.user.id, conversation).await?;
        let mut messages = scoped_context_messages(&mut tx, conversation, joined_after, scope).await?;
        tx.commit().await?;
        let now = self.server_now();
        let is_expired = |message: &AiContextMessage| message.expires_at.is_some_and(|at| at <= now);
        if recent {
            messages.retain(|message| !is_expired(message));
        } else {
            ensure!(
                !messages.iter().any(is_expired),
                "Some selected messages are unavailable in this chat"
            );
        }
        Ok(messages)
    }

    pub async fn is_attachment_cached_for_ai(
        &self,
        conversation: Uuid,
        seq: i64,
        attachment: &Attachment,
    ) -> Result<bool> {
        self.ensure_open()?;
        if !media_store::is_valid_sha256(&attachment.sha256) {
            return Ok(false);
        }
        let mut tx = self.db.begin().await?;
        let joined_after = ensure_normal_chat(&mut tx, self.user.id, conversation).await?;
        let ok = verify_message_attachment_in_tx(
            &mut tx,
            conversation,
            joined_after,
            seq,
            attachment,
            self.server_now(),
        )
        .await
        .is_ok();
        tx.commit().await?;
        if !ok {
            return Ok(false);
        }
        self.has_verified_media(
            conversation,
            seq,
            Some(attachment.id),
            MediaVariant::for_attachment(attachment),
            &attachment.sha256,
            false,
        )
        .await
    }

    pub async fn fetch_authorized_attachment_for_ai(
        &self,
        conversation: Uuid,
        seq: i64,
        attachment: &Attachment,
        cancel: Arc<Notify>,
        progress: impl Fn(u64, u64) + Send + Sync,
    ) -> Result<AuthorizedAiAttachment> {
        self.ensure_open()?;
        ensure!(
            media_store::is_valid_sha256(&attachment.sha256) && attachment.is_valid(),
            "This attachment has no valid integrity digest"
        );
        let cancel_fut = cancel.notified();
        tokio::pin!(cancel_fut);

        // 1. Re-verify authorization, visibility, recall/deletion state, expiry, and attachment identity.
        {
            let mut tx = self.db.begin().await?;
            let joined_after = ensure_normal_chat(&mut tx, self.user.id, conversation).await?;
            verify_message_attachment_in_tx(
                &mut tx,
                conversation,
                joined_after,
                seq,
                attachment,
                self.server_now(),
            )
            .await?;
            tx.commit().await?;
        }

        // 2. Use verified local cache in read-only mode if available.
        let variant = MediaVariant::for_attachment(attachment);
        if let Some((_, path, size)) = self
            .verified_media_path(
                conversation,
                seq,
                Some(attachment.id),
                variant,
                &attachment.sha256,
                true,
            )
            .await?
            && size == attachment.size
        {
            let path_check = path.clone();
            let opened = tokio::task::spawn_blocking(move || -> Result<()> {
                let meta = std::fs::symlink_metadata(&path_check)?;
                ensure!(
                    meta.is_file() && !staging::is_link(&meta) && meta.len() == size,
                    "Cached attachment is unavailable"
                );
                let _ = std::fs::File::open(&path_check)?;
                Ok(())
            })
            .await?;
            if opened.is_ok() {
                progress(attachment.size, attachment.size);
                return Ok(AuthorizedAiAttachment::Cached(path));
            }
        }

        // 3. Download on demand into media_root/tmp with RAII cleanup and cancellation.
        let tmp_dir = media_store::fs_path(&self.media_root().join("tmp"));
        tokio::fs::create_dir_all(&tmp_dir).await?;
        let prefix = format!("ai-att-{}-", Uuid::new_v4().simple());
        let temp_guard = self.track_temp_prefix(&prefix);
        let temp_path = tmp_dir.join(format!("{prefix}part"));
        let mut cleanup = TempCleanupGuard::new(temp_path.clone());

        {
            let mut file = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp_path)
                .await?;
            let mut hasher = Sha256::new();
            let mut offset = 0u64;
            progress(0, attachment.size);

            while offset < attachment.size {
                let expected = (attachment.size - offset).min(CHUNK_SIZE) as usize;
                let chunk = tokio::select! {
                    biased;
                    _ = &mut cancel_fut => bail!("Cancelled"),
                    res = self.read_attachment_chunk(attachment.id, offset, expected) => res?,
                };
                ensure!(
                    chunk.len() == expected,
                    "Attachment chunk length mismatch"
                );
                tokio::select! {
                    biased;
                    _ = &mut cancel_fut => bail!("Cancelled"),
                    res = file.write_all(&chunk) => res?,
                }
                hasher.update(&chunk);
                offset += chunk.len() as u64;
                progress(offset, attachment.size);
            }
            file.flush().await?;
            file.sync_all().await?;

            ensure!(
                offset == attachment.size,
                "Attachment download size mismatch"
            );
            let actual_sha = format!("{:x}", hasher.finalize());
            ensure!(
                actual_sha == attachment.sha256,
                "Attachment SHA-256 verification failed"
            );
        }

        // Re-check DB authorization in case the message was recalled or deleted mid-download.
        self.ensure_ai_attachments_authorized(conversation, &[(seq, attachment.clone())])
            .await?;

        let path = cleanup.disarm().expect("disarmed temp path");
        Ok(AuthorizedAiAttachment::Temporary {
            path,
            _temp_guard: temp_guard,
        })
    }

    /// Final gate before any content leaves for the AI provider.
    pub async fn ensure_ai_attachments_authorized(
        &self,
        conversation: Uuid,
        items: &[(i64, Attachment)],
    ) -> Result<()> {
        self.ensure_open()?;
        let now = self.server_now();
        let mut tx = self.db.begin().await?;
        let joined_after = ensure_normal_chat(&mut tx, self.user.id, conversation).await?;
        for (seq, attachment) in items {
            verify_message_attachment_in_tx(
                &mut tx,
                conversation,
                joined_after,
                *seq,
                attachment,
                now,
            )
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

pub(super) async fn ensure_normal_chat(
    db: &mut SqliteConnection,
    user: Uuid,
    conversation: Uuid,
) -> Result<Option<i64>> {
    let cutoff: Option<Option<i64>> = sqlx::query_scalar(
        "SELECT json_extract(chat.value, '$.joined_after')
         FROM directory,
             json_each(directory.payload, '$.conversations') AS chat,
             json_each(chat.value, '$.members') AS member
         WHERE directory.id = 1
           AND json_extract(chat.value, '$.id') = ?1
           AND json_extract(member.value, '$.id') = ?2
         LIMIT 1",
    )
    .bind(conversation.to_string())
    .bind(user.to_string())
    .fetch_optional(db)
    .await?;
    match cutoff {
        Some(joined_after) => Ok(joined_after),
        None => bail!("AI is only available in your normal chats"),
    }
}

async fn verify_message_attachment_in_tx(
    db: &mut SqliteConnection,
    conversation: Uuid,
    joined_after: Option<i64>,
    seq: i64,
    attachment: &Attachment,
    now: i64,
) -> Result<()> {
    let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
        "SELECT payload FROM inbox
         WHERE seq = ",
    );
    query.push_bind(seq);
    query
        .push(" AND conversation_id = ")
        .push_bind(conversation.to_string());
    query
        .push(" AND attachment_id = ")
        .push_bind(attachment.id.to_string());
    if let Some(cutoff) = joined_after {
        query.push(" AND seq > ").push_bind(cutoff);
    }
    query.push(
        " AND seq NOT IN (SELECT seq FROM hidden_messages)
          AND seq NOT IN (SELECT seq FROM local_deleted_messages)
          AND coalesce(json_extract(payload, '$.recalled'), 0) = 0
          AND json_extract(payload, '$.expired') IS NULL",
    );

    let payload: Option<String> = query.build_query_scalar().fetch_optional(db).await?;
    let payload = payload.context("This attachment is no longer available in the chat")?;
    let message: Message =
        serde_json::from_str(&payload).context("Could not read the attachment message")?;
    ensure!(
        message.attachment.as_ref() == Some(attachment),
        "Attachment metadata does not match the authorized message"
    );
    ensure!(
        message.expires_at.is_none_or(|expires_at| expires_at > now),
        "This attachment has expired and can no longer be sent to AI"
    );
    Ok(())
}

async fn scoped_context_messages(
    db: &mut SqliteConnection,
    conversation: Uuid,
    joined_after: Option<i64>,
    scope: AiScope,
) -> Result<Vec<AiContextMessage>> {
    if matches!(scope, AiScope::Prompt) {
        return Ok(Vec::new());
    }

    let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
        "SELECT payload FROM inbox
         WHERE (json_type(payload, '$.text') = 'text' OR attachment_id IS NOT NULL)
           AND coalesce(json_extract(payload, '$.recalled'), 0) = 0
           AND json_extract(payload, '$.expired') IS NULL
           AND seq NOT IN (SELECT seq FROM hidden_messages)
           AND seq NOT IN (SELECT seq FROM local_deleted_messages)
           AND conversation_id = ",
    );
    query.push_bind(conversation.to_string());
    if let Some(cutoff) = joined_after {
        query.push(" AND seq > ").push_bind(cutoff);
    }

    let expected = match scope {
        AiScope::Prompt => unreachable!(),
        AiScope::Selected { sequences } => {
            let count = sequences.len();
            ensure!(
                (1..=MAX_MESSAGES).contains(&count),
                "Select between 1 and 60 messages"
            );
            let unique: HashSet<i64> = sequences.iter().copied().collect();
            ensure!(
                unique.len() == count,
                "Some selected messages are unavailable in this chat"
            );
            query.push(" AND seq IN (");
            let mut ids = query.separated(", ");
            for sequence in sequences {
                ids.push_bind(sequence);
            }
            ids.push_unseparated(") ORDER BY json_extract(payload, '$.created_at'), seq");
            Some(count)
        }
        AiScope::Recent { limit } => {
            ensure!(
                (1..=MAX_MESSAGES).contains(&limit),
                "Choose up to 60 recent messages"
            );
            query
                .push(" ORDER BY json_extract(payload, '$.created_at') DESC, seq DESC LIMIT ")
                .push_bind(limit as i64);
            None
        }
    };

    let mut rows: Vec<String> = query.build_query_scalar().fetch_all(db).await?;
    if let Some(count) = expected {
        ensure!(
            rows.len() == count,
            "Some selected messages are unavailable in this chat"
        );
    } else {
        rows.reverse();
    }

    rows.into_iter()
        .map(|row| {
            let message: Message =
                serde_json::from_str(&row).context("Could not read a selected message")?;
            ensure!(
                !message.recalled && message.expired.is_none(),
                "Some selected messages are unavailable in this chat"
            );
            ensure!(
                message.text.as_deref().is_some_and(|t| !t.is_empty())
                    || message.attachment.is_some(),
                "The selected message contains no readable content"
            );
            Ok(AiContextMessage {
                seq: message.seq,
                sender: message.sender.handle,
                text: message.text,
                attachment: message.attachment,
                created_at: message.created_at,
                expires_at: message.expires_at,
                expired: message.expired,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_schema(db: &mut SqliteConnection) -> Result<()> {
        sqlx::raw_sql(
            "CREATE TABLE inbox (seq INTEGER PRIMARY KEY, conversation_id TEXT, payload TEXT, attachment_id TEXT);
             CREATE TABLE hidden_messages (seq INTEGER PRIMARY KEY);
             CREATE TABLE local_deleted_messages (seq INTEGER PRIMARY KEY, conversation_id TEXT, attachment_id TEXT);
             CREATE TABLE directory (id INTEGER PRIMARY KEY, payload TEXT NOT NULL);",
        )
        .execute(db)
        .await?;
        Ok(())
    }

    fn make_message(
        seq: i64,
        conversation_id: Uuid,
        sender: User,
        text: Option<&str>,
        attachment: Option<Attachment>,
        recalled: bool,
        expired: Option<ExpiredPayload>,
    ) -> Message {
        Message {
            seq,
            conversation_id,
            sender,
            created_at: seq * 1000,
            text: text.map(str::to_owned),
            attachment,
            ai_provider: None,
            recalled,
            reply_to: None,
            nonce: None,
            expires_at: None,
            expired,
            pending: None,
            local_file: false,
        }
    }

    #[tokio::test]
    async fn ai_scope_is_bounded_to_normal_chat_and_exact_selection() -> Result<()> {
        use sqlx::Connection;
        let mut db = SqliteConnection::connect("sqlite::memory:").await?;
        setup_schema(&mut db).await?;
        let chat = Uuid::new_v4();
        let other = Uuid::new_v4();
        let private = Uuid::new_v4();
        let user = User {
            id: Uuid::new_v4(),
            handle: "alice".into(),
        };
        let directory = serde_json::json!({
            "conversations": [
                { "id": chat, "members": [&user], "joined_after": 1 },
                { "id": other, "members": [{ "id": Uuid::new_v4() }] },
            ],
        });
        sqlx::query("INSERT INTO directory VALUES (1, ?)")
            .bind(directory.to_string())
            .execute(&mut db)
            .await?;
        let cutoff = ensure_normal_chat(&mut db, user.id, chat).await?;
        assert_eq!(cutoff, Some(1));
        assert!(ensure_normal_chat(&mut db, user.id, other).await.is_err());
        assert!(ensure_normal_chat(&mut db, user.id, private).await.is_err());

        let att = Attachment {
            id: Uuid::new_v4(),
            name: "doc.pdf".into(),
            kind: AttachmentKind::File,
            size: 128,
            sha256: "a".repeat(64),
        };
        let hidden_att = Attachment {
            id: Uuid::new_v4(),
            name: "adjacent.png".into(),
            kind: AttachmentKind::File,
            size: 64,
            sha256: "b".repeat(64),
        };

        let messages = [
            make_message(1, chat, user.clone(), Some("before-join"), None, false, None),
            make_message(2, chat, user.clone(), Some("selected"), None, false, None),
            make_message(3, other, user.clone(), Some("other chat"), None, false, None),
            make_message(4, chat, user.clone(), Some("latest"), None, false, None),
            make_message(5, chat, user.clone(), None, Some(att.clone()), false, None),
            make_message(6, chat, user.clone(), Some("adjacent text"), Some(hidden_att), false, None),
            make_message(7, chat, user.clone(), Some("recalled"), None, true, None),
            make_message(8, chat, user.clone(), Some("expired"), None, false, Some(ExpiredPayload::Text)),
            make_message(9, chat, user.clone(), Some("hidden"), None, false, None),
            make_message(10, chat, user.clone(), Some("deleted"), None, false, None),
        ];
        for msg in messages {
            let att_id = msg.attachment.as_ref().map(|a| a.id.to_string());
            let payload = serde_json::to_string(&msg)?;
            sqlx::query("INSERT INTO inbox VALUES (?, ?, ?, ?)")
                .bind(msg.seq)
                .bind(msg.conversation_id.to_string())
                .bind(payload)
                .bind(att_id)
                .execute(&mut db)
                .await?;
        }
        sqlx::query("INSERT INTO hidden_messages VALUES (9)").execute(&mut db).await?;
        sqlx::query("INSERT INTO local_deleted_messages VALUES (10, ?, NULL)")
            .bind(chat.to_string())
            .execute(&mut db)
            .await?;

        // Selected [2] must NEVER pull in adjacent message 4 or attachment 5 or 6.
        let chosen = scoped_context_messages(
            &mut db,
            chat,
            cutoff,
            AiScope::Selected { sequences: vec![2] },
        )
        .await?;
        assert_eq!(chosen.len(), 1);
        assert_eq!(chosen[0].seq, 2);
        assert_eq!(chosen[0].text.as_deref(), Some("selected"));
        assert!(chosen[0].attachment.is_none());

        // Selecting [5] brings only attachment 5, not adjacent 6.
        let chosen_att = scoped_context_messages(
            &mut db,
            chat,
            cutoff,
            AiScope::Selected { sequences: vec![5] },
        )
        .await?;
        assert_eq!(chosen_att.len(), 1);
        assert_eq!(chosen_att[0].seq, 5);
        assert_eq!(chosen_att[0].attachment.as_ref(), Some(&att));

        // Rejects pre-join (1), other chat (3), recalled (7), expired (8), hidden (9), deleted (10).
        for forbidden in [1, 3, 7, 8, 9, 10] {
            assert!(
                scoped_context_messages(
                    &mut db,
                    chat,
                    cutoff,
                    AiScope::Selected {
                        sequences: vec![2, forbidden]
                    },
                )
                .await
                .is_err(),
                "seq {forbidden} must not be selectable"
            );
        }

        // Recent respects cutoff and excludes recalled/expired/hidden/deleted while preserving attachments.
        let recent = scoped_context_messages(&mut db, chat, cutoff, AiScope::Recent { limit: 3 }).await?;
        assert_eq!(
            recent.iter().map(|m| m.seq).collect::<Vec<_>>(),
            vec![4, 5, 6]
        );
        assert!(
            scoped_context_messages(&mut db, chat, cutoff, AiScope::Prompt)
                .await?
                .is_empty()
        );
        assert!(
            scoped_context_messages(&mut db, chat, cutoff, AiScope::Recent { limit: 61 })
                .await
                .is_err()
        );
        Ok(())
    }

    #[tokio::test]
    async fn cached_attachment_past_expires_at_is_rejected_without_expired_marker() -> Result<()> {
        use sqlx::Connection;
        let mut db = SqliteConnection::connect("sqlite::memory:").await?;
        setup_schema(&mut db).await?;
        let chat = Uuid::new_v4();
        let user = User { id: Uuid::new_v4(), handle: "alice".into() };
        let att = Attachment {
            id: Uuid::new_v4(),
            name: "doc.txt".into(),
            kind: AttachmentKind::File,
            size: 8,
            sha256: "c".repeat(64),
        };
        let mut msg = make_message(5, chat, user, None, Some(att.clone()), false, None);
        msg.expires_at = Some(10_000);
        sqlx::query("INSERT INTO inbox VALUES (5, ?, ?, ?)")
            .bind(chat.to_string())
            .bind(serde_json::to_string(&msg)?)
            .bind(att.id.to_string())
            .execute(&mut db)
            .await?;

        verify_message_attachment_in_tx(&mut db, chat, None, 5, &att, 9_999).await?;
        for now in [10_000, 10_001] {
            assert!(
                verify_message_attachment_in_tx(&mut db, chat, None, 5, &att, now)
                    .await
                    .is_err(),
                "attachment must be refused once the clock reaches expires_at"
            );
        }
        Ok(())
    }

    #[test]
    fn temporary_authorized_attachment_deletes_file_on_drop() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("ai-att-test.part");
        std::fs::write(&path, b"temp-payload")?;
        let active = Arc::new(std::sync::Mutex::new(HashSet::new()));
        let guard = TempFileGuard::track(Arc::clone(&active), "ai-att-test-".into());
        {
            let authorized = AuthorizedAiAttachment::Temporary {
                path: path.clone(),
                _temp_guard: guard,
            };
            assert!(authorized.path().exists());
            assert!(!authorized.is_cached());
        }
        assert!(!path.exists(), "temporary AI attachment must be removed on Drop");
        assert!(active.lock().unwrap().is_empty());
        Ok(())
    }
}
