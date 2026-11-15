use crate::{App, Auth, Error, Result};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::Deserialize;
use sqlx::{FromRow, PgConnection, types::Json as DbJson};
use uuid::Uuid;
use zibblu_core::{
    AiProvider, Attachment, Conversation, ConversationCreated, ConversationRequest, DeliveryAck,
    Directory, MAX_AI_REPLY_BYTES, MAX_GROUP_MEMBERS, MAX_MESSAGE_BYTES,
    MAX_PENDING_DELIVERY_BATCHES, MAX_RECEIPT_QUERY, Message, MessageRecall, MessageReceipt,
    PendingContact, PublishAiReply, RECALL_WINDOW_MS, ReadPreference, ReadResult, ReadUpdate,
    ReceiptStatus, SYNC_PAGE_SIZE, SendMessage, SyncPage, User,
};

#[derive(Deserialize)]
pub(crate) struct Cursor {
    after: i64,
    recall_after: Option<i64>,
    #[serde(default)]
    delivery_acks: bool,
}

#[derive(FromRow)]
struct ChatRow {
    id: Uuid,
    title: Option<String>,
    members: DbJson<Vec<User>>,
    activity_at: i64,
    preview: Option<String>,
    preview_seq: Option<i64>,
    latest_seq: i64,
    unread: i64,
    read_through: i64,
}

#[derive(FromRow)]
struct MessageRow {
    seq: i64,
    conversation_id: Uuid,
    sender_id: Uuid,
    handle: String,
    created_at: i64,
    text: Option<String>,
    attachment: Option<DbJson<Attachment>>,
    ai_provider: Option<DbJson<AiProvider>>,
    recalled: bool,
    reply_to: Option<i64>,
}

