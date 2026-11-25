use crate::{App, Auth, Error, Result, sync};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::Deserialize;
use sqlx::{FromRow, PgConnection, types::Json as DbJson};
use uuid::Uuid;
use zibblu_core::{
    Conversation, ConversationCreated, ConversationRequest, Directory, MAX_AI_REPLY_BYTES,
    MAX_GROUP_MEMBERS, MAX_MESSAGE_BYTES, MAX_RECEIPT_QUERY, MessageReceipt, PendingContact,
    Publication, PublishAiReply, RECALL_WINDOW_MS, ReadPreference, ReadResult, ReadUpdate,
    ReceiptStatus, SendMessage, User,
};

#[derive(FromRow)]
struct ChatRow {
    id: Uuid,
    title: Option<String>,
    members: DbJson<Vec<User>>,
    created_at: i64,
    joined_after: i64,
}

pub(crate) async fn directory(db: &mut PgConnection, account: Uuid) -> Result<Directory> {
    let contacts = sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT a.id, a.handle, c.remark FROM contacts c JOIN accounts a ON a.id = c.contact_id
         WHERE c.owner_id = $1 AND a.revoked_at IS NULL ORDER BY a.handle",
    )
    .bind(account)
    .fetch_all(&mut *db)
    .await?;
    let pending = sqlx::query_as::<_, (Uuid, String, bool, Uuid, String)>(
        "SELECT a.id, a.handle, r.recipient_id = $1 AS incoming, r.request_id,
                CASE WHEN r.greeting_expires_at > statement_timestamp() THEN r.greeting ELSE '' END
         FROM contact_requests r
         JOIN accounts a ON a.id = CASE WHEN r.recipient_id = $1 THEN r.requester_id ELSE r.recipient_id END
         WHERE (r.requester_id = $1 OR r.recipient_id = $1) AND a.revoked_at IS NULL ORDER BY a.handle",
    )
    .bind(account)
    .fetch_all(&mut *db)
    .await?;
    let conversations = sqlx::query_as::<_, ChatRow>(
        "SELECT c.id, c.title, roster.members, c.created_at, own.joined_after
         FROM members own JOIN conversations c ON own.conversation_id = c.id
         JOIN LATERAL (
             SELECT jsonb_agg(jsonb_build_object('id', a.id, 'handle', a.handle) ORDER BY a.handle) AS members
             FROM members m JOIN accounts a ON a.id = m.account_id
             WHERE m.conversation_id = c.id AND m.active AND a.revoked_at IS NULL
         ) roster ON true
         WHERE own.account_id = $1 AND own.active ORDER BY c.created_at DESC, c.id",
    ).bind(account).fetch_all(&mut *db).await?;
    let read_receipts = sqlx::query_scalar("SELECT read_receipts FROM accounts WHERE id = $1")
        .bind(account)
        .fetch_one(&mut *db)
        .await?;
    let mut people = vec![account];
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
    ).bind(&people).fetch_all(&mut *db).await?;
    let names = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id, display_name FROM accounts WHERE id = ANY($1) AND display_name <> ''",
    )
    .bind(&people)
    .fetch_all(&mut *db)
    .await?;
    Ok(Directory {
        names: names.into_iter().collect(),
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
                joined_after: Some(row.joined_after),
                activity_at: row.created_at,
                preview: None,
                preview_seq: None,
                latest_seq: 0,
                unread: 0,
                read_through: 0,
                pinned: false,
                manual_unread: false,
                muted: false,
            })
            .collect(),
    })
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
    lock_message_feed(&mut tx).await?;
    auth.require(&mut tx).await?;
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM contacts c JOIN accounts a ON a.id = c.contact_id AND a.revoked_at IS NULL WHERE c.owner_id = $1 AND c.contact_id = ANY($2)
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
            JOIN members peer ON peer.conversation_id = c.id AND peer.account_id = contact.contact_id AND peer.active
            JOIN accounts account ON account.id = peer.account_id AND account.revoked_at IS NULL
            JOIN contacts approval ON approval.owner_id = peer.account_id AND approval.contact_id = $2
            WHERE contact.owner_id = $2
         )
         FROM conversations c JOIN members own ON own.conversation_id = c.id
         WHERE c.id = $1 AND own.account_id = $2 AND own.active",
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

