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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiLocalSource {
    pub number: usize,
    pub conversation_id: Uuid,
    pub seq: i64,
    pub attachment_id: Option<Uuid>,
    pub label: String,
    pub created_at: i64,
}

impl AiLocalSource {
    pub fn reference(&self) -> crate::AiSourceRef {
        crate::AiSourceRef { number: self.number, seq: self.seq, attachment_id: self.attachment_id }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiLocalReply {
    pub id: Uuid,
    pub prompt_id: Uuid,
    pub conversation_id: Uuid,
    pub provider: AiProvider,
    pub text: String,
    pub created_at: i64,
    #[serde(default)]
    pub sources: Vec<AiLocalSource>,
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
    pub async fn list_ai_replies(&self, conversation: Uuid) -> Result<Vec<AiLocalReply>> {
        self.ensure_open()?;
        let mut tx = self.db.begin().await?;
        super::ai_context::ensure_normal_chat(&mut tx, self.user.id, conversation).await?;
        list_ai_replies_in_tx(&mut tx, Some(conversation), None).await
    }

    pub async fn ai_reply(&self, id: Uuid) -> Result<Option<AiLocalReply>> {
        self.ensure_open()?;
        let mut tx = self.db.begin().await?;
        let reply = list_ai_replies_in_tx(&mut tx, None, Some(id)).await?.pop();
        if let Some(reply) = &reply {
            super::ai_context::ensure_normal_chat(&mut tx, self.user.id, reply.conversation_id).await?;
        }
        Ok(reply)
    }

    pub async fn insert_ai_reply(&self, reply: &AiLocalReply) -> Result<bool> {
        self.ensure_open()?;
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        super::ai_context::ensure_normal_chat(&mut tx, self.user.id, reply.conversation_id).await?;
        let inserted = insert_ai_reply_in_tx(&mut tx, reply).await?;
        tx.commit().await?;
        Ok(inserted)
    }

    pub async fn dismiss_ai_reply(&self, id: Uuid) -> Result<Option<Uuid>> {
        let Some(reply) = self.ai_reply(id).await? else { return Ok(None); };
        sqlx::query("DELETE FROM ai_local_replies WHERE id = ?")
            .bind(id.to_string()).execute(&self.db).await?;
        Ok(Some(reply.conversation_id))
    }

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

async fn list_ai_replies_in_tx(
    db: &mut SqliteConnection, conversation: Option<Uuid>, id: Option<Uuid>,
) -> Result<Vec<AiLocalReply>> {
    let rows = sqlx::query(
        "SELECT r.id, r.prompt_id, p.conversation_id, p.provider, r.text, r.sources, r.created_at
         FROM ai_local_replies r JOIN ai_local_prompts p ON p.id = r.prompt_id
         WHERE (?1 IS NULL OR p.conversation_id = ?1) AND (?2 IS NULL OR r.id = ?2)
         ORDER BY r.created_at, r.id",
    )
    .bind(conversation.map(|id| id.to_string()))
    .bind(id.map(|id| id.to_string()))
    .fetch_all(db).await?;
    rows.into_iter().map(|row| {
        Ok(AiLocalReply {
            id: Uuid::parse_str(row.try_get("id")?)?,
            prompt_id: Uuid::parse_str(row.try_get("prompt_id")?)?,
            conversation_id: Uuid::parse_str(row.try_get("conversation_id")?)?,
            provider: parse_ai_provider(row.try_get("provider")?)?,
            text: row.try_get("text")?,
            sources: serde_json::from_str(row.try_get("sources")?)?,
            created_at: row.try_get("created_at")?,
        })
    }).collect()
}

async fn insert_ai_reply_in_tx(db: &mut SqliteConnection, reply: &AiLocalReply) -> Result<bool> {
    let result = sqlx::query(
        "INSERT OR IGNORE INTO ai_local_replies (id, prompt_id, text, sources, created_at)
         SELECT ?1, id, ?2, ?3, ?4 FROM ai_local_prompts
         WHERE id = ?5 AND conversation_id = ?6 AND provider = ?7",
    )
    .bind(reply.id.to_string())
    .bind(&reply.text)
    .bind(serde_json::to_string(&reply.sources)?)
    .bind(reply.created_at)
    .bind(reply.prompt_id.to_string())
    .bind(reply.conversation_id.to_string())
    .bind(reply.provider.as_str())
    .execute(db).await?;
    Ok(result.rows_affected() != 0)
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
        use sqlx::{Connection, sqlite::SqliteConnectOptions};
        let profile = tempfile::tempdir()?;
        let options = SqliteConnectOptions::new().filename(profile.path().join("chat.sqlite3"))
            .create_if_missing(true);
        let mut db = SqliteConnection::connect_with(&options).await?;
        sqlx::raw_sql(
            "CREATE TABLE directory (id INTEGER PRIMARY KEY, payload TEXT NOT NULL);
             CREATE TABLE ai_local_prompts (
                id TEXT PRIMARY KEY,
                conversation_id TEXT NOT NULL,
                provider TEXT NOT NULL,
                prompt_text TEXT NOT NULL,
                created_at INTEGER NOT NULL
             );
             CREATE TABLE ai_local_replies (
                id TEXT PRIMARY KEY,
                prompt_id TEXT NOT NULL UNIQUE REFERENCES ai_local_prompts(id) ON DELETE CASCADE,
                text TEXT NOT NULL,
                sources TEXT NOT NULL,
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

        let reply = AiLocalReply {
            id: Uuid::new_v4(),
            prompt_id: p2.id,
            conversation_id: chat,
            provider: p2.provider,
            text: "本机回复".into(),
            created_at: 201,
            sources: vec![AiLocalSource {
                number: 1, conversation_id: chat, seq: 5, attachment_id: None,
                label: "Alice".into(), created_at: 100,
            }],
        };
        assert!(insert_ai_reply_in_tx(&mut db, &reply).await?);
        assert!(!insert_ai_reply_in_tx(&mut db, &reply).await?);
        db.close().await?;
        let mut db = SqliteConnection::connect_with(&options).await?;
        assert_eq!(list_ai_prompts_in_tx(&mut db, chat).await?, vec![p1_a, p1_b, p2]);
        assert_eq!(list_ai_replies_in_tx(&mut db, Some(chat), None).await?, vec![reply.clone()]);
        assert!(list_ai_replies_in_tx(&mut db, Some(other), None).await?.is_empty());

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
        assert!(!insert_ai_reply_in_tx(&mut db, &reply).await?);
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM ai_local_replies")
            .fetch_one(&mut db).await?, 0);
        Ok(())
    }
}
