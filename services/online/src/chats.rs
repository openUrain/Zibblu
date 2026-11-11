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
    Attachment, Conversation, ConversationCreated, ConversationRequest, Directory,
    MAX_GROUP_MEMBERS, MAX_MESSAGE_BYTES, Message, SYNC_PAGE_SIZE, SendMessage, SyncPage, User,
};

#[derive(Deserialize)]
pub(crate) struct Cursor {
    after: i64,
}

#[derive(FromRow)]
struct ChatRow {
    id: Uuid,
    title: Option<String>,
    members: DbJson<Vec<User>>,
    activity_at: i64,
    preview: Option<String>,
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
}

pub(crate) async fn sync(
    State(app): State<App>,
    auth: Auth,
    Query(cursor): Query<Cursor>,
) -> Result<Json<SyncPage>> {
    if cursor.after < 0 {
        return Err(Error::bad("Invalid message cursor."));
    }
    let mut tx = app.db.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *tx)
        .await?;
    let contacts = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT a.id, a.handle FROM contacts c JOIN accounts a ON a.id = c.contact_id
         WHERE c.owner_id = $1 ORDER BY a.handle",
    )
    .bind(auth.user_id)
    .fetch_all(&mut *tx)
    .await?;
    let conversations = sqlx::query_as::<_, ChatRow>(
        "SELECT c.id, c.title, roster.members, coalesce(latest.created_at, c.created_at) AS activity_at,
                latest.preview
         FROM members own JOIN conversations c ON c.id = own.conversation_id
         JOIN LATERAL (
             SELECT jsonb_agg(jsonb_build_object('id', a.id, 'handle', a.handle) ORDER BY a.handle) AS members
             FROM members m JOIN accounts a ON a.id = m.account_id WHERE m.conversation_id = c.id
         ) roster ON true
         LEFT JOIN LATERAL (
             SELECT m.created_at, coalesce(left(m.text, 100), f.name) AS preview
             FROM messages m LEFT JOIN attachments f ON f.id = m.attachment_id
             WHERE m.conversation_id = c.id AND m.seq > own.joined_after
             ORDER BY m.seq DESC LIMIT 1
         ) latest ON true
         WHERE own.account_id = $1 ORDER BY activity_at DESC, c.id",
    ).bind(auth.user_id).fetch_all(&mut *tx).await?;
    let messages = sqlx::query_as::<_, MessageRow>(
        "SELECT m.seq, m.conversation_id, m.sender_id, a.handle, m.created_at, m.text,
                CASE WHEN f.id IS NOT NULL THEN jsonb_build_object(
                    'id', f.id, 'name', f.name, 'size', f.size, 'sha256', f.sha256, 'kind', f.kind
                ) END AS attachment
         FROM messages m JOIN accounts a ON a.id = m.sender_id
         JOIN members own ON own.conversation_id = m.conversation_id AND own.account_id = $1
         LEFT JOIN attachments f ON f.id = m.attachment_id
         WHERE m.seq > $2 AND m.seq > own.joined_after ORDER BY m.seq LIMIT $3",
    )
    .bind(auth.user_id)
    .bind(cursor.after)
    .bind(SYNC_PAGE_SIZE as i64)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(SyncPage {
        directory: Directory {
            contacts: contacts
                .into_iter()
                .map(|(id, handle)| User { id, handle })
                .collect(),
            conversations: conversations
                .into_iter()
                .map(|row| Conversation {
                    id: row.id,
                    title: row.title,
                    members: row.members.0,
                    activity_at: row.activity_at,
                    preview: row.preview,
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
            })
            .collect(),
    }))
}

pub(crate) async fn create(
    State(app): State<App>,
    auth: Auth,
    Json(input): Json<ConversationRequest>,
) -> Result<Json<ConversationCreated>> {
    let (title, direct_key, peers) = match input {
        ConversationRequest::Direct { contact_id } => {
            let mut pair = [auth.user_id, contact_id];
            pair.sort_unstable();
            (
                None,
                Some(format!("{}:{}", pair[0], pair[1])),
                vec![contact_id],
            )
        }
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
            (Some(title), None, contact_ids)
        }
    };
    let mut tx = app.db.begin().await?;
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM contacts WHERE owner_id = $1 AND contact_id = ANY($2)",
    )
    .bind(auth.user_id)
    .bind(&peers)
    .fetch_one(&mut *tx)
    .await?;
    if count != peers.len() as i64 {
        return Err(Error::bad("Add participants to your contacts first."));
    }
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO conversations (id, title, direct_key) VALUES ($1,$2,$3)
         ON CONFLICT (direct_key) DO UPDATE SET direct_key = excluded.direct_key RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(title)
    .bind(direct_key)
    .fetch_one(&mut *tx)
    .await?;
    let mut members = peers;
    members.push(auth.user_id);
    sqlx::query(
        "INSERT INTO members (conversation_id, account_id)
         SELECT $1, unnest($2::uuid[]) ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(&members)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(ConversationCreated { id }))
}

pub(crate) async fn require_member(
    db: &mut PgConnection,
    conversation_id: Uuid,
    account_id: Uuid,
) -> Result<()> {
    let member: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM members WHERE conversation_id = $1 AND account_id = $2)",
    )
    .bind(conversation_id)
    .bind(account_id)
    .fetch_one(db)
    .await?;
    if !member {
        return Err(Error::missing());
    }
    Ok(())
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
    require_member(&mut tx, id, auth.user_id).await?;
    lock_message_feed(&mut tx).await?;
    let result = sqlx::query(
        "INSERT INTO messages (nonce, conversation_id, sender_id, text) VALUES ($1,$2,$3,$4)
         ON CONFLICT (sender_id, nonce) DO NOTHING",
    )
    .bind(input.nonce)
    .bind(id)
    .bind(auth.user_id)
    .bind(text)
    .execute(&mut *tx)
    .await?;
    if result.rows_affected() == 0 {
        let same: bool = sqlx::query_scalar(
            "SELECT conversation_id = $3 AND text = $4 FROM messages WHERE sender_id = $1 AND nonce = $2",
        ).bind(auth.user_id).bind(input.nonce).bind(id).bind(text).fetch_one(&mut *tx).await?;
        if !same {
            return Err(Error::conflict(
                "This message ID was used for a different message.",
            ));
        }
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
