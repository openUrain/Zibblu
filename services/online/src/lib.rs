mod accounts;
mod chats;
mod files;
mod group_invites;
mod lan_credentials;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, FromRequestParts},
    http::{StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::Semaphore;
use uuid::Uuid;
use zibblu_core::CHUNK_SIZE;

#[derive(Clone)]
struct App {
    db: PgPool,
    password_work: Arc<Semaphore>,
    lan_authority: Arc<lan_credentials::Authority>,
}

pub async fn router(db: PgPool) -> anyhow::Result<Router> {
    sqlx::migrate!().run(&db).await?;
    let lan_authority = Arc::new(lan_credentials::Authority::load(&db).await?);
    let json_routes = Router::new()
        .route("/v1/register", post(accounts::register))
        .route("/v1/login", post(accounts::login))
        .route("/v1/session", delete(accounts::logout))
        .route("/v1/lan/credential", post(lan_credentials::issue))
        .route("/v1/users/{handle}", get(accounts::find_user))
        .route("/v1/contacts", post(accounts::add_contact))
        .route("/v1/contacts/{id}", delete(accounts::remove_contact))
        .route("/v1/sync", get(chats::sync))
        .route("/v1/conversations", post(chats::create))
        .route("/v1/conversations/{id}/invite", post(group_invites::issue))
        .route("/v1/group-invites/preview", post(group_invites::preview))
        .route("/v1/group-invites/join", post(group_invites::join))
        .route("/v1/conversations/{id}/messages", post(chats::send))
        .route("/v1/conversations/{id}/uploads", post(files::begin))
        .route("/v1/files/{id}", delete(files::discard))
        .route("/v1/files/{id}/complete", post(files::complete))
        .layer(DefaultBodyLimit::max(64 * 1024));
    let chunks = Router::new()
        .route(
            "/v1/files/{id}/chunks/{offset}",
            get(files::read_chunk).put(files::write_chunk),
        )
        .layer(DefaultBodyLimit::max(CHUNK_SIZE as usize));
    Ok(json_routes.merge(chunks).with_state(App {
        db,
        password_work: Arc::new(Semaphore::new(4)),
        lan_authority,
    }))
}

struct Auth {
    user_id: Uuid,
    token_hash: Vec<u8>,
    expires_at: i64,
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
        let (user_id, expires_at) = sqlx::query_as(
            "SELECT account_id, floor(extract(epoch FROM expires_at))::BIGINT
             FROM sessions WHERE token_hash = $1 AND expires_at > now()",
        )
        .bind(&token_hash)
        .fetch_optional(&app.db)
        .await?
        .ok_or_else(Error::unauthorized)?;
        Ok(Self {
            user_id,
            token_hash,
            expires_at,
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