pub(crate) async fn leave(
    State(app): State<App>,
    auth: Auth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    let mut tx = app.db.begin().await?;
    lock_message_feed(&mut tx).await?;
    auth.require(&mut tx).await?;
    let changed = sqlx::query(
        "UPDATE members own SET active = false FROM conversations c
         WHERE own.conversation_id = c.id AND c.id = $1 AND c.title IS NOT NULL AND own.account_id = $2",
    ).bind(id).bind(auth.user_id).execute(&mut *tx).await?;
    if changed.rows_affected() == 0 {
        return Err(Error::missing());
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn send(
    State(app): State<App>,
    auth: Auth,
    Path(id): Path<Uuid>,
    Json(input): Json<SendMessage>,
) -> Result<Json<Publication>> {
    let text = input.text.trim();
    if text.is_empty() || text.len() > MAX_MESSAGE_BYTES || text.contains('\0') {
        return Err(Error::bad("Enter a message up to 64 KiB without NUL characters."));
    }
    let mut tx = app.db.begin().await?;
    lock_message_feed(&mut tx).await?;
    auth.require(&mut tx).await?;
    let existing: Option<(i64, bool)> = sqlx::query_as(
        "SELECT seq, conversation_id = $3 AND ai_provider IS NULL AND (text IS NULL OR text = $4)
                AND reply_to IS NOT DISTINCT FROM $5
         FROM messages WHERE sender_id = $1 AND nonce = $2",
    )
    .bind(auth.user_id)
    .bind(input.nonce)
    .bind(id)
    .bind(text)
    .bind(input.reply_to)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some((seq, same)) = existing {
        return if same {
            Ok(Json(sync::publication(&mut tx, seq).await?))
        } else {
            Err(Error::conflict(
                "This message ID was used for a different message.",
            ))
        };
    }
    require_send_access(&mut tx, id, auth.user_id).await?;
    if let Some(source) = input.reply_to {
        let visible: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM m1_visible_messages WHERE seq = $1
               AND conversation_id = $2 AND account_id = $3 AND recall_seq IS NULL)",
        )
        .bind(source)
        .bind(id)
        .bind(auth.user_id)
        .fetch_one(&mut *tx)
        .await?;
        if !visible {
            return Err(Error::missing());
        }
    }
    let seq: i64 = sqlx::query_scalar(
        "INSERT INTO messages (nonce, conversation_id, sender_id, text, reply_to, expires_at)
         VALUES ($1,$2,$3,$4,$5,clock_timestamp() + make_interval(secs => $6::int)) RETURNING seq",
    )
    .bind(input.nonce)
    .bind(id)
    .bind(auth.user_id)
    .bind(text)
    .bind(input.reply_to)
    .bind(app.retention.text_secs)
    .fetch_one(&mut *tx)
    .await?;
    sync::dispatch(&mut tx, seq).await?;
    let publication = sync::publication(&mut tx, seq).await?;
    tx.commit().await?;
    Ok(Json(publication))
}

