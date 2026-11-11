use crate::{
    App, Auth, Error, Result,
    chats::{lock_message_feed, require_member},
};
use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
};
use futures_util::TryStreamExt;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgConnection};
use uuid::Uuid;
use zibblu_core::{Attachment, CHUNK_SIZE, UploadProgress};

const ACCOUNT_BYTES: i64 = 2 * 1024 * 1024 * 1024;
const SERVICE_BYTES: i64 = 16 * 1024 * 1024 * 1024;

#[derive(FromRow)]
struct Upload {
    id: Uuid,
    conversation_id: Uuid,
    owner_id: Uuid,
    name: String,
    kind: String,
    size: i64,
    sha256: String,
    received: i64,
    complete: bool,
}

impl Upload {
    fn progress(&self) -> UploadProgress {
        UploadProgress {
            received: self.received as u64,
            complete: self.complete,
        }
    }
}

pub(crate) async fn begin(
    State(app): State<App>,
    auth: Auth,
    Path(conversation): Path<Uuid>,
    Json(file): Json<Attachment>,
) -> Result<Json<UploadProgress>> {
    if !file.is_valid() {
        return Err(Error::bad(
            "Invalid attachment metadata or file larger than 256 MiB.",
        ));
    }
    let mut tx = app.db.begin().await?;
    require_member(&mut tx, conversation, auth.user_id).await?;
    // Reserve bytes once per transfer, even when multiple uploads begin concurrently.
    sqlx::query("SELECT pg_advisory_xact_lock(1002)")
        .execute(&mut *tx)
        .await?;
    if let Some(existing) = sqlx::query_as::<_, Upload>("SELECT * FROM attachments WHERE id = $1")
        .bind(file.id)
        .fetch_optional(&mut *tx)
        .await?
    {
        if existing.owner_id != auth.user_id || existing.conversation_id != conversation {
            return Err(Error::missing());
        }
        if existing.name != file.name
            || existing.kind != file.kind.as_str()
            || existing.size != file.size as i64
            || existing.sha256 != file.sha256
        {
            return Err(Error::conflict(
                "The upload metadata changed. Share the file again.",
            ));
        }
        return Ok(Json(existing.progress()));
    }
    let (account, account_count, total, total_count): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT coalesce(sum(size) FILTER (WHERE owner_id = $1), 0)::BIGINT,
                count(*) FILTER (WHERE owner_id = $1),
                coalesce(sum(size), 0)::BIGINT, count(*) FROM attachments",
    )
    .bind(auth.user_id)
    .fetch_one(&mut *tx)
    .await?;
    if account + file.size as i64 > ACCOUNT_BYTES
        || account_count >= 256
        || total + file.size as i64 > SERVICE_BYTES
        || total_count >= 4096
    {
        return Err(Error(
            StatusCode::INSUFFICIENT_STORAGE,
            "The service's attachment storage limit was reached.",
        ));
    }
    sqlx::query("INSERT INTO attachments (id, conversation_id, owner_id, name, kind, size, sha256) VALUES ($1,$2,$3,$4,$5,$6,$7)")
        .bind(file.id).bind(conversation).bind(auth.user_id).bind(file.name)
        .bind(file.kind.as_str()).bind(file.size as i64).bind(file.sha256)
        .execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(UploadProgress {
        received: 0,
        complete: false,
    }))
}

async fn lock_upload(db: &mut PgConnection, id: Uuid, owner: Uuid) -> Result<Upload> {
    sqlx::query_as("SELECT * FROM attachments WHERE id = $1 AND owner_id = $2 FOR UPDATE")
        .bind(id)
        .bind(owner)
        .fetch_optional(db)
        .await?
        .ok_or_else(Error::missing)
}

pub(crate) async fn discard(
    State(app): State<App>,
    auth: Auth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    let mut tx = app.db.begin().await?;
    let file = sqlx::query_as::<_, Upload>("SELECT * FROM attachments WHERE id = $1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    if let Some(file) = file {
        if file.owner_id != auth.user_id {
            return Err(Error::missing());
        }
        if file.complete {
            return Err(Error::conflict(
                "This attachment has already been shared. Resume to refresh its status.",
            ));
        }
        sqlx::query("DELETE FROM attachments WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn write_chunk(
    State(app): State<App>,
    auth: Auth,
    Path((id, offset)): Path<(Uuid, i64)>,
    bytes: Bytes,
) -> Result<Json<UploadProgress>> {
    let mut tx = app.db.begin().await?;
    let file = lock_upload(&mut tx, id, auth.user_id).await?;
    if file.complete || offset != file.received {
        return Err(Error::conflict(
            "The upload position changed. Resume to synchronize it.",
        ));
    }
    let expected = (file.size - offset).min(CHUNK_SIZE as i64);
    if expected == 0 || bytes.len() as i64 != expected {
        return Err(Error::bad("Incorrect chunk length."));
    }
    let received = offset + expected;
    sqlx::query(
        "INSERT INTO attachment_chunks (attachment_id, byte_offset, data) VALUES ($1,$2,$3)",
    )
    .bind(id)
    .bind(offset)
    .bind(bytes.as_ref())
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE attachments SET received = $2 WHERE id = $1")
        .bind(id)
        .bind(received)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(UploadProgress {
        received: received as u64,
        complete: false,
    }))
}

pub(crate) async fn complete(
    State(app): State<App>,
    auth: Auth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    let mut tx = app.db.begin().await?;
    let file = lock_upload(&mut tx, id, auth.user_id).await?;
    if file.complete {
        return Ok(StatusCode::NO_CONTENT);
    }
    if file.received != file.size {
        return Err(Error::conflict("The upload still has missing chunks."));
    }
    let hash = {
        let mut chunks = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT data FROM attachment_chunks WHERE attachment_id = $1 ORDER BY byte_offset",
        )
        .bind(id)
        .fetch(&mut *tx);
        let mut hash = Sha256::new();
        while let Some(chunk) = chunks.try_next().await? {
            hash.update(chunk);
        }
        format!("{:x}", hash.finalize())
    };
    if hash != file.sha256 {
        sqlx::query("DELETE FROM attachment_chunks WHERE attachment_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE attachments SET received = 0 WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Err(Error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "File verification failed. Retry the upload.",
        ));
    }
    lock_message_feed(&mut tx).await?;
    sqlx::query("UPDATE attachments SET complete = true WHERE id = $1")
        .bind(file.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO messages (conversation_id, sender_id, attachment_id) VALUES ($1,$2,$3)",
    )
    .bind(file.conversation_id)
    .bind(auth.user_id)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn read_chunk(
    State(app): State<App>,
    auth: Auth,
    Path((id, offset)): Path<(Uuid, i64)>,
) -> Result<Bytes> {
    let bytes: Vec<u8> = sqlx::query_scalar(
        "SELECT c.data FROM attachment_chunks c
         JOIN attachments f ON f.id = c.attachment_id
         JOIN members m ON m.conversation_id = f.conversation_id AND m.account_id = $3
         WHERE c.attachment_id = $1 AND c.byte_offset = $2 AND f.complete",
    )
    .bind(id)
    .bind(offset)
    .bind(auth.user_id)
    .fetch_optional(&app.db)
    .await?
    .ok_or_else(Error::missing)?;
    Ok(bytes.into())
}
