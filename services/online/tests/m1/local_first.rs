use super::*;
use axum::{
    body::{Body, Bytes},
    extract::Request,
    http::StatusCode,
    middleware::{self, Next},
    response::IntoResponse,
};
use sqlx::{PgPool, SqlitePool};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU8, Ordering},
};
use zibblu_core::{
    Attachment, AttachmentKind, ContentAck, DeviceSession, ExpiredPayload, ForwardRequest,
    ForwardResult, ForwardTarget, ForwardUploadProgress, ForwardUploadRequest, MAX_MESSAGE_BYTES,
    MediaDescriptor, MediaVariant, SYNC_PAGE_SIZE,
};
use zibblu_online::retention::Retention;

async fn test_db() -> Result<PgPool> {
    Ok(PgPoolOptions::new()
        .max_connections(6)
        .connect(&std::env::var("ZIBBLU_TEST_DATABASE_URL")?)
        .await?)
}

fn profile(data: &Path, server: &str, account: Uuid) -> PathBuf {
    let id = format!("{:x}", Sha256::digest(format!("{server}/\n{account}")));
    data.join("profiles-v2").join(id)
}

async fn sqlite(data: &Path, server: &str, account: Uuid) -> Result<SqlitePool> {
    Ok(SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(profile(data, server, account).join("chat.sqlite3")),
        )
        .await?)
}

async fn direct(alice: &Client, bob: &Client) -> Result<Uuid> {
    alice.add_contact(bob.user.id).await?;
    bob.respond_contact(alice.user.id, true).await?;
    alice
        .create_conversation(ConversationRequest::Direct {
            contact_id: bob.user.id,
        })
        .await
}

