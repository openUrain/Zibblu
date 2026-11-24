use crate::{
    App, Auth, Error, Result,
    chats::{lock_message_feed, require_send_access},
    sync,
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
use zibblu_core::{Attachment, CHUNK_SIZE, ContentAck, Publication, UploadProgress};

pub(crate) const ACCOUNT_BYTES: i64 = 2 * 1024 * 1024 * 1024;
pub(crate) const SERVICE_BYTES: i64 = 16 * 1024 * 1024 * 1024;

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
    expired: bool,
}

impl Upload {
    fn progress(&self) -> UploadProgress {
        UploadProgress {
            received: self.received as u64,
            complete: self.complete,
        }
    }
}

pub(crate) async fn storage_usage(db: &mut PgConnection, owner: Uuid) -> Result<(i64, i64, i64, i64)> {
    Ok(sqlx::query_as(
        "SELECT coalesce(sum(size) FILTER (WHERE owner_id = $1), 0)::BIGINT,
                count(*) FILTER (WHERE owner_id = $1 AND is_original),
                coalesce(sum(size), 0)::BIGINT, count(*) FILTER (WHERE is_original)
         FROM (
             SELECT owner_id, size, true AS is_original FROM attachments
             UNION ALL
             SELECT a.owner_id, r.size, false FROM video_resources r
             JOIN attachments a ON a.id = r.attachment_id WHERE r.status = 'ready'
         ) payloads",
    ).bind(owner).fetch_one(db).await?)
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
    lock_message_feed(&mut tx).await?;
    auth.require(&mut tx).await?;
    if let Some(existing) = sqlx::query_as::<_, Upload>(
        "SELECT *, coalesce(expires_at <= statement_timestamp(), false) AS expired FROM attachments WHERE id = $1",
    )
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
        if !existing.complete {
            require_send_access(&mut tx, conversation, auth.user_id).await?;
            if existing.expired {
                return Err(upload_expired());
            }
        }
        return Ok(Json(existing.progress()));
    }
    let published: Option<(Uuid, Uuid)> =
        sqlx::query_as("SELECT sender_id, conversation_id FROM messages WHERE attachment_id = $1")
            .bind(file.id)
            .fetch_optional(&mut *tx)
            .await?;
    if let Some((owner, original_chat)) = published {
        if owner != auth.user_id || original_chat != conversation {
            return Err(Error::missing());
        }
        return Ok(Json(UploadProgress {
            received: file.size,
            complete: true,
        }));
    }
    require_send_access(&mut tx, conversation, auth.user_id).await?;
    check_upload_expiry(&mut tx, file.id, auth.user_id).await?;
    let (account, account_count, total, total_count) = storage_usage(&mut tx, auth.user_id).await?;
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
    sqlx::query(
        "INSERT INTO attachments (id, conversation_id, owner_id, name, kind, size, sha256, expires_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,clock_timestamp() + make_interval(secs => $8::int))",
    )
        .bind(file.id).bind(conversation).bind(auth.user_id).bind(file.name)
        .bind(file.kind.as_str()).bind(file.size as i64).bind(file.sha256)
        .bind(app.retention.upload_secs)
        .execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(UploadProgress {
        received: 0,
        complete: false,
    }))
}

async fn lock_upload(db: &mut PgConnection, id: Uuid, owner: Uuid) -> Result<Upload> {
    let file: Option<Upload> = sqlx::query_as(
        "SELECT *, coalesce(expires_at <= statement_timestamp(), false) AS expired
         FROM attachments WHERE id = $1 AND owner_id = $2 FOR UPDATE",
    )
    .bind(id)
    .bind(owner)
    .fetch_optional(&mut *db)
    .await?;
    match file {
        Some(file) if !file.expired => Ok(file),
        Some(_) => Err(upload_expired()),
        None => {
            check_upload_expiry(db, id, owner).await?;
            Err(Error::missing())
        }
    }
}

fn upload_expired() -> Error {
    Error(
        StatusCode::GONE,
        "This upload expired. Share the attachment again.",
    )
}

async fn check_upload_expiry(db: &mut PgConnection, id: Uuid, owner: Uuid) -> Result<()> {
    let expired: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM m1_expired_uploads WHERE id = $1 AND owner_id = $2)",
    )
    .bind(id)
    .bind(owner)
    .fetch_one(db)
    .await?;
    if expired {
        Err(upload_expired())
    } else {
        Ok(())
    }
}