pub(crate) async fn set_read_receipts(
    State(app): State<App>,
    auth: Auth,
    Json(input): Json<ReadPreference>,
) -> Result<StatusCode> {
    let mut tx = app.db.begin().await?;
    lock_message_feed(&mut tx).await?;
    auth.require(&mut tx).await?;
    sqlx::query(
        "UPDATE accounts SET read_receipts = $2, receipts_after = (SELECT committed_through FROM m1_feed)
         WHERE id = $1 AND read_receipts <> $2",
    )
        .bind(auth.user_id)
        .bind(input.enabled)
        .execute(&mut *tx)
        .await?;
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
    lock_message_feed(&mut tx).await?;
    auth.require(&mut tx).await?;
    let enabled: bool = sqlx::query_scalar(
        "SELECT a.read_receipts FROM members own JOIN accounts a ON a.id = own.account_id
         WHERE own.conversation_id = $1 AND own.account_id = $2 AND own.active",
    )
    .bind(id)
    .bind(auth.user_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(Error::missing)?;
    if enabled
        && format.latest
        && let Some(viewed) = input.viewed
    {
        sqlx::query(
            "UPDATE members viewer SET viewed_through = $3
             FROM m1_visible_messages m JOIN conversations c ON c.id = m.conversation_id
             JOIN accounts sender ON sender.id = m.sender_id
             JOIN accounts reader ON reader.id = m.account_id
             JOIN m1_device_events e ON e.event_seq = m.seq AND e.device_id = $4
             WHERE viewer.conversation_id = $1 AND viewer.account_id = $2 AND m.account_id = $2
               AND m.seq = $3 AND m.conversation_id = $1 AND m.sender_id <> $2
               AND c.direct_key IS NOT NULL AND m.ai_provider IS NULL AND m.recall_seq IS NULL
               AND sender.read_receipts AND sender.revoked_at IS NULL
               AND m.seq > greatest(sender.receipts_after, reader.receipts_after, viewer.viewed_through)
               AND e.metadata_state = 'received'",
        ).bind(id).bind(auth.user_id).bind(viewed).bind(auth.device_id).execute(&mut *tx).await?;
    }
    let receipt = if enabled && !input.sent.is_empty() {
        sqlx::query_as::<_, (i64, bool)>(
            "SELECT latest.seq, peer.viewed_through >= latest.seq FROM members own
             JOIN accounts sender ON sender.id = own.account_id
             JOIN conversations c ON c.id = own.conversation_id AND c.direct_key IS NOT NULL
             JOIN members peer ON peer.conversation_id = c.id AND peer.account_id <> own.account_id
             JOIN accounts reader ON reader.id = peer.account_id AND reader.revoked_at IS NULL
             JOIN LATERAL (
                SELECT m.seq, m.sender_id, m.recall_seq, m.ai_provider FROM m1_visible_messages m
                WHERE m.conversation_id = c.id AND m.account_id = own.account_id
                ORDER BY m.created_at DESC, m.seq DESC LIMIT 1
             ) latest ON true
             JOIN m1_recipients r ON r.message_seq = latest.seq AND r.account_id = peer.account_id
             WHERE c.id = $1 AND own.account_id = $2 AND latest.seq = ANY($3)
               AND latest.sender_id = own.account_id AND latest.recall_seq IS NULL AND latest.ai_provider IS NULL
               AND peer.active AND reader.read_receipts AND r.metadata_received
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
    auth.require(&mut tx).await?;
    let (already, in_time): (bool, bool) = sqlx::query_as(
        "SELECT recall_seq IS NOT NULL,
                created_at + $4 >= (extract(epoch FROM clock_timestamp()) * 1000)::BIGINT
         FROM m1_visible_messages WHERE seq = $1 AND conversation_id = $2
           AND account_id = $3 AND sender_id = $3 AND ai_provider IS NULL",
    )
    .bind(seq)
    .bind(id)
    .bind(auth.user_id)
    .bind(RECALL_WINDOW_MS)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(Error::missing)?;
    if already {
        return Ok(StatusCode::NO_CONTENT);
    }
    if !in_time {
        return Err(Error::conflict(
            "Messages can only be recalled within five minutes.",
        ));
    }
    let event: i64 = sqlx::query_scalar(
        "UPDATE messages SET recall_seq = nextval(pg_get_serial_sequence('messages', 'seq')), text = NULL, attachment_fingerprint = NULL
         WHERE seq = $1 RETURNING recall_seq",
    ).bind(seq).fetch_one(&mut *tx).await?;
    sqlx::query(
        "INSERT INTO m1_device_events (device_id, event_seq, message_seq)
         SELECT d.id, $2, m.seq FROM m1_visible_messages m JOIN m1_devices d ON d.account_id = m.account_id
         WHERE m.seq = $1 AND d.retired_at IS NULL AND coalesce(d.active_until > clock_timestamp(), true)
           AND d.epoch = (SELECT epoch FROM m1_feed)",
    ).bind(seq).bind(event).execute(&mut *tx).await?;
    sqlx::query("UPDATE m1_device_events SET content_state = 'revoked' WHERE message_seq = $1 AND content_state = 'pending'")
        .bind(seq).execute(&mut *tx).await?;
    sync::advance_feed(&mut tx, event).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn publish_ai_reply(
    State(app): State<App>,
    auth: Auth,
    Path(id): Path<Uuid>,
    Json(input): Json<PublishAiReply>,
) -> Result<Json<Publication>> {
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
    readers.sort_unstable();
    let mut tx = app.db.begin().await?;
    lock_message_feed(&mut tx).await?;
    auth.require(&mut tx).await?;
    let existing: Option<(i64, bool)> = sqlx::query_as(
        "SELECT m.seq, coalesce(m.conversation_id = $3 AND (m.text IS NULL OR m.text = $4)
                  AND m.ai_provider = $5, false)
                  AND (SELECT array_agg(account_id ORDER BY account_id) FROM ai_reply_readers WHERE message_seq = m.seq)
                    IS NOT DISTINCT FROM $6
         FROM messages m WHERE m.sender_id = $1 AND m.nonce = $2",
    ).bind(auth.user_id).bind(input.nonce).bind(id).bind(text).bind(input.provider.as_str()).bind(&readers)
        .fetch_optional(&mut *tx).await?;
    if let Some((seq, same)) = existing {
        return if same {
            Ok(Json(sync::publication(&mut tx, seq).await?))
        } else {
            Err(Error::conflict(
                "This message ID was used for a different reply or audience.",
            ))
        };
    }
    require_send_access(&mut tx, id, auth.user_id).await?;
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM members m JOIN accounts a ON a.id = m.account_id
         WHERE m.conversation_id = $1 AND m.account_id = ANY($2) AND m.active AND a.revoked_at IS NULL",
    ).bind(id).bind(&readers).fetch_one(&mut *tx).await?;
    if count != readers.len() as i64 {
        return Err(Error::bad(
            "Reply recipients must be distinct members of this chat.",
        ));
    }
    let seq: i64 = sqlx::query_scalar(
        "INSERT INTO messages (nonce, conversation_id, sender_id, text, ai_provider, created_at, expires_at)
         SELECT $1, id, $3, $4, $5,
             least(greatest($6, created_at), (extract(epoch FROM clock_timestamp()) * 1000)::BIGINT),
             clock_timestamp() + make_interval(secs => $7::int)
         FROM conversations WHERE id = $2 RETURNING seq",
    ).bind(input.nonce).bind(id).bind(auth.user_id).bind(text).bind(input.provider.as_str())
        .bind(input.created_at).bind(app.retention.text_secs).fetch_one(&mut *tx).await?;
    sqlx::query(
        "INSERT INTO ai_reply_readers (message_seq, account_id) SELECT $1, unnest($2::uuid[])",
    )
    .bind(seq)
    .bind(&readers)
    .execute(&mut *tx)
    .await?;
    sync::dispatch(&mut tx, seq).await?;
    let publication = sync::publication(&mut tx, seq).await?;
    tx.commit().await?;
    Ok(Json(publication))
}
