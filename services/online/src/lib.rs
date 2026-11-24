mod accounts;
mod chats;
mod devices;
mod files;
mod group_invites;
mod lan_credentials;
pub mod media;
mod profiles;
pub mod retention;
mod sync;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, FromRequestParts, State},
    http::{StatusCode, header, request::Parts},
    middleware,
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, PgPool};
use std::{sync::Arc, time::Duration};
use tokio::sync::Semaphore;
use uuid::Uuid;
use zibblu_core::{CHUNK_SIZE, MAX_MESSAGE_BYTES, MAX_PROFILE_IMAGE_BYTES};

#[derive(Clone)]
struct App {
    db: PgPool,
    password_work: Arc<Semaphore>,
    lan_authority: Arc<lan_credentials::Authority>,
    retention: retention::Retention,
}

pub async fn router(db: PgPool) -> anyhow::Result<Router> {
    router_with_retention(db, retention::Retention::default()).await
}

pub async fn router_with_retention(
    db: PgPool,
    retention: retention::Retention,
) -> anyhow::Result<Router> {
    sqlx::migrate!().run(&db).await?;
    let lan_authority = Arc::new(lan_credentials::Authority::load(&db).await?);
    let json_routes = Router::new()
        .route("/v1/register", post(upgrade_required))
        .route("/v1/login", post(upgrade_required))
        .route("/v1/sync", get(upgrade_required))
        .route("/v2/register", post(accounts::register))
        .route("/v2/login", post(accounts::login))
        .route(
            "/v2/session",
            get(devices::identity).delete(accounts::logout),
        )
        .route("/v2/devices", get(devices::list))
        .route("/v2/devices/{id}", delete(devices::retire))
        .route("/v1/lan/credential", post(lan_credentials::issue))
        .route("/v1/users/{handle}", get(accounts::find_user))
        .route("/v1/contacts", post(accounts::add_contact))
        .route(
            "/v1/contacts/{id}",
            delete(accounts::remove_contact).put(profiles::set_notes),
        )
        .route("/v1/profiles/{id}", get(profiles::profile))
        .route("/v1/profiles/me/bio", put(profiles::set_bio))
        .route("/v1/profiles/me/name", put(profiles::set_name))
        .route("/v1/profiles/{id}/images/{version}", get(profiles::image))
        .route(
            "/v1/contacts/{id}/decision",
            post(accounts::respond_contact),
        )
        .route("/v2/sync", get(sync::page))
        .route("/v2/sync/ack", post(sync::ack))
        .route("/v2/history/status", post(sync::history_status))
        .route("/v1/conversations", post(chats::create))
        .route("/v1/conversations/{id}/membership", delete(chats::leave))
        .route("/v1/conversations/{id}/invite", post(group_invites::issue))
        .route("/v1/group-invites/preview", post(group_invites::preview))
        .route("/v1/group-invites/join", post(group_invites::join))
        .route("/v1/conversations/{id}/read", post(chats::read))
        .route("/v1/read-receipts", put(chats::set_read_receipts))
        .route(
            "/v1/conversations/{id}/messages/{seq}",
            delete(chats::recall),
        )
        .route(
            "/v1/conversations/{id}/ai-replies",
            post(chats::publish_ai_reply),
        )
        .route("/v1/conversations/{id}/uploads", post(files::begin))
        .route("/v1/files/{id}", delete(files::discard))
        .route("/v1/files/{id}/complete", post(files::complete))
        .route("/v2/files/{id}/download", post(files::start_download))
        .route("/v2/files/{id}/ack", post(files::ack_content))
        .route("/v2/files/{id}/media", get(media::metadata))
        .route("/v2/files/{id}/media/{kind}", get(media::read))
        .layer(DefaultBodyLimit::max(64 * 1024));
    let text_publish = Router::new()
        .route("/v1/conversations/{id}/messages", post(chats::send))
        .layer(DefaultBodyLimit::max(MAX_MESSAGE_BYTES * 6 + 1024));
    let chunks = Router::new()
        .route(
            "/v1/files/{id}/chunks/{offset}",
            get(files::read_chunk).put(files::write_chunk),
        )
        .layer(DefaultBodyLimit::max(CHUNK_SIZE as usize));
    let profile_upload = Router::new()
        .route("/v1/profiles/me/images/{kind}", put(profiles::set_image))
        .layer(DefaultBodyLimit::max(MAX_PROFILE_IMAGE_BYTES));
    Ok(json_routes
        .merge(text_publish)
        .merge(chunks)
        .merge(profile_upload)
        .route("/healthz", get(health))
        .layer(middleware::map_response(
            |mut response: Response| async move {
                response.headers_mut().insert(
                    header::CACHE_CONTROL,
                    header::HeaderValue::from_static("private, no-store"),
                );
                response
            },
        ))
        .with_state(App {
            db,
            password_work: Arc::new(Semaphore::new(4)),
            lan_authority,
            retention,
        }))
}

