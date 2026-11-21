use super::*;
use sqlx::SqliteConnection;

const MAX_PREVIEW_BYTES: i64 = 64 * 1024 * 1024;
const MAX_PREVIEW_ENTRIES: i64 = 256;

impl Client {
    pub async fn cached_preview(
        &self,
        seq: i64,
        sha256: &str,
        variant: i64,
    ) -> Result<Option<(String, Vec<u8>)>> {
        self.ensure_open()?;
        Ok(sqlx::query_as(
            "UPDATE media_previews SET touched = ?1
             WHERE message_seq = ?2 AND sha256 = ?3 AND variant = ?4
               AND EXISTS (SELECT 1 FROM preview_eligible WHERE seq = ?2 AND sha256 = ?3 AND expires_at > ?1)
             RETURNING mime, bytes",
        )
        .bind(self.server_now())
        .bind(seq)
        .bind(sha256)
        .bind(variant)
        .fetch_optional(&self.db)
        .await?)
    }

    pub async fn remember_preview(
        &self,
        seq: i64,
        sha256: &str,
        variant: i64,
        mime: &str,
        bytes: &[u8],
    ) -> Result<()> {
        self.ensure_open()?;
        let now = self.server_now();
        if bytes.is_empty() || bytes.len() > MAX_PASTED_IMAGE_BYTES {
            return Ok(());
        }
        let mut tx = self.db.begin().await?;
        sqlx::query(
            "INSERT INTO media_previews (message_seq, sha256, variant, mime, bytes, touched)
             SELECT ?1, ?2, ?3, ?4, ?5, ?6 FROM preview_eligible
             WHERE seq = ?1 AND sha256 = ?2 AND expires_at > ?6
             ON CONFLICT (message_seq, sha256, variant) DO UPDATE SET touched = excluded.touched",
        )
        .bind(seq)
        .bind(sha256)
        .bind(variant)
        .bind(mime)
        .bind(bytes)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        self.prune_media_previews(&mut tx).await?;
        sqlx::query(
            "DELETE FROM media_previews WHERE rowid IN (
                 SELECT entry FROM (
                     SELECT rowid AS entry, row_number() OVER recent AS count,
                         sum(length(bytes)) OVER recent AS budget
                     FROM media_previews WINDOW recent AS (ORDER BY touched DESC, rowid DESC)
                 ) WHERE count > ? OR budget > ?
             )",
        )
        .bind(MAX_PREVIEW_ENTRIES)
        .bind(MAX_PREVIEW_BYTES)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub(super) async fn prune_media_previews(&self, db: &mut SqliteConnection) -> Result<()> {
        sqlx::query(
            "DELETE FROM media_previews AS p WHERE NOT EXISTS (
                 SELECT 1 FROM preview_eligible
                 WHERE seq = p.message_seq AND sha256 = p.sha256 AND expires_at > ?
             )",
        )
        .bind(self.server_now())
        .execute(db)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn previews_survive_reopening_and_respect_scope_expiry_and_budget() -> Result<()> {
        let root = tempfile::tempdir()?;
        let id = Uuid::new_v4();
        let conversation = Uuid::new_v4();
        let device = Uuid::new_v4();
        let session = |account| DeviceSession {
            user: User {
                id: account,
                handle: "preview_fixture".into(),
            },
            device_id: device,
            cursor: Uuid::new_v4(),
        };
        let service = || Api::new("http://127.0.0.1:7841");
        let client = Client::open(root.path(), service()?, session(id), false).await?;
        let directory = |cutoff| {
            serde_json::json!({
                "conversations": [{"id": conversation, "joined_after": cutoff}]
            })
            .to_string()
        };
        sqlx::query("INSERT INTO directory VALUES (1, ?)")
            .bind(directory(Some(0)))
            .execute(&client.db)
            .await?;
        sqlx::query(
            "INSERT INTO inbox (seq, conversation_id, attachment_id, payload) VALUES (1, ?, ?, ?)",
        )
        .bind(conversation.to_string())
        .bind(Uuid::new_v4().to_string())
        .bind(
            serde_json::json!({"attachment":{"sha256":"fixture"}, "expires_at":i64::MAX})
                .to_string(),
        )
        .execute(&client.db)
        .await?;
        let bytes = vec![1; MAX_PASTED_IMAGE_BYTES];
        for variant in 0..5 {
            client
                .remember_preview(1, "fixture", variant, "video/mp4", &bytes)
                .await?;
        }
        assert!(client.cached_preview(1, "fixture", 0).await?.is_none());
        client
            .remember_preview(1, "fixture", 5, "image/png", &[2, 3])
            .await?;
        client.close().await;
        drop(client);

        let client = Client::open(root.path(), service()?, session(id), true).await?;
        assert_eq!(
            client
                .cached_preview(1, "fixture", 5)
                .await?
                .map(|(_, bytes)| bytes),
            Some(vec![2, 3])
        );
        let other = Client::open(root.path(), service()?, session(Uuid::new_v4()), false).await?;
        let other_server = Client::open(
            root.path(),
            Api::new("http://127.0.0.1:7842")?,
            session(id),
            false,
        )
        .await?;
        assert!(other.cached_preview(1, "fixture", 5).await?.is_none());
        assert!(
            other_server
                .cached_preview(1, "fixture", 5)
                .await?
                .is_none()
        );
        assert!(client.cached_preview(1, "wrong digest", 5).await?.is_none());

        sqlx::query("UPDATE inbox SET payload = json_set(payload, '$.expires_at', 0)")
            .execute(&client.db)
            .await?;
        assert!(client.cached_preview(1, "fixture", 5).await?.is_none());
        sqlx::query("UPDATE inbox SET payload = json_set(payload, '$.expires_at', ?)")
            .bind(i64::MAX)
            .execute(&client.db)
            .await?;
        for cutoff in [None, Some(1)] {
            sqlx::query("UPDATE directory SET payload = ?")
                .bind(directory(cutoff))
                .execute(&client.db)
                .await?;
            assert!(client.cached_preview(1, "fixture", 5).await?.is_none());
        }
        let mut tx = client.db.begin().await?;
        client.prune_media_previews(&mut tx).await?;
        tx.commit().await?;
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM media_previews")
            .fetch_one(&client.db)
            .await?;
        assert_eq!(count, 0);
        Ok(())
    }
}
