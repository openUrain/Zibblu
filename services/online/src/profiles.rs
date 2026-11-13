use crate::{App, Auth, Error, Result};
use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
};
use sqlx::FromRow;
use std::io::Cursor;
use uuid::Uuid;
use zibblu_core::{ContactNotes, Profile, ProfileBio, ProfileImage, ProfileRelation, User};

#[derive(FromRow)]
struct ProfileRow {
    handle: String,
    bio: Option<String>,
    avatar: Option<Uuid>,
    cover: Option<Uuid>,
    remark: Option<String>,
    contact_info: Option<String>,
    requester: Option<Uuid>,
}

pub(crate) async fn profile(
    State(app): State<App>,
    auth: Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<Profile>> {
    let row: ProfileRow = sqlx::query_as(
        "SELECT a.handle, c.remark, c.contact_info, r.requester_id AS requester,
                CASE WHEN a.id = $1 OR c.owner_id IS NOT NULL THEN a.bio END AS bio,
                avatar.version AS avatar, cover.version AS cover
         FROM accounts a
         LEFT JOIN contacts c ON c.owner_id = $1 AND c.contact_id = a.id
         LEFT JOIN contact_requests r ON (r.requester_id = $1 AND r.recipient_id = a.id)
                                      OR (r.recipient_id = $1 AND r.requester_id = a.id)
         LEFT JOIN profile_images avatar ON avatar.account_id = a.id AND avatar.kind = 'avatar'
         LEFT JOIN profile_images cover ON cover.account_id = a.id AND cover.kind = 'cover'
                                      AND (a.id = $1 OR c.owner_id IS NOT NULL)
         WHERE a.id = $2",
    )
    .bind(auth.user_id)
    .bind(id)
    .fetch_optional(&app.db)
    .await?
    .ok_or_else(Error::missing)?;
    let relation = if id == auth.user_id {
        ProfileRelation::Own
    } else if row.remark.is_some() {
        ProfileRelation::Contact
    } else if row.requester == Some(auth.user_id) {
        ProfileRelation::Outgoing
    } else if row.requester.is_some() {
        ProfileRelation::Incoming
    } else {
        ProfileRelation::Stranger
    };
    Ok(Json(Profile {
        user: User {
            id,
            handle: row.handle,
        },
        relation,
        avatar: row.avatar,
        cover: row.cover,
        bio: row.bio,
        notes: row
            .remark
            .zip(row.contact_info)
            .map(|(remark, contact_info)| ContactNotes {
                remark,
                contact_info,
            }),
    }))
}

pub(crate) async fn set_bio(
    State(app): State<App>,
    auth: Auth,
    Json(input): Json<ProfileBio>,
) -> Result<StatusCode> {
    let bio = input.bio.trim();
    if bio.chars().count() > 300 || bio.contains('\0') {
        return Err(Error::bad("Enter a bio of up to 300 characters."));
    }
    sqlx::query("UPDATE accounts SET bio = $2 WHERE id = $1")
        .bind(auth.user_id)
        .bind(bio)
        .execute(&app.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn set_notes(
    State(app): State<App>,
    auth: Auth,
    Path(id): Path<Uuid>,
    Json(input): Json<ContactNotes>,
) -> Result<StatusCode> {
    let remark = input.remark.trim();
    let info = input.contact_info.trim();
    if remark.chars().count() > 64
        || remark.chars().any(char::is_control)
        || info.chars().count() > 300
        || info.contains('\0')
    {
        return Err(Error::bad(
            "Use up to 64 characters for the remark and 300 for contact details.",
        ));
    }
    let updated = sqlx::query("UPDATE contacts SET remark = $3, contact_info = $4 WHERE owner_id = $1 AND contact_id = $2")
        .bind(auth.user_id).bind(id).bind(remark).bind(info).execute(&app.db).await?;
    if updated.rows_affected() == 0 {
        return Err(Error::missing());
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn image(
    State(app): State<App>,
    auth: Auth,
    Path((id, version)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse> {
    let bytes: Vec<u8> = sqlx::query_scalar(
        "SELECT data FROM profile_images WHERE account_id = $2 AND version = $3
           AND (kind = 'avatar' OR account_id = $1 OR EXISTS (
               SELECT 1 FROM contacts WHERE owner_id = $1 AND contact_id = $2
           ))",
    )
    .bind(auth.user_id)
    .bind(id)
    .bind(version)
    .fetch_optional(&app.db)
    .await?
    .ok_or_else(Error::missing)?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        bytes,
    ))
}

pub(crate) async fn set_image(
    State(app): State<App>,
    auth: Auth,
    Path(kind): Path<ProfileImage>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Result<Json<Uuid>> {
    if headers
        .get(header::CONTENT_TYPE)
        .is_none_or(|value| value != "image/png")
        || !valid_png(&bytes)
    {
        return Err(Error::bad(
            "Choose a PNG image up to 2 MiB and 1024 × 1024 pixels.",
        ));
    }
    let version = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO profile_images (account_id, kind, version, data) VALUES ($1,$2,$3,$4)
         ON CONFLICT (account_id, kind) DO UPDATE SET version = excluded.version, data = excluded.data",
    )
    .bind(auth.user_id).bind(kind.as_str()).bind(version).bind(&bytes[..])
    .execute(&app.db).await?;
    Ok(Json(version))
}

fn valid_png(bytes: &[u8]) -> bool {
    let decoder = png::Decoder::new_with_limits(
        Cursor::new(bytes),
        png::Limits {
            bytes: 8 * 1024 * 1024,
        },
    );
    let Ok(mut reader) = decoder.read_info() else {
        return false;
    };
    let info = reader.info();
    if !(1..=1024).contains(&info.width)
        || !(1..=1024).contains(&info.height)
        || info.animation_control.is_some()
        || info.bit_depth == png::BitDepth::Sixteen
    {
        return false;
    }
    reader.output_buffer_size().is_some_and(|size| {
        reader.next_frame(&mut vec![0; size]).is_ok() && reader.finish().is_ok()
    })
}
