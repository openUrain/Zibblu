use crate::{App, Auth, Error, Result};
use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use zibblu_core::{ContactRequest, Credentials, Session, User, valid_handle};

pub(crate) async fn register(
    State(app): State<App>,
    Json(input): Json<Credentials>,
) -> Result<Json<Session>> {
    let handle = input.handle.trim().to_ascii_lowercase();
    if !valid_handle(&handle) || input.password.chars().count() < 12 || input.password.len() > 256 {
        return Err(Error::bad(
            "Use a handle of 3–32 letters, numbers or underscores starting with a letter, and a password of at least 12 characters (256 bytes maximum).",
        ));
    }
    let hash = password_job(&app, move || {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(input.password.as_bytes(), &salt)
            .map(|hash| hash.to_string())
            .map_err(|_| Error::internal())
    })
    .await?;
    let user = User {
        id: Uuid::new_v4(),
        handle,
    };
    let mut tx = app.db.begin().await?;
    let result = sqlx::query("INSERT INTO accounts (id, handle, password_hash) VALUES ($1,$2,$3) ON CONFLICT (handle) DO NOTHING")
        .bind(user.id).bind(&user.handle).bind(hash)
        .execute(&mut *tx).await?;
    if result.rows_affected() == 0 {
        return Err(Error::conflict("That handle is already in use."));
    }
    let session = issue_session(&mut tx, user).await?;
    tx.commit().await?;
    Ok(Json(session))
}

pub(crate) async fn login(
    State(app): State<App>,
    Json(input): Json<Credentials>,
) -> Result<Json<Session>> {
    let handle = input.handle.trim().to_ascii_lowercase();
    let invalid = || Error(StatusCode::UNAUTHORIZED, "Check your handle and password.");
    if !valid_handle(&handle) || input.password.len() > 256 {
        return Err(invalid());
    }
    let (id, hash): (Uuid, String) =
        sqlx::query_as("SELECT id, password_hash FROM accounts WHERE handle = $1")
            .bind(&handle)
            .fetch_optional(&app.db)
            .await?
            .ok_or_else(invalid)?;
    password_job(&app, move || {
        let hash = PasswordHash::new(&hash).map_err(|_| Error::internal())?;
        Argon2::default()
            .verify_password(input.password.as_bytes(), &hash)
            .map_err(|_| invalid())
    })
    .await?;
    let mut tx = app.db.begin().await?;
    sqlx::query("DELETE FROM sessions WHERE account_id = $1 AND expires_at <= now()")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let session = issue_session(&mut tx, User { id, handle }).await?;
    tx.commit().await?;
    Ok(Json(session))
}

async fn password_job<T: Send + 'static>(
    app: &App,
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    let permit = app.password_work.clone().try_acquire_owned().map_err(|_| {
        Error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Sign-in is busy. Try again in a moment.",
        )
    })?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    })
    .await
    .map_err(|_| Error::internal())?
}

async fn issue_session(db: &mut sqlx::PgConnection, user: User) -> Result<Session> {
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    sqlx::query("INSERT INTO sessions (token_hash, account_id) VALUES ($1,$2)")
        .bind(Sha256::digest(token.as_bytes()).as_slice())
        .bind(user.id)
        .execute(db)
        .await?;
    Ok(Session { user, token })
}

pub(crate) async fn logout(State(app): State<App>, auth: Auth) -> Result<StatusCode> {
    sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
        .bind(auth.token_hash)
        .execute(&app.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn find_user(
    State(app): State<App>,
    _auth: Auth,
    Path(handle): Path<String>,
) -> Result<Json<User>> {
    let handle = handle.to_ascii_lowercase();
    if !valid_handle(&handle) {
        return Err(Error::missing());
    }
    let id = sqlx::query_scalar("SELECT id FROM accounts WHERE handle = $1")
        .bind(&handle)
        .fetch_optional(&app.db)
        .await?
        .ok_or_else(Error::missing)?;
    Ok(Json(User { id, handle }))
}

pub(crate) async fn add_contact(
    State(app): State<App>,
    auth: Auth,
    Json(input): Json<ContactRequest>,
) -> Result<StatusCode> {
    if auth.user_id == input.user_id {
        return Err(Error::bad("Choose another account."));
    }
    let mut tx = app.db.begin().await?;
    sqlx::query("SELECT id FROM accounts WHERE id = $1 FOR NO KEY UPDATE")
        .bind(auth.user_id)
        .execute(&mut *tx)
        .await?;
    let (exists, count): (bool, i64) = sqlx::query_as(
        "SELECT EXISTS(SELECT 1 FROM accounts WHERE id = $2),
                (SELECT count(*) FROM contacts WHERE owner_id = $1 AND contact_id <> $2)",
    )
    .bind(auth.user_id)
    .bind(input.user_id)
    .fetch_one(&mut *tx)
    .await?;
    if !exists {
        return Err(Error::missing());
    }
    if count >= 200 {
        return Err(Error::conflict("You can save up to 200 contacts."));
    }
    sqlx::query(
        "INSERT INTO contacts (owner_id, contact_id) VALUES ($1,$2) ON CONFLICT DO NOTHING",
    )
    .bind(auth.user_id)
    .bind(input.user_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn remove_contact(
    State(app): State<App>,
    auth: Auth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    sqlx::query("DELETE FROM contacts WHERE owner_id = $1 AND contact_id = $2")
        .bind(auth.user_id)
        .bind(id)
        .execute(&app.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
