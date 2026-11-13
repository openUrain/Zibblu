use super::*;
use crate::assistant::AiMessage;
use sqlx::SqliteConnection;

const MAX_MESSAGES: usize = 60;

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AiScope {
    Prompt,
    Selected { sequences: Vec<i64> },
    Recent { limit: usize },
}

impl Client {
    pub fn profile_dir(&self) -> &Path {
        self.staging.parent().expect("profile staging directory")
    }

    pub async fn ai_messages(&self, conversation: Uuid, scope: AiScope) -> Result<Vec<AiMessage>> {
        let mut tx = self.db.begin().await?;
        ensure_normal_chat(&mut tx, self.user.id, conversation).await?;
        let messages = scoped_messages(&mut tx, conversation, scope).await?;
        tx.commit().await?;
        Ok(messages)
    }
}

async fn ensure_normal_chat(
    db: &mut SqliteConnection,
    user: Uuid,
    conversation: Uuid,
) -> Result<()> {
    let member: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM directory,
                json_each(directory.payload, '$.conversations') AS chat,
                json_each(chat.value, '$.members') AS member
            WHERE directory.id = 1 AND json_extract(chat.value, '$.id') = ?
                AND json_extract(member.value, '$.id') = ?
        )",
    )
    .bind(conversation.to_string())
    .bind(user.to_string())
    .fetch_one(db)
    .await?;
    ensure!(member, "AI is only available in your normal chats");
    Ok(())
}

async fn scoped_messages(
    db: &mut SqliteConnection,
    conversation: Uuid,
    scope: AiScope,
) -> Result<Vec<AiMessage>> {
    let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
        "SELECT payload FROM inbox WHERE json_type(payload, '$.text') = 'text' AND attachment_id IS NULL
         AND seq NOT IN (SELECT seq FROM hidden_messages) AND conversation_id = ",
    );
    query.push_bind(conversation.to_string());
    let expected = match scope {
        AiScope::Prompt => return Ok(Vec::new()),
        AiScope::Selected { sequences } => {
            let count = sequences.len();
            ensure!(
                (1..=MAX_MESSAGES).contains(&count),
                "Select between 1 and 60 messages"
            );
            query.push(" AND seq IN (");
            let mut ids = query.separated(", ");
            for sequence in sequences {
                ids.push_bind(sequence);
            }
            ids.push_unseparated(") ORDER BY json_extract(payload, '$.created_at'), seq");
            Some(count)
        }
        AiScope::Recent { limit } => {
            ensure!(
                (1..=MAX_MESSAGES).contains(&limit),
                "Choose up to 60 recent messages"
            );
            query
                .push(" ORDER BY json_extract(payload, '$.created_at') DESC, seq DESC LIMIT ")
                .push_bind(limit as i64);
            None
        }
    };
    let mut rows: Vec<String> = query.build_query_scalar().fetch_all(db).await?;
    if let Some(count) = expected {
        ensure!(
            rows.len() == count,
            "Some selected messages are unavailable in this chat"
        );
    } else {
        rows.reverse();
    }
    rows.into_iter()
        .map(|row| {
            let message: Message =
                serde_json::from_str(&row).context("Could not read a selected message")?;
            Ok(AiMessage {
                sender: message.sender.handle,
                text: message
                    .text
                    .context("The selected message contains no text")?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ai_scope_is_bounded_to_normal_chat_and_exact_selection() -> Result<()> {
        use sqlx::Connection;
        let mut db = SqliteConnection::connect("sqlite::memory:").await?;
        sqlx::raw_sql(
            "CREATE TABLE inbox (seq INTEGER PRIMARY KEY, conversation_id TEXT, payload TEXT, attachment_id TEXT);
             CREATE TABLE hidden_messages (seq INTEGER PRIMARY KEY);
             CREATE TABLE directory (id INTEGER PRIMARY KEY, payload TEXT NOT NULL);",
        )
        .execute(&mut db)
        .await?;
        let chat = Uuid::new_v4();
        let other = Uuid::new_v4();
        let private = Uuid::new_v4();
        let user = User {
            id: Uuid::new_v4(),
            handle: "alice".into(),
        };
        let directory = serde_json::json!({
            "conversations": [
                { "id": chat, "members": [&user] },
                { "id": other, "members": [{ "id": Uuid::new_v4() }] },
            ],
        });
        sqlx::query("INSERT INTO directory VALUES (1, ?)")
            .bind(directory.to_string())
            .execute(&mut db)
            .await?;
        ensure_normal_chat(&mut db, user.id, chat).await?;
        assert!(ensure_normal_chat(&mut db, user.id, other).await.is_err());
        assert!(ensure_normal_chat(&mut db, user.id, private).await.is_err());
        for (seq, conversation, text) in [
            (1, chat, "unselected"),
            (2, chat, "selected"),
            (3, other, "other chat"),
            (4, chat, "latest"),
            (5, chat, "attachment"),
        ] {
            let payload = serde_json::to_string(&Message {
                seq,
                conversation_id: conversation,
                sender: user.clone(),
                created_at: seq,
                text: Some(text.into()),
                attachment: None,
                ai_provider: None,
                recalled: false,
                reply_to: None,
            })?;
            sqlx::query("INSERT INTO inbox VALUES (?, ?, ?, ?)")
                .bind(seq)
                .bind(conversation.to_string())
                .bind(payload)
                .bind((seq == 5).then_some("attachment"))
                .execute(&mut db)
                .await?;
        }
        let chosen =
            scoped_messages(&mut db, chat, AiScope::Selected { sequences: vec![2] }).await?;
        assert_eq!(
            chosen
                .iter()
                .map(|row| row.text.as_str())
                .collect::<Vec<_>>(),
            ["selected"]
        );
        assert!(
            scoped_messages(
                &mut db,
                chat,
                AiScope::Selected {
                    sequences: vec![2, 3]
                }
            )
            .await
            .is_err()
        );
        let recent = scoped_messages(&mut db, chat, AiScope::Recent { limit: 2 }).await?;
        assert_eq!(
            recent
                .iter()
                .map(|row| row.text.as_str())
                .collect::<Vec<_>>(),
            ["selected", "latest"]
        );
        assert!(
            scoped_messages(&mut db, chat, AiScope::Prompt)
                .await?
                .is_empty()
        );
        assert!(
            scoped_messages(&mut db, chat, AiScope::Recent { limit: 61 })
                .await
                .is_err()
        );
        Ok(())
    }
}