pub(crate) async fn sync(
    State(app): State<App>,
    auth: Auth,
    Query(cursor): Query<Cursor>,
) -> Result<Json<SyncPage>> {
    if cursor.after < 0 || cursor.recall_after.is_some_and(|after| after < 0) {
        return Err(Error::bad("Invalid message cursor."));
    }
    let mut tx = app.db.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *tx)
        .await?;
    let contacts = sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT a.id, a.handle, c.remark FROM contacts c JOIN accounts a ON a.id = c.contact_id
         WHERE c.owner_id = $1 ORDER BY a.handle",
    )
    .bind(auth.user_id)
    .fetch_all(&mut *tx)
    .await?;
    let pending = sqlx::query_as::<_, (Uuid, String, bool, Uuid, String)>(
        "SELECT a.id, a.handle, r.recipient_id = $1 AS incoming, r.request_id, r.greeting
         FROM contact_requests r
         JOIN accounts a ON a.id = CASE WHEN r.recipient_id = $1 THEN r.requester_id ELSE r.recipient_id END
         WHERE r.requester_id = $1 OR r.recipient_id = $1 ORDER BY a.handle",
    )
    .bind(auth.user_id)
    .fetch_all(&mut *tx)
    .await?;
    let conversations = sqlx::query_as::<_, ChatRow>(
        "SELECT c.id, c.title, roster.members, coalesce(latest.created_at, c.created_at) AS activity_at,
                latest.preview, latest.seq AS preview_seq, coalesce(newest.seq, 0) AS latest_seq,
                stats.unread, own.read_through
         FROM members own JOIN conversations c ON c.id = own.conversation_id
         JOIN LATERAL (
             SELECT jsonb_agg(jsonb_build_object('id', a.id, 'handle', a.handle) ORDER BY a.handle) AS members
             FROM members m JOIN accounts a ON a.id = m.account_id WHERE m.conversation_id = c.id
         ) roster ON true
         LEFT JOIN LATERAL (
             SELECT m.seq, m.created_at, CASE WHEN m.recall_seq IS NOT NULL THEN 'Message recalled'
                    ELSE coalesce(left(m.text, 100), f.name) END AS preview
             FROM messages m LEFT JOIN attachments f ON f.id = m.attachment_id
             WHERE m.conversation_id = c.id AND m.seq > own.joined_after
               AND (m.ai_provider IS NULL OR EXISTS (
                   SELECT 1 FROM ai_reply_readers r WHERE r.message_seq = m.seq AND r.account_id = $1
               ))
             ORDER BY m.created_at DESC, m.seq DESC LIMIT 1
         ) latest ON true
         JOIN LATERAL (
             SELECT count(*) FILTER (WHERE m.sender_id <> $1 AND m.recall_seq IS NULL) AS unread
             FROM messages m
             WHERE m.conversation_id = c.id AND m.seq > greatest(own.joined_after, own.read_through)
               AND (m.ai_provider IS NULL OR EXISTS (
                   SELECT 1 FROM ai_reply_readers r WHERE r.message_seq = m.seq AND r.account_id = $1
               ))
         ) stats ON true
         LEFT JOIN LATERAL (
             SELECT m.seq FROM messages m WHERE m.conversation_id = c.id AND m.seq > own.joined_after
               AND (m.ai_provider IS NULL OR EXISTS (
                   SELECT 1 FROM ai_reply_readers r WHERE r.message_seq = m.seq AND r.account_id = $1
               )) ORDER BY m.seq DESC LIMIT 1
         ) newest ON true
         WHERE own.account_id = $1 ORDER BY activity_at DESC, c.id",
    ).bind(auth.user_id).fetch_all(&mut *tx).await?;
    let read_receipts = sqlx::query_scalar("SELECT read_receipts FROM accounts WHERE id = $1")
        .bind(auth.user_id)
        .fetch_one(&mut *tx)
        .await?;
    let mut people = vec![auth.user_id];
    people.extend(contacts.iter().map(|(id, ..)| *id));
    people.extend(pending.iter().map(|(id, ..)| *id));
    people.extend(
        conversations
            .iter()
            .flat_map(|row| row.members.0.iter().map(|user| user.id)),
    );
    people.sort_unstable();
    people.dedup();
    let avatars = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT account_id, version FROM profile_images WHERE kind = 'avatar' AND account_id = ANY($1)",
    ).bind(&people).fetch_all(&mut *tx).await?;
    let messages = sqlx::query_as::<_, MessageRow>(
        "SELECT m.seq, m.conversation_id, m.sender_id, a.handle, m.created_at,
                CASE WHEN m.recall_seq IS NULL THEN m.text END AS text,
                to_jsonb(m.ai_provider) AS ai_provider, m.recall_seq IS NOT NULL AS recalled,
                CASE WHEN m.recall_seq IS NULL THEN quote.seq END AS reply_to,
                CASE WHEN f.id IS NOT NULL AND m.recall_seq IS NULL THEN jsonb_build_object(
                    'id', f.id, 'name', f.name, 'size', f.size, 'sha256', f.sha256, 'kind', f.kind
                ) END AS attachment
         FROM messages m JOIN accounts a ON a.id = m.sender_id
         JOIN members own ON own.conversation_id = m.conversation_id AND own.account_id = $1
         LEFT JOIN attachments f ON f.id = m.attachment_id
         LEFT JOIN messages quote ON quote.seq = m.reply_to AND quote.conversation_id = m.conversation_id
           AND quote.seq > own.joined_after
           AND (quote.ai_provider IS NULL OR EXISTS (
               SELECT 1 FROM ai_reply_readers r WHERE r.message_seq = quote.seq AND r.account_id = $1
           ))
         WHERE m.seq > $2 AND m.seq > own.joined_after
           AND (m.ai_provider IS NULL OR EXISTS (
               SELECT 1 FROM ai_reply_readers r WHERE r.message_seq = m.seq AND r.account_id = $1
           ))
         ORDER BY m.seq LIMIT $3",
    )
    .bind(auth.user_id)
    .bind(cursor.after)
    .bind(SYNC_PAGE_SIZE as i64)
    .fetch_all(&mut *tx)
    .await?;
    let recalls = if let Some(after) = cursor.recall_after {
        sqlx::query_as::<_, (i64, i64)>(
            "SELECT m.recall_seq, m.seq FROM messages m
             JOIN members own ON own.conversation_id = m.conversation_id AND own.account_id = $1
             WHERE m.recall_seq > $2 AND m.seq > own.joined_after
             ORDER BY m.recall_seq LIMIT $3",
        )
        .bind(auth.user_id)
        .bind(after)
        .bind(SYNC_PAGE_SIZE as i64)
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .map(|(seq, message_seq)| MessageRecall { seq, message_seq })
        .collect()
    } else {
        Vec::new()
    };
    let server_time =
        sqlx::query_scalar("SELECT (extract(epoch FROM clock_timestamp()) * 1000)::BIGINT")
            .fetch_one(&mut *tx)
            .await?;
    tx.commit().await?;
    let delivery_batch = if cursor.delivery_acks {
        let sequences = messages
            .iter()
            .filter(|message| {
                message.sender_id != auth.user_id
                    && !message.recalled
                    && message.ai_provider.is_none()
            })
            .map(|message| message.seq)
            .collect::<Vec<_>>();
        if sequences.is_empty() {
            None
        } else {
            Some(offer_delivery(&app, &auth, cursor.after, &sequences).await?)
        }
    } else {
        None
    };
    Ok(Json(SyncPage {
        directory: Directory {
            contacts: contacts
                .iter()
                .map(|(id, handle, _)| User {
                    id: *id,
                    handle: handle.clone(),
                })
                .collect(),
            remarks: contacts
                .into_iter()
                .filter_map(|(id, _, remark)| (!remark.is_empty()).then_some((id, remark)))
                .collect(),
            avatars: avatars.into_iter().collect(),
            read_receipts,
            pending_contacts: pending
                .into_iter()
                .map(
                    |(id, handle, incoming, request_id, greeting)| PendingContact {
                        user: User { id, handle },
                        incoming,
                        request_id: Some(request_id),
                        greeting,
                    },
                )
                .collect(),
            conversations: conversations
                .into_iter()
                .map(|row| Conversation {
                    id: row.id,
                    title: row.title,
                    members: row.members.0,
                    activity_at: row.activity_at,
                    preview: row.preview,
                    preview_seq: row.preview_seq,
                    latest_seq: row.latest_seq,
                    unread: row.unread,
                    read_through: row.read_through,
                })
                .collect(),
        },
        messages: messages
            .into_iter()
            .map(|row| Message {
                seq: row.seq,
                conversation_id: row.conversation_id,
                sender: User {
                    id: row.sender_id,
                    handle: row.handle,
                },
                created_at: row.created_at,
                text: row.text,
                attachment: row.attachment.map(|value| value.0),
                ai_provider: row.ai_provider.map(|value| value.0),
                recalled: row.recalled,
                reply_to: row.reply_to,
            })
            .collect(),
        recalls,
        server_time: Some(server_time),
        delivery_batch,
    }))
}

