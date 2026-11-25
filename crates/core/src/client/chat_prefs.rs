use super::{Client, unix_millis};
use crate::protocol::Conversation;
use anyhow::{Result, ensure};
use serde::Serialize;
use sqlx::SqlitePool;
use std::{cmp::Reverse, collections::HashMap};
use uuid::Uuid;

#[derive(Clone, Debug, Default)]
pub(super) struct ConversationPref {
    pub pinned_at: Option<i64>,
    pub manual_unread: bool,
    pub muted: bool,
    pub hidden_at_seq: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ConversationPrefState {
    pub conversation_id: Uuid,
    pub pinned: bool,
    pub manual_unread: bool,
    pub muted: bool,
}

impl Client {
    async fn ensure_normal_member(&self, conversation: Uuid) -> Result<()> {
        self.ensure_open()?;
        let directory = self.membership_directory().await?;
        ensure!(
            directory.conversations.iter().any(|chat| {
                chat.id == conversation && chat.members.iter().any(|member| member.id == self.user.id)
            }),
            "This conversation is unavailable"
        );
        Ok(())
    }

    pub async fn set_conversation_pinned(&self, conversation: Uuid, pinned: bool) -> Result<()> {
        self.ensure_normal_member(conversation).await?;
        let pinned_at = pinned.then(unix_millis);
        sqlx::query(
            "INSERT INTO conversation_prefs (conversation_id, pinned_at) VALUES (?, ?)
             ON CONFLICT (conversation_id) DO UPDATE SET pinned_at = excluded.pinned_at",
        )
        .bind(conversation.to_string())
        .bind(pinned_at)
        .execute(&self.db)
        .await?;
        self.history_changed();
        Ok(())
    }

    pub async fn set_conversation_manual_unread(
        &self,
        conversation: Uuid,
        unread: bool,
    ) -> Result<()> {
        self.ensure_normal_member(conversation).await?;
        sqlx::query(
            "INSERT INTO conversation_prefs (conversation_id, manual_unread) VALUES (?, ?)
             ON CONFLICT (conversation_id) DO UPDATE SET manual_unread = excluded.manual_unread",
        )
        .bind(conversation.to_string())
        .bind(i64::from(unread))
        .execute(&self.db)
        .await?;
        self.history_changed();
        Ok(())
    }

    pub async fn set_conversation_muted(&self, conversation: Uuid, muted: bool) -> Result<()> {
        self.ensure_normal_member(conversation).await?;
        sqlx::query(
            "INSERT INTO conversation_prefs (conversation_id, muted) VALUES (?, ?)
             ON CONFLICT (conversation_id) DO UPDATE SET muted = excluded.muted",
        )
        .bind(conversation.to_string())
        .bind(i64::from(muted))
        .execute(&self.db)
        .await?;
        self.history_changed();
        Ok(())
    }

    pub async fn hide_conversation(&self, conversation: Uuid) -> Result<()> {
        self.ensure_normal_member(conversation).await?;
        let id = conversation.to_string();
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        let max_seq: i64 =
            sqlx::query_scalar("SELECT coalesce(max(seq), 0) FROM inbox WHERE conversation_id = ?")
                .bind(&id)
                .fetch_one(&mut *tx)
                .await?;
        sqlx::query(
            "INSERT INTO conversation_prefs (conversation_id, hidden_at_seq) VALUES (?, ?)
             ON CONFLICT (conversation_id) DO UPDATE SET hidden_at_seq = excluded.hidden_at_seq",
        )
        .bind(&id)
        .bind(max_seq)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.history_changed();
        Ok(())
    }

    pub async fn unhide_conversation(&self, conversation: Uuid) -> Result<()> {
        self.ensure_normal_member(conversation).await?;
        sqlx::query("UPDATE conversation_prefs SET hidden_at_seq = NULL WHERE conversation_id = ?")
            .bind(conversation.to_string())
            .execute(&self.db)
            .await?;
        self.history_changed();
        Ok(())
    }

