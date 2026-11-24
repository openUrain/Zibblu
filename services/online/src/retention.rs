use std::{env, time::Duration};

use anyhow::{Context, ensure};
use sqlx::PgPool;

use crate::chats::lock_message_feed;

#[derive(Clone, Copy)]
pub struct Retention {
    pub text_secs: i32,
    pub file_secs: Option<i32>,
    pub upload_secs: Option<i32>,
    pub device_idle_secs: Option<i32>,
    pub device_limit: Option<i32>,
    pub sweep_interval: Option<Duration>,
}

impl Default for Retention {
    fn default() -> Self {
        Self {
            text_secs: 72 * 60 * 60,
            file_secs: None,
            upload_secs: None,
            device_idle_secs: None,
            device_limit: None,
            sweep_interval: None,
        }
    }
}

impl Retention {
    pub fn from_env() -> anyhow::Result<Self> {
        fn positive(name: &str) -> anyhow::Result<Option<i32>> {
            match env::var(name) {
                Ok(value) if value.is_empty() => Ok(None),
                Ok(value) => {
                    let value = value.parse().with_context(|| format!("Invalid {name}"))?;
                    ensure!(value > 0, "{name} must be positive");
                    Ok(Some(value))
                }
                Err(env::VarError::NotPresent) => Ok(None),
                Err(error) => Err(error).with_context(|| format!("Invalid {name}")),
            }
        }
        Ok(Self {
            text_secs: positive("ZIBBLU_TEXT_TTL_SECONDS")?.unwrap_or(Self::default().text_secs),
            file_secs: positive("ZIBBLU_FILE_TTL_SECONDS")?,
            upload_secs: positive("ZIBBLU_UPLOAD_TTL_SECONDS")?,
            device_idle_secs: positive("ZIBBLU_DEVICE_IDLE_SECONDS")?,
            device_limit: positive("ZIBBLU_DEVICE_LIMIT")?,
            sweep_interval: positive("ZIBBLU_CLEANUP_INTERVAL_SECONDS")?
                .map(|seconds| Duration::from_secs(seconds as u64)),
        })
    }
}

pub async fn sweep(db: &PgPool) -> anyhow::Result<()> {
    let mut tx = db.begin().await?;
    lock_message_feed(&mut tx)
        .await
        .map_err(|_| anyhow::anyhow!("Could not lock the delivery queue"))?;
    let statements =
        "UPDATE messages SET text = NULL WHERE seq IN (
            SELECT seq FROM messages WHERE text IS NOT NULL AND expires_at <= clock_timestamp()
            ORDER BY expires_at, seq LIMIT 100 FOR UPDATE SKIP LOCKED
         );
         UPDATE contact_requests SET greeting = '' WHERE request_id IN (
            SELECT request_id FROM contact_requests WHERE greeting <> '' AND greeting_expires_at <= clock_timestamp()
            ORDER BY greeting_expires_at, request_id LIMIT 100 FOR UPDATE SKIP LOCKED
         );
         WITH removed AS (
             DELETE FROM attachments WHERE id IN (
                 SELECT f.id FROM attachments f LEFT JOIN messages m ON m.attachment_id = f.id
                 WHERE (f.expires_at <= clock_timestamp() OR m.recall_seq IS NOT NULL
                        OR (f.complete AND m.seq IS NULL))
                   AND NOT EXISTS (SELECT 1 FROM attachments alias WHERE alias.payload_id = f.id)
                 ORDER BY f.expires_at, f.id LIMIT 16 FOR UPDATE OF f SKIP LOCKED
             ) RETURNING id, owner_id, conversation_id, complete
         ) INSERT INTO m1_expired_uploads (id, owner_id, conversation_id, forget_at)
           SELECT id, owner_id, conversation_id, clock_timestamp() + interval '72 hours'
           FROM removed WHERE NOT complete ON CONFLICT DO NOTHING;
         DELETE FROM m1_expired_uploads WHERE id IN (
             SELECT id FROM m1_expired_uploads WHERE forget_at <= clock_timestamp()
             ORDER BY forget_at, id LIMIT 100
         );
         UPDATE m1_devices SET retired_at = clock_timestamp(), page_id = NULL, page_next = NULL, page_offered = NULL
         WHERE id IN (
             SELECT d.id FROM m1_devices d JOIN accounts a ON a.id = d.account_id
             WHERE d.retired_at IS NULL AND (d.active_until <= clock_timestamp() OR a.revoked_at IS NOT NULL)
             ORDER BY d.active_until, d.id LIMIT 100
         );
         DELETE FROM sessions WHERE token_hash IN (
             SELECT s.token_hash FROM sessions s JOIN m1_devices d ON d.id = s.device_id
             WHERE d.retired_at IS NOT NULL OR s.expires_at <= clock_timestamp()
             ORDER BY s.expires_at LIMIT 100
         );
         WITH closing AS (
             SELECT e.device_id, e.event_seq, v.recall_seq IS NOT NULL OR v.seq IS NULL AS revoked
             FROM m1_device_events e JOIN m1_devices d ON d.id = e.device_id
             LEFT JOIN m1_visible_messages v ON v.seq = e.message_seq AND v.account_id = d.account_id
             WHERE e.content_state = 'pending'
               AND (v.seq IS NULL OR v.recall_seq IS NOT NULL OR v.expires_at <= clock_timestamp())
             LIMIT 100
         ) UPDATE m1_device_events e SET content_state = CASE WHEN closing.revoked THEN 'revoked' ELSE 'expired' END
           FROM closing WHERE (e.device_id, e.event_seq) = (closing.device_id, closing.event_seq);
         WITH finished AS (
             SELECT e.device_id, e.event_seq
             FROM m1_device_events e JOIN m1_devices d ON d.id = e.device_id
             JOIN messages m ON m.seq = e.message_seq JOIN conversations c ON c.id = m.conversation_id
             WHERE d.retired_at IS NOT NULL OR (
                 e.event_seq <= d.acked_after AND e.metadata_state <> 'pending'
                 AND (e.content_state IS NULL OR e.content_state IN ('expired', 'revoked')
                      OR m.expires_at <= clock_timestamp() OR m.recall_seq IS NOT NULL)
                 AND (e.metadata_state <> 'received' OR c.direct_key IS NULL OR m.ai_provider IS NOT NULL
                      OR m.recall_seq IS NOT NULL OR m.sender_id = d.account_id
                      OR EXISTS (
                          SELECT 1 FROM m1_visible_messages newer WHERE newer.account_id = d.account_id
                          AND newer.conversation_id = m.conversation_id AND newer.ai_provider IS NULL
                          AND (newer.created_at, newer.seq) > (m.created_at, m.seq)
                      ))
             ) LIMIT 100
         ) DELETE FROM m1_device_events e USING finished
           WHERE (e.device_id, e.event_seq) = (finished.device_id, finished.event_seq);";
    for statement in statements.split_terminator(';') {
        sqlx::query(statement).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}
