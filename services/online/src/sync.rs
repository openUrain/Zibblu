use axum::{
    Json,
    extract::{Query, State},
};
use sqlx::{FromRow, PgConnection, types::Json as DbJson};
use uuid::Uuid;
use zibblu_core::{
    Message, MessageRecall, Publication, SYNC_PAGE_SIZE, SyncAck, SyncEvent, SyncPage, SyncPosition,
};

use crate::{App, Auth, Error, Result, chats};

pub(crate) async fn dispatch(db: &mut PgConnection, seq: i64) -> Result<()> {
    sqlx::query(
        "INSERT INTO m1_recipients (message_seq, account_id)
         SELECT m.seq, own.account_id FROM messages m
         JOIN members own ON own.conversation_id = m.conversation_id
         JOIN accounts a ON a.id = own.account_id
         WHERE m.seq = $1 AND own.active AND m.seq > own.joined_after AND a.revoked_at IS NULL
           AND (m.ai_provider IS NULL OR EXISTS (
               SELECT 1 FROM ai_reply_readers r WHERE r.message_seq = m.seq AND r.account_id = own.account_id
           ))",
    ).bind(seq).execute(&mut *db).await?;
    sqlx::query(
        "INSERT INTO m1_device_events (device_id, event_seq, message_seq, content_state)
         SELECT d.id, m.seq, m.seq, CASE WHEN m.attachment_id IS NOT NULL THEN 'pending' END
         FROM m1_visible_messages m JOIN m1_devices d ON d.account_id = m.account_id
         WHERE m.seq = $1 AND d.retired_at IS NULL
           AND coalesce(d.active_until > clock_timestamp(), true) AND d.epoch = (SELECT epoch FROM m1_feed)",
    ).bind(seq).execute(&mut *db).await?;
    advance_feed(db, seq).await
}

pub(crate) async fn advance_feed(db: &mut PgConnection, seq: i64) -> Result<()> {
    sqlx::query("UPDATE m1_feed SET committed_through = $1")
        .bind(seq)
        .execute(db)
        .await?;
    Ok(())
}

pub(crate) async fn publication(db: &mut PgConnection, seq: i64) -> Result<Publication> {
    let (created_at, expires_at, recalled) = sqlx::query_as(
        "SELECT created_at, (extract(epoch FROM expires_at) * 1000)::BIGINT, recall_seq IS NOT NULL
         FROM messages WHERE seq = $1",
    )
    .bind(seq)
    .fetch_one(db)
    .await?;
    Ok(Publication {
        seq,
        created_at,
        expires_at,
        recalled,
    })
}

#[derive(FromRow)]
struct Checkpoint {
    cursor: Uuid,
    acked_after: i64,
    page_id: Option<Uuid>,
    page_next: Option<i64>,
    page_offered: Option<Vec<i64>>,
    page_more: Option<bool>,
    committed_through: i64,
}

async fn checkpoint(db: &mut PgConnection, auth: &Auth) -> Result<Checkpoint> {
    auth.require(db).await?;
    Ok(sqlx::query_as(
        "SELECT d.cursor, d.acked_after, d.page_id, d.page_next, d.page_offered, d.page_more, f.committed_through
         FROM m1_devices d JOIN m1_feed f ON f.epoch = d.epoch WHERE d.id = $1 FOR UPDATE OF d",
    ).bind(auth.device_id).fetch_one(db).await?)
}