    pub async fn hidden_group_conversations(&self) -> Result<Vec<Conversation>> {
        self.ensure_open()?;
        let hidden: Vec<String> = sqlx::query_scalar(
            "SELECT conversation_id FROM conversation_prefs WHERE hidden_at_seq IS NOT NULL",
        )
        .fetch_all(&self.db)
        .await?;
        let mut groups: Vec<Conversation> = self
            .membership_directory()
            .await?
            .conversations
            .into_iter()
            .filter(|chat| {
                chat.title.is_some()
                    && hidden.contains(&chat.id.to_string())
                    && chat.members.iter().any(|member| member.id == self.user.id)
            })
            .collect();
        sort_conversations(&mut groups);
        Ok(groups)
    }

    pub async fn mark_conversation_read_local(&self, conversation: Uuid) -> Result<()> {
        self.ensure_normal_member(conversation).await?;
        let id = conversation.to_string();
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query("UPDATE conversation_prefs SET manual_unread = 0 WHERE conversation_id = ?")
            .bind(&id)
            .execute(&mut *tx)
            .await?;
        let current_read: i64 = sqlx::query_scalar(
            "SELECT read_through FROM local_read_positions WHERE conversation_id = ?",
        )
        .bind(&id)
        .fetch_optional(&mut *tx)
        .await?
        .unwrap_or(0);
        let max_visible_seq: Option<i64> = sqlx::query_scalar(
            "SELECT max(seq) FROM inbox
             WHERE conversation_id = ? AND seq NOT IN (SELECT seq FROM hidden_messages)",
        )
        .bind(&id)
        .fetch_one(&mut *tx)
        .await?;
        if let Some(max_seq) = max_visible_seq
            && max_seq > current_read
        {
            sqlx::query(
                "INSERT INTO local_read_positions (conversation_id, read_through) VALUES (?, ?)
                 ON CONFLICT (conversation_id) DO UPDATE SET read_through = max(read_through, excluded.read_through)",
            )
            .bind(&id)
            .bind(max_seq)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        self.history_changed();
        Ok(())
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) async fn load_prefs_map(db: &SqlitePool) -> Result<HashMap<String, ConversationPref>> {
    let rows: Vec<(String, Option<i64>, i64, i64, Option<i64>)> = sqlx::query_as(
        "SELECT conversation_id, pinned_at, manual_unread, muted, hidden_at_seq FROM conversation_prefs",
    )
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, pinned_at, manual_unread, muted, hidden_at_seq)| {
            (
                id,
                ConversationPref {
                    pinned_at,
                    manual_unread: manual_unread != 0,
                    muted: muted != 0,
                    hidden_at_seq,
                },
            )
        })
        .collect())
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn is_conversation_hidden(hidden_at_seq: Option<i64>, max_visible_inbox_seq: i64) -> bool {
    hidden_at_seq.is_some_and(|hidden| max_visible_inbox_seq <= hidden)
}

pub fn sort_conversations(conversations: &mut [Conversation]) {
    conversations.sort_by_key(|chat| (!chat.pinned, Reverse(chat.activity_at), chat.id));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Api;
    use crate::client::local_sync::persist_message;
    use crate::{DeviceSession, Message, User};

    async fn open_test_client(
        data_dir: &std::path::Path,
        user_id: Uuid,
        device_id: Uuid,
        restore: bool,
    ) -> Result<std::sync::Arc<Client>> {
        let session = DeviceSession {
            user: User {
                id: user_id,
                handle: "tester".into(),
            },
            device_id,
            cursor: Uuid::new_v4(),
        };
        Client::open(data_dir, Api::new("http://127.0.0.1:7841")?, session, restore).await
    }

    fn make_conversation(id: Uuid, activity_at: i64, pinned: bool, own_id: Uuid, peer_id: Uuid) -> Conversation {
        Conversation {
            id,
            title: None,
            members: vec![
                User { id: own_id, handle: "tester".into() },
                User { id: peer_id, handle: "peer".into() },
            ],
            joined_after: None,
            activity_at,
            preview: None,
            preview_seq: None,
            latest_seq: 0,
            unread: 0,
            read_through: 0,
            pinned,
            manual_unread: false,
            muted: false,
        }
    }

    async fn seed_directory(client: &Client, conversations: Vec<Conversation>) -> Result<()> {
        let directory = serde_json::json!({
            "contacts": [],
            "conversations": conversations,
            "pending_contacts": [],
            "remarks": {},
            "names": {},
            "avatars": {},
            "read_receipts": false
        });
        sqlx::query(
            "INSERT INTO directory (id, payload) VALUES (1, ?)
             ON CONFLICT (id) DO UPDATE SET payload = excluded.payload",
        )
        .bind(directory.to_string())
        .execute(&client.db)
        .await?;
        Ok(())
    }

    fn sample_message(conversation_id: Uuid, seq: i64, sender: User, text: &str) -> Message {
        Message {
            seq,
            conversation_id,
            sender,
            created_at: 1_700_000_000_000 + seq * 1_000,
            text: Some(text.into()),
            attachment: None,
            ai_provider: None,
            recalled: false,
            reply_to: None,
            nonce: None,
            expires_at: None,
            expired: None,
            pending: None,
            local_file: false,
        }
    }

    #[test]
    fn sort_conversations_places_pinned_first_and_restores_on_unpin() {
        let own = Uuid::new_v4();
        let peer = Uuid::new_v4();
        let id_low = Uuid::from_u128(1);
        let id_mid = Uuid::from_u128(2);
        let id_high = Uuid::from_u128(3);
        let id_tie_a = Uuid::from_u128(4);
        let id_tie_b = Uuid::from_u128(5);

        let mut list = vec![
            make_conversation(id_high, 300, false, own, peer),
            make_conversation(id_low, 100, true, own, peer),
            make_conversation(id_tie_b, 200, false, own, peer),
            make_conversation(id_mid, 150, true, own, peer),
            make_conversation(id_tie_a, 200, false, own, peer),
        ];
        sort_conversations(&mut list);
        assert_eq!(
            list.iter().map(|c| c.id).collect::<Vec<_>>(),
            vec![id_mid, id_low, id_high, id_tie_a, id_tie_b]
        );

        // Unpinning returns the conversation to its chronological position.
        list.iter_mut().find(|c| c.id == id_low).unwrap().pinned = false;
        sort_conversations(&mut list);
        assert_eq!(
            list.iter().map(|c| c.id).collect::<Vec<_>>(),
            vec![id_mid, id_high, id_tie_a, id_tie_b, id_low]
        );
    }

    #[tokio::test]
    async fn manual_unread_does_not_alter_read_through_or_forge_unread_count() -> Result<()> {
        let data = tempfile::tempdir()?;
        let own = Uuid::new_v4();
        let peer = Uuid::new_v4();
        let conv = Uuid::new_v4();
        let client = open_test_client(data.path(), own, Uuid::new_v4(), false).await?;
        seed_directory(&client, vec![make_conversation(conv, 1000, false, own, peer)]).await?;

        let peer_user = User { id: peer, handle: "peer".into() };
        let mut tx = client.db.begin().await?;
        for seq in 1..=5 {
            persist_message(&mut tx, sample_message(conv, seq, peer_user.clone(), "hi")).await?;
        }
        tx.commit().await?;
        sqlx::query("INSERT INTO local_read_positions (conversation_id, read_through) VALUES (?, 5)")
            .bind(conv.to_string())
            .execute(&client.db)
            .await?;

        client.set_conversation_manual_unread(conv, true).await?;
        let prefs = load_prefs_map(&client.db).await?;
        assert!(prefs[&conv.to_string()].manual_unread);
        let read_through: i64 = sqlx::query_scalar(
            "SELECT read_through FROM local_read_positions WHERE conversation_id = ?",
        )
        .bind(conv.to_string())
        .fetch_one(&client.db)
        .await?;
        assert_eq!(read_through, 5);
        let dir = client.directory().await?;
        assert_eq!(dir.conversations[0].unread, 0);

        // New message arrives (seq 6) so local unread = 1; mark_conversation_read_local clears both
        // manual_unread and advances local read_through to 6 without network receipt.
        let mut tx = client.db.begin().await?;
        persist_message(&mut tx, sample_message(conv, 6, peer_user, "new")).await?;
        tx.commit().await?;
        client.mark_conversation_read_local(conv).await?;
        let prefs = load_prefs_map(&client.db).await?;
        assert!(!prefs[&conv.to_string()].manual_unread);
        let read_through: i64 = sqlx::query_scalar(
            "SELECT read_through FROM local_read_positions WHERE conversation_id = ?",
        )
        .bind(conv.to_string())
        .fetch_one(&client.db)
        .await?;
        assert_eq!(read_through, 6);
        assert_eq!(client.directory().await?.conversations[0].unread, 0);

        // Rejects unknown / M2 private / non-member conversation ids.
        let outsider_conv = Uuid::new_v4();
        let other_a = Uuid::new_v4();
        let other_b = Uuid::new_v4();
        seed_directory(
            &client,
            vec![
                make_conversation(conv, 1000, false, own, peer),
                make_conversation(outsider_conv, 1000, false, other_a, other_b),
            ],
        )
        .await?;
        assert!(client.set_conversation_manual_unread(Uuid::new_v4(), true).await.is_err());
        assert!(client.set_conversation_pinned(outsider_conv, true).await.is_err());
        assert!(client.set_conversation_muted(outsider_conv, true).await.is_err());
        assert!(client.hide_conversation(outsider_conv).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn hide_preserves_history_ignores_older_seq_and_unhides_on_new_seq() -> Result<()> {
        let data = tempfile::tempdir()?;
        let own = Uuid::new_v4();
        let peer = Uuid::new_v4();
        let conv = Uuid::new_v4();
        let client = open_test_client(data.path(), own, Uuid::new_v4(), false).await?;
        seed_directory(&client, vec![make_conversation(conv, 1000, false, own, peer)]).await?;

        let peer_user = User { id: peer, handle: "peer".into() };
        let mut tx = client.db.begin().await?;
        persist_message(&mut tx, sample_message(conv, 1, peer_user.clone(), "m1")).await?;
        persist_message(&mut tx, sample_message(conv, 3, peer_user.clone(), "m3")).await?;
        tx.commit().await?;

        client.hide_conversation(conv).await?;
        let prefs = load_prefs_map(&client.db).await?;
        let pref = &prefs[&conv.to_string()];
        assert_eq!(pref.hidden_at_seq, Some(3));
        // Messages remain intact in inbox.
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM inbox WHERE conversation_id = ?")
            .bind(conv.to_string())
            .fetch_one(&client.db)
            .await?;
        assert_eq!(count, 2);

        // Syncing an older seq (e.g. seq = 2) keeps max visible seq <= 3, so it stays hidden.
        let mut tx = client.db.begin().await?;
        persist_message(&mut tx, sample_message(conv, 2, peer_user.clone(), "m2")).await?;
        tx.commit().await?;
        let max_seq: i64 = sqlx::query_scalar(
            "SELECT coalesce(max(seq), 0) FROM inbox
             WHERE conversation_id = ? AND seq NOT IN (SELECT seq FROM hidden_messages)",
        )
        .bind(conv.to_string())
        .fetch_one(&client.db)
        .await?;
        assert!(is_conversation_hidden(pref.hidden_at_seq, max_seq));

        // A genuinely new message (seq = 4 > hidden_at_seq) makes the conversation visible again.
        let mut tx = client.db.begin().await?;
        persist_message(&mut tx, sample_message(conv, 4, peer_user, "m4")).await?;
        tx.commit().await?;
        let max_seq: i64 = sqlx::query_scalar(
            "SELECT coalesce(max(seq), 0) FROM inbox
             WHERE conversation_id = ? AND seq NOT IN (SELECT seq FROM hidden_messages)",
        )
        .bind(conv.to_string())
        .fetch_one(&client.db)
        .await?;
        assert!(!is_conversation_hidden(pref.hidden_at_seq, max_seq));

        // Explicit unhide clears hidden_at_seq.
        client.unhide_conversation(conv).await?;
        let prefs = load_prefs_map(&client.db).await?;
        assert_eq!(prefs[&conv.to_string()].hidden_at_seq, None);
        Ok(())
    }

    #[tokio::test]
    async fn prefs_persist_across_client_reopen() -> Result<()> {
        let data = tempfile::tempdir()?;
        let own = Uuid::new_v4();
        let peer = Uuid::new_v4();
        let conv = Uuid::new_v4();
        let device_id = Uuid::new_v4();
        let client = open_test_client(data.path(), own, device_id, false).await?;
        seed_directory(&client, vec![make_conversation(conv, 1000, false, own, peer)]).await?;

        client.set_conversation_pinned(conv, true).await?;
        client.set_conversation_manual_unread(conv, true).await?;
        client.set_conversation_muted(conv, true).await?;
        client.hide_conversation(conv).await?;
        client.close().await;
        drop(client);

        let reopened = open_test_client(data.path(), own, device_id, true).await?;
        let prefs = load_prefs_map(&reopened.db).await?;
        let pref = &prefs[&conv.to_string()];
        assert!(pref.pinned_at.is_some());
        assert!(pref.manual_unread);
        assert!(pref.muted);
        assert_eq!(pref.hidden_at_seq, Some(0));
        assert!(is_conversation_hidden(pref.hidden_at_seq, 0));

        reopened.set_conversation_muted(conv, false).await?;
        reopened.set_conversation_pinned(conv, false).await?;
        let prefs = load_prefs_map(&reopened.db).await?;
        let pref = &prefs[&conv.to_string()];
        assert!(pref.pinned_at.is_none());
        assert!(!pref.muted);
        Ok(())
    }

    #[tokio::test]
    async fn hidden_group_can_be_listed_and_reopened_after_restart_with_prefs_kept() -> Result<()> {
        let data = tempfile::tempdir()?;
        let own = Uuid::new_v4();
        let peer = Uuid::new_v4();
        let group = Uuid::new_v4();
        let direct = Uuid::new_v4();
        let device_id = Uuid::new_v4();
        let client = open_test_client(data.path(), own, device_id, false).await?;
        let mut team = make_conversation(group, 2000, false, own, peer);
        team.title = Some("Team".into());
        seed_directory(&client, vec![team, make_conversation(direct, 1000, false, own, peer)]).await?;

        client.set_conversation_pinned(group, true).await?;
        client.hide_conversation(group).await?;
        client.hide_conversation(direct).await?;
        client.close().await;
        drop(client);

        let reopened = open_test_client(data.path(), own, device_id, true).await?;
        assert!(!reopened.directory().await?.conversations.iter().any(|chat| chat.id == group));
        let hidden = reopened.hidden_group_conversations().await?;
        assert_eq!(hidden.iter().map(|chat| chat.id).collect::<Vec<_>>(), vec![group]);

        reopened.unhide_conversation(group).await?;
        assert!(reopened.hidden_group_conversations().await?.is_empty());
        let reopened_chat = reopened
            .directory()
            .await?
            .conversations
            .into_iter()
            .find(|chat| chat.id == group)
            .expect("group is open again");
        assert!(reopened_chat.pinned);
        assert_eq!(reopened_chat.members.len(), 2);
        Ok(())
    }
}
