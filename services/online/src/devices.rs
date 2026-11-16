use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use sha2::{Digest, Sha256};
use sqlx::PgConnection;
use uuid::Uuid;
use zibblu_core::{Device, DeviceSession};

use crate::{App, Auth, Error, Result, chats::lock_message_feed};

pub(crate) fn credential_hash(key: &str) -> Result<Vec<u8>> {
    if key.len() != 64 || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::bad("A valid installation credential is required."));
    }
    Ok(Sha256::digest(key.as_bytes()).to_vec())
}

pub(crate) async fn enroll(
    db: &mut PgConnection,
    app: &App,
    account: Uuid,
    key: &[u8],
) -> Result<Uuid> {
    lock_message_feed(db).await?;
    let (epoch, through): (Uuid, i64) =
        sqlx::query_as("SELECT epoch, committed_through FROM m1_feed")
            .fetch_one(&mut *db)
            .await?;
    let enabled: bool = sqlx::query_scalar("SELECT revoked_at IS NULL FROM accounts WHERE id = $1")
        .bind(account)
        .fetch_one(&mut *db)
        .await?;
    if !enabled {
        return Err(Error::unauthorized());
    }
    let existing: Option<(Uuid, bool)> = sqlx::query_as(
        "SELECT id, retired_at IS NULL AND coalesce(active_until > clock_timestamp(), true) AND epoch = $3
         FROM m1_devices WHERE account_id = $1 AND credential_hash = $2",
    ).bind(account).bind(key).bind(epoch).fetch_optional(&mut *db).await?;
    if let Some((device, active)) = existing {
        return if active {
            Ok(device)
        } else {
            Err(Error(
                StatusCode::GONE,
                "This device was retired. Register this installation again.",
            ))
        };
    }
    if let Some(limit) = app.retention.device_limit {
        let active: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM m1_devices WHERE account_id = $1 AND epoch = $2
             AND retired_at IS NULL AND coalesce(active_until > clock_timestamp(), true)",
        )
        .bind(account)
        .bind(epoch)
        .fetch_one(&mut *db)
        .await?;
        if active >= i64::from(limit) {
            return Err(Error::conflict(
                "Device limit reached. Retire an unused device from a signed-in device.",
            ));
        }
    }
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO m1_devices (id, account_id, credential_hash, active_until, epoch, acked_after, cursor)
         VALUES ($1, $2, $3, clock_timestamp() + make_interval(secs => $4::int), $5, $6, $7)",
    ).bind(id).bind(account).bind(key).bind(app.retention.device_idle_secs)
        .bind(epoch).bind(through).bind(Uuid::new_v4()).execute(&mut *db).await?;
    let earliest: Option<i64> = sqlx::query_scalar(
        "WITH backlog AS (
            INSERT INTO m1_device_events (device_id, event_seq, message_seq, content_state)
            SELECT $1, seq, seq, CASE WHEN attachment_id IS NOT NULL THEN 'pending' END
            FROM m1_visible_messages
            WHERE account_id = $2 AND recall_seq IS NULL
              AND coalesce(expires_at > clock_timestamp(), true)
              AND (NOT metadata_received OR (attachment_id IS NOT NULL AND NOT content_verified))
            RETURNING event_seq
         ) SELECT min(event_seq) FROM backlog",
    )
    .bind(id)
    .bind(account)
    .fetch_one(&mut *db)
    .await?;
    if let Some(first) = earliest {
        sqlx::query("UPDATE m1_devices SET acked_after = $2 WHERE id = $1")
            .bind(id)
            .bind(first - 1)
            .execute(db)
            .await?;
    }
    Ok(id)
}

pub(crate) async fn identity(State(app): State<App>, auth: Auth) -> Result<Json<DeviceSession>> {
    let (handle, cursor) = sqlx::query_as(
        "SELECT a.handle, d.cursor FROM accounts a JOIN m1_devices d ON d.account_id = a.id WHERE d.id = $1",
    ).bind(auth.device_id).fetch_one(&app.db).await?;
    Ok(Json(DeviceSession {
        user: zibblu_core::User {
            id: auth.user_id,
            handle,
        },
        device_id: auth.device_id,
        cursor,
    }))
}

pub(crate) async fn list(State(app): State<App>, auth: Auth) -> Result<Json<Vec<Device>>> {
    let rows: Vec<(Uuid, i64)> = sqlx::query_as(
        "SELECT id, (extract(epoch FROM last_seen_at) * 1000)::BIGINT
         FROM m1_devices WHERE account_id = $1 AND retired_at IS NULL
           AND coalesce(active_until > clock_timestamp(), true)
           AND epoch = (SELECT epoch FROM m1_feed)
         ORDER BY last_seen_at DESC, id",
    )
    .bind(auth.user_id)
    .fetch_all(&app.db)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(id, last_seen_at)| Device {
                id,
                last_seen_at,
                current: id == auth.device_id,
            })
            .collect(),
    ))
}

pub(crate) async fn retire(
    State(app): State<App>,
    auth: Auth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    let mut tx = app.db.begin().await?;
    lock_message_feed(&mut tx).await?;
    auth.require(&mut tx).await?;
    let updated = sqlx::query(
        "UPDATE m1_devices SET retired_at = coalesce(retired_at, clock_timestamp()),
           page_id = NULL, page_next = NULL, page_offered = NULL
         WHERE account_id = $1 AND id = $2",
    )
    .bind(auth.user_id)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(Error::missing());
    }
    sqlx::query("DELETE FROM sessions WHERE device_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM m1_device_events WHERE device_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
