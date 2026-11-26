use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use sqlx::postgres::PgPoolOptions;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::{path::Path, process::Command, sync::Arc, time::Duration};
use uuid::Uuid;
use zibblu_core::{
    AiProvider, AiScope, CHUNK_SIZE, Client, ContactNotes, ConversationRequest, Credentials,
    DeviceLogin, Direction, FileFilter, MAX_PROFILE_IMAGE_BYTES, MessageReceipt, ProfileImage,
    ProfileRelation, PublishAiReply, ReadUpdate, ReceiptStatus, SendMessage, Session, SyncAck,
    SyncEvent, SyncPage, Transfer, TransferStatus,
};

#[path = "m1/local_first.rs"]
mod local_first;

#[test]
fn non_loopback_bind_requires_container_opt_in() -> Result<()> {
    for (address, mode, expected_error) in [
        ("0.0.0.0:7841", "", "Bind the service to loopback"),
        ("0.0.0.0:7841", "true", "Bind the service to loopback"),
        ("0.0.0.0:8080", "1", "Bind the service to loopback"),
        ("0.0.0.0:7841", "1", "Set DATABASE_URL"),
        ("127.0.0.1:7841", "", "Set DATABASE_URL"),
    ] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_zibblu-online"));
        command
            .env("ZIBBLU_BIND", address)
            .env_remove("DATABASE_URL");
        if mode.is_empty() {
            command.env_remove("ZIBBLU_CONTAINER");
        } else {
            command.env("ZIBBLU_CONTAINER", mode);
        }
        let output = command.output()?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success() && stderr.contains(expected_error),
            "unexpected startup result for {address} with container mode {mode:?}: {stderr}"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn normal_chat_and_files() -> Result<()> {
    let url = std::env::var("ZIBBLU_TEST_DATABASE_URL")
        .context("Set ZIBBLU_TEST_DATABASE_URL to a disposable PostgreSQL database")?;
    let db = PgPoolOptions::new()
        .max_connections(6)
        .connect(&url)
        .await?;
    let app = zibblu_online::router(db.clone()).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let server = format!("http://{}", listener.local_addr()?);
    let task = tokio::spawn(async move { axum::serve(listener, app).await });
    let first = tempfile::tempdir()?;
    let second = tempfile::tempdir()?;
    let suffix = &Uuid::new_v4().simple().to_string()[..10];
    let password = format!("acceptance-{suffix}");
    let alice_handle = format!("alice_{suffix}");
    let bob_handle = format!("Bob_{suffix}");
    let outsider_handle = format!("outsider_{suffix}");
    let alice = sign_in(first.path(), &server, &alice_handle, &password, true).await?;
    let bob = sign_in(second.path(), &server, &bob_handle, &password, true).await?;

    let contact = alice.find_user(&bob_handle.to_lowercase()).await?;
    assert_eq!(contact.id, bob.user.id);
    assert_eq!(contact.handle, bob_handle);
    assert!(
        sign_in(
            second.path(),
            &server,
            &bob_handle.to_lowercase(),
            &password,
            true
        )
        .await
        .is_err()
    );
    let request = || ConversationRequest::Direct {
        contact_id: contact.id,
    };
    alice.add_contact(contact.id).await?;
    assert!(alice.create_conversation(request()).await.is_err());
    assert!(alice.respond_contact(contact.id, true).await.is_err());
    let outgoing = alice.sync().await?.pending_contacts;
    assert_eq!(outgoing[0].user.id, bob.user.id);
    assert!(!outgoing[0].incoming);
    let incoming = bob.sync().await?.pending_contacts;
    assert_eq!(incoming[0].user.id, alice.user.id);
    assert!(incoming[0].incoming);
    bob.respond_contact(alice.user.id, false).await?;
    assert!(alice.sync().await?.pending_contacts.is_empty());
    alice.add_contact(contact.id).await?;
    bob.respond_contact(alice.user.id, true).await?;
    assert_eq!(alice.sync().await?.contacts, [bob.user.clone()]);
    assert_eq!(bob.sync().await?.contacts, [alice.user.clone()]);
    let chat = alice.create_conversation(request()).await?;
    send(&alice, chat, "Hello from Alice").await?;
    bob.sync().await?;
    assert_eq!(
        bob.messages(chat, None).await?.items[0].text.as_deref(),
        Some("Hello from Alice")
    );
    send(&bob, chat, "Hello from Bob").await?;
    bob.sync().await?;
    alice.sync().await?;
    assert_eq!(
        alice.messages(chat, None).await?.items[0].text.as_deref(),
        Some("Hello from Bob")
    );
    bob.logout().await?;
    drop(bob);
    send(&alice, chat, "This arrived while you were away").await?;
    let bob = sign_in(
        second.path(),
        &server,
        &bob_handle.to_lowercase(),
        &password,
        false,
    )
    .await?;
    assert_eq!(bob.user.handle, bob_handle);
    assert_eq!(
        bob.messages(chat, None).await?.items.len(),
        2,
        "Read local SQLite before a network sync"
    );
    println!("Two independent clients exchanged messages and reopened persistent SQLite history");
    bob.sync().await?;
    let history = bob.messages(chat, None).await?.items;
    assert_eq!(history.len(), 3);
    assert_eq!(
        history[0].text.as_deref(),
        Some("This arrived while you were away")
    );
    println!("An offline recipient received its queued message after signing in and syncing");

    let source = first.path().join("sample.bin");
    let fixture = (0..(24 * CHUNK_SIZE + 137) as usize)
        .map(|index| (index ^ (index >> 8) ^ (index >> 18)) as u8)
        .collect::<Vec<_>>();
    std::fs::write(&source, &fixture)?;
    let upload = alice.queue_upload(chat, source).await?;
    let id = upload.attachment.id;
    wait_transfer(&alice, id, Direction::Upload, |transfer| {
        transfer.received >= CHUNK_SIZE
    })
    .await?;
    alice.pause(id, Direction::Upload).await?;
    let paused = wait_transfer(&alice, id, Direction::Upload, |transfer| {
        transfer.status == TransferStatus::Paused
    })
    .await?;
    assert!(paused.received > 0 && paused.received < upload.attachment.size);
    alice.logout().await?;
    drop(alice);
    let alice = sign_in(first.path(), &server, &alice_handle, &password, false).await?;
    let restored = alice.transfers().await?;
    assert_eq!(restored[0].status, TransferStatus::Paused);
    assert_eq!(restored[0].received, paused.received);
    alice.resume(id, Direction::Upload).await?;
    wait_transfer(&alice, id, Direction::Upload, |transfer| {
        transfer.status == TransferStatus::Complete
    })
    .await?;
    bob.sync().await?;
    for filter in [
        FileFilter::default(),
        FileFilter {
            conversation_id: Some(chat),
            ..FileFilter::default()
        },
    ] {
        assert_eq!(
            bob.files(filter, None).await?.items[0]
                .attachment
                .as_ref()
                .unwrap()
                .id,
            id
        );
    }
    let destination = second.path().join("download.bin");
    bob.queue_download(id, destination.clone()).await?;
    wait_transfer(&bob, id, Direction::Download, |transfer| {
        transfer.status == TransferStatus::Complete
    })
    .await?;
    let expected = format!("{:x}", Sha256::digest(&fixture));
    assert_eq!(
        format!("{:x}", Sha256::digest(std::fs::read(&destination)?)),
        expected
    );
    assert_eq!(upload.attachment.sha256, expected);
    assert!(
        bob.queue_download(id, destination).await.is_err(),
        "Protect an existing destination"
    );
    let transfer_profile = tempfile::tempdir()?;
    let reader = sign_in(
        transfer_profile.path(),
        &server,
        &bob_handle,
        &password,
        false,
    )
    .await?;
    reader.sync().await?;
    let shared = reader.attachment(id).await?;
    let saved = transfer_profile.path().join("resume-hidden.bin");
    reader.queue_download(id, saved.clone()).await?;
    reader.pause(id, Direction::Download).await?;
    wait_transfer(&reader, id, Direction::Download, |transfer| {
        transfer.status == TransferStatus::Paused
    })
    .await?;
    reader.hide_message(chat, shared.seq).await?;
    assert!(reader.attachment(id).await.is_err());
    assert!(
        reader
            .files(FileFilter::default(), None)
            .await?
            .items
            .is_empty()
    );
    assert!(bob.attachment(id).await.is_ok());
    reader.logout().await?;
    drop(reader);
    let reader = sign_in(
        transfer_profile.path(),
        &server,
        &bob_handle,
        &password,
        false,
    )
    .await?;
    assert!(
        reader
            .transfers()
            .await?
            .iter()
            .any(|transfer| transfer.attachment.id == id
                && transfer.status == TransferStatus::Paused)
    );
    reader.resume(id, Direction::Download).await?;
    tokio::time::timeout(Duration::from_secs(40), async {
        while !saved.exists() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await?;
    assert_eq!(
        format!("{:x}", Sha256::digest(std::fs::read(saved)?)),
        expected
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while !reader.transfers().await?.is_empty() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    println!(
        "Upload resumed from byte {} of {}, appeared in both Files views, and matched SHA-256 after download",
        paused.received, upload.attachment.size
    );

    let http = reqwest::Client::builder().no_proxy().build()?;
    let outsider: Session = http
        .post(format!("{server}/v2/register"))
        .json(&DeviceLogin {
            credentials: Credentials {
                handle: outsider_handle,
                password: password.clone(),
            },
            device_key: device_key(),
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let chunk = format!("{server}/v1/files/{id}/chunks/0");
    assert_eq!(http.get(&chunk).send().await?.status(), 401);
    assert_eq!(
        http.get(&chunk)
            .bearer_auth(&outsider.token)
            .send()
            .await?
            .status(),
        404
    );
    assert_eq!(
        http.put(&chunk)
            .bearer_auth(&outsider.token)
            .body(vec![0u8; CHUNK_SIZE as usize])
            .send()
            .await?
            .status(),
        404
    );
    println!("The service rejected unauthenticated reads and nonmember reads/writes");

    let old = history[2].seq;
    sqlx::query(
        "UPDATE messages SET created_at = (extract(epoch FROM clock_timestamp()) * 1000)::BIGINT
         - $2 - 1000 WHERE seq = $1",
    )
    .bind(old)
    .bind(zibblu_core::RECALL_WINDOW_MS)
    .execute(&db)
    .await?;
    assert!(alice.recall_message(chat, old).await.is_err());
    let incoming = history[1].seq;
    assert!(alice.recall_message(chat, incoming).await.is_err());
    let recalled = history[0].seq;
    alice.recall_message(chat, recalled).await?;
    alice.recall_message(chat, recalled).await?;
    let changes = bob.sync_updates().await?;
    assert_eq!(changes.recalled, [recalled]);
    let tombstone = bob
        .messages(chat, None)
        .await?
        .items
        .into_iter()
        .find(|item| item.seq == recalled)
        .unwrap();
    assert!(tombstone.recalled && tombstone.text.is_none());
    assert!(
        bob.search_messages(chat, None, Some("while you were away"))
            .await?
            .items
            .is_empty()
    );
    assert!(
        bob.ai_messages(
            chat,
            AiScope::Selected {
                sequences: vec![recalled]
            }
        )
        .await
        .is_err()
    );
    let file_seq = bob.files(FileFilter::default(), None).await?.items[0].seq;
    alice.recall_message(chat, file_seq).await?;
    assert!(bob.attachment(id).await.is_ok());
    let reader: Session = http
        .post(format!("{server}/v2/login"))
        .json(&DeviceLogin {
            credentials: Credentials {
                handle: bob_handle.clone(),
                password: password.clone(),
            },
            device_key: device_key(),
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        http.get(&chunk)
            .bearer_auth(&reader.token)
            .send()
            .await?
            .status(),
        404
    );
    bob.sync().await?;
    assert!(
        bob.files(FileFilter::default(), None)
            .await?
            .items
            .is_empty()
    );
    assert!(bob.attachment(id).await.is_err());
    println!(
        "Recall synchronized to an existing SQLite history, denied other senders and expired messages, and blocked file access"
    );

    alice.add_contact(outsider.user.id).await?;
    http.post(format!("{server}/v1/contacts/{}/decision", alice.user.id))
        .bearer_auth(&outsider.token)
        .json(&serde_json::json!({"accept": true}))
        .send()
        .await?
        .error_for_status()?;
    let group = alice
        .create_conversation(ConversationRequest::Group {
            title: "QR acceptance".into(),
            contact_ids: vec![bob.user.id, outsider.user.id],
        })
        .await?;
    alice.sync().await?;
    send(&alice, group, "Before joining").await?;
    let old_path = first.path().join("before-join.bin");
    std::fs::write(&old_path, b"Old attachment")?;
    let old_attachment = alice.queue_upload(group, old_path).await?.attachment.id;
    wait_transfer(&alice, old_attachment, Direction::Upload, |transfer| {
        transfer.status == TransferStatus::Complete
    })
    .await?;
    let third = tempfile::tempdir()?;
    let scanner_handle = format!("scanner_{suffix}");
    let scanner = sign_in(third.path(), &server, &scanner_handle, &password, true).await?;
    assert!(scanner.issue_group_invite(group).await.is_err());
    let invite = alice.issue_group_invite(group).await?;
    assert_eq!(
        scanner
            .preview_group_invite(invite.code)
            .await?
            .member_count,
        3
    );
    assert_eq!(scanner.join_group_invite(invite.code).await?, group);
    let directory = scanner.sync().await?;
    assert!(
        directory
            .conversations
            .iter()
            .find(|chat| chat.id == group)
            .unwrap()
            .preview
            .is_none()
    );
    assert!(scanner.messages(group, None).await?.items.is_empty());
    let scanner_session: Session = http
        .post(format!("{server}/v2/login"))
        .json(&DeviceLogin {
            credentials: Credentials {
                handle: scanner_handle,
                password,
            },
            device_key: device_key(),
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        http.get(format!("{server}/v1/files/{old_attachment}/chunks/0"))
            .bearer_auth(&scanner_session.token)
            .send()
            .await?
            .status(),
        404
    );
    send(&alice, group, "After joining").await?;
    let new_path = first.path().join("after-join.bin");
    std::fs::write(&new_path, b"New attachment")?;
    let new_attachment = alice.queue_upload(group, new_path).await?.attachment.id;
    wait_transfer(&alice, new_attachment, Direction::Upload, |transfer| {
        transfer.status == TransferStatus::Complete
    })
    .await?;
    assert_eq!(
        scanner.join_group_invite(invite.code).await?,
        group,
        "Retry is idempotent"
    );
    scanner.sync().await?;
    let messages = scanner.messages(group, None).await?.items;
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].attachment.as_ref().unwrap().id, new_attachment);
    assert_eq!(messages[1].text.as_deref(), Some("After joining"));
    assert_eq!(
        http.get(format!("{server}/v1/files/{new_attachment}/chunks/0"))
            .bearer_auth(&scanner_session.token)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?
            .as_ref(),
        b"New attachment"
    );
    let image = [
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
        0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 8, 215, 99, 248, 207, 192, 240, 31,
        0, 5, 0, 1, 255, 137, 153, 61, 29, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    let pasted = alice.queue_pasted_image(chat, image.to_vec()).await?;
    let image_id = pasted.attachment.id;
    wait_transfer(&alice, image_id, Direction::Upload, |job| {
        job.status == TransferStatus::Complete
    })
    .await?;
    bob.sync().await?;
    assert_eq!(pasted.attachment.preview_mime(), Some("image/png"));
    assert_eq!(
        bob.read_attachment_chunk(image_id, 0, image.len()).await?,
        image
    );
    let global = bob.files(FileFilter::default(), None).await?.items;
    assert!(global.iter().any(|item| item.conversation_id == group));
    assert_eq!(global[0].attachment.as_ref().unwrap().id, image_id);
    assert!(
        global
            .windows(2)
            .all(|pair| pair[0].created_at >= pair[1].created_at)
    );
    let scoped = bob
        .files(
            FileFilter {
                conversation_id: Some(chat),
                ..FileFilter::default()
            },
            None,
        )
        .await?
        .items;
    assert_eq!(scoped.len(), 1);
    assert_eq!(scoped[0].attachment.as_ref().unwrap().id, image_id);
    let shared_by_bob = bob
        .files(
            FileFilter {
                sender_id: Some(bob.user.id),
                ..FileFilter::default()
            },
            None,
        )
        .await?;
    assert!(
        shared_by_bob
            .items
            .iter()
            .all(|item| item.sender.id == bob.user.id)
    );
    let newest = bob.files(FileFilter::default(), None).await?.items;
    let oldest = bob
        .files(
            FileFilter {
                oldest_first: true,
                ..FileFilter::default()
            },
            None,
        )
        .await?
        .items;
    assert!(newest.len() >= 2);
    assert_eq!(
        newest.iter().map(|item| item.seq).collect::<Vec<_>>(),
        oldest.iter().rev().map(|item| item.seq).collect::<Vec<_>>()
    );
    assert_eq!(
        bob.files(
            FileFilter {
                name: Some("PASTED IMAGE".into()),
                ..FileFilter::default()
            },
            None
        )
        .await?
        .items
        .len(),
        1
    );

    let nonce = Uuid::new_v4();
    let generated_at: i64 =
        sqlx::query_scalar("SELECT (extract(epoch FROM clock_timestamp()) * 1000)::BIGINT - 1")
            .fetch_one(&db)
            .await?;
    send(&alice, group, "Later text").await?;
    let ai_reply = |readers| PublishAiReply {
        nonce,
        provider: AiProvider::DeepSeek,
        text: "Scoped AI reply".into(),
        created_at: generated_at,
        readers,
        sources: Vec::new(),
    };
    let directory_before = scanner.sync().await?;
    assert!(
        alice
            .publish_ai_reply(
                group,
                PublishAiReply {
                    nonce: Uuid::new_v4(),
                    ..ai_reply(vec![Uuid::new_v4()])
                }
            )
            .await
            .is_err()
    );
    alice
        .publish_ai_reply(group, ai_reply(vec![bob.user.id]))
        .await?;
    alice
        .publish_ai_reply(group, ai_reply(vec![bob.user.id]))
        .await?;
    assert!(
        alice
            .publish_ai_reply(group, ai_reply(vec![scanner.user.id]))
            .await
            .is_err()
    );
    let reader_preview = bob.sync().await?;
    assert_eq!(
        reader_preview
            .conversations
            .iter()
            .find(|chat| chat.id == group)
            .unwrap()
            .preview
            .as_deref(),
        Some("Later text")
    );
    let ordered = bob.messages(group, None).await?.items;
    let reply = &ordered[1];
    assert_eq!(reply.ai_provider, Some(AiProvider::DeepSeek));
    assert_eq!(reply.text.as_deref(), Some("Scoped AI reply"));
    assert_eq!(reply.created_at, generated_at);
    assert_eq!(ordered[0].text.as_deref(), Some("Later text"));
    assert!(ordered[0].created_at >= reply.created_at && ordered[0].seq < reply.seq);
    assert_eq!(
        bob.messages(group, Some(ordered[0].seq)).await?.items[0].seq,
        reply.seq
    );
    let directory_after = scanner.sync().await?;
    let preview = |directory: zibblu_core::Directory| {
        directory
            .conversations
            .into_iter()
            .find(|chat| chat.id == group)
            .unwrap()
            .preview
    };
    assert_ne!(preview(directory_before), Some("Scoped AI reply".into()));
    assert_eq!(preview(directory_after), Some("Later text".into()));
    assert!(
        scanner
            .search_messages(group, None, Some("Scoped AI reply"))
            .await?
            .items
            .is_empty()
    );
    assert!(
        scanner
            .ai_messages(
                group,
                AiScope::Selected {
                    sequences: vec![reply.seq]
                }
            )
            .await
            .is_err()
    );
    let preview_nonce = Uuid::new_v4();
    alice
        .publish_ai_reply(
            group,
            PublishAiReply {
                nonce: preview_nonce,
                provider: AiProvider::OpenAi,
                text: "Latest restricted preview".into(),
                created_at: i64::MAX,
                readers: vec![bob.user.id],
                sources: Vec::new(),
            },
        )
        .await?;
    assert_eq!(
        preview(bob.sync().await?),
        Some("Latest restricted preview".into())
    );
    assert_eq!(preview(scanner.sync().await?), Some("Later text".into()));
    assert_eq!(
        ordered
            .iter()
            .filter(|item| item.ai_provider.is_some())
            .count(),
        1
    );
    scanner.leave_group(group).await?;
    let group_chunk = format!("{server}/v1/files/{new_attachment}/chunks/0");
    assert_eq!(
        http.get(&group_chunk)
            .bearer_auth(scanner.session_token())
            .send()
            .await?
            .status(),
        404
    );
    assert!(
        scanner
            .sync()
            .await?
            .conversations
            .iter()
            .all(|chat| chat.id != group)
    );
    assert_eq!(scanner.join_group_invite(invite.code).await?, group);
    assert_eq!(
        http.get(&group_chunk)
            .bearer_auth(scanner.session_token())
            .send()
            .await?
            .status(),
        404
    );
    send(&alice, group, "After joining again").await?;
    scanner.sync().await?;
    assert_eq!(
        scanner.messages(group, None).await?.items[0]
            .text
            .as_deref(),
        Some("After joining again")
    );
    alice.remove_contact(bob.user.id).await?;
    assert!(
        send(&bob, chat, "A removed contact cannot send")
            .await
            .is_err()
    );
    println!(
        "Pasted bytes, file-name filtering and conversation isolation verified; AI audience enforced in feed, preview and SQLite context"
    );
    println!("QR admission exposed only post-join messages, previews and attachments");
    task.abort();
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn desktop_experience_permissions_and_sync() -> Result<()> {
    let url = std::env::var("ZIBBLU_TEST_DATABASE_URL")?;
    let db = PgPoolOptions::new()
        .max_connections(6)
        .connect(&url)
        .await?;
    let app = zibblu_online::router(db).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let server = format!("http://{}", listener.local_addr()?);
    let task = tokio::spawn(async move { axum::serve(listener, app).await });
    let data = tempfile::tempdir()?;
    let suffix = &Uuid::new_v4().simple().to_string()[..10];
    let password = format!("acceptance-{suffix}");
    let alice_handle = format!("alice_{suffix}");
    let bob_handle = format!("bob_{suffix}");
    let alice = sign_in(data.path(), &server, &alice_handle, &password, true).await?;
    let bob = sign_in(data.path(), &server, &bob_handle, &password, true).await?;
    let outsider = sign_in(
        data.path(),
        &server,
        &format!("other_{suffix}"),
        &password,
        true,
    )
    .await?;

    let mut image = Vec::new();
    png::Encoder::new(&mut image, 1, 1)
        .write_header()?
        .write_image_data(&[0])?;
    alice.set_bio("A private test bio".into()).await?;
    assert!(
        alice
            .set_profile_image(ProfileImage::Avatar, b"not PNG".to_vec())
            .await
            .is_err()
    );
    assert!(
        alice
            .set_profile_image(ProfileImage::Avatar, vec![0; MAX_PROFILE_IMAGE_BYTES + 1])
            .await
            .is_err()
    );
    alice
        .set_profile_image(ProfileImage::Avatar, image.clone())
        .await?;
    alice
        .set_profile_image(ProfileImage::Cover, image.clone())
        .await?;
    let own = alice.profile(alice.user.id).await?;
    let cover = own.cover.context("own cover version")?;
    let basic = bob.profile(alice.user.id).await?;
    assert!(matches!(basic.relation, ProfileRelation::Stranger));
    assert_eq!(
        bob.profile_image(alice.user.id, basic.avatar.unwrap())
            .await?,
        image
    );
    assert!(basic.cover.is_none() && basic.bio.is_none() && basic.notes.is_none());
    assert!(bob.profile_image(alice.user.id, cover).await.is_err());
    assert!(
        bob.set_contact_notes(
            alice.user.id,
            ContactNotes {
                remark: "Denied".into(),
                contact_info: "Denied".into()
            }
        )
        .await
        .is_err()
    );

    alice
        .request_contact(bob.user.id, "Decline this greeting".into())
        .await?;
    let pending = bob.sync().await?.pending_contacts.remove(0);
    assert_eq!(pending.greeting, "Decline this greeting");
    assert!(outsider.sync().await?.pending_contacts.is_empty());
    assert!(bob.directory().await?.conversations.is_empty());
    bob.decide_contact(alice.user.id, false, pending.request_id)
        .await?;
    assert!(alice.sync().await?.conversations.is_empty());
    alice
        .request_contact(bob.user.id, "Hello after approval".into())
        .await?;
    let incoming = bob.sync().await?.pending_contacts.remove(0);
    assert!(
        bob.decide_contact(alice.user.id, true, pending.request_id)
            .await
            .is_err()
    );
    assert!(
        outsider
            .decide_contact(alice.user.id, true, incoming.request_id)
            .await
            .is_err()
    );
    let (first, retry) = tokio::join!(
        bob.decide_contact(alice.user.id, true, incoming.request_id),
        bob.decide_contact(alice.user.id, true, incoming.request_id)
    );
    first?;
    retry?;
    let synced = bob.sync().await?;
    let chat = synced.conversations[0].id;
    assert_eq!(synced.conversations[0].unread, 1);
    let greetings = bob.messages(chat, None).await?.items;
    assert_eq!(greetings.len(), 1);
    assert_eq!(greetings[0].text.as_deref(), Some("Hello after approval"));
    let greeting = greetings[0].seq;
    let profile = bob.profile(alice.user.id).await?;
    assert!(matches!(profile.relation, ProfileRelation::Contact));
    assert_eq!(profile.bio.as_deref(), Some("A private test bio"));
    assert_eq!(bob.profile_image(alice.user.id, cover).await?, image);
    bob.set_contact_notes(
        alice.user.id,
        ContactNotes {
            remark: "Local nickname".into(),
            contact_info: "Private number".into(),
        },
    )
    .await?;
    assert_eq!(
        bob.sync()
            .await?
            .remarks
            .get(&alice.user.id)
            .map(String::as_str),
        Some("Local nickname")
    );
    assert_eq!(
        bob.profile(alice.user.id)
            .await?
            .notes
            .unwrap()
            .contact_info,
        "Private number"
    );
    assert!(
        alice
            .profile(bob.user.id)
            .await?
            .notes
            .unwrap()
            .contact_info
            .is_empty()
    );
    assert!(outsider.profile(alice.user.id).await?.notes.is_none());

    let send_receipt = |through| ReadUpdate {
        through: Some(through),
        sent: vec![],
        viewed: None,
    };
    let query = |seq| ReadUpdate {
        through: None,
        sent: vec![seq],
        viewed: None,
    };
    bob.read_update(chat, send_receipt(greeting)).await?;
    assert_eq!(bob.sync().await?.conversations[0].unread, 0);
    alice.set_read_receipts(true).await?;
    bob.set_read_receipts(true).await?;
    assert!(alice.read_update(chat, query(greeting)).await?.is_empty());
    send(&alice, chat, "Read by both accounts").await?;
    let http = reqwest::Client::builder().no_proxy().build()?;
    let bob_session: Session = http
        .post(format!("{server}/v2/login"))
        .json(&DeviceLogin {
            credentials: Credentials {
                handle: bob_handle.clone(),
                password: password.clone(),
            },
            device_key: device_key(),
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        http.get(format!("{server}/v1/sync?after={greeting}"))
            .send()
            .await?
            .status(),
        426
    );
    let url = format!("{server}/v2/sync?cursor={}", bob_session.cursor);
    let page: SyncPage = http
        .get(&url)
        .bearer_auth(&bob_session.token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let repeated: SyncPage = http
        .get(&url)
        .bearer_auth(&bob_session.token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(page.page_id, repeated.page_id);
    let sent = page
        .events
        .iter()
        .find_map(|event| match event {
            SyncEvent::Published(message) => Some(message.seq),
            SyncEvent::Recalled(_) => None,
        })
        .context("Expected an unconfirmed message")?;
    assert_eq!(alice.update_receipt(chat, query(sent)).await?, None);
    let foreign = http
        .post(format!("{server}/v2/sync/ack"))
        .bearer_auth(alice.session_token())
        .json(&SyncAck {
            page_id: page.page_id,
            metadata_received: vec![sent],
        })
        .send()
        .await?;
    assert_eq!(foreign.status(), 409);
    assert_eq!(
        http.get(&url)
            .bearer_auth(alice.session_token())
            .send()
            .await?
            .status(),
        409
    );
    http.post(format!("{server}/v1/conversations/{chat}/read?latest=true"))
        .bearer_auth(&bob_session.token)
        .json(&ReadUpdate {
            through: None,
            sent: vec![],
            viewed: Some(sent),
        })
        .send()
        .await?
        .error_for_status()?;
    assert_eq!(alice.update_receipt(chat, query(sent)).await?, None);

    let bob_profile = format!(
        "{:x}",
        Sha256::digest(format!("{server}/\n{}", bob.user.id))
    );
    let sqlite = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new().filename(
                data.path()
                    .join("profiles-v2")
                    .join(bob_profile)
                    .join("chat.sqlite3"),
            ),
        )
        .await?;
    sqlx::raw_sql(
        "CREATE TRIGGER reject_fixture_insert BEFORE INSERT ON inbox
        BEGIN SELECT RAISE(ABORT, 'fixture commit failed'); END;",
    )
    .execute(&sqlite)
    .await?;
    assert!(bob.sync().await.is_err());
    assert_eq!(alice.update_receipt(chat, query(sent)).await?, None);
    sqlx::query("DROP TRIGGER reject_fixture_insert")
        .execute(&sqlite)
        .await?;
    bob.sync().await?;
    let delivered = Some(MessageReceipt {
        seq: sent,
        status: ReceiptStatus::Delivered,
    });
    assert_eq!(alice.update_receipt(chat, query(sent)).await?, delivered);
    assert!(outsider.read_update(chat, query(sent)).await.is_err());
    assert!(bob.read_update(chat, send_receipt(sent + 1)).await.is_err());
    bob.read_update(chat, send_receipt(sent)).await?;
    assert_eq!(alice.update_receipt(chat, query(sent)).await?, delivered);
    bob.update_receipt(
        chat,
        ReadUpdate {
            viewed: Some(sent),
            ..send_receipt(sent)
        },
    )
    .await?;
    assert_eq!(
        alice.update_receipt(chat, query(sent)).await?,
        Some(MessageReceipt {
            seq: sent,
            status: ReceiptStatus::Read
        })
    );
    assert!(bob.read_update(chat, query(sent)).await?.is_empty());
    assert_eq!(alice.read_update(chat, query(sent)).await?, [sent]);
    let other_device = tempfile::tempdir()?;
    let bob_on_second_client =
        sign_in(other_device.path(), &server, &bob_handle, &password, false).await?;
    let second_sync = bob_on_second_client.sync().await?;
    assert_eq!(second_sync.conversations[0].unread, 0);
    assert!(second_sync.read_receipts);
    bob.set_read_receipts(false).await?;
    assert_eq!(alice.update_receipt(chat, query(sent)).await?, None);
    assert!(alice.read_update(chat, query(sent)).await?.is_empty());
    bob.set_read_receipts(true).await?;
    assert_eq!(alice.update_receipt(chat, query(sent)).await?, None);
    assert!(alice.read_update(chat, query(sent)).await?.is_empty());

    bob.send_message(
        chat,
        SendMessage {
            nonce: Uuid::new_v4(),
            text: "Reply to Alice".into(),
            reply_to: Some(sent),
        },
    )
    .await?;
    alice.sync().await?;
    assert_eq!(alice.update_receipt(chat, query(sent)).await?, None);
    assert_eq!(
        alice.messages(chat, None).await?.items[0].reply_to,
        Some(sent)
    );
    bob.hide_message(chat, sent).await?;
    assert!(bob.hide_message(Uuid::new_v4(), sent).await.is_err());
    assert!(bob.quoted_message(chat, sent).await?.is_none());
    assert!(
        bob.ai_messages(
            chat,
            AiScope::Selected {
                sequences: vec![sent]
            }
        )
        .await
        .is_err()
    );
    assert!(alice.quoted_message(chat, sent).await?.is_some());
    assert!(
        bob_on_second_client
            .quoted_message(chat, sent)
            .await?
            .is_none()
    );
    assert!(
        bob.sync()
            .await?
            .conversations
            .iter()
            .any(|item| item.id == chat)
    );
    assert!(
        bob.messages(chat, None)
            .await?
            .items
            .iter()
            .all(|item| item.seq != sent)
    );

    alice.remove_contact(bob.user.id).await?;
    assert!(bob.profile_image(alice.user.id, cover).await.is_err());
    assert!(bob.profile(alice.user.id).await?.notes.is_none());
    assert!(bob.sync().await?.remarks.is_empty());
    task.abort();
    Ok(())
}

fn device_key() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

async fn sign_in(
    root: &Path,
    server: &str,
    handle: &str,
    password: &str,
    register: bool,
) -> Result<Arc<Client>> {
    Client::login(
        root,
        server,
        Credentials {
            handle: handle.into(),
            password: password.into(),
        },
        register,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn session_restore_validates_account_before_opening_the_local_profile() -> Result<()> {
    let db = PgPoolOptions::new()
        .max_connections(6)
        .connect(&std::env::var("ZIBBLU_TEST_DATABASE_URL")?)
        .await?;
    let app = zibblu_online::router(db.clone()).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let server = format!("http://{}", listener.local_addr()?);
    let task = tokio::spawn(async move { axum::serve(listener, app).await });
    let data = tempfile::tempdir()?;
    let rejected = tempfile::tempdir()?;
    let suffix = &Uuid::new_v4().simple().to_string()[..10];
    let password = format!("acceptance-{suffix}");
    let alice = sign_in(
        data.path(),
        &server,
        &format!("resume_{suffix}"),
        &password,
        true,
    )
    .await?;
    let bob = sign_in(
        data.path(),
        &server,
        &format!("peer_{suffix}"),
        &password,
        true,
    )
    .await?;
    let alice_id = alice.user.id;
    assert!(
        Client::restore(rejected.path(), &server, alice_id, bob.session_token())
            .await
            .is_err()
    );
    assert!(
        Client::restore(rejected.path(), &server, alice_id, "invalid")
            .await
            .is_err()
    );
    assert!(!rejected.path().join("profiles-v2").exists());

    alice.add_contact(bob.user.id).await?;
    bob.respond_contact(alice_id, true).await?;
    let conversation = alice
        .create_conversation(ConversationRequest::Direct {
            contact_id: bob.user.id,
        })
        .await?;
    send(&alice, conversation, "History before app exit").await?;
    alice.sync().await?;
    let token = alice.session_token().to_owned();
    drop(alice);
    let restored = Client::restore(data.path(), &server, alice_id, &token).await?;
    assert_eq!(restored.user.id, alice_id);
    assert_eq!(
        restored.messages(conversation, None).await?.items[0]
            .text
            .as_deref(),
        Some("History before app exit")
    );
    restored.logout().await?;
    assert!(
        Client::restore(rejected.path(), &server, alice_id, &token)
            .await
            .is_err()
    );

    sqlx::query(
        "UPDATE sessions SET expires_at = now() - interval '1 second' WHERE token_hash = $1",
    )
    .bind(Sha256::digest(bob.session_token().as_bytes()).as_slice())
    .execute(&db)
    .await?;
    assert!(
        Client::restore(rejected.path(), &server, bob.user.id, bob.session_token())
            .await
            .is_err()
    );
    assert!(!rejected.path().join("profiles-v2").exists());
    task.abort();
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn conversation_media_cache_end_to_end() -> Result<()> {
    let url = std::env::var("ZIBBLU_TEST_DATABASE_URL")
        .context("Set ZIBBLU_TEST_DATABASE_URL to a disposable PostgreSQL database")?;
    let db = PgPoolOptions::new()
        .max_connections(6)
        .connect(&url)
        .await?;
    let app = zibblu_online::router(db.clone()).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let server = format!("http://{}", listener.local_addr()?);
    let task = tokio::spawn(async move { axum::serve(listener, app).await });

    let shared_data = tempfile::tempdir()?;
    let suffix = &Uuid::new_v4().simple().to_string()[..10];
    let password = format!("media-cache-{suffix}");
    let alice = sign_in(
        shared_data.path(),
        &server,
        &format!("mc_alice_{suffix}"),
        &password,
        true,
    )
    .await?;
    let bob = sign_in(
        shared_data.path(),
        &server,
        &format!("mc_bob_{suffix}"),
        &password,
        true,
    )
    .await?;
    let bob_id = bob.user.id;
    let bob_token = bob.session_token().to_owned();

    alice.add_contact(bob.user.id).await?;
    bob.respond_contact(alice.user.id, true).await?;
    alice.sync().await?;
    bob.sync().await?;

    let carol = sign_in(
        shared_data.path(),
        &server,
        &format!("mc_carol_{suffix}"),
        &password,
        true,
    )
    .await?;
    alice.add_contact(carol.user.id).await?;
    carol.respond_contact(alice.user.id, true).await?;
    alice.sync().await?;
    carol.sync().await?;
    let conv_a = alice
        .create_conversation(ConversationRequest::Direct {
            contact_id: bob.user.id,
        })
        .await?;
    let conv_b = alice
        .create_conversation(ConversationRequest::Group {
            title: "Media Cache Group".into(),
            contact_ids: vec![bob.user.id, carol.user.id],
        })
        .await?;
    alice.sync().await?;
    bob.sync().await?;

    // Valid 1x1 PNG bytes for chat media and two distinct valid PNGs for avatar v1/v2.
    let png_chat: Vec<u8> = vec![
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
        0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 8, 215, 99, 248, 207, 192, 240, 31,
        0, 5, 0, 1, 255, 137, 153, 61, 29, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    let encode = |side: u32| -> Result<Vec<u8>> {
        let mut out = Vec::new();
        png::Encoder::new(&mut out, side, side)
            .write_header()?
            .write_image_data(&vec![0; (side * side) as usize])?;
        Ok(out)
    };
    let png_avatar_v1 = encode(1)?;
    let png_avatar_v2 = encode(2)?;

    // Alice sends identical image once to conv_a and twice to conv_b.
    let up_a = alice.queue_pasted_image(conv_a, png_chat.clone()).await?;
    wait_transfer(&alice, up_a.attachment.id, Direction::Upload, |job| {
        job.status == TransferStatus::Complete
    })
    .await?;
    let up_b1 = alice.queue_pasted_image(conv_b, png_chat.clone()).await?;
    wait_transfer(&alice, up_b1.attachment.id, Direction::Upload, |job| {
        job.status == TransferStatus::Complete
    })
    .await?;
    let up_b2 = alice.queue_pasted_image(conv_b, png_chat.clone()).await?;
    wait_transfer(&alice, up_b2.attachment.id, Direction::Upload, |job| {
        job.status == TransferStatus::Complete
    })
    .await?;

    // Set Alice avatar v1 and sync Bob.
    let avatar_v1 = alice
        .set_profile_image(ProfileImage::Avatar, png_avatar_v1.clone())
        .await?;
    bob.sync().await?;

    // Verify account isolation under shared_data: two distinct profile dirs exist.
    let profile_dirs: Vec<_> = std::fs::read_dir(shared_data.path().join("profiles-v2"))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();
    assert_eq!(profile_dirs.len(), 3);

    let msgs_a = bob.messages(conv_a, None).await?.items;
    let msgs_b = bob.messages(conv_b, None).await?.items;
    let msg_a = msgs_a.iter().find(|m| m.attachment.is_some()).context("missing msg_a")?;
    let att_a = msg_a.attachment.as_ref().unwrap();
    let msgs_b_att: Vec<_> = msgs_b.iter().filter(|m| m.attachment.is_some()).collect();
    assert_eq!(msgs_b_att.len(), 2);

    // Bob fetches & commits image into conv_a and both messages of conv_b.
    for (conv, msg) in [
        (conv_a, msg_a),
        (conv_b, msgs_b_att[0]),
        (conv_b, msgs_b_att[1]),
    ] {
        let att = msg.attachment.as_ref().unwrap();
        let bytes = bob.read_attachment_chunk(att.id, 0, att.size as usize).await?;
        assert_eq!(bytes, png_chat);
        bob.remember_image_variant(conv, msg.seq, Some(att.id), &att.sha256, false, "image/png", &bytes)
            .await?;
        bob.remember_image_variant(conv, msg.seq, Some(att.id), &att.sha256, true, "image/png", &bytes)
            .await?;
    }

    // Verify on-disk directory & SQLite state for Bob.
    let bob_profile_key = format!("{:x}", Sha256::digest(format!("{server}/\n{bob_id}")));
    let bob_profile = shared_data.path().join("profiles-v2").join(bob_profile_key);
    let bob_sqlite = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(SqliteConnectOptions::new().filename(bob_profile.join("chat.sqlite3")))
        .await?;

    let conv_a_paths: Vec<String> = sqlx::query_scalar(
        "SELECT rel_path FROM media_files WHERE conversation_id = ? AND variant = 'image_original' AND status = 'available'",
    )
    .bind(conv_a.to_string())
    .fetch_all(&bob_sqlite)
    .await?;
    let conv_b_paths: Vec<String> = sqlx::query_scalar(
        "SELECT rel_path FROM media_files WHERE conversation_id = ? AND variant = 'image_original' AND status = 'available'",
    )
    .bind(conv_b.to_string())
    .fetch_all(&bob_sqlite)
    .await?;
    assert_eq!(conv_a_paths.len(), 1);
    assert_eq!(conv_b_paths.len(), 1);
    assert_ne!(conv_a_paths[0], conv_b_paths[0]);
    assert!(conv_a_paths[0].contains(&format!("/conversations/{conv_a}/images/")));
    assert!(conv_b_paths[0].contains(&format!("/conversations/{conv_b}/images/")));

    // Same-conversation dedup: 2 message refs pointing to 1 media_files row in conv_b.
    let conv_b_refs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM media_refs WHERE conversation_id = ? AND variant = 'image_original'",
    )
    .bind(conv_b.to_string())
    .fetch_one(&bob_sqlite)
    .await?;
    assert_eq!(conv_b_refs, 2);

    // Locate device media root and verify both physical files exist independently.
    let device_dir = std::fs::read_dir(bob_profile.join("media"))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.is_dir())
        .context("missing Bob device media dir")?;
    let phys_a = bob_profile.join(&conv_a_paths[0]);
    let phys_b = bob_profile.join(&conv_b_paths[0]);
    assert!(phys_a.is_file());
    assert!(phys_b.is_file());

    // External Save As copy from conv_a before deleting conv_a.
    let export_dir = tempfile::tempdir()?;
    let saved_copy = export_dir.path().join("exported.png");
    bob.queue_download(att_a.id, saved_copy.clone()).await?;
    wait_transfer(&bob, att_a.id, Direction::Download, |job| {
        job.status == TransferStatus::Complete
    })
    .await?;
    assert_eq!(std::fs::read(&saved_copy)?, png_chat);

    // Fetch Alice avatar v1 into public cache, then update Alice to v2.
    let fetched_v1 = bob
        .profile_image_verified(alice.user.id, avatar_v1, |b| {
            ensure!(b.starts_with(b"\x89PNG"));
            Ok(())
        })
        .await?;
    assert_eq!(fetched_v1, png_avatar_v1);
    let v1_rel: String = sqlx::query_scalar(
        "SELECT f.rel_path FROM public_cache_refs r JOIN public_cache_files f ON f.id = r.active_file_id
         WHERE r.user_id = ? AND r.kind = 'avatar'",
    )
    .bind(alice.user.id.to_string())
    .fetch_one(&bob_sqlite)
    .await?;
    let v1_phys = bob_profile.join(&v1_rel);
    assert!(v1_phys.is_file());

    let avatar_v2 = alice
        .set_profile_image(ProfileImage::Avatar, png_avatar_v2.clone())
        .await?;
    bob.sync().await?;
    // Before v2 is fetched, fallback still returns v1.
    let fallback = bob
        .active_profile_image_for_request(alice.user.id, avatar_v2)
        .await;
    assert_eq!(fallback.as_ref().map(|(v, b)| (*v, b.as_slice())), Some((avatar_v1, png_avatar_v1.as_slice())));
    // Commit v2 atomically and confirm v1 physical file is automatically reclaimed.
    let fetched_v2 = bob
        .profile_image_verified(alice.user.id, avatar_v2, |b| {
            ensure!(b.starts_with(b"\x89PNG"));
            Ok(())
        })
        .await?;
    assert_eq!(fetched_v2, png_avatar_v2);
    assert!(!v1_phys.exists());

    // Delete/hide conv_a message: conv_a file reclaimed, conv_b file + external Save As unaffected.
    bob.hide_message(conv_a, msg_a.seq).await?;
    assert!(!phys_a.exists());
    assert!(phys_b.is_file());
    assert_eq!(std::fs::read(&saved_copy)?, png_chat);

    // Create a stale temp file, run clear_cache, and confirm conv_b media & saved_copy survive.
    let stale_tmp = device_dir.join("tmp").join("stale-acceptance.part");
    std::fs::create_dir_all(stale_tmp.parent().unwrap())?;
    std::fs::write(&stale_tmp, b"partial")?;
    let cleanup = bob.clear_cache().await?;
    assert!(cleanup.removed_temp_files >= 1);
    assert!(!stale_tmp.exists());
    assert!(phys_b.is_file());
    assert_eq!(std::fs::read(&saved_copy)?, png_chat);

    // Hide first message in conv_b: physical file MUST remain because 2nd message still references it.
    bob.hide_message(conv_b, msgs_b_att[0].seq).await?;
    assert!(phys_b.is_file());

    // Simulate process restart via Client::restore, then shut down server and verify conv_b reads offline!
    bob_sqlite.close().await;
    drop(bob);
    let bob_restarted = Client::restore(shared_data.path(), &server, bob_id, &bob_token).await?;
    task.abort();
    let b2 = msgs_b_att[1];
    let b2_att = b2.attachment.as_ref().unwrap();
    let offline_orig = bob_restarted
        .cached_image_variant(conv_b, b2.seq, Some(b2_att.id), &b2_att.sha256, false)
        .await?;
    assert_eq!(offline_orig.as_ref().map(|(_, b)| b.as_slice()), Some(png_chat.as_slice()));
    let offline_thumb = bob_restarted
        .cached_image_variant(conv_b, b2.seq, Some(b2_att.id), &b2_att.sha256, true)
        .await?;
    assert_eq!(offline_thumb.as_ref().map(|(_, b)| b.as_slice()), Some(png_chat.as_slice()));

    // Hiding the final remaining reference in conv_b reclaims phys_b.
    bob_restarted.hide_message(conv_b, b2.seq).await?;
    assert!(!phys_b.exists());
    assert_eq!(std::fs::read(&saved_copy)?, png_chat);
    Ok(())
}

async fn send(client: &Client, chat: Uuid, text: &str) -> Result<()> {
    client
        .send_message(
            chat,
            SendMessage {
                nonce: Uuid::new_v4(),
                text: text.into(),
                reply_to: None,
            },
        )
        .await
}

async fn wait_transfer(
    client: &Client,
    id: Uuid,
    direction: Direction,
    ready: impl Fn(&Transfer) -> bool,
) -> Result<Transfer> {
    tokio::time::timeout(Duration::from_secs(40), async {
        loop {
            if let Some(transfer) =
                client.transfers().await?.into_iter().find(|transfer| {
                    transfer.attachment.id == id && transfer.direction == direction
                })
            {
                ensure!(
                    transfer.status != TransferStatus::Failed,
                    "Transfer failed: {:?}",
                    transfer.error
                );
                if ready(&transfer) {
                    return Ok(transfer);
                }
                ensure!(
                    transfer.status != TransferStatus::Complete,
                    "Transfer completed before interruption"
                );
            }
            tokio::time::sleep(Duration::from_millis(4)).await;
        }
    })
    .await
    .context("Transfer did not reach the expected state")?
}
