use crate::{App, Auth, Error, Result, chats::lock_message_feed};
use axum::{
    Json,
    extract::{Path, State},
};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use zibblu_core::{
    ConversationCreated, GroupInvite, GroupInvitePreview, GroupInviteRequest, MAX_GROUP_MEMBERS,
};

pub(crate) async fn issue(
    State(app): State<App>,
    auth: Auth,
    Path(conversation): Path<Uuid>,
) -> Result<Json<GroupInvite>> {
    let code = Uuid::new_v4();
    let expires_at = sqlx::query_scalar(
        "INSERT INTO group_invites (conversation_id, code_hash, expires_at)
         SELECT c.id, $3, now() + interval '7 days'
         FROM conversations c JOIN members m ON m.conversation_id = c.id
         WHERE c.id = $1 AND m.account_id = $2 AND c.title IS NOT NULL
         ON CONFLICT (conversation_id) DO UPDATE
         SET code_hash = excluded.code_hash, expires_at = excluded.expires_at
         RETURNING (extract(epoch FROM expires_at) * 1000)::BIGINT",
    )
    .bind(conversation)
    .bind(auth.user_id)
    .bind(Sha256::digest(code.as_bytes()).as_slice())
    .fetch_optional(&app.db)
    .await?
    .ok_or_else(Error::missing)?;
    Ok(Json(GroupInvite { code, expires_at }))
}

pub(crate) async fn preview(
    State(app): State<App>,
    _auth: Auth,
    Json(input): Json<GroupInviteRequest>,
) -> Result<Json<GroupInvitePreview>> {
    let (id, title, member_count) = sqlx::query_as(
        "SELECT c.id, c.title, (SELECT count(*) FROM members WHERE conversation_id = c.id)
         FROM group_invites i JOIN conversations c ON c.id = i.conversation_id
         WHERE i.code_hash = $1 AND i.expires_at > now() AND c.title IS NOT NULL",
    )
    .bind(Sha256::digest(input.code.as_bytes()).as_slice())
    .fetch_optional(&app.db)
    .await?
    .ok_or_else(Error::missing)?;
    Ok(Json(GroupInvitePreview {
        id,
        title,
        member_count,
    }))
}

pub(crate) async fn join(
    State(app): State<App>,
    auth: Auth,
    Json(input): Json<GroupInviteRequest>,
) -> Result<Json<ConversationCreated>> {
    let mut tx = app.db.begin().await?;
    // The same lock orders publishes and the new member's history cutoff.
    lock_message_feed(&mut tx).await?;
    let id: Uuid = sqlx::query_scalar(
        "SELECT i.conversation_id FROM group_invites i
         JOIN conversations c ON c.id = i.conversation_id
         WHERE i.code_hash = $1 AND i.expires_at > clock_timestamp() AND c.title IS NOT NULL
         FOR SHARE OF i",
    )
    .bind(Sha256::digest(input.code.as_bytes()).as_slice())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(Error::missing)?;
    let (count, joined): (i64, bool) = sqlx::query_as(
        "SELECT count(*), coalesce(bool_or(account_id = $2), false)
         FROM members WHERE conversation_id = $1",
    )
    .bind(id)
    .bind(auth.user_id)
    .fetch_one(&mut *tx)
    .await?;
    if !joined {
        if count >= MAX_GROUP_MEMBERS as i64 {
            return Err(Error::conflict("This group is full."));
        }
        sqlx::query(
            "INSERT INTO members (conversation_id, account_id, joined_after)
             SELECT $1, $2, coalesce(max(seq), 0) FROM messages WHERE conversation_id = $1",
        )
        .bind(id)
        .bind(auth.user_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(Json(ConversationCreated { id }))
}