async fn offer_delivery(app: &App, auth: &Auth, after: i64, sequences: &[i64]) -> Result<Uuid> {
    let batch = Uuid::new_v4();
    let mut tx = app.db.begin().await?;
    sqlx::query("DELETE FROM pending_delivery_batches WHERE expires_at <= now()")
        .execute(&mut *tx)
        .await?;
    sqlx::query_scalar::<_, i32>(
        "SELECT 1 FROM sessions WHERE token_hash = $1 AND expires_at > now() FOR UPDATE",
    )
    .bind(&auth.token_hash)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(Error::unauthorized)?;
    sqlx::query(
        "INSERT INTO pending_delivery_batches (session_hash, served_after, id, recipient_id, sequences)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (session_hash, served_after) DO UPDATE SET id = excluded.id, sequences = excluded.sequences,
             expires_at = excluded.expires_at",
    ).bind(&auth.token_hash).bind(after).bind(batch).bind(auth.user_id).bind(sequences)
        .execute(&mut *tx).await?;
    sqlx::query(
        "DELETE FROM pending_delivery_batches WHERE session_hash = $1 AND id IN (
            SELECT id FROM pending_delivery_batches WHERE session_hash = $1
            ORDER BY expires_at DESC, served_after DESC OFFSET $2
         )",
    )
    .bind(&auth.token_hash)
    .bind(MAX_PENDING_DELIVERY_BATCHES as i64)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(batch)
}

