use super::{Client, OnlineError, local_sync::persist_message, unix_millis};
use crate::{
    ContactRequest, MAX_AI_REPLY_BYTES, MAX_MESSAGE_BYTES, Message, PendingSend, Publication,
    PublishAiReply, SendMessage,
};
use anyhow::{Context, Result, ensure};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, SqliteConnection};
use std::sync::atomic::Ordering;
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Work {
    Text(SendMessage),
    Ai(PublishAiReply),
    Greeting(ContactRequest),
}

#[derive(FromRow)]
struct Pending {
    nonce: String,
    conversation_id: Option<String>,
    work: String,
    draft: Option<String>,
    attempts: i64,
}

impl Client {
    pub async fn send_message(&self, conversation: Uuid, message: SendMessage) -> Result<()> {
        let nonce = message.nonce;
        self.queue_message(conversation, message).await?;
        self.try_send(nonce).await
    }

    pub async fn queue_message(&self, conversation: Uuid, mut message: SendMessage) -> Result<()> {
        message.text = message.text.trim().to_owned();
        ensure!(
            !message.text.is_empty()
                && message.text.len() <= MAX_MESSAGE_BYTES
                && !message.text.contains('\0'),
            "Enter a message up to {} KiB without NUL characters",
            MAX_MESSAGE_BYTES / 1024
        );
        if let Some(seq) = message.reply_to {
            ensure!(
                self.quoted_message(conversation, seq).await?.is_some(),
                "This quoted message is unavailable"
            );
        }
        self.enqueue(message.nonce, Some(conversation), Work::Text(message))
            .await
    }

    pub async fn publish_ai_reply(
        &self,
        conversation: Uuid,
        mut reply: PublishAiReply,
    ) -> Result<()> {
        reply.text = reply.text.trim().to_owned();
        ensure!(
            !reply.text.is_empty()
                && reply.text.len() <= MAX_AI_REPLY_BYTES
                && !reply.text.contains('\0'),
            "Choose an AI reply up to 24 KiB"
        );
        reply.readers.sort_unstable();
        let nonce = reply.nonce;
        self.enqueue(reply.nonce, Some(conversation), Work::Ai(reply))
            .await?;
        self.try_send(nonce).await
    }

    pub async fn request_contact(&self, user_id: Uuid, greeting: String) -> Result<()> {
        let greeting = greeting.trim().to_owned();
        ensure!(
            greeting.chars().count() <= 200 && !greeting.contains('\0'),
            "Enter a greeting of up to 200 characters"
        );
        let request_id = Uuid::new_v4();
        self.enqueue(
            request_id,
            None,
            Work::Greeting(ContactRequest {
                user_id,
                request_id,
                greeting,
            }),
        )
        .await?;
        self.try_send(request_id).await
    }

