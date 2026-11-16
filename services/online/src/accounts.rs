use crate::{
    App, Auth, Error, Result,
    chats::{insert_conversation, lock_message_feed},
    devices, sync,
};
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
use zibblu_core::{ContactDecision, ContactRequest, DeviceLogin, Session, User, valid_handle};

pub(crate) async fn register(
    State(app): State<App>,
    Json(input): Json<DeviceLogin>,
) -> Result<Json<Session>> {
    let key = devices::credential_hash(&input.device_key)?;
    let input = input.credentials;
    let handle = input.handle.trim().to_owned();
    if !valid_handle(&handle) || input.password.chars().count() < 12 || input.password.len() > 256 {
        return Err(Error::bad(
            "Use a username of 3–32 letters, numbers or underscores starting with a letter, and a password of at least 12 characters (256 bytes maximum).",
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
    let result = sqlx::query(
        "INSERT INTO accounts (id, handle, password_hash) VALUES ($1,$2,$3) ON CONFLICT DO NOTHING",
    )
    .bind(user.id)
    .bind(&user.handle)
    .bind(hash)
    .execute(&mut *tx)
    .await?;
    if result.rows_affected() == 0 {
        return Err(Error::conflict("That username is already in use."));
    }
    let session = issue_session(&mut tx, &app, user, &key).await?;
    tx.commit().await?;
    Ok(Json(session))
}

pub(crate) async fn login(
    State(app): State<App>,
    Json(input): Json<DeviceLogin>,
) -> Result<Json<Session>> {
    let key = devices::credential_hash(&input.device_key)?;
    let input = input.credentials;
    let handle = input.handle.trim().to_ascii_lowercase();
    let invalid = || {
        Error(
            StatusCode::UNAUTHORIZED,
            "Check your username and password.",
        )
    };
    if !valid_handle(&handle) || input.password.len() > 256 {
        return Err(invalid());
    }
    let (id, handle, hash): (Uuid, String, String) = sqlx::query_as(
        "SELECT id, handle, password_hash FROM accounts WHERE lower(handle COLLATE \"C\") = $1 AND revoked_at IS NULL",
    )
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
    let session = issue_session(&mut tx, &app, User { id, handle }, &key).await?;
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

async fn issue_session(
    db: &mut sqlx::PgConnection,
    app: &App,
    user: User,
    key: &[u8],
) -> Result<Session> {
    let device_id = devices::enroll(&mut *db, app, user.id, key).await?;
    let cursor = sqlx::query_scalar("SELECT cursor FROM m1_devices WHERE id = $1")
        .bind(device_id)
        .fetch_one(&mut *db)
        .await?;
    sqlx::query("DELETE FROM sessions WHERE device_id = $1")
        .bind(device_id)
        .execute(&mut *db)
        .await?;
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    sqlx::query("INSERT INTO sessions (token_hash, account_id, device_id) VALUES ($1,$2,$3)")
        .bind(Sha256::digest(token.as_bytes()).as_slice())
        .bind(user.id)
        .bind(device_id)
        .execute(db)
        .await?;
    Ok(Session {
        user,
        token,
        device_id,
        cursor,
    })
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
    let (id, handle) =
        sqlx::query_as("SELECT id, handle FROM accounts WHERE lower(handle COLLATE \"C\") = $1")
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
    let greeting = input.greeting.trim();
    if greeting.chars().count() > 200 || greeting.contains('\0') {
        return Err(Error::bad("Enter a greeting of up to 200 characters."));
    }
    let mut tx = app.db.begin().await?;
    lock_pair(&mut tx, auth.user_id, input.user_id).await?;
    let inserted =
        sqlx::query("INSERT INTO m1_contact_keys VALUES ($1, $2, $3) ON CONFLICT DO NOTHING")
            .bind(input.request_id)
            .bind(auth.user_id)
            .bind(input.user_id)
            .execute(&mut *tx)
            .await?;
    if inserted.rows_affected() == 0 {
        let same: bool = sqlx::query_scalar(
            "SELECT requester_id = $2 AND recipient_id = $3 FROM m1_contact_keys WHERE request_id = $1",
        ).bind(input.request_id).bind(auth.user_id).bind(input.user_id).fetch_one(&mut *tx).await?;
        return if same {
            Ok(StatusCode::NO_CONTENT)
        } else {
            Err(Error::conflict(
                "This request ID was used for another contact request.",
            ))
        };
    }
    let (approved, request, outgoing): (bool, Option<Uuid>, i64) = sqlx::query_as(
        "SELECT EXISTS(SELECT 1 FROM contacts WHERE owner_id = $1 AND contact_id = $2),
                (SELECT requester_id FROM contact_requests WHERE
                    (requester_id = $1 AND recipient_id = $2) OR (requester_id = $2 AND recipient_id = $1)),
                (SELECT count(*) FROM contact_requests WHERE requester_id = $1)",
    )
    .bind(auth.user_id)
    .bind(input.user_id)
    .fetch_one(&mut *tx)
    .await?;
    if approved || request == Some(auth.user_id) {
        tx.commit().await?;
        return Ok(StatusCode::NO_CONTENT);
    }
    if request.is_some() {
        return Err(Error::conflict(
            "This person already sent you a request. Respond in Contacts.",
        ));
    }
    if outgoing >= 200 {
        return Err(Error::conflict(
            "You can have up to 200 outgoing contact requests.",
        ));
    }
    sqlx::query(
        "INSERT INTO contact_requests (requester_id, recipient_id, greeting, request_id, greeting_expires_at)
         VALUES ($1,$2,$3,$4,clock_timestamp() + make_interval(secs => $5::int))",
    )
    .bind(auth.user_id)
    .bind(input.user_id)
    .bind(greeting)
    .bind(input.request_id)
    .bind(app.retention.text_secs)
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
    let mut tx = app.db.begin().await?;
    lock_message_feed(&mut tx).await?;
    auth.require(&mut tx).await?;
    lock_pair(&mut tx, auth.user_id, id).await?;
    sqlx::query(
        "DELETE FROM contacts WHERE (owner_id = $1 AND contact_id = $2)
                                OR (owner_id = $2 AND contact_id = $1)",
    )
    .bind(auth.user_id)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM contact_requests WHERE requester_id = $1 AND recipient_id = $2")
        .bind(auth.user_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn respond_contact(
    State(app): State<App>,
    auth: Auth,
    Path(requester): Path<Uuid>,
    Json(decision): Json<ContactDecision>,
) -> Result<StatusCode> {
    let mut tx = app.db.begin().await?;
    if decision.accept {
        lock_message_feed(&mut tx).await?;
    }
    auth.require(&mut tx).await?;
    lock_pair(&mut tx, auth.user_id, requester).await?;
    let pending: Option<(Uuid, String)> = sqlx::query_as(
        "DELETE FROM contact_requests WHERE requester_id = $1 AND recipient_id = $2
         AND ($3::uuid IS NULL OR request_id = $3)
         RETURNING request_id, CASE WHEN greeting_expires_at > clock_timestamp() THEN greeting ELSE '' END",
    )
    .bind(requester)
    .bind(auth.user_id)
    .bind(decision.request_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((request_id, greeting)) = pending else {
        let approved: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM contacts WHERE owner_id = $1 AND contact_id = $2)",
        )
        .bind(auth.user_id)
        .bind(requester)
        .fetch_one(&mut *tx)
        .await?;
        return if decision.accept && approved {
            Ok(StatusCode::NO_CONTENT)
        } else {
            Err(Error::missing())
        };
    };
    if decision.accept {
        let full: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM contacts WHERE owner_id IN ($1, $2)
                           GROUP BY owner_id HAVING count(*) >= 200)",
        )
        .bind(auth.user_id)
        .bind(requester)
        .fetch_one(&mut *tx)
        .await?;
        if full {
            return Err(Error::conflict(
                "An account has reached the 200-contact limit.",
            ));
        }
        sqlx::query("INSERT INTO contacts (owner_id, contact_id) VALUES ($1,$2), ($2,$1)")
            .bind(auth.user_id)
            .bind(requester)
            .execute(&mut *tx)
            .await?;
        let id = insert_conversation(&mut tx, auth.user_id, None, vec![requester]).await?;
        if !greeting.is_empty() {
            let seq: i64 = sqlx::query_scalar(
                "INSERT INTO messages (nonce, conversation_id, sender_id, text, expires_at)
                 VALUES ($1,$2,$3,$4,clock_timestamp() + make_interval(secs => $5::int)) RETURNING seq",
            ).bind(request_id).bind(id).bind(requester).bind(greeting).bind(app.retention.text_secs)
                .fetch_one(&mut *tx).await?;
            sync::dispatch(&mut tx, seq).await?;
        }
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn lock_pair(db: &mut sqlx::PgConnection, a: Uuid, b: Uuid) -> Result<()> {
    let users: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM accounts WHERE id IN ($1, $2) AND revoked_at IS NULL ORDER BY id FOR NO KEY UPDATE",
    )
    .bind(a)
    .bind(b)
    .fetch_all(db)
    .await?;
    if users.len() != 2 {
        return Err(Error::missing());
    }
    Ok(())
}