pub(crate) async fn ack_delivery(
    State(app): State<App>,
    auth: Auth,
    Json(ack): Json<DeliveryAck>,
) -> Result<StatusCode> {
    if ack.batches.is_empty() || ack.batches.len() > MAX_PENDING_DELIVERY_BATCHES {
        return Err(Error::bad("Invalid delivery confirmation."));
    }
    sqlx::query(
        "WITH acknowledged AS (
           DELETE FROM pending_delivery_batches WHERE id = ANY($2) AND recipient_id = $1 AND expires_at > now()
           RETURNING sequences
         ), delivered AS (
           SELECT m.conversation_id, max(m.seq) AS seq
           FROM acknowledged batch CROSS JOIN LATERAL unnest(batch.sequences) AS msg(seq)
           JOIN messages m ON m.seq = msg.seq
           WHERE m.sender_id <> $1 AND m.recall_seq IS NULL AND m.ai_provider IS NULL
           GROUP BY m.conversation_id
         )
         UPDATE members own SET delivered_through = delivered.seq FROM delivered
         WHERE own.account_id = $1 AND own.conversation_id = delivered.conversation_id
           AND delivered.seq > greatest(own.delivered_through, own.joined_after)",
    ).bind(auth.user_id).bind(ack.batches).execute(&app.db).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn create(
    State(app): State<App>,
    auth: Auth,
    Json(input): Json<ConversationRequest>,
) -> Result<Json<ConversationCreated>> {
    let (title, peers) = match input {
        ConversationRequest::Direct { contact_id } => (None, vec![contact_id]),
        ConversationRequest::Group {
            title,
            mut contact_ids,
        } => {
            let title = title.trim().to_string();
            if contact_ids.len() >= MAX_GROUP_MEMBERS
                || title.is_empty()
                || title.chars().count() > 64
                || title.chars().any(char::is_control)
            {
                return Err(Error::bad(
                    "Choose a group name up to 64 characters and up to 15 contacts.",
                ));
            }
            contact_ids.sort_unstable();
            contact_ids.dedup();
            if contact_ids.len() < 2 {
                return Err(Error::bad("Choose at least two contacts."));
            }
            (Some(title), contact_ids)
        }
    };
    let mut tx = app.db.begin().await?;
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM contacts c WHERE c.owner_id = $1 AND c.contact_id = ANY($2)
           AND EXISTS (SELECT 1 FROM contacts peer WHERE peer.owner_id = c.contact_id AND peer.contact_id = $1)",
    )
    .bind(auth.user_id)
    .bind(&peers)
    .fetch_one(&mut *tx)
    .await?;
    if count != peers.len() as i64 {
        return Err(Error::bad(
            "Participants must approve your contact request first.",
        ));
    }
    let id = insert_conversation(&mut tx, auth.user_id, title, peers).await?;
    tx.commit().await?;
    Ok(Json(ConversationCreated { id }))
}

pub(crate) async fn insert_conversation(
    db: &mut PgConnection,
    account: Uuid,
    title: Option<String>,
    mut peers: Vec<Uuid>,
) -> Result<Uuid> {
    let direct_key = title.is_none().then(|| {
        let mut pair = [account, peers[0]];
        pair.sort_unstable();
        format!("{}:{}", pair[0], pair[1])
    });
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO conversations (id, title, direct_key) VALUES ($1,$2,$3)
         ON CONFLICT (direct_key) DO UPDATE SET direct_key = excluded.direct_key RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(title)
    .bind(direct_key)
    .fetch_one(&mut *db)
    .await?;
    peers.push(account);
    sqlx::query(
        "INSERT INTO members (conversation_id, account_id)
         SELECT $1, unnest($2::uuid[]) ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(&peers)
    .execute(db)
    .await?;
    Ok(id)
}

pub(crate) async fn require_send_access(
    db: &mut PgConnection,
    conversation_id: Uuid,
    account_id: Uuid,
) -> Result<()> {
    let allowed: Option<bool> = sqlx::query_scalar(
        "SELECT c.direct_key IS NULL OR EXISTS (
            SELECT 1 FROM contacts contact
            JOIN members peer ON peer.conversation_id = c.id AND peer.account_id = contact.contact_id
            JOIN contacts approval ON approval.owner_id = peer.account_id AND approval.contact_id = $2
            WHERE contact.owner_id = $2
         )
         FROM conversations c JOIN members own ON own.conversation_id = c.id
         WHERE c.id = $1 AND own.account_id = $2",
    )
    .bind(conversation_id).bind(account_id).fetch_optional(db).await?;
    match allowed {
        Some(true) => Ok(()),
        Some(false) => Err(Error::conflict(
            "An approved contact is required to send to this chat.",
        )),
        None => Err(Error::missing()),
    }
}

