use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use sqlx::postgres::PgPoolOptions;
use std::{path::Path, sync::Arc, time::Duration};
use uuid::Uuid;
use zibblu_core::{
    CHUNK_SIZE, Client, ConversationRequest, Credentials, Direction, FileFilter, SendMessage,
    Session, Transfer, TransferStatus,
};

#[tokio::test(flavor = "multi_thread")]
async fn normal_chat_and_files() -> Result<()> {
    let url = std::env::var("ZIBBLU_TEST_DATABASE_URL")
        .context("Set ZIBBLU_TEST_DATABASE_URL to a disposable PostgreSQL database")?;
    let db = PgPoolOptions::new()
        .max_connections(6)
        .connect(&url)
        .await?;
    let app = zibblu_online::router(db).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let server = format!("http://{}", listener.local_addr()?);
    let task = tokio::spawn(async move { axum::serve(listener, app).await });
    let first = tempfile::tempdir()?;
    let second = tempfile::tempdir()?;
    let suffix = &Uuid::new_v4().simple().to_string()[..10];
    let password = format!("acceptance-{suffix}");
    let alice_handle = format!("alice_{suffix}");
    let bob_handle = format!("bob_{suffix}");
    let outsider_handle = format!("outsider_{suffix}");
    let alice = sign_in(first.path(), &server, &alice_handle, &password, true).await?;
    let bob = sign_in(second.path(), &server, &bob_handle, &password, true).await?;

    let contact = alice.find_user(&bob_handle).await?;
    assert_eq!(contact.id, bob.user.id);
    alice.add_contact(contact.id).await?;
    let chat = alice
        .create_conversation(ConversationRequest::Direct {
            contact_id: contact.id,
        })
        .await?;
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
    let bob = sign_in(second.path(), &server, &bob_handle, &password, false).await?;
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
    println!(
        "Upload resumed from byte {} of {}, appeared in both Files views, and matched SHA-256 after download",
        paused.received, upload.attachment.size
    );

    let http = reqwest::Client::builder().no_proxy().build()?;
    let outsider: Session = http
        .post(format!("{server}/v1/register"))
        .json(&Credentials {
            handle: outsider_handle,
            password: password.clone(),
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

    alice.add_contact(outsider.user.id).await?;
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
        .post(format!("{server}/v1/login"))
        .json(&Credentials {
            handle: scanner_handle,
            password,
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
    println!("QR admission exposed only post-join messages, previews and attachments");
    task.abort();
    Ok(())
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

async fn send(client: &Client, chat: Uuid, text: &str) -> Result<()> {
    client
        .send_message(
            chat,
            SendMessage {
                nonce: Uuid::new_v4(),
                text: text.into(),
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