    async fn enqueue(&self, nonce: Uuid, conversation: Option<Uuid>, work: Work) -> Result<()> {
        self.ensure_open()?;
        let nonce_key = nonce.to_string();
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        let serialized = serde_json::to_string(&work)?;
        let chat = conversation.map(|id| id.to_string());
        let confirmed: Option<String> =
            sqlx::query_scalar("SELECT request_hash FROM confirmed_sends WHERE nonce = ?")
                .bind(&nonce_key)
                .fetch_optional(&mut *tx)
                .await?;
        if let Some(confirmed) = confirmed {
            ensure!(
                confirmed == request_hash(chat.as_deref(), &serialized),
                "This message ID is already in use"
            );
            return Ok(());
        }
        let suppressed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_suppressed_sends WHERE kind = 'text' AND id = ?)")
            .bind(&nonce_key).fetch_one(&mut *tx).await?;
        ensure!(!suppressed, "This message ID is already in use");
        let inserted: Option<i64> = sqlx::query_scalar(
            "INSERT INTO outbox (nonce, conversation_id, work) VALUES (?,?,?) ON CONFLICT(nonce) DO NOTHING RETURNING id",
        ).bind(&nonce_key).bind(&chat).bind(&serialized).fetch_optional(&mut *tx).await?;
        if let Some(id) = inserted {
            if let Some(conversation_id) = conversation {
                let conv = conversation_id.to_string();
                sqlx::query("DELETE FROM local_deleted_chats WHERE conversation_id = ?")
                    .bind(&conv).execute(&mut *tx).await?;
                sqlx::query(
                    "UPDATE conversation_prefs SET hidden_at_seq = NULL
                     WHERE conversation_id = ? AND hidden_at_seq IS NOT NULL",
                )
                .bind(&conv)
                .execute(&mut *tx)
                .await?;
                let (text, created_at, reply_to, ai_provider) = match work {
                    Work::Text(input) => (input.text, unix_millis(), input.reply_to, None),
                    Work::Ai(input) => (input.text, input.created_at, None, Some(input.provider)),
                    Work::Greeting(_) => {
                        unreachable!("greetings do not have a conversation before approval")
                    }
                };
                let draft = Message {
                    seq: -id,
                    conversation_id,
                    sender: self.user.clone(),
                    created_at,
                    text: Some(text),
                    attachment: None,
                    ai_provider,
                    ai_sources: Vec::new(),
                    recalled: false,
                    reply_to,
                    nonce: Some(nonce),
                    expires_at: None,
                    expired: None,
                    pending: Some(PendingSend {
                        error: None,
                        blocked: false,
                    }),
                    local_file: false,
                };
                sqlx::query("UPDATE outbox SET draft = ? WHERE id = ?")
                    .bind(serde_json::to_string(&draft)?)
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
            }
        } else {
            let same: bool = sqlx::query_scalar(
                "SELECT work = ? AND conversation_id IS ? FROM outbox WHERE nonce = ?",
            )
            .bind(&serialized)
            .bind(&chat)
            .bind(&nonce_key)
            .fetch_one(&mut *tx)
            .await?;
            ensure!(same, "This message ID is already in use");
        }
        tx.commit().await?;
        self.history_changed();
        Ok(())
    }

    async fn try_send(&self, nonce: Uuid) -> Result<()> {
        if let Ok(_gate) = self.outbox_gate.try_lock() {
            self.flush_outbox(Some(nonce)).await?;
        }
        Ok(())
    }

    pub async fn retry_message(&self, nonce: Uuid) -> Result<()> {
        let _gate = self.outbox_gate.lock().await;
        self.ensure_open()?;
        sqlx::query(
            "UPDATE outbox SET blocked = 0, next_attempt = 0, attempts = 0 WHERE nonce = ?",
        )
        .bind(nonce.to_string())
        .execute(&self.db)
        .await?;
        self.flush_outbox(Some(nonce)).await
    }

    pub async fn flush_pending_messages(&self) -> Result<bool> {
        let _gate = self.outbox_gate.lock().await;
        let before = self.history_revision.load(Ordering::Relaxed);
        self.flush_outbox(None).await?;
        Ok(before != self.history_revision.load(Ordering::Relaxed))
    }

    async fn flush_outbox(&self, only: Option<Uuid>) -> Result<()> {
        self.ensure_open()?;
        let only = only.map(|nonce| nonce.to_string());
        let pending: Vec<Pending> = if let Some(ref nonce) = only {
            sqlx::query_as(
                "SELECT nonce, conversation_id, work, draft, attempts FROM outbox
                 WHERE nonce = ? AND blocked = 0 AND submitted = 0",
            )
            .bind(nonce)
            .fetch_all(&self.db)
            .await?
        } else {
            // A delayed text keeps its place ahead of later text in that conversation.
            sqlx::query_as(
                "SELECT nonce, conversation_id, work, draft, attempts FROM outbox AS pending
                 WHERE blocked = 0 AND submitted = 0 AND next_attempt <= ?1
                   AND (NOT ?2 OR json_extract(work, '$.kind') <> 'text' OR NOT EXISTS (
                     SELECT 1 FROM outbox AS earlier
                     WHERE earlier.id < pending.id AND earlier.conversation_id = pending.conversation_id
                       AND earlier.blocked = 0 AND earlier.submitted = 0 AND earlier.next_attempt > ?1
                       AND json_extract(earlier.work, '$.kind') = 'text'
                   ))
                 ORDER BY id LIMIT 16",
            )
            .bind(unix_millis())
            .bind(cfg!(windows))
            .fetch_all(&self.db)
            .await?
        };
        for item in pending {
            self.ensure_open()?;
            let work: Work = serde_json::from_str(&item.work)?;
            let reply = match &work {
                Work::Text(input) => {
                    let chat = item
                        .conversation_id
                        .as_deref()
                        .context("Missing queued chat")?;
                    self.api
                        .json::<Publication>(
                            self.api
                                .request(
                                    Method::POST,
                                    &format!("/v1/conversations/{chat}/messages"),
                                )
                                .json(input)
                                .timeout(std::time::Duration::from_secs(3)),
                        )
                        .await
                        .map(Some)
                }
                Work::Ai(input) => {
                    let chat = item
                        .conversation_id
                        .as_deref()
                        .context("Missing queued chat")?;
                    self.api
                        .json::<Publication>(
                            self.api
                                .request(
                                    Method::POST,
                                    &format!("/v1/conversations/{chat}/ai-replies"),
                                )
                                .json(input)
                                .timeout(std::time::Duration::from_secs(3)),
                        )
                        .await
                        .map(Some)
                }
                Work::Greeting(input) => self
                    .api
                    .send(
                        self.api
                            .request(Method::POST, "/v1/contacts")
                            .json(input)
                            .timeout(std::time::Duration::from_secs(3)),
                    )
                    .await
                    .map(|_| None),
            };
            match reply {
                Ok(Some(stamp)) => self.confirm_send(item, stamp).await?,
                Ok(None) => {
                    sqlx::query("UPDATE outbox SET submitted = 1 WHERE nonce = ?")
                        .bind(&item.nonce)
                        .execute(&self.db)
                        .await?;
                }
                Err(
                    error @ OnlineError::Rejected {
                        status: StatusCode::UNAUTHORIZED,
                        ..
                    },
                ) => return Err(error.into()),
                Err(error) => {
                    let blocked = matches!(&error, OnlineError::Rejected { status, .. }
                        if status.is_client_error() && !matches!(*status, StatusCode::TOO_MANY_REQUESTS | StatusCode::REQUEST_TIMEOUT));
                    let windows_text = cfg!(windows) && matches!(&work, Work::Text(_));
                    let max_exponent = match &error {
                        OnlineError::Network(cause) if !cause.is_timeout() => 1,
                        OnlineError::Network(_) if windows_text => 2,
                        OnlineError::Rejected { status, .. }
                            if windows_text
                                && (status.is_server_error()
                                    || *status == StatusCode::REQUEST_TIMEOUT) =>
                        {
                            2
                        }
                        _ => 6,
                    };
                    let jitter = if windows_text && !blocked {
                        i64::from(Uuid::new_v4().as_bytes()[0]) * 2
                    } else {
                        0
                    };
                    let next_attempt =
                        unix_millis() + 1000 * (1 << item.attempts.min(max_exponent)) + jitter;
                    let mut draft: Option<Message> = item
                        .draft
                        .as_deref()
                        .map(serde_json::from_str)
                        .transpose()?;
                    if let Some(ref mut draft) = draft {
                        draft.pending = Some(PendingSend {
                            error: blocked.then(|| error.to_string()),
                            blocked,
                        });
                    }
                    let draft = draft.as_ref().map(serde_json::to_string).transpose()?;
                    let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
                    let suppressed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_suppressed_sends WHERE kind = 'text' AND id = ?)")
                        .bind(&item.nonce).fetch_one(&mut *tx).await?;
                    if suppressed && blocked {
                        let removed = sqlx::query("DELETE FROM outbox WHERE nonce = ?")
                            .bind(&item.nonce).execute(&mut *tx).await?.rows_affected();
                        if removed != 0 {
                            sqlx::query("INSERT OR IGNORE INTO send_failure_notices (kind, id) VALUES ('text', ?)")
                                .bind(&item.nonce).execute(&mut *tx).await?;
                        }
                    } else {
                        sqlx::query(
                            "UPDATE outbox SET draft = ?, blocked = ?, next_attempt = ?, attempts = attempts + 1 WHERE nonce = ?",
                        ).bind(&draft).bind(blocked)
                            .bind(next_attempt)
                            .bind(&item.nonce).execute(&mut *tx).await?;
                    }
                    tx.commit().await?;
                    if item.draft != draft || suppressed && blocked {
                        self.history_changed();
                    }
                    if suppressed && blocked {
                        continue;
                    }
                    if !blocked {
                        break;
                    }
                    if only.is_some() {
                        return Err(error.into());
                    }
                }
            }
        }
        Ok(())
    }

    async fn confirm_send(&self, item: Pending, stamp: Publication) -> Result<()> {
        ensure!(stamp.seq > 0, OnlineError::InvalidResponse);
        let mut message: Message =
            serde_json::from_str(item.draft.as_deref().context("Missing pending message")?)?;
        message.seq = stamp.seq;
        message.created_at = stamp.created_at;
        message.expires_at = stamp.expires_at;
        message.recalled = stamp.recalled;
        message.pending = None;
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        persist_message(&mut tx, message).await?;
        confirm_outbox(
            &mut tx,
            &item.nonce,
            item.conversation_id.as_deref(),
            &item.work,
        )
        .await?;
        tx.commit().await?;
        self.history_changed();
        Ok(())
    }

    pub(super) async fn merge_outbox(
        &self,
        db: &mut SqliteConnection,
        nonce: Uuid,
        message: &mut Message,
    ) -> Result<()> {
        let pending: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT work, conversation_id FROM outbox WHERE nonce = ? AND blocked = 0",
        )
        .bind(nonce.to_string())
        .fetch_optional(&mut *db)
        .await?;
        if let Some((serialized, conversation)) = pending {
            if conversation
                .as_ref()
                .is_some_and(|chat| chat != &message.conversation_id.to_string())
            {
                return Ok(());
            }
            if message.expired.is_some() && !message.recalled {
                message.expired = None;
                message.text = Some(match serde_json::from_str(&serialized)? {
                    Work::Text(input) => input.text,
                    Work::Ai(input) => input.text,
                    Work::Greeting(input) => input.greeting,
                });
            }
            confirm_outbox(db, &nonce.to_string(), conversation.as_deref(), &serialized).await?;
        }
        Ok(())
    }
}

fn request_hash(chat: Option<&str>, work: &str) -> String {
    format!(
        "{:x}",
        Sha256::digest(format!("{}\n{work}", chat.unwrap_or_default()))
    )
}

async fn confirm_outbox(
    db: &mut SqliteConnection,
    nonce: &str,
    chat: Option<&str>,
    work: &str,
) -> Result<()> {
    let hash = request_hash(chat, work);
    let inserted = sqlx::query(
        "INSERT INTO confirmed_sends (nonce, request_hash) VALUES (?,?) ON CONFLICT(nonce) DO NOTHING",
    )
        .bind(nonce)
        .bind(&hash)
        .execute(&mut *db)
        .await?
        .rows_affected();
    if inserted == 0 {
        let same: bool = sqlx::query_scalar("SELECT request_hash = ? FROM confirmed_sends WHERE nonce = ?")
            .bind(hash)
            .bind(nonce)
            .fetch_one(&mut *db)
            .await?;
        ensure!(same, "This message ID is already in use");
    }
    sqlx::query("DELETE FROM outbox WHERE nonce = ?")
        .bind(nonce)
        .execute(db)
        .await?;
    Ok(())
}
