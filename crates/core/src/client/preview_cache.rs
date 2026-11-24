use super::{MediaDescriptor, MediaVariant, media_store, *};
use sha2::{Digest, Sha256};

pub const IMAGE_THUMBNAIL: i64 = -2;

impl Client {
    pub async fn cached_thumbnail_sizes(
        &self,
        messages: &[Message],
    ) -> Result<HashMap<i64, (u32, u32)>> {
        self.ensure_open()?;
        let keys: HashMap<_, _> = messages
            .iter()
            .filter_map(|message| {
                let file = message.attachment.as_ref()?;
                file.preview_mime()?
                    .starts_with("image/")
                    .then_some((message.seq, &file.sha256))
            })
            .collect();
        if keys.is_empty() {
            return Ok(HashMap::new());
        }
        let rows: Vec<(i64, i64, i64)> = sqlx::query_as(
            "SELECT e.seq, f.width, f.height FROM json_each(?1) k
             JOIN preview_eligible e
               ON e.seq = CAST(k.key AS INTEGER) AND e.sha256 = k.value
             JOIN media_files f
               ON f.device_id = ?2
              AND f.conversation_id = e.conversation_id
              AND f.variant = 'image_thumbnail'
              AND f.content_key = e.sha256
              AND f.status = 'available'
             WHERE f.width > 0 AND f.height > 0",
        )
        .bind(serde_json::to_string(&keys)?)
        .bind(self.device_id.to_string())
        .fetch_all(&self.db)
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(seq, w, h)| {
                let width = u32::try_from(w).ok()?;
                let height = u32::try_from(h).ok()?;
                (width != 0 && height != 0).then_some((seq, (width, height)))
            })
            .collect())
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

    pub async fn cached_image_variant(
        &self,
        conversation_id: Uuid,
        seq: i64,
        attachment_id: Option<Uuid>,
        source_sha256: &str,
        thumbnail: bool,
    ) -> Result<Option<(String, Vec<u8>)>> {
        let variant = if thumbnail {
            MediaVariant::ImageThumbnail
        } else {
            MediaVariant::ImageOriginal
        };
        self.read_media_bytes(conversation_id, seq, attachment_id, variant, source_sha256)
            .await
    }

    pub async fn remember_image_variant(
        &self,
        conversation_id: Uuid,
        seq: i64,
        attachment_id: Option<Uuid>,
        source_sha256: &str,
        thumbnail: bool,
        mime: &str,
        bytes: &[u8],
    ) -> Result<()> {
        self.ensure_open()?;
        if bytes.is_empty() || bytes.len() > MAX_PASTED_IMAGE_BYTES {
            return Ok(());
        }
        if !self
            .ensure_message_authorized_for_cache(conversation_id, seq)
            .await?
        {
            return Ok(());
        }
        if thumbnail {
            let thumb_sha = format!("{:x}", Sha256::digest(bytes));
            let desc = MediaDescriptor {
                conversation_id,
                message_seq: seq,
                attachment_id,
                variant: MediaVariant::ImageThumbnail,
                content_key: source_sha256,
                expected_sha256: &thumb_sha,
                expected_size: bytes.len() as u64,
                mime: "image/png",
                dimensions: media_store::png_dimensions(bytes),
            };
            self.commit_media_bytes(&desc, bytes).await?;
        } else {
            let desc = MediaDescriptor {
                conversation_id,
                message_seq: seq,
                attachment_id,
                variant: MediaVariant::ImageOriginal,
                content_key: source_sha256,
                expected_sha256: source_sha256,
                expected_size: bytes.len() as u64,
                mime,
                dimensions: None,
            };
            self.commit_media_bytes(&desc, bytes).await?;
        }
        Ok(())
    }

    pub async fn remember_single_chunk_video(
        &self,
        conversation_id: Uuid,
        seq: i64,
        attachment: &Attachment,
        mime: &str,
        bytes: &[u8],
    ) -> Result<()> {
        self.ensure_open()?;
        if bytes.len() as u64 != attachment.size {
            return Ok(());
        }
        if !self
            .ensure_message_authorized_for_cache(conversation_id, seq)
            .await?
        {
            return Ok(());
        }
        let desc = MediaDescriptor {
            conversation_id,
            message_seq: seq,
            attachment_id: Some(attachment.id),
            variant: MediaVariant::VideoOriginal,
            content_key: &attachment.sha256,
            expected_sha256: &attachment.sha256,
            expected_size: attachment.size,
            mime,
            dimensions: None,
        };
        self.commit_media_bytes(&desc, bytes).await?;
        Ok(())
    }
}