pub(crate) async fn page(
    State(app): State<App>,
    auth: Auth,
    Query(input): Query<SyncPosition>,
) -> Result<Json<SyncPage>> {
    let mut tx = app.db.begin().await?;
    chats::lock_message_feed(&mut tx).await?;
    let current = checkpoint(&mut tx, &auth).await?;
    if input.cursor != current.cursor {
        return Err(Error::conflict(
            "This cursor is not the confirmed position for this device.",
        ));
    }
    // Keep new pages below the clients' 16 MiB JSON limit even with six-byte escapes.
    const PAGE_EVENTS: usize = 32;
    let (page_id, next, more) = match (current.page_id, current.page_next, current.page_more) {
        (Some(id), Some(next), Some(more)) => (id, next, more),
        _ => {
            let scanned: Vec<i64> = sqlx::query_scalar(
                "SELECT event_seq FROM m1_device_events WHERE device_id = $1 AND event_seq > $2
                   AND event_seq <= $3 AND metadata_state = 'pending' ORDER BY event_seq LIMIT $4",
            )
            .bind(auth.device_id)
            .bind(current.acked_after)
            .bind(current.committed_through)
            .bind(PAGE_EVENTS as i64 + 1)
            .fetch_all(&mut *tx)
            .await?;
            let more = scanned.len() > PAGE_EVENTS;
            let next = if more {
                scanned[PAGE_EVENTS - 1]
            } else {
                current.committed_through
            };
            (Uuid::new_v4(), next, more)
        }
    };
    let events = events(
        &mut tx,
        &auth,
        current.acked_after,
        next,
        current.page_offered.as_deref(),
    )
    .await?;
    if current.page_id.is_none() {
        let offered = events
            .iter()
            .filter_map(|event| match event {
                SyncEvent::Published(m) if !m.recalled && m.expired.is_none() => Some(m.seq),
                _ => None,
            })
            .collect::<Vec<_>>();
        sqlx::query("UPDATE m1_devices SET page_id = $2, page_next = $3, page_offered = $4, page_more = $5 WHERE id = $1")
            .bind(auth.device_id).bind(page_id).bind(next).bind(offered).bind(more).execute(&mut *tx).await?;
    }
    let directory = chats::directory(&mut tx, auth.user_id).await?;
    let server_time =
        sqlx::query_scalar("SELECT (extract(epoch FROM clock_timestamp()) * 1000)::BIGINT")
            .fetch_one(&mut *tx)
            .await?;
    tx.commit().await?;
    Ok(Json(SyncPage {
        directory,
        events,
        server_time,
        page_id,
        has_more: more,
    }))
}

async fn events(
    db: &mut PgConnection,
    auth: &Auth,
    after: i64,
    next: i64,
    offered: Option<&[i64]>,
) -> Result<Vec<SyncEvent>> {
    let rows: Vec<(i64, i64, Option<DbJson<Message>>)> = sqlx::query_as(
        "SELECT e.event_seq, m.seq,
           CASE WHEN e.event_seq = m.seq THEN jsonb_build_object(
               'seq', m.seq, 'conversation_id', m.conversation_id,
               'sender', jsonb_build_object('id', a.id, 'handle', a.handle),
               'created_at', m.created_at, 'recalled', m.recall_seq IS NOT NULL,
               'nonce', CASE WHEN m.sender_id = $2 THEN coalesce(m.nonce, m.attachment_id) END,
               'expires_at', (extract(epoch FROM m.expires_at) * 1000)::BIGINT,
               'ai_provider', m.ai_provider,
               'reply_to', CASE WHEN live.payload THEN quote.seq END,
               'text', CASE WHEN live.payload THEN m.text END,
               'attachment', CASE WHEN live.payload AND f.id IS NOT NULL THEN jsonb_build_object(
                   'id', f.id, 'name', f.name, 'size', f.size, 'sha256', f.sha256, 'kind', f.kind
               ) END,
               'expired', CASE WHEN m.recall_seq IS NULL AND NOT live.payload
                   THEN CASE WHEN m.attachment_id IS NULL THEN 'text' ELSE 'file' END END
           ) END
         FROM m1_device_events e JOIN m1_visible_messages m ON m.seq = e.message_seq AND m.account_id = $2
         JOIN accounts a ON a.id = m.sender_id
         LEFT JOIN attachments f ON f.id = m.attachment_id
         LEFT JOIN m1_visible_messages quote ON quote.seq = m.reply_to AND quote.account_id = $2
         CROSS JOIN LATERAL (
             SELECT m.recall_seq IS NULL AND coalesce(m.expires_at > statement_timestamp(), true)
                  AND ($5::bigint[] IS NULL OR m.seq = ANY($5))
                  AND (m.text IS NOT NULL OR f.id IS NOT NULL) AS payload
         ) live
         WHERE e.device_id = $1 AND e.event_seq > $3 AND e.event_seq <= $4
           AND e.metadata_state = 'pending'
         ORDER BY e.event_seq",
    ).bind(auth.device_id).bind(auth.user_id).bind(after).bind(next).bind(offered).fetch_all(db).await?;
    Ok(rows
        .into_iter()
        .map(|(seq, message_seq, payload)| match payload {
            Some(message) => SyncEvent::Published(message.0),
            None => SyncEvent::Recalled(MessageRecall { seq, message_seq }),
        })
        .collect())
}

