use super::Client;
use crate::{AiProvider, MAX_AI_PROMPT_BYTES};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqliteConnection};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiLocalPrompt {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub provider: AiProvider,
    pub prompt_text: String,
    pub created_at: i64,
}

fn parse_ai_provider(value: &str) -> Result<AiProvider> {
    match value {
        "open_ai" => Ok(AiProvider::OpenAi),
        "deep_seek" => Ok(AiProvider::DeepSeek),
        "grok" => Ok(AiProvider::Grok),
        _ => bail!("Unsupported AI provider: {value}"),
    }
}

impl Client {
    pub async fn list_ai_prompts(&self, conversation: Uuid) -> Result<Vec<AiLocalPrompt>> {
        self.ensure_open()?;
        let mut tx = self.db.begin().await?;
        super::ai_context::ensure_normal_chat(&mut tx, self.user.id, conversation).await?;
        let rows = list_ai_prompts_in_tx(&mut tx, conversation).await?;
        tx.commit().await?;
        Ok(rows)
    }

    pub async fn insert_ai_prompt(&self, prompt: &AiLocalPrompt) -> Result<()> {
        self.ensure_open()?;
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        super::ai_context::ensure_normal_chat(&mut tx, self.user.id, prompt.conversation_id)
            .await?;
        insert_ai_prompt_in_tx(&mut tx, prompt).await?;
        tx.commit().await?;
        Ok(())
    }
}

async fn list_ai_prompts_in_tx(
    db: &mut SqliteConnection,
    conversation: Uuid,
) -> Result<Vec<AiLocalPrompt>> {
    let rows = sqlx::query(
        "SELECT id, conversation_id, provider, prompt_text, created_at
         FROM ai_local_prompts
         WHERE conversation_id = ?1
         ORDER BY created_at ASC, id ASC",
    )
    .bind(conversation.to_string())
    .fetch_all(db)
    .await?;

    rows.into_iter()
        .map(|row| {
            let id: String = row.try_get("id")?;
            let conversation_id: String = row.try_get("conversation_id")?;
            let provider: String = row.try_get("provider")?;
            let prompt_text: String = row.try_get("prompt_text")?;
            let created_at: i64 = row.try_get("created_at")?;
            Ok(AiLocalPrompt {
                id: Uuid::parse_str(&id).context("Invalid prompt id")?,
                conversation_id: Uuid::parse_str(&conversation_id)
                    .context("Invalid conversation id")?,
                provider: parse_ai_provider(&provider)?,
                prompt_text,
                created_at,
            })
        })
        .collect()
}

async fn insert_ai_prompt_in_tx(
    db: &mut SqliteConnection,
    prompt: &AiLocalPrompt,
) -> Result<()> {
    let trimmed = prompt.prompt_text.trim();
    ensure!(
        !trimmed.is_empty() && trimmed.len() <= MAX_AI_PROMPT_BYTES,
        "Enter a prompt of up to 8 KiB"
    );
    sqlx::query(
        "INSERT OR IGNORE INTO ai_local_prompts (id, conversation_id, provider, prompt_text, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(prompt.id.to_string())
    .bind(prompt.conversation_id.to_string())
    .bind(prompt.provider.as_str())
    .bind(prompt.prompt_text.as_str())
    .bind(prompt.created_at)
    .execute(db)
    .await?;
    Ok(())
}

pub(super) async fn delete_ai_prompts_in_tx(
    db: &mut SqliteConnection,
    conversation: Uuid,
) -> Result<()> {
    sqlx::query("DELETE FROM ai_local_prompts WHERE conversation_id = ?")
        .bind(conversation.to_string())
        .execute(db)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::User;

    #[tokio::test]
    async fn ai_prompts_persist_locally_with_membership_check_and_cleanup() -> Result<()> {
        use sqlx::Connection;
        let mut db = SqliteConnection::connect("sqlite::memory:").await?;
        sqlx::raw_sql(
            "CREATE TABLE directory (id INTEGER PRIMARY KEY, payload TEXT NOT NULL);
             CREATE TABLE ai_local_prompts (
                id TEXT PRIMARY KEY,
                conversation_id TEXT NOT NULL,
                provider TEXT NOT NULL,
                prompt_text TEXT NOT NULL,
                created_at INTEGER NOT NULL
             );
             CREATE TABLE inbox (seq INTEGER PRIMARY KEY, conversation_id TEXT, payload TEXT);
             CREATE TABLE outbox (id INTEGER PRIMARY KEY AUTOINCREMENT, nonce TEXT, conversation_id TEXT, work TEXT, draft TEXT);",
        )
        .execute(&mut db)
        .await?;

        let user = User {
            id: Uuid::new_v4(),
            handle: "alice".into(),
        };
        let chat = Uuid::new_v4();
        let other = Uuid::new_v4();
        let directory = serde_json::json!({
            "conversations": [
                { "id": chat, "members": [&user] },
                { "id": other, "members": [{ "id": Uuid::new_v4(), "handle": "bob" }] }
            ]
        });
        sqlx::query("INSERT INTO directory VALUES (1, ?)")
            .bind(directory.to_string())
            .execute(&mut db)
            .await?;

        let p2 = AiLocalPrompt {
            id: Uuid::parse_str("00000000-0000-0000-0000-000000000002")?,
            conversation_id: chat,
            provider: AiProvider::OpenAi,
            prompt_text: "Second prompt".into(),
            created_at: 200,
        };
        let p1_b = AiLocalPrompt {
            id: Uuid::parse_str("00000000-0000-0000-0000-00000000000b")?,
            conversation_id: chat,
            provider: AiProvider::DeepSeek,
            prompt_text: "Tie B".into(),
            created_at: 100,
        };
        let p1_a = AiLocalPrompt {
            id: Uuid::parse_str("00000000-0000-0000-0000-00000000000a")?,
            conversation_id: chat,
            provider: AiProvider::Grok,
            prompt_text: "Tie A".into(),
            created_at: 100,
        };

        for p in [&p2, &p1_b, &p1_a, &p1_a] {
            super::super::ai_context::ensure_normal_chat(&mut db, user.id, p.conversation_id)
                .await?;
            insert_ai_prompt_in_tx(&mut db, p).await?;
        }

        // Non-member chat must be rejected.
        assert!(
            super::super::ai_context::ensure_normal_chat(&mut db, user.id, other)
                .await
                .is_err()
        );

        let listed = list_ai_prompts_in_tx(&mut db, chat).await?;
        assert_eq!(listed, vec![p1_a, p1_b, p2]);

        let inbox_count: i64 = sqlx::query_scalar("SELECT count(*) FROM inbox")
            .fetch_one(&mut db)
            .await?;
        let outbox_count: i64 = sqlx::query_scalar("SELECT count(*) FROM outbox")
            .fetch_one(&mut db)
            .await?;
        assert_eq!(inbox_count, 0);
        assert_eq!(outbox_count, 0);

        delete_ai_prompts_in_tx(&mut db, chat).await?;
        assert!(list_ai_prompts_in_tx(&mut db, chat).await?.is_empty());
        Ok(())
    }
}
