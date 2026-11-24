use crate::{
    App, Auth, Error, Result,
    chats::{lock_message_feed, require_send_access},
    files::{ACCOUNT_BYTES, SERVICE_BYTES, storage_usage},
    sync,
};
use axum::{Json, extract::{Path, State}, http::StatusCode};
use futures_util::TryStreamExt;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgConnection, types::Json as DbJson};
use std::collections::HashSet;
use uuid::Uuid;
use zibblu_core::{
    ForwardRequest, ForwardResult, ForwardTarget, ForwardUploadProgress, ForwardUploadRequest,
    Publication, UploadProgress,
};

#[derive(FromRow)]
struct Source {
    conversation_id: Uuid,
    expired: bool,
    payload_id: Option<Uuid>,
    fingerprint: Option<Vec<u8>>,
}

async fn source(db: &mut PgConnection, id: Uuid, account: Uuid) -> Result<Source> {
    let source: Source = sqlx::query_as(
        "SELECT v.conversation_id, coalesce(v.expires_at <= clock_timestamp(), false) AS expired,
                CASE WHEN a.complete THEN coalesce(a.payload_id, a.id) END AS payload_id,
                m.attachment_fingerprint AS fingerprint
         FROM m1_visible_messages v JOIN messages m ON m.seq = v.seq
         LEFT JOIN attachments a ON a.id = v.attachment_id
         WHERE v.attachment_id = $1 AND v.account_id = $2 AND v.recall_seq IS NULL",
    ).bind(id).bind(account).fetch_optional(&mut *db).await?.ok_or_else(Error::missing)?;
    require_send_access(db, source.conversation_id, account).await?;
    if source.fingerprint.is_none() || (!source.expired && source.payload_id.is_none()) {
        return Err(Error::missing());
    }
    Ok(source)
}

fn needs_original() -> Error {
    Error(StatusCode::GONE, "The original expired. A verified local original is required.")
}

fn validate(input: &ForwardRequest) -> Result<()> {
    let mut chats = HashSet::new();
    let mut ids = HashSet::new();
    if !(1..=9).contains(&input.targets.len()) || input.batch_id.is_nil()
        || !input.targets.iter().all(|target| {
            !target.attachment_id.is_nil() && chats.insert(target.conversation_id)
                && ids.insert(target.attachment_id)
        }) {
        return Err(Error::bad("Choose one to nine distinct target conversations and attachment IDs."));
    }
    Ok(())
}

#[derive(FromRow)]
struct Batch {
    owner_id: Uuid,
    source_id: Uuid,
    targets: DbJson<Vec<ForwardTarget>>,
    upload_id: Option<Uuid>,
}

async fn batch(db: &mut PgConnection, input: &ForwardRequest, id: Uuid, account: Uuid) -> Result<Batch> {
    validate(input)?;
    let existing: Option<Batch> = sqlx::query_as(
        "SELECT owner_id, source_id, targets, upload_id FROM m1_forward_batches WHERE id = $1",
    ).bind(input.batch_id).fetch_optional(&mut *db).await?;
    if let Some(existing) = existing {
        if existing.owner_id != account { return Err(Error::missing()); }
        if existing.source_id != id || existing.targets.0 != input.targets {
            return Err(Error::conflict("The forwarding batch changed. Use the original request."));
        }
        return Ok(existing);
    }
    source(db, id, account).await?;
    sqlx::query("INSERT INTO m1_forward_batches (id, owner_id, source_id, targets) VALUES ($1,$2,$3,$4)")
        .bind(input.batch_id).bind(account).bind(id).bind(DbJson(&input.targets))
        .execute(db).await?;
    Ok(Batch { owner_id: account, source_id: id, targets: DbJson(input.targets.clone()), upload_id: None })
}

async fn staged_source(db: &mut PgConnection, id: Uuid, owner: Uuid) -> Result<Option<Uuid>> {
    let row: Option<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT owner_id, source_id FROM m1_forward_batches WHERE upload_id = $1",
    ).bind(id).fetch_optional(db).await?;
    match row {
        Some((account, source_id)) if account == owner => Ok(Some(source_id)),
        Some(_) => Err(Error::missing()),
        None => Ok(None),
    }
}

pub(crate) async fn reject_staged_publication(db: &mut PgConnection, id: Uuid) -> Result<()> {
    let staged: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM m1_forward_batches WHERE upload_id = $1)",
    ).bind(id).fetch_one(db).await?;
    if staged { return Err(Error::missing()); }
    Ok(())
}

pub(crate) async fn check_staged_chunk(db: &mut PgConnection, id: Uuid, owner: Uuid) -> Result<()> {
    if let Some(source_id) = staged_source(db, id, owner).await? {
        if !source(db, source_id, owner).await?.expired {
            return Err(Error::conflict("The source is still available for direct forwarding."));
        }
    }
    Ok(())
}