pub(crate) async fn lock_message_feed(db: &mut PgConnection) -> Result<()> {
    // Sequence values must become visible in order for the client's durable cursor.
    sqlx::query("SELECT pg_advisory_xact_lock(1001)")
        .execute(db)
        .await?;
    Ok(())
}

pub(crate) async fn send(
    State(app): State<App>,
    auth: Auth,
    Path(id): Path<Uuid>,
    Json(input): Json<SendMessage>,
) -> Result<StatusCode> {
    let text = input.text.trim();
    if text.is_empty() || text.len() > MAX_MESSAGE_BYTES || text.contains('\0') {
        return Err(Error::bad("Enter a message up to 8 KiB."));
    }
    let mut tx = app.db.begin().await?;
    lock_message_feed(&mut tx).await?;
    require_send_access(&mut tx, id, auth.user_id).await?;
    let existing: Option<bool> = sqlx::query_scalar(
        "SELECT conversation_id = $3 AND ai_provider IS NULL AND (text = $4 OR recall_seq IS NOT NULL)
                AND reply_to IS NOT DISTINCT FROM $5
         FROM messages WHERE sender_id = $1 AND nonce = $2",
    ).bind(auth.user_id).bind(input.nonce).bind(id).bind(text).bind(input.reply_to)
    .fetch_optional(&mut *tx).await?;
    if let Some(same) = existing {
        return if same {
            Ok(StatusCode::NO_CONTENT)
        } else {
            Err(Error::conflict(
                "This message ID was used for a different message.",
            ))
        };
    }
    if let Some(source) = input.reply_to {
        let visible: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM messages m JOIN members own ON own.conversation_id = m.conversation_id
              AND own.account_id = $3 WHERE m.seq = $1 AND m.conversation_id = $2 AND m.seq > own.joined_after
              AND m.recall_seq IS NULL AND (m.ai_provider IS NULL OR EXISTS (
                 SELECT 1 FROM ai_reply_readers WHERE message_seq = m.seq AND account_id = $3)))",
        ).bind(source).bind(id).bind(auth.user_id).fetch_one(&mut *tx).await?;
        if !visible {
            return Err(Error::missing());
        }
    }
    sqlx::query(
        "INSERT INTO messages (nonce, conversation_id, sender_id, text, reply_to) VALUES ($1,$2,$3,$4,$5)",
    )
    .bind(input.nonce)
    .bind(id)
    .bind(auth.user_id)
    .bind(text)
    .bind(input.reply_to)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn set_read_receipts(
    State(app): State<App>,
    auth: Auth,
    Json(input): Json<ReadPreference>,
) -> Result<StatusCode> {
    let mut tx = app.db.begin().await?;
    lock_message_feed(&mut tx).await?;
    sqlx::query(
        "UPDATE accounts SET read_receipts = $2, receipts_after = coalesce((SELECT max(seq) FROM messages), 0)
         WHERE id = $1 AND read_receipts <> $2",
    )
        .bind(auth.user_id)
        .bind(input.enabled)
        .execute(&mut *tx)
        .await?;
    if !input.enabled {
        sqlx::query("DELETE FROM message_receipts WHERE account_id = $1")
            .bind(auth.user_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(crate) struct ReceiptFormat {
    #[serde(default)]
    latest: bool,
}

pub(crate) async fn read(
    State(app): State<App>,
    auth: Auth,
    Path(id): Path<Uuid>,
    Query(format): Query<ReceiptFormat>,
    Json(input): Json<ReadUpdate>,
) -> Result<Json<ReadResult>> {
    if input.sent.len() > MAX_RECEIPT_QUERY
        || [input.through, input.viewed]
            .into_iter()
            .flatten()
            .any(|seq| seq <= 0)
    {
        return Err(Error::bad("Invalid read position or receipt query."));
    }
    let mut tx = app.db.begin().await?;
    let enabled: bool =
        sqlx::query_scalar("SELECT read_receipts FROM accounts WHERE id = $1 FOR SHARE")
            .bind(auth.user_id)
            .fetch_one(&mut *tx)
            .await?;
    if input.through.is_none() {
        let member: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM members WHERE conversation_id = $1 AND account_id = $2)",
        )
        .bind(id)
        .bind(auth.user_id)
        .fetch_one(&mut *tx)
        .await?;
        if !member {
            return Err(Error::missing());
        }
    }
    if let Some(through) = input.through {
        let (joined_after, read_through): (i64, i64) = sqlx::query_as(
        "SELECT joined_after, read_through FROM members WHERE conversation_id = $1 AND account_id = $2 FOR UPDATE",
        ).bind(id).bind(auth.user_id).fetch_optional(&mut *tx).await?.ok_or_else(Error::missing)?;
        if through > read_through {
            let visible: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM messages m WHERE m.seq = $1 AND m.conversation_id = $2 AND m.seq > $4
                  AND (m.ai_provider IS NULL OR EXISTS (
                      SELECT 1 FROM ai_reply_readers WHERE message_seq = m.seq AND account_id = $3)))",
        ).bind(through).bind(id).bind(auth.user_id).bind(joined_after).fetch_one(&mut *tx).await?;
            if !visible {
                return Err(Error::missing());
            }
            sqlx::query("UPDATE members SET read_through = $3 WHERE conversation_id = $1 AND account_id = $2")
            .bind(id).bind(auth.user_id).bind(through).execute(&mut *tx).await?;
        }
    }
    if enabled
        && format.latest
        && let Some(viewed) = input.viewed
    {
        sqlx::query(
            "UPDATE members viewer SET viewed_through = $3
             FROM messages m JOIN conversations c ON c.id = m.conversation_id
             JOIN accounts sender ON sender.id = m.sender_id
             JOIN accounts reader ON reader.id = $2
             WHERE viewer.conversation_id = $1 AND viewer.account_id = $2
               AND m.seq = $3 AND m.conversation_id = $1 AND m.sender_id <> $2
               AND c.direct_key IS NOT NULL AND m.ai_provider IS NULL AND m.recall_seq IS NULL
               AND sender.read_receipts AND m.seq > greatest(sender.receipts_after, reader.receipts_after,
                                                            viewer.joined_after, viewer.viewed_through)
               AND viewer.delivered_through >= m.seq",
        ).bind(id).bind(auth.user_id).bind(viewed).execute(&mut *tx).await?;
    }
    let receipt = if enabled && !input.sent.is_empty() {
        sqlx::query_as::<_, (i64, bool)>(
            "SELECT latest.seq, peer.viewed_through >= latest.seq FROM members own
             JOIN accounts sender ON sender.id = own.account_id
             JOIN conversations c ON c.id = own.conversation_id AND c.direct_key IS NOT NULL
             JOIN members peer ON peer.conversation_id = c.id AND peer.account_id <> own.account_id
             JOIN accounts reader ON reader.id = peer.account_id
             JOIN LATERAL (
                SELECT m.seq, m.sender_id, m.recall_seq, m.ai_provider FROM messages m
                WHERE m.conversation_id = c.id AND m.seq > own.joined_after
                  AND (m.ai_provider IS NULL OR EXISTS (
                    SELECT 1 FROM ai_reply_readers WHERE message_seq = m.seq AND account_id = own.account_id))
                ORDER BY m.created_at DESC, m.seq DESC LIMIT 1
             ) latest ON true
             WHERE c.id = $1 AND own.account_id = $2 AND latest.seq = ANY($3)
               AND latest.sender_id = own.account_id AND latest.recall_seq IS NULL AND latest.ai_provider IS NULL
               AND reader.read_receipts AND peer.delivered_through >= latest.seq
               AND latest.seq > greatest(peer.joined_after, sender.receipts_after, reader.receipts_after)",
        )
        .bind(id)
        .bind(auth.user_id)
        .bind(&input.sent)
        .fetch_optional(&mut *tx)
        .await?
        .map(|(seq, viewed)| MessageReceipt { seq, status: if viewed { ReceiptStatus::Read } else { ReceiptStatus::Delivered } })
    } else {
        None
    };
    tx.commit().await?;
    Ok(Json(if format.latest {
        ReadResult::Latest { receipt }
    } else {
        ReadResult::Legacy(
            receipt
                .filter(|value| value.status == ReceiptStatus::Read)
                .map(|value| value.seq)
                .into_iter()
                .collect(),
        )
    }))
}