#[tokio::test(flavor = "multi_thread")]
async fn outbox_and_inbox_progress_independently_and_confirm_the_same_nonce() -> Result<()> {
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let hold = Arc::new(AtomicU8::new(1));
    let request_started = started.clone();
    let release_request = release.clone();
    let hold_request = hold.clone();
    let app = zibblu_online::router(test_db().await?).await?.layer(middleware::from_fn(
        move |request: Request, next: Next| {
            let started = request_started.clone();
            let release = release_request.clone();
            let hold = hold_request.clone();
            async move {
                let path = request.uri().path();
                if path == "/v2/sync" && hold.compare_exchange(1, 0, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                    // Pause before entering the handler so no server lock is held.
                    started.notify_one();
                    release.notified().await;
                }
                let wait_for_publish = path.ends_with("/messages")
                    && hold.compare_exchange(2, 0, Ordering::SeqCst, Ordering::SeqCst).is_ok();
                let response = next.run(request).await;
                if wait_for_publish && response.status().is_success() {
                    // Keep the committed message visible to sync while its response is in flight.
                    started.notify_one();
                    release.notified().await;
                }
                response
            }
        },
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let server = format!("http://{}", listener.local_addr()?);
    let task = tokio::spawn(async move { axum::serve(listener, app).await });
    let a = tempfile::tempdir()?;
    let b = tempfile::tempdir()?;
    let suffix = Uuid::new_v4().simple().to_string();
    let alice = sign_in(a.path(), &server, &format!("send_{}", &suffix[..10]), "fixture-password", true).await?;
    let bob = sign_in(b.path(), &server, &format!("recv_{}", &suffix[..10]), "fixture-password", true).await?;
    let chat = direct(&alice, &bob).await?;
    let syncing = tokio::spawn({
        let alice = alice.clone();
        async move { alice.sync_inbox().await }
    });
    tokio::time::timeout(Duration::from_secs(3), started.notified()).await?;
    let text = "Durable while the download of a sync page is stalled";
    alice.queue_message(chat, SendMessage { nonce: Uuid::new_v4(), text: text.into(), reply_to: None }).await?;
    let queued = alice.messages(chat, None).await?.items.pop().context("missing queued message")?;
    assert!(queued.seq < 0 && queued.pending.is_some());
    let published = tokio::time::timeout(Duration::from_secs(2), alice.flush_pending_messages()).await;
    release.notify_one();
    assert!(published.context("outbox blocked on inbox sync")??);
    syncing.await??;
    bob.sync().await?;
    let received = bob.messages(chat, None).await?.items;
    assert_eq!(received.iter().filter(|row| row.text.as_deref() == Some(text)).count(), 1);
    assert_eq!(alice.messages(chat, None).await?.items[0].seq, received[0].seq);

    let delayed_nonce = Uuid::new_v4();
    let following_nonce = Uuid::new_v4();
    for (nonce, text) in [(delayed_nonce, "Confirmed by sync first"), (following_nonce, "Next queued message")] {
        alice.queue_message(chat, SendMessage { nonce, text: text.into(), reply_to: None }).await?;
    }
    hold.store(2, Ordering::SeqCst);
    let flushing = tokio::spawn({
        let alice = alice.clone();
        async move { alice.flush_pending_messages().await }
    });
    tokio::time::timeout(Duration::from_secs(3), started.notified()).await?;
    alice.sync_inbox().await?;
    let saved = alice.messages(chat, None).await?.items;
    assert!(saved.iter().any(|row| row.nonce == Some(delayed_nonce) && row.pending.is_none()));
    assert!(saved.iter().any(|row| row.nonce == Some(following_nonce) && row.pending.is_some()));
    let deleted_seq = saved.iter().find(|row| row.nonce == Some(delayed_nonce))
        .context("missing sync-confirmed message")?.seq;
    alice.hide_message(chat, deleted_seq).await?;
    release.notify_one();
    assert!(flushing.await??);
    assert!(!alice.messages(chat, None).await?.items.iter().any(|row| row.seq == deleted_seq));
    assert!(alice.queue_message(chat, SendMessage {
        nonce: delayed_nonce,
        text: "Different text with the same ID".into(),
        reply_to: None,
    }).await.is_err());
    bob.sync().await?;
    let received = bob.messages(chat, None).await?.items;
    for text in ["Confirmed by sync first", "Next queued message"] {
        assert_eq!(received.iter().filter(|row| row.text.as_deref() == Some(text)).count(), 1);
    }
    task.abort();
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn deleted_conversation_detaches_sends_and_reopens_only_for_new_history() -> Result<()> {
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let hold = Arc::new(AtomicU8::new(1));
    let app = zibblu_online::router(test_db().await?).await?.layer(middleware::from_fn({
        let started = started.clone();
        let release = release.clone();
        let hold = hold.clone();
        move |request: Request, next: Next| {
            let started = started.clone();
            let release = release.clone();
            let hold = hold.clone();
            async move {
                let wait = request.uri().path().ends_with("/complete")
                    && hold.compare_exchange(1, 0, Ordering::SeqCst, Ordering::SeqCst).is_ok();
                let response = next.run(request).await;
                if wait && response.status().is_success() {
                    started.notify_one();
                    release.notified().await;
                }
                response
            }
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let server = format!("http://{}", listener.local_addr()?);
    let task = tokio::spawn(async move { axum::serve(listener, app).await });
    let a = tempfile::tempdir()?;
    let b = tempfile::tempdir()?;
    let suffix = &Uuid::new_v4().simple().to_string()[..10];
    let alice_handle = format!("delete_a_{suffix}");
    let bob_handle = format!("delete_b_{suffix}");
    let alice = sign_in(a.path(), &server, &alice_handle, "fixture-password", true).await?;
    let bob = sign_in(b.path(), &server, &bob_handle, "fixture-password", true).await?;
    let chat = direct(&alice, &bob).await?;
    alice.sync().await?;
    bob.sync().await?;
    bob.send_message(chat, SendMessage { nonce: Uuid::new_v4(), text: "old history".into(), reply_to: None }).await?;
    alice.sync().await?;
    alice.queue_message(chat, SendMessage { nonce: Uuid::new_v4(), text: "detached text".into(), reply_to: None }).await?;
    alice.delete_conversation(chat).await?;
    assert!(alice.messages(chat, None).await?.items.is_empty());
    assert!(!alice.directory().await?.conversations.iter().any(|row| row.id == chat));
    assert!(alice.membership_directory().await?.conversations.iter().any(|row| row.id == chat));
    alice.close().await;
    drop(alice);
    let alice = sign_in(a.path(), &server, &alice_handle, "fixture-password", false).await?;
    alice.sync().await?;
    bob.sync().await?;
    assert!(bob.messages(chat, None).await?.items.iter().any(|m| m.text.as_deref() == Some("detached text")));
    assert!(alice.messages(chat, None).await?.items.is_empty());
    assert!(!alice.directory().await?.conversations.iter().any(|row| row.id == chat));
    assert_eq!(alice.take_send_failure_notices().await?, 0);

    assert_eq!(alice.create_conversation(ConversationRequest::Direct { contact_id: bob.user.id }).await?, chat);
    assert!(alice.directory().await?.conversations.iter().any(|row| row.id == chat));
    let source = a.path().join("detached.txt");
    std::fs::write(&source, b"detached attachment")?;
    let upload = alice.queue_upload(chat, source).await?;
    let id = upload.attachment.id;
    tokio::time::timeout(Duration::from_secs(5), started.notified()).await?;
    alice.delete_conversation(chat).await?;
    alice.sync_inbox().await?;
    assert!(alice.messages(chat, None).await?.items.is_empty());
    assert!(!alice.directory().await?.conversations.iter().any(|row| row.id == chat));
    release.notify_one();
    let local = sqlite(a.path(), &server, alice.user.id).await?;
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let pending: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM transfers WHERE id = ? AND direction = 'upload')")
                .bind(id.to_string()).fetch_one(&local).await?;
            let spool = profile(a.path(), &server, alice.user.id).join("transfers").join(format!("upload-{id}.part"));
            if !pending && !spool.exists() { break; }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Ok::<_, anyhow::Error>(())
    }).await??;
    bob.sync().await?;
    assert!(bob.attachment(id).await.is_ok());
    assert!(alice.attachment(id).await.is_err());
    assert_eq!(alice.take_send_failure_notices().await?, 0);
    bob.send_message(chat, SendMessage { nonce: Uuid::new_v4(), text: "new history".into(), reply_to: None }).await?;
    alice.sync().await?;
    let rows = alice.messages(chat, None).await?.items;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].text.as_deref(), Some("new history"));
    assert!(alice.directory().await?.conversations.iter().any(|row| row.id == chat));

    // Server definitively rejects a fresh nonce after the contact is removed.
    alice.queue_message(chat, SendMessage { nonce: Uuid::new_v4(), text: "terminal failure".into(), reply_to: None }).await?;
    alice.delete_conversation(chat).await?;
    bob.remove_contact(alice.user.id).await?;
    alice.flush_pending_messages().await?;
    assert_eq!(alice.take_send_failure_notices().await?, 1);
    assert_eq!(alice.take_send_failure_notices().await?, 0);
    assert!(alice.messages(chat, None).await?.items.is_empty());
    let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM outbox WHERE conversation_id = ?")
        .bind(chat.to_string()).fetch_one(&local).await?;
    assert_eq!(pending, 0);
    task.abort();
    Ok(())
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
async fn recovery_retries_earlier_text_before_later_text_in_the_same_chat() -> Result<()> {
    let db = test_db().await?;
    let app = zibblu_online::router(db.clone()).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let server = format!("http://{}", listener.local_addr()?);
    let task = tokio::spawn(async move { axum::serve(listener, app).await });
    let a = tempfile::tempdir()?;
    let b = tempfile::tempdir()?;
    let suffix = &Uuid::new_v4().simple().to_string()[..10];
    let alice = sign_in(
        a.path(),
        &server,
        &format!("first_{suffix}"),
        "fixture-password",
        true,
    )
    .await?;
    let bob = sign_in(
        b.path(),
        &server,
        &format!("second_{suffix}"),
        "fixture-password",
        true,
    )
    .await?;
    let chat = direct(&alice, &bob).await?;
    let earlier = Uuid::new_v4();
    let later = Uuid::new_v4();
    for (nonce, text) in [
        (earlier, "Queued during the outage"),
        (later, "Queued after recovery"),
    ] {
        alice
            .queue_message(
                chat,
                SendMessage {
                    nonce,
                    text: text.into(),
                    reply_to: None,
                },
            )
            .await?;
    }

    let local = sqlite(a.path(), &server, alice.user.id).await?;
    sqlx::query("UPDATE outbox SET attempts = 4, next_attempt = ? WHERE nonce = ?")
        .bind(i64::MAX)
        .bind(earlier.to_string())
        .execute(&local)
        .await?;
    assert!(!alice.flush_pending_messages().await?);
    let queued = alice.messages(chat, None).await?.items;
    assert_eq!(queued.len(), 2);
    assert!(queued.iter().all(|row| row.pending.is_some()));

    sqlx::query("UPDATE outbox SET next_attempt = 0 WHERE nonce = ?")
        .bind(earlier.to_string())
        .execute(&local)
        .await?;
    assert!(alice.flush_pending_messages().await?);
    assert!(!alice.flush_pending_messages().await?);
    let order: Vec<Uuid> = sqlx::query_scalar(
        "SELECT nonce FROM messages WHERE conversation_id = $1 AND sender_id = $2 ORDER BY seq",
    )
    .bind(chat)
    .bind(alice.user.id)
    .fetch_all(&db)
    .await?;
    assert_eq!(order, vec![earlier, later]);
    bob.sync().await?;
    assert_eq!(bob.messages(chat, None).await?.items.len(), 2);
    task.abort();
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn maximum_utf8_text_survives_sync_pagination_and_over_limit_is_rejected() -> Result<()> {
    let app = zibblu_online::router(test_db().await?).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let server = format!("http://{}", listener.local_addr()?);
    let task = tokio::spawn(async move { axum::serve(listener, app).await });
    let a = tempfile::tempdir()?;
    let b = tempfile::tempdir()?;
    let suffix = Uuid::new_v4().simple().to_string();
    let alice = sign_in(a.path(), &server, &format!("long_{}", &suffix[..10]), "fixture-password", true).await?;
    let bob = sign_in(b.path(), &server, &format!("page_{}", &suffix[..10]), "fixture-password", true).await?;
    let chat = direct(&alice, &bob).await?;
    // Controls with six-byte JSON escapes exercise the largest encoded sync page.
    let boundary = format!("a{}界", "\u{1}".repeat(MAX_MESSAGE_BYTES - 4));
    let rejected = SendMessage { nonce: Uuid::new_v4(), text: format!("{boundary}x"), reply_to: None };
    let status = reqwest::Client::builder().no_proxy().build()?
        .post(format!("{server}/v1/conversations/{chat}/messages"))
        .bearer_auth(alice.session_token()).json(&rejected).send().await?.status();
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(alice.queue_message(chat, rejected).await.is_err());
    for _ in 0..33 {
        send(&alice, chat, &boundary).await?;
    }
    bob.sync().await?;
    let stored = bob.messages(chat, None).await?.items;
    assert_eq!(stored.len(), 33);
    assert!(stored.iter().all(|row| row.text.as_deref() == Some(&boundary) && row.attachment.is_none()));
    task.abort();
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn outbox_replay_ack_recovery_and_device_handoff() -> Result<()> {
    let db = test_db().await?;
    let app = zibblu_online::router_with_retention(
        db.clone(),
        Retention {
            device_idle_secs: Some(3_600),
            device_limit: Some(2),
            ..Retention::default()
        },
    )
    .await?;
    let fault = Arc::new(AtomicU8::new(0));
    let mode = fault.clone();
    let app = app.layer(middleware::from_fn(move |request: Request, next: Next| {
        let mode = mode.clone();
        async move {
            let path = request.uri().path();
            let lose = match mode.load(Ordering::SeqCst) {
                1 if path.ends_with("/messages") => {
                    return StatusCode::SERVICE_UNAVAILABLE.into_response();
                }
                2 if path.ends_with("/messages") => 2,
                3 if path == "/v2/sync/ack" => 3,
                _ => 0,
            };
            let response = next.run(request).await;
            if lose != 0 && response.status().is_success() {
                mode.store(0, Ordering::SeqCst);
                let stream = futures_util::stream::once(async {
                    Err::<Bytes, _>(std::io::Error::new(
                        std::io::ErrorKind::ConnectionReset,
                        "discarded response after commit",
                    ))
                });
                return Body::from_stream(stream).into_response();
            }
            response
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let server = format!("http://{}", listener.local_addr()?);
    let task = tokio::spawn(async move { axum::serve(listener, app).await });
    let a = tempfile::tempdir()?;
    let b = tempfile::tempdir()?;
    let b2 = tempfile::tempdir()?;
    let suffix = &Uuid::new_v4().simple().to_string()[..10];
    let password = "local-first-test-only";
    let alice_handle = format!("outbox_{suffix}");
    let bob_handle = format!("device_{suffix}");
    let alice = sign_in(a.path(), &server, &alice_handle, password, true).await?;
    let bob = sign_in(b.path(), &server, &bob_handle, password, true).await?;
    let chat = direct(&alice, &bob).await?;
    alice.sync().await?;
    bob.sync().await?;
    let sender = alice.user.id;
    let token = alice.session_token().to_owned();
    let outbox = sqlite(a.path(), &server, sender).await?;
    let local_bob = sqlite(b.path(), &server, bob.user.id).await?;

    let offline = Uuid::new_v4();
    fault.store(1, Ordering::SeqCst);
    alice
        .send_message(
            chat,
            SendMessage {
                nonce: offline,
                text: "Stored before reaching the service".into(),
                reply_to: None,
            },
        )
        .await?;
    let draft = alice.messages(chat, None).await?.items.remove(0);
    assert!(draft.seq < 0 && draft.nonce == Some(offline) && draft.pending.is_some());
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM messages WHERE nonce = $1")
        .bind(offline)
        .fetch_one(&db)
        .await?;
    assert_eq!(count, 0);
    drop(alice);
    fault.store(0, Ordering::SeqCst);
    let alice = Client::restore(a.path(), &server, sender, &token).await?;
    assert_eq!(
        alice.messages(chat, None).await?.items[0].nonce,
        Some(offline)
    );
    alice.retry_message(offline).await?;

    let lost = Uuid::new_v4();
    fault.store(2, Ordering::SeqCst);
    alice
        .send_message(
            chat,
            SendMessage {
                nonce: lost,
                text: "Committed with the response lost".into(),
                reply_to: None,
            },
        )
        .await?;
    let seq: i64 = sqlx::query_scalar("SELECT seq FROM messages WHERE nonce = $1")
        .bind(lost)
        .fetch_one(&db)
        .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM outbox WHERE nonce = ?")
            .bind(lost.to_string())
            .fetch_one(&outbox)
            .await?,
        1
    );
    drop(alice);
    let alice = Client::restore(a.path(), &server, sender, &token).await?;
    alice.retry_message(lost).await?;
    alice.sync().await?;
    assert_eq!(alice.messages(chat, None).await?.items[0].seq, seq);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM messages WHERE nonce = $1")
            .bind(lost)
            .fetch_one(&db)
            .await?,
        1
    );

    fault.store(3, Ordering::SeqCst);
    assert!(bob.sync().await.is_err());
    let pending: String = sqlx::query_scalar("SELECT pending_ack FROM sync_checkpoint")
        .fetch_one(&local_bob)
        .await?;
    let pending: SyncAck = serde_json::from_str(&pending)?;
    assert!(pending.metadata_received.contains(&seq));
    assert_eq!(bob.messages(chat, None).await?.items.len(), 2);
    let bob_id = bob.user.id;
    let bob_token = bob.session_token().to_owned();
    drop(bob);
    let bob = Client::restore(b.path(), &server, bob_id, &bob_token).await?;
    bob.sync().await?;
    assert_eq!(bob.messages(chat, None).await?.items.len(), 2);
    assert!(
        sqlx::query_scalar::<_, Option<String>>("SELECT pending_ack FROM sync_checkpoint")
            .fetch_one(&local_bob)
            .await?
            .is_none()
    );

    send(&alice, chat, "Waiting on a different installation").await?;
    let second = sign_in(b2.path(), &server, &bob_handle, password, false).await?;
    let third = tempfile::tempdir()?;
    assert!(
        sign_in(third.path(), &server, &bob_handle, password, false)
            .await
            .is_err()
    );
    let first_device = bob
        .devices()
        .await?
        .into_iter()
        .find(|device| device.current)
        .unwrap()
        .id;
    assert!(alice.retire_device(first_device).await.is_err());
    second.retire_device(first_device).await?;
    assert!(bob.sync().await.is_err());
    second.sync().await?;
    let backlog = second.messages(chat, None).await?.items;
    assert_eq!(backlog.len(), 1);
    assert_eq!(
        backlog[0].text.as_deref(),
        Some("Waiting on a different installation")
    );
    let second_device = second
        .devices()
        .await?
        .into_iter()
        .find(|device| device.current)
        .unwrap()
        .id;

    let http = reqwest::Client::builder().no_proxy().build()?;
    let identity: DeviceSession = http
        .get(format!("{server}/v2/session"))
        .bearer_auth(second.session_token())
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let fetch = || {
        http.get(format!("{server}/v2/sync?cursor={}", identity.cursor))
            .bearer_auth(second.session_token())
    };
    let empty: SyncPage = fetch().send().await?.error_for_status()?.json().await?;
    assert!(empty.events.is_empty() && !empty.has_more);
    for n in 0..SYNC_PAGE_SIZE + 1 {
        send(&alice, chat, &format!("Backlog page {n}")).await?;
    }
    let fixed: SyncPage = fetch().send().await?.error_for_status()?.json().await?;
    assert_eq!(fixed.page_id, empty.page_id);
    assert!(fixed.events.is_empty() && !fixed.has_more);
    second.sync().await?;
    second.sync().await?;
    let received: i64 = sqlx::query_scalar("SELECT count(*) FROM inbox")
        .fetch_one(&sqlite(b2.path(), &server, bob_id).await?)
        .await?;
    assert_eq!(received, SYNC_PAGE_SIZE as i64 + 2);

    alice.recall_message(chat, seq).await?;
    drop(bob);
    let rebuilt = sign_in(b.path(), &server, &bob_handle, password, false).await?;
    assert!(rebuilt.device_re_registered());
    assert_ne!(
        rebuilt
            .devices()
            .await?
            .into_iter()
            .find(|device| device.current)
            .unwrap()
            .id,
        first_device
    );
    let restored = rebuilt.messages(chat, None).await?.items;
    assert_eq!(restored.len(), 2);
    assert!(
        restored
            .iter()
            .any(|m| m.seq == seq && m.recalled && m.text.is_none())
    );

    sqlx::query("UPDATE m1_devices SET active_until = clock_timestamp() - interval '1 second' WHERE id = $1")
        .bind(second_device).execute(&db).await?;
    assert!(second.sync().await.is_err());
    sqlx::query("UPDATE accounts SET revoked_at = clock_timestamp() WHERE id = $1")
        .bind(bob_id)
        .execute(&db)
        .await?;
    assert!(
        sign_in(third.path(), &server, &bob_handle, password, false)
            .await
            .is_err()
    );
    assert!(send(&alice, chat, "Revoked peer").await.is_err());
    task.abort();
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn expiry_reclaims_payloads_and_keeps_verified_local_files() -> Result<()> {
    let db = test_db().await?;
    let app = zibblu_online::router_with_retention(
        db.clone(),
        Retention {
            text_secs: 120,
            file_secs: Some(120),
            upload_secs: Some(120),
            ..Retention::default()
        },
    )
    .await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let server = format!("http://{}", listener.local_addr()?);
    let task = tokio::spawn(async move { axum::serve(listener, app).await });
    let first = tempfile::tempdir()?;
    let second = tempfile::tempdir()?;
    let third = tempfile::tempdir()?;
    let suffix = &Uuid::new_v4().simple().to_string()[..10];
    let password = "local-first-expiry-fixture";
    let bob_handle = format!("expiry_b_{suffix}");
    let alice = sign_in(
        first.path(),
        &server,
        &format!("expiry_a_{suffix}"),
        password,
        true,
    )
    .await?;
    let bob = sign_in(second.path(), &server, &bob_handle, password, true).await?;
    let offline = sign_in(third.path(), &server, &bob_handle, password, false).await?;
    let chat = direct(&alice, &bob).await?;
    alice.sync().await?;
    send(&alice, chat, "Saved plaintext stays on this device").await?;
    let source = first.path().join("fixture.bin");
    let contents = b"fixture payload for SHA-256";
    std::fs::write(&source, contents)?;
    let upload = alice.queue_upload(chat, source).await?;
    let id = upload.attachment.id;
    wait_transfer(&alice, id, Direction::Upload, |job| {
        job.status == TransferStatus::Complete
    })
    .await?;
    bob.sync().await?;
    let file_msg = bob.attachment(id).await?;
    let file_seq = file_msg.seq;
    let http = reqwest::Client::builder().no_proxy().build()?;
    let chunk = format!("{server}/v1/files/{id}/chunks/0");
    assert_eq!(
        bob.read_attachment_chunk(id, 0, contents.len()).await?,
        contents
    );
    let flags = || {
        sqlx::query_as::<_, (bool, bool)>(
        "SELECT metadata_received, content_verified FROM m1_recipients WHERE message_seq = $1 AND account_id = $2",
    ).bind(file_seq).bind(bob.user.id)
    };
    assert_eq!(flags().fetch_one(&db).await?, (true, false));
    assert_eq!(
        http.post(format!("{server}/v2/files/{id}/ack"))
            .bearer_auth(bob.session_token())
            .json(&ContentAck {
                sha256: "0".repeat(64)
            })
            .send()
            .await?
            .status(),
        422
    );

    let spool = profile(second.path(), &server, bob.user.id)
        .join("transfers")
        .join(format!("download-{id}.part"));
    std::fs::write(&spool, vec![0; contents.len()])?;
    let dest = second.path().join("verify.bin");
    bob.queue_download(id, dest.clone()).await?;
    tokio::time::timeout(Duration::from_secs(15), async {
        while !bob
            .transfers()
            .await?
            .iter()
            .any(|job| job.direction == Direction::Download && job.status == TransferStatus::Failed)
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    bob.sync().await?;
    assert!(!dest.exists());
    assert_eq!(flags().fetch_one(&db).await?, (true, false));
    bob.resume(id, Direction::Download).await?;
    wait_transfer(&bob, id, Direction::Download, |job| {
        job.status == TransferStatus::Complete
    })
    .await?;
    bob.sync().await?;
    assert_eq!(flags().fetch_one(&db).await?, (true, true));
    assert_eq!(std::fs::read(dest)?, contents);

    let abandoned = Attachment {
        id: Uuid::new_v4(),
        name: "abandoned.bin".into(),
        kind: AttachmentKind::File,
        size: 1,
        sha256: format!("{:x}", Sha256::digest([7])),
    };
    let begin = || {
        http.post(format!("{server}/v1/conversations/{chat}/uploads"))
            .bearer_auth(alice.session_token())
            .json(&abandoned)
    };
    begin().send().await?.error_for_status()?;
    http.put(format!("{server}/v1/files/{}/chunks/0", abandoned.id))
        .bearer_auth(alice.session_token())
        .body(vec![7])
        .send()
        .await?
        .error_for_status()?;
    sqlx::query("UPDATE messages SET expires_at = clock_timestamp() - interval '1 second' WHERE conversation_id = $1")
        .bind(chat).execute(&db).await?;
    sqlx::query("UPDATE attachments SET expires_at = clock_timestamp() - interval '1 second' WHERE conversation_id = $1")
        .bind(chat).execute(&db).await?;
    assert_eq!(
        http.get(&chunk)
            .bearer_auth(offline.session_token())
            .send()
            .await?
            .status(),
        410
    );
    assert_eq!(begin().send().await?.status(), 410);

    let fixture = format!("reject_cleanup_{}", Uuid::new_v4().simple());
    sqlx::query(&format!(
        "CREATE FUNCTION {fixture}() RETURNS trigger LANGUAGE plpgsql AS
         $$ BEGIN IF OLD.id = '{id}'::uuid THEN RAISE EXCEPTION 'cleanup fixture'; END IF; RETURN OLD; END $$"
    )).execute(&db).await?;
    sqlx::query(&format!("CREATE TRIGGER {fixture} BEFORE DELETE ON attachments FOR EACH ROW EXECUTE FUNCTION {fixture}()"))
        .execute(&db).await?;
    let failure = zibblu_online::retention::sweep(&db).await;
    sqlx::query(&format!("DROP TRIGGER {fixture} ON attachments"))
        .execute(&db)
        .await?;
    sqlx::query(&format!("DROP FUNCTION {fixture}()"))
        .execute(&db)
        .await?;
    assert!(failure.is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM messages WHERE conversation_id = $1 AND text IS NOT NULL"
        )
        .bind(chat)
        .fetch_one(&db)
        .await?,
        1
    );
    zibblu_online::retention::sweep(&db).await?;
    zibblu_online::retention::sweep(&db).await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM attachments WHERE conversation_id = $1")
            .bind(chat)
            .fetch_one(&db)
            .await?,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM messages WHERE conversation_id = $1 AND text IS NOT NULL"
        )
        .bind(chat)
        .fetch_one(&db)
        .await?,
        0
    );
    assert_eq!(begin().send().await?.status(), 410);
    assert_eq!(
        http.get(&chunk)
            .bearer_auth(offline.session_token())
            .send()
            .await?
            .status(),
        410
    );
    assert_eq!(
        http.post(format!("{server}/v2/files/{id}/ack"))
            .bearer_auth(offline.session_token())
            .json(&ContentAck {
                sha256: upload.attachment.sha256.clone()
            })
            .send()
            .await?
            .status(),
        410
    );
    offline.sync().await?;
    let history = offline.messages(chat, None).await?.items;
    assert!(
        history
            .iter()
            .any(|m| m.expired == Some(ExpiredPayload::Text) && m.text.is_none())
    );
    assert!(
        history
            .iter()
            .any(|m| m.expired == Some(ExpiredPayload::File) && m.attachment.is_none())
    );
    assert_eq!(offline.directory().await?.conversations[0].unread, 0);
    bob.sync().await?;
    assert_eq!(
        bob.search_messages(chat, None, Some("Saved plaintext"))
            .await?
            .items
            .len(),
        1
    );
    assert!(bob.attachment(id).await?.local_file);
    assert_eq!(
        bob.read_attachment_chunk(id, 0, contents.len()).await?,
        contents
    );
    let saved_again = second.path().join("saved-after-expiry.bin");
    bob.queue_download(id, saved_again.clone()).await?;
    wait_transfer(&bob, id, Direction::Download, |job| {
        job.status == TransferStatus::Complete
    })
    .await?;
    assert_eq!(std::fs::read(saved_again)?, contents);
    task.abort();
    Ok(())
}


fn forwarding_request(chats: &[Uuid]) -> ForwardRequest {
    ForwardRequest {
        batch_id: Uuid::new_v4(),
        targets: chats.iter().map(|&conversation_id| ForwardTarget {
            conversation_id, attachment_id: Uuid::new_v4(),
        }).collect(),
    }
}

async fn expire_attachment(db: &PgPool, id: Uuid) -> Result<()> {
    let mut tx = db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(1001)").execute(&mut *tx).await?;
    sqlx::query("UPDATE messages SET expires_at=clock_timestamp()-interval '1 second' WHERE attachment_id=$1")
        .bind(id).execute(&mut *tx).await?;
    sqlx::query("UPDATE attachments SET expires_at=clock_timestamp()-interval '1 second' WHERE id=$1")
        .bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn attachment_forwarding_retries_expiry_and_local_original() -> Result<()> {
    let db = test_db().await?;
    let lose_response = Arc::new(AtomicU8::new(0));
    let response_gate = lose_response.clone();
    let app = zibblu_online::router_with_retention(db.clone(), Retention {
        file_secs: Some(604800), upload_secs: Some(86400), ..Retention::default()
    }).await?.layer(middleware::from_fn(move |request: Request, next: Next| {
        let gate = response_gate.clone();
        async move {
            let forward = request.uri().path().ends_with("/forward");
            let response = next.run(request).await;
            if forward && response.status() == StatusCode::OK
                && gate.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1)).ok() == Some(1) {
                // The database has committed, but the client cannot know the outcome.
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            response
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let server = format!("http://{}", listener.local_addr()?);
    let task = tokio::spawn(async move { axum::serve(listener, app).await });
    let first = tempfile::tempdir()?;
    let second = tempfile::tempdir()?;
    let third = tempfile::tempdir()?;
    let suffix = &Uuid::new_v4().simple().to_string()[..10];
    let password = "attachment-forwarding-fixture";
    let alice = sign_in(first.path(), &server, &format!("fw_a_{suffix}"), password, true).await?;
    let bob = sign_in(second.path(), &server, &format!("fw_b_{suffix}"), password, true).await?;
    let carol = sign_in(third.path(), &server, &format!("fw_c_{suffix}"), password, true).await?;
    let source_chat = direct(&alice, &bob).await?;
    let target_chat = direct(&bob, &carol).await?;
    let mut chats = vec![target_chat];
    for index in 0..8 {
        chats.push(bob.create_conversation(ConversationRequest::Group {
            title: format!("forward {index} {suffix}"), contact_ids: vec![alice.user.id, carol.user.id],
        }).await?);
    }
    alice.sync().await?;
    bob.sync().await?;
    carol.sync().await?;
    let path = first.path().join("forward.bin");
    let bytes = b"verified independent forwarding payload";
    std::fs::write(&path, bytes)?;
    let id = alice.queue_upload(source_chat, path.clone()).await?.attachment.id;
    wait_transfer(&alice, id, Direction::Upload, |job| job.status == TransferStatus::Complete).await?;
    bob.sync().await?;
    assert!(!bob.attachment(id).await?.local_file);

    let http = reqwest::Client::builder().no_proxy().build()?;
    let url = format!("{server}/v2/files/{id}/forward");
    let over_limit = forwarding_request(&[chats.clone(), vec![source_chat]].concat());
    assert_eq!(http.post(&url).bearer_auth(bob.session_token()).json(&over_limit).send().await?.status(), 400);
    let duplicate = forwarding_request(&[target_chat, target_chat]);
    assert_eq!(http.post(&url).bearer_auth(bob.session_token()).json(&duplicate).send().await?.status(), 400);
    let request = forwarding_request(&chats);
    assert_eq!(http.post(&url).bearer_auth(carol.session_token()).json(&request).send().await?.status(), 404);

    // Simulate forwarding on day six without downloading the original.
    sqlx::query("UPDATE messages SET expires_at=clock_timestamp()+interval '1 day' WHERE attachment_id=$1")
        .bind(id).execute(&db).await?;
    sqlx::query("UPDATE attachments SET expires_at=clock_timestamp()+interval '1 day' WHERE id=$1")
        .bind(id).execute(&db).await?;
    bob.remove_contact(carol.user.id).await?;
    lose_response.store(1, Ordering::SeqCst);
    assert!(bob.forward_attachment(id, &request).await.is_err());
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM messages WHERE forward_batch=$1")
        .bind(request.batch_id).fetch_one(&db).await?, 8);
    let (first_results, replay) = tokio::join!(
        bob.forward_attachment(id, &request), bob.forward_attachment(id, &request)
    );
    let first_results = first_results?;
    let replay = replay?;
    assert_eq!(first_results[0].status, 409);
    assert_eq!(replay[0].status, 409);
    for (left, right) in first_results[1..].iter().zip(&replay[1..]) {
        let a = left.publication.as_ref().context("group target failed")?;
        let b = right.publication.as_ref().context("group retry failed")?;
        assert_eq!(left.status, 200);
        assert_eq!((a.seq, a.expires_at), (b.seq, b.expires_at));
        assert!((a.expires_at.unwrap() - a.created_at - 604800000).abs() < 1000);
    }
    assert_eq!(direct(&bob, &carol).await?, target_chat);
    let all = bob.forward_attachment(id, &request).await?;
    assert!(all.iter().all(|item| item.status == 200));
    for (before, after) in first_results[1..].iter().zip(&all[1..]) {
        let a = before.publication.as_ref().unwrap();
        let b = after.publication.as_ref().unwrap();
        assert_eq!((a.seq, a.expires_at), (b.seq, b.expires_at));
    }
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM attachments WHERE payload_id=$1")
        .bind(id).fetch_one(&db).await?, 9);
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM attachment_chunks WHERE attachment_id=$1")
        .bind(id).fetch_one(&db).await?, 1);
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM attachment_chunks WHERE attachment_id=ANY($1)")
        .bind(request.targets.iter().map(|t| t.attachment_id).collect::<Vec<_>>()).fetch_one(&db).await?, 0);
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM messages WHERE forward_batch=$1")
        .bind(request.batch_id).fetch_one(&db).await?, 9);
    let changed = forwarding_request(&chats[..1]);
    let changed = ForwardRequest { batch_id: request.batch_id, ..changed };
    assert_eq!(http.post(&url).bearer_auth(bob.session_token()).json(&changed).send().await?.status(), 409);

    let target_id = request.targets[0].attachment_id;
    let chunk = |id| format!("{server}/v1/files/{id}/chunks/0");
    assert_eq!(http.get(chunk(id)).bearer_auth(carol.session_token()).send().await?.status(), 404);
    assert_eq!(http.get(chunk(target_id)).bearer_auth(alice.session_token()).send().await?.status(), 404);
    // Fill 255 logical slots and seven 256 MiB reservations. The eighth
    // maximum-size reservation fits only once Alice's original grant expires.
    let reserve: Vec<Uuid> = sqlx::query_scalar(
        "INSERT INTO attachments (id, conversation_id, owner_id, name, kind, size, sha256)
         SELECT gen_random_uuid(), $1, $2, 'quota-fixture.bin', 'file',
                CASE WHEN n <= 7 THEN 268435456 ELSE 0 END, repeat('0',64)
         FROM generate_series(1,255) n RETURNING id",
    ).bind(source_chat).bind(alice.user.id).fetch_all(&db).await?;
    let final_reservation = Attachment {
        id: Uuid::new_v4(), name: "quota-boundary.bin".into(), kind: AttachmentKind::File,
        size: 256 * CHUNK_SIZE, sha256: "0".repeat(64),
    };
    let begin = || http.post(format!("{server}/v1/conversations/{source_chat}/uploads"))
        .bearer_auth(alice.session_token()).json(&final_reservation);
    assert_eq!(begin().send().await?.status(), 507);
    expire_attachment(&db, id).await?;
    zibblu_online::retention::sweep(&db).await?;
    assert_eq!(begin().send().await?.status(), 200);
    let logical: (i64,i64) = sqlx::query_as(
        "SELECT count(*), coalesce(sum(bytes),0)::BIGINT FROM m1_attachment_quota WHERE owner_id=$1",
    ).bind(alice.user.id).fetch_one(&db).await?;
    assert_eq!(logical, (256, 2 * 1024 * 1024 * 1024));
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM attachments WHERE id=$1")
        .bind(id).fetch_one(&db).await?, 1);
    sqlx::query("DELETE FROM attachments WHERE id=ANY($1) OR id=$2")
        .bind(&reserve).bind(final_reservation.id).execute(&db).await?;
    assert_eq!(http.get(chunk(id)).bearer_auth(bob.session_token()).send().await?.status(), 410);
    assert_eq!(&http.get(chunk(target_id)).bearer_auth(carol.session_token()).send().await?.error_for_status()?.bytes().await?[..], bytes);
    let no_local = forwarding_request(&[target_chat]);
    assert_eq!(bob.forward_attachment(id, &no_local).await?[0].status, 410);
    let source_seq = bob.attachment(id).await?.seq;
    http.delete(format!("{server}/v1/conversations/{source_chat}/messages/{source_seq}"))
        .bearer_auth(alice.session_token()).send().await?.error_for_status()?;
    assert_eq!(http.post(&url).bearer_auth(bob.session_token()).json(&forwarding_request(&[target_chat])).send().await?.status(), 404);
    let committed = bob.forward_attachment(id, &request).await?;
    assert_eq!(committed[0].publication.as_ref().unwrap().seq, all[0].publication.as_ref().unwrap().seq);
    assert_eq!(&http.get(chunk(target_id)).bearer_auth(carol.session_token()).send().await?.error_for_status()?.bytes().await?[..], bytes);

    // A second source is downloaded and checked before natural expiry and physical reclamation.
    let second_upload = alice.queue_upload(source_chat, path).await?.attachment;
    let expired_id = second_upload.id;
    wait_transfer(&alice, expired_id, Direction::Upload, |job| job.status == TransferStatus::Complete).await?;
    bob.sync().await?;
    bob.queue_download(expired_id, second.path().join("saved.bin")).await?;
    wait_transfer(&bob, expired_id, Direction::Download, |job| job.status == TransferStatus::Complete).await?;
    bob.sync().await?;
    assert!(bob.attachment(expired_id).await?.local_file);
    expire_attachment(&db, expired_id).await?;
    for _ in 0..4 { zibblu_online::retention::sweep(&db).await?; }
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM attachments WHERE id=$1")
        .bind(expired_id).fetch_one(&db).await?, 0);
    let fallback = forwarding_request(&chats[..2]);
    let forward_url = format!("{server}/v2/files/{expired_id}/forward");
    let upload_url = format!("{server}/v2/files/{expired_id}/forward-upload");
    let expired_seq = bob.attachment(expired_id).await?.seq;
    let variant = MediaVariant::for_attachment(&second_upload);
    let (_, local_path, _) = bob.verified_media_path(
        source_chat, expired_seq, Some(expired_id), variant, &second_upload.sha256, true,
    ).await?.context("verified source cache missing")?;
    std::fs::write(&local_path, vec![0; bytes.len()])?;
    assert!(bob.forward_attachment(expired_id, &fallback).await?.iter().all(|item| item.status == 422));
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM messages WHERE forward_batch=$1")
        .bind(fallback.batch_id).fetch_one(&db).await?, 0);
    // Integrity verification marks the index corrupted; restore through the
    // same cache commit path used by transfers, not by reviving the old files layout.
    let restored = second.path().join("saved.bin");
    let desc = MediaDescriptor {
        conversation_id: source_chat, message_seq: expired_seq, attachment_id: Some(expired_id),
        variant, content_key: &second_upload.sha256, expected_sha256: &second_upload.sha256,
        expected_size: second_upload.size, mime: "application/octet-stream", dimensions: None,
    };
    bob.commit_media_file(&desc, &restored).await?;
    let mut incorrect = second_upload.clone();
    incorrect.sha256 = "0".repeat(64);
    assert_eq!(http.post(&upload_url).bearer_auth(bob.session_token()).json(&ForwardUploadRequest {
        request: fallback.clone(), original: incorrect,
    }).send().await?.status(), 422);
    // First forward returns 410, then the actual fallback publication commits
    // but its response is lost. The same batch must recover caches without reuploading.
    lose_response.store(2, Ordering::SeqCst);
    assert!(bob.forward_attachment(expired_id, &fallback).await.is_err());
    let committed_before: Vec<(Uuid, i64, i64)> = sqlx::query_as(
        "SELECT attachment_id, seq, (extract(epoch FROM expires_at)*1000)::BIGINT
         FROM messages WHERE forward_batch=$1 ORDER BY attachment_id",
    ).bind(fallback.batch_id).fetch_all(&db).await?;
    assert_eq!(committed_before.len(), 2);
    let root_before: Uuid = sqlx::query_scalar("SELECT upload_id FROM m1_forward_batches WHERE id=$1")
        .bind(fallback.batch_id).fetch_one(&db).await?;
    let results = bob.forward_attachment(expired_id, &fallback).await?;
    assert!(results.iter().all(|item| item.status == 200 && item.cache_error.is_none()));
    let root: Uuid = sqlx::query_scalar("SELECT upload_id FROM m1_forward_batches WHERE id=$1")
        .bind(fallback.batch_id).fetch_one(&db).await?;
    assert_eq!(root, root_before);
    assert_eq!(sqlx::query_as::<_, (Uuid, i64, i64)>(
        "SELECT attachment_id, seq, (extract(epoch FROM expires_at)*1000)::BIGINT
         FROM messages WHERE forward_batch=$1 ORDER BY attachment_id",
    ).bind(fallback.batch_id).fetch_all(&db).await?, committed_before);
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM m1_attachment_quota WHERE id=$1")
        .bind(root).fetch_one(&db).await?, 0);
    assert_eq!(sqlx::query_as::<_, (i64,i64)>(
        "SELECT count(*), coalesce(sum(bytes),0)::BIGINT FROM m1_attachment_quota WHERE owner_id=$1",
    ).bind(bob.user.id).fetch_one(&db).await?, (11, 11 * bytes.len() as i64));
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM attachments WHERE payload_id=$1")
        .bind(root).fetch_one(&db).await?, 2);
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM attachment_chunks WHERE attachment_id=$1")
        .bind(root).fetch_one(&db).await?, 1);
    let mut cached_paths = Vec::new();
    for target in &fallback.targets {
        let message = bob.attachment(target.attachment_id).await?;
        assert!(message.local_file);
        let file = message.attachment.context("target attachment missing")?;
        let (_, path, _) = bob.verified_media_path(
            target.conversation_id, message.seq, Some(target.attachment_id),
            MediaVariant::for_attachment(&file), &file.sha256, true,
        ).await?.context("target original cache missing")?;
        assert_eq!(std::fs::read(&path)?, bytes);
        assert_ne!(path, local_path);
        cached_paths.push(path);
        assert_eq!(&http.get(chunk(target.attachment_id)).bearer_auth(carol.session_token()).send().await?.error_for_status()?.bytes().await?[..], bytes);
    }
    assert_ne!(cached_paths[0], cached_paths[1]);
    bob.sync().await?;
    for target in &fallback.targets {
        assert!(sqlx::query_scalar::<_, bool>(
            "SELECT content_verified FROM m1_recipients r JOIN messages m ON m.seq=r.message_seq
             WHERE m.attachment_id=$1 AND r.account_id=$2",
        ).bind(target.attachment_id).bind(bob.user.id).fetch_one(&db).await?);
    }

    // An upload begun while expired cannot be published after a later source recall.
    let blocked = forwarding_request(&[target_chat]);
    let staged: ForwardUploadProgress = http.post(&upload_url).bearer_auth(bob.session_token())
        .json(&ForwardUploadRequest { request: blocked.clone(), original: second_upload.clone() })
        .send().await?.error_for_status()?.json().await?;
    http.put(format!("{server}/v1/files/{}/chunks/0", staged.id)).bearer_auth(bob.session_token())
        .body(bytes.to_vec()).send().await?.error_for_status()?;
    assert_eq!(http.post(format!("{server}/v1/files/{}/complete", staged.id))
        .bearer_auth(bob.session_token()).send().await?.status(), 404);
    bob.remove_contact(alice.user.id).await?;
    let denied_by_contact: Vec<ForwardResult> = http.post(&forward_url).bearer_auth(bob.session_token())
        .json(&blocked).send().await?.error_for_status()?.json().await?;
    assert_eq!(denied_by_contact[0].status, 409);
    assert_eq!(http.post(&upload_url).bearer_auth(bob.session_token())
        .json(&ForwardUploadRequest { request: blocked.clone(), original: second_upload.clone() })
        .send().await?.status(), 409);
    assert_eq!(direct(&alice, &bob).await?, source_chat);
    let seq = bob.attachment(expired_id).await?.seq;
    http.delete(format!("{server}/v1/conversations/{source_chat}/messages/{seq}"))
        .bearer_auth(alice.session_token()).send().await?.error_for_status()?;
    let denied: Vec<ForwardResult> = http.post(&forward_url).bearer_auth(bob.session_token())
        .json(&blocked).send().await?.error_for_status()?.json().await?;
    assert_eq!(denied[0].status, 404);
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM messages WHERE forward_batch=$1")
        .bind(blocked.batch_id).fetch_one(&db).await?, 0);
    assert_eq!(&http.get(chunk(fallback.targets[0].attachment_id)).bearer_auth(carol.session_token()).send().await?.error_for_status()?.bytes().await?[..], bytes);

    for target in request.targets.iter().chain(&fallback.targets) {
        expire_attachment(&db, target.attachment_id).await?;
    }
    for _ in 0..5 { zibblu_online::retention::sweep(&db).await?; }
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM attachments WHERE id=ANY($1)")
        .bind(vec![id, root]).fetch_one(&db).await?, 0);
    task.abort();
    Ok(())
}