pub(crate) async fn begin_upload(
    State(app): State<App>, auth: Auth, Path(id): Path<Uuid>,
    Json(input): Json<ForwardUploadRequest>,
) -> Result<Json<ForwardUploadProgress>> {
    let file = input.original;
    if file.id != id || !file.is_valid() {
        return Err(Error::bad("Invalid original attachment metadata."));
    }
    let mut tx = app.db.begin().await?;
    lock_message_feed(&mut tx).await?;
    auth.require(&mut tx).await?;
    let batch = batch(&mut tx, &input.request, id, auth.user_id).await?;
    let source = source(&mut tx, id, auth.user_id).await?;
    if !source.expired {
        return Err(Error::conflict("The source is still available for direct forwarding."));
    }
    let fingerprint: Vec<u8> = sqlx::query_scalar("SELECT m1_attachment_fingerprint($1,$2,$3,$4)")
        .bind(&file.name).bind(file.kind.as_str()).bind(file.size as i64).bind(&file.sha256)
        .fetch_one(&mut *tx).await?;
    if source.fingerprint.as_deref() != Some(fingerprint.as_slice()) {
        return Err(Error(StatusCode::UNPROCESSABLE_ENTITY, "The local original does not match the source."));
    }
    if let Some(upload_id) = batch.upload_id {
        let existing: Option<(i64, bool, bool)> = sqlx::query_as(
            "SELECT received, complete, coalesce(expires_at <= clock_timestamp(), false)
             FROM attachments WHERE id = $1 FOR UPDATE",
        ).bind(upload_id).fetch_optional(&mut *tx).await?;
        if let Some((received, complete, expired)) = existing {
            if complete || !expired {
                return Ok(Json(ForwardUploadProgress { id: upload_id,
                    progress: UploadProgress { received: received as u64, complete } }));
            }
            sqlx::query("DELETE FROM attachments WHERE id = $1").bind(upload_id).execute(&mut *tx).await?;
        }
    }
    // A batch only needs its original if at least one unpublished target is currently sendable.
    let mut allowed = false;
    for target in &input.request.targets {
        let published: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM messages WHERE attachment_id = $1 AND forward_batch = $2)",
        ).bind(target.attachment_id).bind(input.request.batch_id).fetch_one(&mut *tx).await?;
        if !published && require_send_access(&mut tx, target.conversation_id, auth.user_id).await.is_ok() {
            allowed = true;
            break;
        }
    }
    if !allowed { return Err(Error::conflict("No unpublished forwarding target is currently available.")); }
    let (account, count, total, total_count) = storage_usage(&mut tx, auth.user_id).await?;
    if account + file.size as i64 > ACCOUNT_BYTES || count >= 256
        || total + file.size as i64 > SERVICE_BYTES || total_count >= 4096 {
        return Err(quota());
    }
    let upload_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO attachments (id, conversation_id, owner_id, name, kind, size, sha256, expires_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,clock_timestamp() + make_interval(secs => $8::int))",
    ).bind(upload_id).bind(source.conversation_id).bind(auth.user_id).bind(file.name)
        .bind(file.kind.as_str()).bind(file.size as i64).bind(file.sha256)
        .bind(app.retention.upload_secs).execute(&mut *tx).await?;
    sqlx::query("UPDATE m1_forward_batches SET upload_id = $2 WHERE id = $1")
        .bind(input.request.batch_id).bind(upload_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(ForwardUploadProgress { id: upload_id, progress: UploadProgress { received: 0, complete: false } }))
}

fn quota() -> Error {
    Error(StatusCode::INSUFFICIENT_STORAGE, "The service's attachment storage limit was reached.")
}

async fn verified_payload(db: &mut PgConnection, upload_id: Option<Uuid>, owner: Uuid) -> Result<Uuid> {
    let id = upload_id.ok_or_else(needs_original)?;
    let (size, received, digest, kind, complete, expired): (i64, i64, String, String, bool, bool) =
        sqlx::query_as(
            "SELECT size, received, sha256, kind, complete, coalesce(expires_at <= clock_timestamp(), false)
             FROM attachments WHERE id = $1 AND owner_id = $2 AND payload_id IS NULL FOR UPDATE",
        ).bind(id).bind(owner).fetch_optional(&mut *db).await?.ok_or_else(needs_original)?;
    if complete { return Ok(id); }
    if expired || size != received { return Err(needs_original()); }
    let (hash, video_candidate) = {
        let mut chunks = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT data FROM attachment_chunks WHERE attachment_id = $1 ORDER BY byte_offset",
        ).bind(id).fetch(&mut *db);
        let mut hash = Sha256::new();
        let mut count = 0;
        let mut candidate = false;
        while let Some(chunk) = chunks.try_next().await? {
            if count == 0 { candidate = kind == "file" && crate::media::candidate(&chunk); }
            count += chunk.len() as i64;
            hash.update(chunk);
        }
        if count != size { return Err(needs_original()); }
        (format!("{:x}", hash.finalize()), candidate)
    };
    if hash != digest {
        return Err(Error(StatusCode::UNPROCESSABLE_ENTITY, "The uploaded original failed verification."));
    }
    sqlx::query("UPDATE attachments SET complete = true WHERE id = $1").bind(id).execute(&mut *db).await?;
    if video_candidate { crate::media::enqueue(db, id).await?; }
    Ok(id)
}

