use super::*;
use sqlx::SqliteConnection;

const MAX_PREVIEW_BYTES: i64 = 64 * 1024 * 1024;
const MAX_PREVIEW_ENTRIES: i64 = 256;
pub const IMAGE_THUMBNAIL: i64 = -2;

impl Client {
    pub async fn cached_thumbnail_sizes(&self, messages: &[Message]) -> Result<HashMap<i64, (u32, u32)>> {
        let keys: HashMap<_, _> = messages.iter().filter_map(|message| {
            let file = message.attachment.as_ref()?;
            file.preview_mime()?.starts_with("image/").then_some((message.seq, &file.sha256))
        }).collect();
        if keys.is_empty() {
            return Ok(HashMap::new());
        }
        let headers: Vec<(i64, Vec<u8>)> = sqlx::query_as(
            "SELECT p.message_seq, substr(p.bytes, 1, 24) FROM json_each(?1) k
             JOIN media_previews p ON p.message_seq = CAST(k.key AS INTEGER) AND p.sha256 = k.value
             JOIN preview_eligible e ON e.seq = p.message_seq AND e.sha256 = p.sha256
             WHERE p.variant = ?2 AND (e.expires_at IS NULL OR e.expires_at > ?3)",
        )
        .bind(serde_json::to_string(&keys)?)
        .bind(IMAGE_THUMBNAIL)
        .bind(self.server_now())
        .fetch_all(&self.db)
        .await?;
        Ok(headers.into_iter().filter_map(|(seq, header)| {
            if !header.starts_with(b"\x89PNG\r\n\x1a\n") || header.get(12..16)? != b"IHDR" {
                return None;
            }
            let width = u32::from_be_bytes(header.get(16..20)?.try_into().ok()?);
            let height = u32::from_be_bytes(header.get(20..24)?.try_into().ok()?);
            (width != 0 && height != 0).then_some((seq, (width, height)))
        }).collect())
    }

    pub async fn preview_attachment(&self, id: Uuid) -> Result<Message> {
        self.ensure_open()?;
        let payload: String = sqlx::query_scalar(
            "SELECT t.payload FROM timeline t JOIN directory d ON d.id = 1
             JOIN json_each(d.payload, '$.conversations') c
               ON json_extract(c.value, '$.id') = t.conversation_id
              AND (json_extract(c.value, '$.joined_after') IS NULL
                   OR t.seq > json_extract(c.value, '$.joined_after'))
             WHERE t.attachment_id = ?1 AND t.seq NOT IN (SELECT seq FROM hidden_messages)
               AND json_extract(t.payload, '$.recalled') IS NOT 1
               AND (json_extract(t.payload, '$.local_file') = 1 OR
                    (json_extract(t.payload, '$.expired') IS NULL
                     AND coalesce(json_extract(t.payload, '$.expires_at'), ?2 + 1) > ?2))",
        )
        .bind(id.to_string())
        .bind(self.server_now())
        .fetch_optional(&self.db)
        .await?
        .context("This attachment is unavailable")?;
        Ok(serde_json::from_str(&payload)?)
    }

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
               AND EXISTS (SELECT 1 FROM preview_eligible WHERE seq = ?2 AND sha256 = ?3
                           AND (expires_at IS NULL OR expires_at > ?1))
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
             WHERE seq = ?1 AND sha256 = ?2 AND (expires_at IS NULL OR expires_at > ?6)
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
                 WHERE seq = p.message_seq AND sha256 = p.sha256
                   AND (expires_at IS NULL OR expires_at > ?)
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
        let attachment_id = Uuid::new_v4();
        let message: Message = serde_json::from_value(serde_json::json!({
            "seq": 1, "conversation_id": conversation, "sender": client.user, "created_at": 0, "text": null,
            "attachment": {"id": attachment_id, "name": "fixture.png", "kind": "file", "size": 24, "sha256": "fixture"},
            "expires_at": i64::MAX,
        }))?;
        sqlx::query(
            "INSERT INTO inbox (seq, conversation_id, attachment_id, payload) VALUES (1, ?, ?, ?)",
        )
        .bind(conversation.to_string())
        .bind(attachment_id.to_string())
        .bind(serde_json::to_string(&message)?)
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
        let png_header = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR\x00\x00\x00\x78\x00\x00\x00\x50";
        client.remember_preview(1, "fixture", IMAGE_THUMBNAIL, "image/png", png_header).await?;
        client.close().await;
        drop(client);

        let client = Client::open(root.path(), service()?, session(id), true).await?;
        assert_eq!(client.cached_thumbnail_sizes(std::slice::from_ref(&message)).await?.get(&1), Some(&(120, 80)));
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
        assert!(client.cached_thumbnail_sizes(std::slice::from_ref(&message)).await?.is_empty());
        sqlx::query("UPDATE inbox SET payload = json_remove(payload, '$.expires_at')")
            .execute(&client.db)
            .await?;
        assert!(client.cached_preview(1, "fixture", 5).await?.is_some());
        for cutoff in [None, Some(1)] {
            sqlx::query("UPDATE directory SET payload = ?")
                .bind(directory(cutoff))
                .execute(&client.db)
                .await?;
            assert!(client.cached_preview(1, "fixture", 5).await?.is_none());
            assert!(client.cached_thumbnail_sizes(std::slice::from_ref(&message)).await?.is_empty());
            assert_eq!(client.preview_attachment(attachment_id).await.is_ok(), cutoff.is_none());
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