// Real FFmpeg + authenticated HTTP + disposable PostgreSQL; CI installs the Linux tools.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn video_derivatives_and_retention() -> Result<()> {
    let db = test_db().await?;
    let retention = Retention { file_secs: Some(604800), ..Retention::default() };
    let app = zibblu_online::router_with_retention(db.clone(), retention).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let server = format!("http://{}", listener.local_addr()?);
    let server_task = tokio::spawn(async move { axum::serve(listener, app).await });
    let first = tempfile::tempdir()?;
    let second = tempfile::tempdir()?;
    let third = tempfile::tempdir()?;
    let suffix = &Uuid::new_v4().simple().to_string()[..10];
    let password = "video-derivatives-fixture";
    let alice = sign_in(first.path(), &server, &format!("video_a_{suffix}"), password, true).await?;
    let bob = sign_in(second.path(), &server, &format!("video_b_{suffix}"), password, true).await?;
    let outsider = sign_in(third.path(), &server, &format!("video_c_{suffix}"), password, true).await?;
    let chat = direct(&alice, &bob).await?;
    alice.sync().await?;
    bob.sync().await?;
    let source = first.path().join("sample.mp4");
    ensure!(Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=30",
            "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000", "-t", "4", "-c:v", "libx264",
            "-threads", "2", "-crf", "10", "-c:a", "aac"])
        .arg(&source).status()?.success(), "FFmpeg fixture generation failed");
    let original = std::fs::read(&source)?;
    let digest = format!("{:x}", Sha256::digest(&original));
    let http = reqwest::Client::builder().no_proxy().build()?;
    let upload = alice.queue_upload(chat, source.clone()).await?;
    let id = upload.attachment.id;
    wait_transfer(&alice, id, Direction::Upload, |job| job.status == TransferStatus::Complete).await?;
    bob.sync().await?;
    let seq = bob.attachment(id).await?.seq;
    assert_eq!(upload.attachment.sha256, digest);
    let media = format!("{server}/v2/files/{id}/media");
    let preview_url = format!("{media}/preview");
    let cover_url = format!("{media}/cover");
    let get = |url: &str| http.get(url).bearer_auth(bob.session_token());
    let pending: serde_json::Value = get(&media).send().await?.error_for_status()?.json().await?;
    assert_eq!(pending["preview"]["status"], "pending");
    assert_eq!(get(&preview_url).send().await?.status(), 409);
    assert_eq!(http.get(&media).send().await?.status(), 401);
    assert_eq!(http.get(&preview_url).send().await?.status(), 401);
    assert_eq!(get(&format!("{media}/original")).send().await?.status(), 404);
    for url in [&media, &cover_url, &preview_url] {
        assert_eq!(http.get(url).bearer_auth(outsider.session_token()).send().await?.status(), 404);
    }
    assert!(zibblu_online::media::process_one(&db).await?);
    let ready: serde_json::Value = get(&media).send().await?.error_for_status()?.json().await?;
    assert_eq!(ready["cover"]["status"], "ready");
    assert_eq!(ready["preview"]["status"], "ready");
    assert_ne!(ready["cover"]["id"], ready["preview"]["id"]);
    assert_eq!(ready["cover"]["mime"], "image/jpeg");
    assert_eq!(ready["preview"]["mime"], "video/mp4");
    let cover = get(&cover_url).send().await?.error_for_status()?.bytes().await?;
    assert!(cover.starts_with(&[0xff, 0xd8, 0xff]));
    assert_eq!(ready["cover"]["size"].as_u64(), Some(cover.len() as u64));
    assert_eq!(ready["cover"]["sha256"], format!("{:x}", Sha256::digest(&cover)));
    let response = get(&preview_url).send().await?.error_for_status()?;
    assert_eq!(response.headers()["content-type"], "video/mp4");
    assert_eq!(response.headers()["accept-ranges"], "bytes");
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    let preview = response.bytes().await?;
    assert!(!preview.is_empty() && preview.len() < original.len());
    assert_eq!(ready["preview"]["size"].as_u64(), Some(preview.len() as u64));
    assert_eq!(ready["preview"]["sha256"], format!("{:x}", Sha256::digest(&preview)));
    let preview_path = second.path().join("preview.mp4");
    std::fs::write(&preview_path, &preview)?;
    ensure!(Command::new("ffmpeg").args(["-nostdin", "-v", "error", "-ss", "2", "-i"])
        .arg(&preview_path).args(["-frames:v", "1", "-f", "null", "-"]).status()?.success());
    let head = http.head(&preview_url).bearer_auth(bob.session_token()).send().await?;
    assert_eq!(head.status(), 200);
    assert_eq!(head.headers()["content-length"].to_str()?.parse::<usize>()?, preview.len());
    assert!(head.bytes().await?.is_empty());
    let start = if preview.len() > CHUNK_SIZE as usize + 40 {
        CHUNK_SIZE as usize - 17
    } else { preview.len() / 2 };
    let end = (start + 53).min(preview.len() - 1);
    let partial = get(&preview_url).header("Range", format!("bytes={start}-{end}")).send().await?;
    assert_eq!(partial.status(), 206);
    assert_eq!(partial.headers()["content-range"].to_str()?, format!("bytes {start}-{end}/{}", preview.len()));
    assert_eq!(partial.headers()["content-length"].to_str()?.parse::<usize>()?, end - start + 1);
    assert_eq!(&partial.bytes().await?[..], &preview[start..=end]);
    let tail = get(&preview_url).header("Range", "bytes=-31").send().await?;
    assert_eq!(tail.status(), 206);
    assert_eq!(&tail.bytes().await?[..], &preview[preview.len() - 31..]);
    let invalid = get(&preview_url).header("Range", format!("bytes={}-", preview.len())).send().await?;
    assert_eq!(invalid.status(), 416);
    assert_eq!(invalid.headers()["content-range"].to_str()?, format!("bytes */{}", preview.len()));
    let mut downloaded = Vec::new();
    for offset in (0..original.len()).step_by(CHUNK_SIZE as usize) {
        let data = get(&format!("{server}/v1/files/{id}/chunks/{offset}")).send().await?.error_for_status()?.bytes().await?;
        downloaded.extend_from_slice(&data);
    }
    assert_eq!(downloaded, original);
    assert_eq!(format!("{:x}", Sha256::digest(&downloaded)), digest);
    println!("video original={} sha256={digest}; cover={} sha256={:x}; preview={} sha256={:x}; Range {start}-{end} verified",
        original.len(), cover.len(), Sha256::digest(&cover), preview.len(), Sha256::digest(&preview));

    // Reconstructing the router must not reset ready resources or their identities.
    let _reopened = zibblu_online::router_with_retention(db.clone(), retention).await?;
    assert!(!zibblu_online::media::process_one(&db).await?);
    assert_eq!(get(&media).send().await?.error_for_status()?.json::<serde_json::Value>().await?, ready);
    http.delete(format!("{server}/v1/conversations/{chat}/messages/{seq}"))
        .bearer_auth(alice.session_token()).send().await?.error_for_status()?;
    for url in [&media, &cover_url, &preview_url] {
        assert_eq!(get(url).send().await?.status(), 404);
    }
    zibblu_online::retention::sweep(&db).await?;
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM video_resource_chunks WHERE attachment_id=$1")
        .bind(id).fetch_one(&db).await?, 0);

    // A malformed MP4 signature is not accepted as real video, but still publishes as an original.
    let invalid_bytes = b"\0\0\0\x18ftypisomjunk";
    let invalid_path = first.path().join("broken.mp4");
    std::fs::write(&invalid_path, invalid_bytes)?;
    let bad = alice.queue_upload(chat, invalid_path).await?.attachment.id;
    wait_transfer(&alice, bad, Direction::Upload, |job| job.status == TransferStatus::Complete).await?;
    assert!(zibblu_online::media::process_one(&db).await?);
    let bad_url = format!("{server}/v2/files/{bad}/media");
    let bad_meta: serde_json::Value = get(&bad_url).send().await?.error_for_status()?.json().await?;
    assert_eq!(bad_meta["preview"]["status"], "unsupported");
    assert_eq!(get(&format!("{bad_url}/preview")).send().await?.status(), 415);
    assert_eq!(&get(&format!("{server}/v1/files/{bad}/chunks/0")).send().await?.error_for_status()?.bytes().await?[..], invalid_bytes);

    // A highly compressed, valid tiny MP4 still gets a cover, not a larger preview.
    let tiny_path = first.path().join("tiny.mp4");
    ensure!(Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-f", "lavfi", "-i", "color=c=black:s=64x64:r=1",
            "-t", "1", "-c:v", "libx264", "-threads", "2", "-crf", "35"])
        .arg(&tiny_path).status()?.success());
    let tiny_bytes = std::fs::read(&tiny_path)?;
    let tiny = alice.queue_upload(chat, tiny_path).await?.attachment.id;
    wait_transfer(&alice, tiny, Direction::Upload, |job| job.status == TransferStatus::Complete).await?;
    assert!(zibblu_online::media::process_one(&db).await?);
    let tiny_url = format!("{server}/v2/files/{tiny}/media");
    let tiny_meta: serde_json::Value = get(&tiny_url).send().await?.error_for_status()?.json().await?;
    assert_eq!(tiny_meta["cover"]["status"], "ready");
    assert_eq!(tiny_meta["preview"]["status"], "not_smaller");
    assert!(tiny_meta["preview"]["size"].is_null());
    assert_eq!(&get(&format!("{server}/v1/files/{tiny}/chunks/0")).send().await?.error_for_status()?.bytes().await?[..], tiny_bytes.as_slice());

    // Reserve the remaining original byte quota through the real upload endpoint.
    // The background job can finish processing but cannot reserve extra persistent bytes.
    let limited = alice.queue_upload(chat, source.clone()).await?.attachment.id;
    wait_transfer(&alice, limited, Direction::Upload, |job| job.status == TransferStatus::Complete).await?;
    let used: i64 = sqlx::query_scalar(
        "SELECT (coalesce((SELECT sum(size) FROM attachments WHERE owner_id=$1),0)
          + coalesce((SELECT sum(r.size) FROM video_resources r JOIN attachments a ON a.id=r.attachment_id
                     WHERE a.owner_id=$1 AND r.status='ready'),0))::BIGINT",
    ).bind(alice.user.id).fetch_one(&db).await?;
    let mut remaining = (2_i64 * 1024 * 1024 * 1024 - used) as u64;
    let mut reservations = Vec::new();
    while remaining > 0 {
        let size = remaining.min(256 * CHUNK_SIZE);
        let reservation = Attachment {
            id: Uuid::new_v4(), name: "reservation.bin".into(), kind: AttachmentKind::File,
            size, sha256: "0".repeat(64),
        };
        http.post(format!("{server}/v1/conversations/{chat}/uploads"))
            .bearer_auth(alice.session_token()).json(&reservation).send().await?.error_for_status()?;
        reservations.push(reservation.id);
        remaining -= size;
    }
    assert!(zibblu_online::media::process_one(&db).await?);
    let limited_url = format!("{server}/v2/files/{limited}/media");
    let limited_meta: serde_json::Value = get(&limited_url).send().await?.error_for_status()?.json().await?;
    assert_eq!(limited_meta["cover"]["status"], "quota");
    assert_eq!(limited_meta["preview"]["status"], "quota");
    assert_eq!(get(&format!("{limited_url}/preview")).send().await?.status(), 507);
    assert_eq!(&get(&format!("{server}/v1/files/{limited}/chunks/0")).send().await?.error_for_status()?.bytes().await?[..], &original[..CHUNK_SIZE as usize]);
    for reservation in reservations {
        http.delete(format!("{server}/v1/files/{reservation}"))
            .bearer_auth(alice.session_token()).send().await?.error_for_status()?;
    }

    // Kill the actual service process after FFmpeg, before the derivative commit.
    // A fresh binary instance must resume the pending job without changing original bytes.
    let restart_id = alice.queue_upload(chat, source.clone()).await?.attachment.id;
    wait_transfer(&alice, restart_id, Direction::Upload, |job| job.status == TransferStatus::Complete).await?;
    let restart_url = format!("{server}/v2/files/{restart_id}/media");
    let mut restart_fence = db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(1001)").execute(&mut *restart_fence).await?;
    let spawn_service = || -> Result<tokio::process::Child> {
        Ok(tokio::process::Command::new(env!("CARGO_BIN_EXE_zibblu-online"))
            .env("DATABASE_URL", std::env::var("ZIBBLU_TEST_DATABASE_URL")?)
            .env("ZIBBLU_BIND", "127.0.0.1:0")
            .env("ZIBBLU_FILE_TTL_SECONDS", "604800")
            .env_remove("ZIBBLU_CONTAINER")
            .env_remove("ZIBBLU_CLEANUP_INTERVAL_SECONDS")
            .env("TMPDIR", first.path())
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
            .kill_on_drop(true).spawn()?)
    };
    let mut child = spawn_service()?;
    wait_for_media_commit(&mut restart_fence).await?;
    child.kill().await?;
    restart_fence.commit().await?;
    let pending_after_kill: serde_json::Value = get(&restart_url).send().await?.error_for_status()?.json().await?;
    assert_eq!(pending_after_kill["preview"]["status"], "pending");
    let mut restarted = spawn_service()?;
    let restarted_meta = tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            let metadata: serde_json::Value = get(&restart_url).send().await?.error_for_status()?.json().await?;
            if metadata["preview"]["status"] != "pending" { return Ok::<_, anyhow::Error>(metadata); }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }).await??;
    assert_eq!(restarted_meta["preview"]["status"], "ready");
    restarted.kill().await?;
    assert_eq!(get(&restart_url).send().await?.error_for_status()?.json::<serde_json::Value>().await?, restarted_meta);

    // Group membership revocation immediately removes access to both derivatives.
    alice.add_contact(outsider.user.id).await?;
    outsider.respond_contact(alice.user.id, true).await?;
    let group = alice.create_conversation(ConversationRequest::Group {
        title: format!("video group {suffix}"), contact_ids: vec![bob.user.id, outsider.user.id],
    }).await?;
    alice.sync().await?;
    bob.sync().await?;
    outsider.sync().await?;
    let group_id = alice.queue_upload(group, source.clone()).await?.attachment.id;
    wait_transfer(&alice, group_id, Direction::Upload, |job| job.status == TransferStatus::Complete).await?;
    assert!(zibblu_online::media::process_one(&db).await?);
    let group_url = format!("{server}/v2/files/{group_id}/media");
    let group_meta: serde_json::Value = get(&group_url).send().await?.error_for_status()?.json().await?;
    assert_eq!(group_meta["preview"]["status"], "ready");
    assert_eq!(get(&format!("{group_url}/preview")).header("Range", "bytes=0-31").send().await?.status(), 206);
    http.delete(format!("{server}/v1/conversations/{group}/membership"))
        .bearer_auth(bob.session_token()).send().await?.error_for_status()?;
    for url in [&group_url, &format!("{group_url}/cover"), &format!("{group_url}/preview")] {
        assert_eq!(get(url).send().await?.status(), 404);
    }
    assert_eq!(http.get(&group_url).bearer_auth(alice.session_token()).send().await?.status(), 200);
    assert_eq!(http.get(format!("{server}/v1/files/{group_id}/chunks/0"))
        .bearer_auth(alice.session_token()).send().await?.status(), 200);

    // Queue a real recall transaction ahead of a completed transcode. The original
    // feed lock serializes the recall and derivative publication.
    let recalled = alice.queue_upload(chat, source.clone()).await?.attachment.id;
    wait_transfer(&alice, recalled, Direction::Upload, |job| job.status == TransferStatus::Complete).await?;
    bob.sync().await?;
    let recalled_seq = bob.attachment(recalled).await?.seq;
    let recalled_url = format!("{server}/v2/files/{recalled}/media");
    let mut recall_fence = db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(1001)").execute(&mut *recall_fence).await?;
    let recall_request = http.delete(format!("{server}/v1/conversations/{chat}/messages/{recalled_seq}"))
        .bearer_auth(alice.session_token());
    let recall_task = tokio::spawn(async move { recall_request.send().await });
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_locks WHERE locktype='advisory' AND objid=1001 AND NOT granted)"
            ).fetch_one(&mut *recall_fence).await?;
            if waiting { break; }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        Ok::<_, anyhow::Error>(())
    }).await??;
    let worker_db = db.clone();
    let recall_worker = tokio::spawn(async move { zibblu_online::media::process_one(&worker_db).await });
    wait_for_media_commit(&mut recall_fence).await?;
    recall_fence.commit().await?;
    recall_task.await??.error_for_status()?;
    assert!(recall_worker.await??);
    assert_eq!(get(&recalled_url).send().await?.status(), 404);
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM video_resource_chunks WHERE attachment_id=$1")
        .bind(recalled).fetch_one(&db).await?, 0);

    // Hold the real feed lock until the worker finishes FFmpeg and attempts to commit.
    // Expire the source in that transaction; late generated output must not become ready.
    let late = alice.queue_upload(chat, source).await?.attachment.id;
    wait_transfer(&alice, late, Direction::Upload, |job| job.status == TransferStatus::Complete).await?;
    let late_url = format!("{server}/v2/files/{late}/media");
    let mut fence = db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(1001)").execute(&mut *fence).await?;
    let worker_db = db.clone();
    let worker = tokio::spawn(async move { zibblu_online::media::process_one(&worker_db).await });
    wait_for_media_commit(&mut fence).await?;
    sqlx::query("UPDATE messages SET expires_at=clock_timestamp()-interval '1 second' WHERE attachment_id=$1")
        .bind(late).execute(&mut *fence).await?;
    sqlx::query("UPDATE attachments SET expires_at=clock_timestamp()-interval '1 second' WHERE id=$1")
        .bind(late).execute(&mut *fence).await?;
    fence.commit().await?;
    assert!(worker.await??);
    assert_eq!(get(&late_url).send().await?.status(), 410);
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM video_resource_chunks WHERE attachment_id=$1")
        .bind(late).fetch_one(&db).await?, 0);
    zibblu_online::retention::sweep(&db).await?;
    assert_eq!(get(&late_url).send().await?.status(), 410);
    assert_eq!(get(&bad_url).send().await?.status(), 200);

    // A ready video can be forwarded without the recipient obtaining its original;
    // the new target keeps all three resources after the source expires.
    let target_chat = direct(&bob, &outsider).await?;
    bob.sync().await?;
    outsider.sync().await?;
    assert!(!bob.attachment(restart_id).await?.local_file);
    let forwarding = forwarding_request(&[target_chat]);
    let target_id = forwarding.targets[0].attachment_id;
    let forwarded = bob.forward_attachment(restart_id, &forwarding).await?;
    assert_eq!(forwarded[0].status, 200);
    let target_url = format!("{server}/v2/files/{target_id}/media");
    let target_get = |url: &str| http.get(url).bearer_auth(outsider.session_token());
    let target_meta: serde_json::Value = target_get(&target_url).send().await?.error_for_status()?.json().await?;
    assert_eq!(target_meta["preview"]["sha256"], restarted_meta["preview"]["sha256"]);
    assert_eq!(target_meta["cover"]["sha256"], restarted_meta["cover"]["sha256"]);
    assert_ne!(target_meta["preview"]["id"], restarted_meta["preview"]["id"]);
    expire_attachment(&db, restart_id).await?;
    zibblu_online::retention::sweep(&db).await?;
    assert_eq!(get(&restart_url).send().await?.status(), 410);
    assert_eq!(target_get(&format!("{target_url}/preview")).header("Range", "bytes=0-31").send().await?.status(), 206);
    assert_eq!(target_get(&format!("{target_url}/cover")).send().await?.status(), 200);
    let mut forwarded_original = Vec::new();
    for offset in (0..original.len()).step_by(CHUNK_SIZE as usize) {
        forwarded_original.extend_from_slice(&target_get(&format!("{server}/v1/files/{target_id}/chunks/{offset}"))
            .send().await?.error_for_status()?.bytes().await?);
    }
    assert_eq!(forwarded_original, original);
    expire_attachment(&db, target_id).await?;
    zibblu_online::retention::sweep(&db).await?;
    zibblu_online::retention::sweep(&db).await?;
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM video_resource_chunks WHERE attachment_id=$1")
        .bind(restart_id).fetch_one(&db).await?, 0);
    server_task.abort();
    Ok(())
}

#[cfg(target_os = "linux")]
async fn wait_for_media_commit(fence: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_locks held JOIN pg_locks waiting ON waiting.pid=held.pid
                 WHERE held.locktype='advisory' AND held.objid=1002 AND held.granted
                 AND waiting.locktype='advisory' AND waiting.objid=1001 AND NOT waiting.granted)",
            ).fetch_one(&mut **fence).await?;
            if waiting { break; }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        Ok::<_, anyhow::Error>(())
    }).await??;
    Ok(())
}