pub(crate) async fn ack(
    State(app): State<App>,
    auth: Auth,
    Json(input): Json<SyncAck>,
) -> Result<Json<SyncPosition>> {
    if input.metadata_received.len() > SYNC_PAGE_SIZE {
        return Err(Error::bad("Invalid metadata confirmation."));
    }
    let mut tx = app.db.begin().await?;
    chats::lock_message_feed(&mut tx).await?;
    let current = checkpoint(&mut tx, &auth).await?;
    if input.page_id == current.cursor && current.page_id != Some(input.page_id) {
        return Ok(Json(SyncPosition {
            cursor: current.cursor,
        }));
    }
    let (Some(page), Some(next), Some(offered)) =
        (current.page_id, current.page_next, current.page_offered)
    else {
        return Err(Error::conflict("This device has no pending sync page."));
    };
    if input.page_id != page {
        return Err(Error::conflict("This page is not pending for this device."));
    }
    if input
        .metadata_received
        .iter()
        .any(|seq| !offered.contains(seq))
    {
        return Err(Error::bad("The sync page did not offer this message."));
    }
    let required: Vec<i64> = sqlx::query_scalar(
        "SELECT seq FROM m1_visible_messages WHERE account_id = $1 AND seq = ANY($2)
           AND recall_seq IS NULL AND coalesce(expires_at > statement_timestamp(), true)",
    )
    .bind(auth.user_id)
    .bind(&offered)
    .fetch_all(&mut *tx)
    .await?;
    if required
        .iter()
        .any(|seq| !input.metadata_received.contains(seq))
    {
        return Err(Error::bad(
            "Confirm all available messages after saving the page.",
        ));
    }
    sqlx::query(
        "WITH confirmed AS (
             UPDATE m1_device_events SET metadata_state = CASE WHEN event_seq = ANY($4)
                  THEN 'received' ELSE 'processed' END
             WHERE device_id = $1 AND event_seq > $2 AND event_seq <= $3 AND metadata_state = 'pending'
             RETURNING message_seq, metadata_state
         )
         UPDATE m1_recipients r SET metadata_received = true FROM confirmed c
         WHERE r.account_id = $5 AND r.message_seq = c.message_seq AND c.metadata_state = 'received'",
    ).bind(auth.device_id).bind(current.acked_after).bind(next)
        .bind(&required).bind(auth.user_id).execute(&mut *tx).await?;
    sqlx::query(
        "UPDATE m1_devices SET cursor = $2, acked_after = $3, page_id = NULL, page_next = NULL,
            page_offered = NULL, page_more = NULL, last_seen_at = clock_timestamp(),
            active_until = clock_timestamp() + make_interval(secs => $4::int)
         WHERE id = $1",
    )
    .bind(auth.device_id)
    .bind(page)
    .bind(next)
    .bind(app.retention.device_idle_secs)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(SyncPosition { cursor: page }))
}

pub(crate) async fn history_status(
    State(app): State<App>,
    auth: Auth,
    Json(known): Json<Vec<i64>>,
) -> Result<Json<Vec<MessageRecall>>> {
    if known.len() > SYNC_PAGE_SIZE || known.iter().any(|seq| *seq <= 0) {
        return Err(Error::bad("Invalid local history range."));
    }
    let recalls: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT recall_seq, seq FROM m1_visible_messages
         WHERE account_id = $1 AND seq = ANY($2) AND recall_seq IS NOT NULL ORDER BY recall_seq",
    )
    .bind(auth.user_id)
    .bind(known)
    .fetch_all(&app.db)
    .await?;
    Ok(Json(
        recalls
            .into_iter()
            .map(|(seq, message_seq)| MessageRecall { seq, message_seq })
            .collect(),
    ))
}