async fn publish_target(
    app: &App, auth: &Auth, source_id: Uuid, input: &ForwardRequest, target: &ForwardTarget,
) -> Result<Publication> {
    let mut tx = app.db.begin().await?;
    lock_message_feed(&mut tx).await?;
    auth.require(&mut tx).await?;
    let batch = batch(&mut tx, input, source_id, auth.user_id).await?;
    let existing: Option<(i64, Uuid, Uuid, Option<Uuid>)> = sqlx::query_as(
        "SELECT seq, sender_id, conversation_id, forward_batch FROM messages WHERE attachment_id = $1",
    ).bind(target.attachment_id).fetch_optional(&mut *tx).await?;
    if let Some((seq, owner, conversation, key)) = existing {
        if owner != auth.user_id || conversation != target.conversation_id || key != Some(input.batch_id) {
            return Err(Error::conflict("The forwarding attachment ID is already in use."));
        }
        return sync::publication(&mut tx, seq).await;
    }
    let source = source(&mut tx, source_id, auth.user_id).await?;
    require_send_access(&mut tx, target.conversation_id, auth.user_id).await?;
    let root = if source.expired {
        match verified_payload(&mut tx, batch.upload_id, auth.user_id).await {
            Ok(id) => id,
            Err(error @ Error(StatusCode::UNPROCESSABLE_ENTITY, _)) => {
                let id = batch.upload_id.ok_or_else(Error::internal)?;
                sqlx::query("DELETE FROM attachment_chunks WHERE attachment_id = $1")
                    .bind(id).execute(&mut *tx).await?;
                sqlx::query("UPDATE attachments SET received = 0 WHERE id = $1")
                    .bind(id).execute(&mut *tx).await?;
                tx.commit().await?;
                return Err(error);
            }
            Err(error) => return Err(error),
        }
    } else {
        source.payload_id.ok_or_else(Error::missing)?
    };
    let size: i64 = sqlx::query_scalar("SELECT size FROM attachments WHERE id = $1 AND complete AND payload_id IS NULL")
        .bind(root).fetch_optional(&mut *tx).await?.ok_or_else(Error::missing)?;
    let (account, count, _, total_count) = storage_usage(&mut tx, auth.user_id).await?;
    if account + size > ACCOUNT_BYTES || count >= 256 || total_count >= 4096 { return Err(quota()); }
    let collision: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM attachments WHERE id = $1)")
        .bind(target.attachment_id).fetch_one(&mut *tx).await?;
    if collision { return Err(Error::conflict("The forwarding attachment ID is already in use.")); }

    // The feed lock orders recalls, membership changes, expiry sweeps and publication.
    // Recheck the clock after potentially hashing an uploaded original.
    auth.require(&mut tx).await?;
    let current = self::source(&mut tx, source_id, auth.user_id).await?;
    if current.expired != source.expired {
        return Err(needs_original());
    }
    let seq: i64 = sqlx::query_scalar(
        "INSERT INTO messages (conversation_id, sender_id, attachment_id, expires_at, attachment_fingerprint, forward_batch)
         SELECT $1,$2,$3,clock_timestamp() + make_interval(secs => $4::int),$5,$6
         FROM messages src WHERE src.attachment_id = $7 AND src.recall_seq IS NULL
           AND coalesce(src.expires_at <= clock_timestamp(), false) = $8
         RETURNING seq",
    ).bind(target.conversation_id).bind(auth.user_id).bind(target.attachment_id)
        .bind(app.retention.file_secs).bind(current.fingerprint).bind(input.batch_id)
        .bind(source_id).bind(source.expired).fetch_optional(&mut *tx).await?.ok_or_else(needs_original)?;
    sqlx::query(
        "INSERT INTO attachments (id, conversation_id, owner_id, name, kind, size, sha256, received, complete, expires_at, payload_id)
         SELECT $1,$2,$3,name,kind,size,sha256,size,true,(SELECT expires_at FROM messages WHERE seq = $4),id
         FROM attachments WHERE id = $5",
    ).bind(target.attachment_id).bind(target.conversation_id).bind(auth.user_id).bind(seq).bind(root)
        .execute(&mut *tx).await?;
    sync::dispatch(&mut tx, seq).await?;
    let publication = sync::publication(&mut tx, seq).await?;
    tx.commit().await?;
    Ok(publication)
}

pub(crate) async fn publish(
    State(app): State<App>, auth: Auth, Path(id): Path<Uuid>, Json(input): Json<ForwardRequest>,
) -> Result<Json<Vec<ForwardResult>>> {
    let mut tx = app.db.begin().await?;
    lock_message_feed(&mut tx).await?;
    auth.require(&mut tx).await?;
    batch(&mut tx, &input, id, auth.user_id).await?;
    tx.commit().await?;
    let mut results = Vec::with_capacity(input.targets.len());
    for target in &input.targets {
        let (status, publication, error) = match publish_target(&app, &auth, id, &input, target).await {
            Ok(stamp) => (200, Some(stamp), None),
            Err(Error(status, message)) => (status.as_u16(), None, Some(message.to_owned())),
        };
        results.push(ForwardResult { target: target.clone(), status, publication, error });
    }
    Ok(Json(results))
}
