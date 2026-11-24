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
    Attachment, AttachmentKind, ContentAck, DeviceSession, ExpiredPayload, MAX_MESSAGE_BYTES,
    SYNC_PAGE_SIZE,
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
    release.notify_one();
    assert!(flushing.await??);
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
    let start = (CHUNK_SIZE as usize - 17).min(preview.len() / 2);
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

    // Hold the real feed lock until the worker finishes FFmpeg and attempts to commit.
    // Expire the source in that transaction; late generated output must not become ready.
    let late = alice.queue_upload(chat, source).await?.attachment.id;
    wait_transfer(&alice, late, Direction::Upload, |job| job.status == TransferStatus::Complete).await?;
    let late_url = format!("{server}/v2/files/{late}/media");
    let mut fence = db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(1001)").execute(&mut *fence).await?;
    let worker_db = db.clone();
    let worker = tokio::spawn(async move { zibblu_online::media::process_one(&worker_db).await });
    tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_locks held JOIN pg_locks waiting ON waiting.pid=held.pid
                 WHERE held.locktype='advisory' AND held.objid=1002 AND held.granted
                 AND waiting.locktype='advisory' AND waiting.objid=1001 AND NOT waiting.granted)",
            ).fetch_one(&mut *fence).await?;
            if waiting { break; }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        Ok::<_, anyhow::Error>(())
    }).await??;
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
    server_task.abort();
    Ok(())
}