async fn upgrade_required() -> Error {
    Error(
        StatusCode::UPGRADE_REQUIRED,
        "Update Zibblu to sign in and sync with this service.",
    )
}

async fn health(State(app): State<App>) -> impl IntoResponse {
    let probe = sqlx::query("SELECT 1").execute(&app.db);
    let status = match tokio::time::timeout(Duration::from_secs(2), probe).await {
        Ok(Ok(_)) => StatusCode::OK,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    (status, [(header::CACHE_CONTROL, "no-store")])
}

struct Auth {
    user_id: Uuid,
    token_hash: Vec<u8>,
    expires_at: i64,
    device_id: Uuid,
}

const ACTIVE_SESSION: &str =
    "SELECT s.account_id, floor(extract(epoch FROM s.expires_at))::BIGINT, s.device_id
     FROM sessions s JOIN accounts a ON a.id = s.account_id
     JOIN m1_devices d ON d.id = s.device_id JOIN m1_feed f ON f.epoch = d.epoch
     WHERE s.token_hash = $1 AND s.expires_at > clock_timestamp()
       AND a.revoked_at IS NULL AND d.retired_at IS NULL AND coalesce(d.active_until > clock_timestamp(), true)";

impl Auth {
    async fn require(&self, db: &mut PgConnection) -> Result<()> {
        sqlx::query_as::<_, (Uuid, i64, Uuid)>(ACTIVE_SESSION)
            .bind(&self.token_hash)
            .fetch_optional(db)
            .await?
            .ok_or_else(Error::unauthorized)?;
        Ok(())
    }
}

impl FromRequestParts<App> for Auth {
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, app: &App) -> Result<Self> {
        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .filter(|token| token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(Error::unauthorized)?;
        let token_hash = Sha256::digest(token.as_bytes()).to_vec();
        let (user_id, expires_at, device_id) = sqlx::query_as(ACTIVE_SESSION)
            .bind(&token_hash)
            .fetch_optional(&app.db)
            .await?
            .ok_or_else(Error::unauthorized)?;
        Ok(Self {
            user_id,
            token_hash,
            expires_at,
            device_id,
        })
    }
}

type Result<T> = std::result::Result<T, Error>;

struct Error(StatusCode, &'static str);

impl Error {
    fn bad(message: &'static str) -> Self {
        Self(StatusCode::BAD_REQUEST, message)
    }
    fn unauthorized() -> Self {
        Self(StatusCode::UNAUTHORIZED, "Sign in to continue.")
    }
    fn missing() -> Self {
        Self(StatusCode::NOT_FOUND, "This item is unavailable.")
    }
    fn conflict(message: &'static str) -> Self {
        Self(StatusCode::CONFLICT, message)
    }
    fn internal() -> Self {
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "The service could not complete the request.",
        )
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

impl From<sqlx::Error> for Error {
    fn from(error: sqlx::Error) -> Self {
        let code = error.as_database_error().and_then(|error| error.code());
        eprintln!(
            "Database operation failed ({})",
            code.as_deref().unwrap_or("connection or query error")
        );
        Self::internal()
    }
}