pub(crate) async fn recall(
    State(app): State<App>,
    auth: Auth,
    Path((id, seq)): Path<(Uuid, i64)>,
) -> Result<StatusCode> {
    let mut tx = app.db.begin().await?;
    lock_message_feed(&mut tx).await?;
    let (already, in_time): (bool, bool) = sqlx::query_as(
        "SELECT recall_seq IS NOT NULL,
                created_at + $4 >= (extract(epoch FROM clock_timestamp()) * 1000)::BIGINT
         FROM messages WHERE seq = $1 AND conversation_id = $2 AND sender_id = $3 AND ai_provider IS NULL",
    )
    .bind(seq).bind(id).bind(auth.user_id).bind(RECALL_WINDOW_MS)
    .fetch_optional(&mut *tx).await?.ok_or_else(Error::missing)?;
    if already {
        return Ok(StatusCode::NO_CONTENT);
    }
    if !in_time {
        return Err(Error::conflict(
            "Messages can only be recalled within five minutes.",
        ));
    }
    sqlx::query(
        "UPDATE messages SET recall_seq = nextval(pg_get_serial_sequence('messages', 'seq')),
             text = CASE WHEN text IS NOT NULL THEN '' END WHERE seq = $1",
    )
    .bind(seq)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn publish_ai_reply(
    State(app): State<App>,
    auth: Auth,
    Path(id): Path<Uuid>,
    Json(input): Json<PublishAiReply>,
) -> Result<StatusCode> {
    let text = input.text.trim();
    if text.is_empty()
        || text.len() > MAX_AI_REPLY_BYTES
        || text.contains('\0')
        || input.readers.is_empty()
        || input.readers.len() >= MAX_GROUP_MEMBERS
        || input.readers.contains(&auth.user_id)
    {
        return Err(Error::bad(
            "Choose chat members and an AI reply up to 24 KiB.",
        ));
    }
    let mut readers = input.readers;
    readers.push(auth.user_id);
    let mut tx = app.db.begin().await?;
    lock_message_feed(&mut tx).await?;
    require_send_access(&mut tx, id, auth.user_id).await?;
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM members WHERE conversation_id = $1 AND account_id = ANY($2)",
    )
    .bind(id)
    .bind(&readers)
    .fetch_one(&mut *tx)
    .await?;
    if count != readers.len() as i64 {
        return Err(Error::bad(
            "Reply recipients must be distinct members of this chat.",
        ));
    }
    let seq: Option<i64> = sqlx::query_scalar(
        "INSERT INTO messages (nonce, conversation_id, sender_id, text, ai_provider, created_at)
         SELECT $1, id, $3, $4, $5,
             least(greatest($6, created_at), (extract(epoch FROM clock_timestamp()) * 1000)::BIGINT)
         FROM conversations WHERE id = $2
         ON CONFLICT (sender_id, nonce) DO NOTHING RETURNING seq",
    )
    .bind(input.nonce)
    .bind(id)
    .bind(auth.user_id)
    .bind(text)
    .bind(input.provider.as_str())
    .bind(input.created_at)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(seq) = seq {
        sqlx::query(
            "INSERT INTO ai_reply_readers (message_seq, account_id) SELECT $1, unnest($2::uuid[])",
        )
        .bind(seq)
        .bind(&readers)
        .execute(&mut *tx)
        .await?;
    } else {
        let same: bool = sqlx::query_scalar(
            "SELECT coalesce(m.conversation_id = $3 AND m.text = $4 AND m.ai_provider = $5, false)
                  AND (SELECT array_agg(account_id ORDER BY account_id) FROM ai_reply_readers WHERE message_seq = m.seq)
                    IS NOT DISTINCT FROM ARRAY(SELECT unnest($6::uuid[]) ORDER BY 1)
             FROM messages m WHERE m.sender_id = $1 AND m.nonce = $2",
        )
        .bind(auth.user_id).bind(input.nonce).bind(id).bind(text).bind(input.provider.as_str()).bind(&readers)
        .fetch_one(&mut *tx).await?;
        if !same {
            return Err(Error::conflict(
                "This message ID was used for a different reply or audience.",
            ));
        }
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