pub(crate) async fn discard(
    State(app): State<App>,
    auth: Auth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    let mut tx = app.db.begin().await?;
    let file = sqlx::query_as::<_, Upload>(
        "SELECT *, coalesce(expires_at <= statement_timestamp(), false) AS expired FROM attachments WHERE id = $1 FOR UPDATE",
    )
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
    lock_message_feed(&mut tx).await?;
    auth.require(&mut tx).await?;
    let file = lock_upload(&mut tx, id, auth.user_id).await?;
    require_send_access(&mut tx, file.conversation_id, auth.user_id).await?;
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
) -> Result<Json<Publication>> {
    let mut tx = app.db.begin().await?;
    lock_message_feed(&mut tx).await?;
    auth.require(&mut tx).await?;
    let published: Option<i64> =
        sqlx::query_scalar("SELECT seq FROM messages WHERE attachment_id = $1 AND sender_id = $2")
            .bind(id)
            .bind(auth.user_id)
            .fetch_optional(&mut *tx)
            .await?;
    if let Some(seq) = published {
        return Ok(Json(sync::publication(&mut tx, seq).await?));
    }
    let file = lock_upload(&mut tx, id, auth.user_id).await?;
    require_send_access(&mut tx, file.conversation_id, auth.user_id).await?;
    if file.received != file.size {
        return Err(Error::conflict("The upload still has missing chunks."));
    }
    let mut video_candidate = false;
    let hash = {
        let mut chunks = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT data FROM attachment_chunks WHERE attachment_id = $1 ORDER BY byte_offset",
        )
        .bind(id)
        .fetch(&mut *tx);
        let mut hash = Sha256::new();
        let mut first = true;
        while let Some(chunk) = chunks.try_next().await? {
            if first {
                video_candidate = file.kind == "file" && crate::media::candidate(&chunk);
                first = false;
            }
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
    let seq: i64 = sqlx::query_scalar(
        "INSERT INTO messages (conversation_id, sender_id, attachment_id, expires_at)
         SELECT $1, $2, $3, clock_timestamp() + make_interval(secs => $4::int)
         FROM attachments WHERE id = $3 AND coalesce(expires_at > clock_timestamp(), true)
         RETURNING seq",
    )
    .bind(file.conversation_id)
    .bind(auth.user_id)
    .bind(id)
    .bind(app.retention.file_secs)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(upload_expired)?;
    sqlx::query("UPDATE attachments SET complete = true, expires_at = (SELECT expires_at FROM messages WHERE seq = $2) WHERE id = $1")
        .bind(file.id).bind(seq).execute(&mut *tx).await?;
    if video_candidate {
        crate::media::enqueue(&mut tx, id).await?;
    }
    sync::dispatch(&mut tx, seq).await?;
    let publication = sync::publication(&mut tx, seq).await?;
    tx.commit().await?;
    Ok(Json(publication))
}

pub(crate) async fn read_chunk(
    State(app): State<App>,
    auth: Auth,
    Path((id, offset)): Path<(Uuid, i64)>,
) -> Result<Bytes> {
    let (expired, bytes): (bool, Option<Vec<u8>>) = sqlx::query_as(
        "SELECT coalesce(m.expires_at <= statement_timestamp(), false), c.data
         FROM m1_visible_messages m LEFT JOIN attachment_chunks c ON c.attachment_id = m.attachment_id
           AND c.byte_offset = $2 AND coalesce(m.expires_at > statement_timestamp(), true)
         WHERE m.attachment_id = $1 AND m.account_id = $3 AND m.recall_seq IS NULL",
    )
    .bind(id)
    .bind(offset)
    .bind(auth.user_id)
    .fetch_optional(&app.db)
    .await?
    .ok_or_else(Error::missing)?;
    if expired {
        return Err(file_expired());
    }
    Ok(bytes.ok_or_else(Error::missing)?.into())
}

fn file_expired() -> Error {
    Error(
        StatusCode::GONE,
        "The file expired. Ask the sender to share it again.",
    )
}

async fn downloadable(db: &mut PgConnection, id: Uuid, account: Uuid) -> Result<(i64, String)> {
    let (seq, expired, digest): (i64, bool, Option<String>) = sqlx::query_as(
        "SELECT m.seq, coalesce(m.expires_at <= statement_timestamp(), false), f.sha256
         FROM m1_visible_messages m LEFT JOIN attachments f ON f.id = m.attachment_id
         WHERE m.attachment_id = $1 AND m.account_id = $2 AND m.recall_seq IS NULL",
    )
    .bind(id)
    .bind(account)
    .fetch_optional(db)
    .await?
    .ok_or_else(Error::missing)?;
    if expired {
        return Err(file_expired());
    }
    Ok((seq, digest.ok_or_else(Error::missing)?))
}

pub(crate) async fn start_download(
    State(app): State<App>,
    auth: Auth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    let mut tx = app.db.begin().await?;
    lock_message_feed(&mut tx).await?;
    auth.require(&mut tx).await?;
    let (seq, _) = downloadable(&mut tx, id, auth.user_id).await?;
    sqlx::query(
        "INSERT INTO m1_device_events (device_id, event_seq, message_seq, metadata_state, content_state)
         VALUES ($1, $2, $2, 'processed', 'pending') ON CONFLICT DO NOTHING",
    ).bind(auth.device_id).bind(seq).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn ack_content(
    State(app): State<App>,
    auth: Auth,
    Path(id): Path<Uuid>,
    Json(input): Json<ContentAck>,
) -> Result<StatusCode> {
    let mut tx = app.db.begin().await?;
    lock_message_feed(&mut tx).await?;
    auth.require(&mut tx).await?;
    let (seq, digest) = downloadable(&mut tx, id, auth.user_id).await?;
    if input.sha256 != digest {
        return Err(Error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "The downloaded file did not match its SHA-256.",
        ));
    }
    let updated = sqlx::query(
        "UPDATE m1_device_events SET content_state = 'verified'
         WHERE device_id = $1 AND event_seq = $2 AND content_state IN ('pending', 'verified')",
    )
    .bind(auth.device_id)
    .bind(seq)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(Error::missing());
    }
    sqlx::query("UPDATE m1_recipients SET content_verified = true WHERE message_seq = $1 AND account_id = $2")
        .bind(seq).bind(auth.user_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
