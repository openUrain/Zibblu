use super::{MediaDescriptor, MediaVariant, media_store, *};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::io::{Read as _, Seek as _, SeekFrom as StdSeekFrom, Write as _};

pub const IMAGE_THUMBNAIL: i64 = -2;

#[derive(Clone, Debug)]
pub struct VideoStreamInfo {
    pub mime: String,
    pub total_size: u64,
    pub from_original: bool,
}

#[derive(Clone, Debug)]
pub struct VideoStreamChunk {
    pub mime: String,
    pub total_size: u64,
    pub start: u64,
    pub end: u64,
    pub bytes: Vec<u8>,
    pub from_original: bool,
}

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
               AND (json_extract(t.payload, '$.local_file') = 1
                    OR (json_extract(t.payload, '$.expired') IS NULL
                        AND coalesce(json_extract(t.payload, '$.expires_at'), ?2 + 1) > ?2)
                    OR EXISTS (
                        SELECT 1 FROM media_files f
                        WHERE f.device_id = ?3
                          AND f.conversation_id = t.conversation_id
                          AND f.content_key = json_extract(t.payload, '$.attachment.sha256')
                          AND f.status = 'available'
                          AND f.variant IN ('image_original', 'image_thumbnail', 'video_original', 'video_preview', 'video_cover')
                    )
                    OR EXISTS (
                        SELECT 1 FROM video_preview_partials p
                        WHERE p.device_id = ?3
                          AND p.conversation_id = t.conversation_id
                          AND p.content_key = json_extract(t.payload, '$.attachment.sha256')
                    ))",
        )
        .bind(id.to_string())
        .bind(self.server_now())
        .bind(self.device_id.to_string())
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

    pub async fn local_video_original_path(
        &self,
        conversation_id: Uuid,
        seq: i64,
        attachment: &Attachment,
        verify_full_hash: bool,
    ) -> Result<Option<PathBuf>> {
        self.ensure_open()?;
        if !self
            .ensure_message_authorized_for_cache(conversation_id, seq)
            .await?
        {
            return Ok(None);
        }
        Ok(self
            .verified_media_path(
                conversation_id,
                seq,
                Some(attachment.id),
                MediaVariant::VideoOriginal,
                &attachment.sha256,
                verify_full_hash,
            )
            .await?
            .map(|(_, path, _)| path))
    }

    pub async fn has_cached_media_variant(
        &self,
        conversation_id: Uuid,
        seq: i64,
        attachment_id: Option<Uuid>,
        variant: MediaVariant,
        content_key: &str,
    ) -> Result<bool> {
        self.ensure_open()?;
        if !self
            .ensure_message_authorized_for_cache(conversation_id, seq)
            .await?
        {
            return Ok(false);
        }
        self.has_verified_media(
            conversation_id,
            seq,
            attachment_id,
            variant,
            content_key,
            false,
        )
        .await
    }

    pub async fn video_media_metadata(&self, id: Uuid) -> Result<VideoMediaMetadata> {
        self.ensure_open()?;
        Ok(self
            .api
            .json(self.api.request(Method::GET, &format!("/v2/files/{id}/media")))
            .await?)
    }

    pub async fn video_cover(
        &self,
        conversation_id: Uuid,
        seq: i64,
        attachment: &Attachment,
    ) -> Result<Option<(String, Vec<u8>)>> {
        self.ensure_open()?;
        if !self
            .ensure_message_authorized_for_cache(conversation_id, seq)
            .await?
        {
            return Ok(None);
        }
        if let Some(cached) = self
            .read_media_bytes(
                conversation_id,
                seq,
                Some(attachment.id),
                MediaVariant::VideoCover,
                &attachment.sha256,
            )
            .await?
        {
            return Ok(Some(cached));
        }
        let meta = self.video_media_metadata(attachment.id).await?;
        if meta.cover.status != "ready" {
            return Ok(None);
        }
        let (Some(expected_size), Some(expected_sha)) = (meta.cover.size, meta.cover.sha256) else {
            return Err(OnlineError::InvalidResponse.into());
        };
        ensure!(
            expected_size > 0 && expected_size <= MAX_PROFILE_IMAGE_BYTES as u64,
            OnlineError::InvalidResponse
        );
        let response = self
            .api
            .send(
                self.api
                    .request(Method::GET, &format!("/v2/files/{}/media/cover", attachment.id)),
            )
            .await?;
        let bytes = read_limited(response, MAX_PROFILE_IMAGE_BYTES).await?;
        ensure!(
            bytes.len() as u64 == expected_size
                && format!("{:x}", Sha256::digest(&bytes)) == expected_sha,
            "Video cover verification failed"
        );
        let desc = MediaDescriptor {
            conversation_id,
            message_seq: seq,
            attachment_id: Some(attachment.id),
            variant: MediaVariant::VideoCover,
            content_key: &attachment.sha256,
            expected_sha256: &expected_sha,
            expected_size,
            mime: &meta.cover.mime,
            dimensions: None,
        };
        self.commit_media_bytes(&desc, &bytes).await?;
        Ok(Some((meta.cover.mime, bytes)))
    }

    pub async fn video_stream_info(
        &self,
        message: &Message,
        file: &Attachment,
        original_only: bool,
    ) -> Result<VideoStreamInfo> {
        self.ensure_open()?;
        ensure!(
            self.ensure_message_authorized_for_cache(message.conversation_id, message.seq)
                .await?,
            "This video is unavailable"
        );
        if let Some((_, _, size)) = self
            .verified_media_path(
                message.conversation_id,
                message.seq,
                Some(file.id),
                MediaVariant::VideoOriginal,
                &file.sha256,
                original_only,
            )
            .await?
        {
            return Ok(VideoStreamInfo {
                mime: file.preview_mime().unwrap_or("video/mp4").to_owned(),
                total_size: size,
                from_original: true,
            });
        }
        ensure!(!original_only, "Original video is not downloaded locally");
        if let Some((_, _, size)) = self
            .verified_media_path(
                message.conversation_id,
                message.seq,
                Some(file.id),
                MediaVariant::VideoPreview,
                &file.sha256,
                false,
            )
            .await?
        {
            return Ok(VideoStreamInfo {
                mime: "video/mp4".into(),
                total_size: size,
                from_original: false,
            });
        }
        if let Some((_, preview_size, mime, _, _)) =
            self.partial_video_preview_row(message.conversation_id, &file.sha256).await?
        {
            return Ok(VideoStreamInfo {
                mime,
                total_size: preview_size,
                from_original: false,
            });
        }
        let meta = self.video_media_metadata(file.id).await?;
        match meta.preview.status.as_str() {
            "ready" => {
                let size = meta.preview.size.ok_or(OnlineError::InvalidResponse)?;
                ensure!(size > 0, OnlineError::InvalidResponse);
                Ok(VideoStreamInfo {
                    mime: meta.preview.mime,
                    total_size: size,
                    from_original: false,
                })
            }
            "pending" => anyhow::bail!("Video preview is being prepared. Try again in a moment."),
            "quota" => anyhow::bail!("Preview storage limit reached. Download original video to play."),
            _ => anyhow::bail!("Compressed preview unavailable. Download original video to play."),
        }
    }

    pub async fn read_video_stream_range(
        &self,
        message: &Message,
        file: &Attachment,
        start: u64,
        end: Option<u64>,
        original_only: bool,
    ) -> Result<VideoStreamChunk> {
        self.ensure_open()?;
        ensure!(
            self.ensure_message_authorized_for_cache(message.conversation_id, message.seq)
                .await?,
            "This video is unavailable"
        );
        let _lock = self.video_stream_locks[media_store::lock_slot(&file.sha256)]
            .lock()
            .await;

        if let Some((file_id, path, size)) = self
            .verified_media_path(
                message.conversation_id,
                message.seq,
                Some(file.id),
                MediaVariant::VideoOriginal,
                &file.sha256,
                false,
            )
            .await?
        {
            let clamped_end = end.unwrap_or(size.saturating_sub(1)).min(size.saturating_sub(1));
            ensure!(size > 0 && start <= clamped_end, "Invalid byte range");
            let read_end = clamped_end.min(start.saturating_add(CHUNK_SIZE - 1));
            let len = (read_end - start + 1) as usize;
            let read_path = path.clone();
            match tokio::task::spawn_blocking(move || read_file_slice(&read_path, size, start, len))
                .await?
            {
                Ok(bytes) => {
                    return Ok(VideoStreamChunk {
                        mime: file.preview_mime().unwrap_or("video/mp4").to_owned(),
                        total_size: size,
                        start,
                        end: read_end,
                        bytes,
                        from_original: true,
                    });
                }
                Err(_) => {
                    self.mark_media_status(file_id, "corrupted").await;
                    ensure!(!original_only, "Local original video verification failed");
                }
            }
        }
        ensure!(!original_only, "Original video is not downloaded locally");

        if let Some((file_id, path, size)) = self
            .verified_media_path(
                message.conversation_id,
                message.seq,
                Some(file.id),
                MediaVariant::VideoPreview,
                &file.sha256,
                false,
            )
            .await?
        {
            let clamped_end = end.unwrap_or(size.saturating_sub(1)).min(size.saturating_sub(1));
            ensure!(size > 0 && start <= clamped_end, "Invalid byte range");
            let read_end = clamped_end.min(start.saturating_add(CHUNK_SIZE - 1));
            let len = (read_end - start + 1) as usize;
            let read_path = path.clone();
            match tokio::task::spawn_blocking(move || read_file_slice(&read_path, size, start, len))
                .await?
            {
                Ok(bytes) => {
                    return Ok(VideoStreamChunk {
                        mime: "video/mp4".into(),
                        total_size: size,
                        start,
                        end: read_end,
                        bytes,
                        from_original: false,
                    });
                }
                Err(_) => {
                    self.mark_media_status(file_id, "corrupted").await;
                }
            }
        }

        let (preview_sha, preview_size, mime, rel_path, mut chunks) = if let Some(row) =
            self.partial_video_preview_row(message.conversation_id, &file.sha256).await?
        {
            row
        } else {
            let meta = self.video_media_metadata(file.id).await?;
            ensure!(
                meta.preview.status == "ready",
                match meta.preview.status.as_str() {
                    "pending" => "Video preview is being prepared. Try again in a moment.",
                    "quota" => "Preview storage limit reached. Download original video to play.",
                    _ => "Compressed preview unavailable. Download original video to play.",
                }
            );
            let (Some(size), Some(sha)) = (meta.preview.size, meta.preview.sha256) else {
                return Err(OnlineError::InvalidResponse.into());
            };
            ensure!(
                size > 0 && size <= MAX_ATTACHMENT_SIZE && media_store::is_valid_sha256(&sha),
                OnlineError::InvalidResponse
            );
            let rel_path = self.partial_video_preview_rel_path(message.conversation_id, &file.sha256);
            self.upsert_partial_video_preview(
                message.conversation_id,
                &file.sha256,
                &sha,
                size,
                &meta.preview.mime,
                &rel_path,
                &BTreeSet::new(),
            )
            .await?;
            (sha, size, meta.preview.mime, rel_path, BTreeSet::new())
        };

        ensure!(preview_size > 0 && start < preview_size, "Invalid byte range");
        let chunk_idx = start / CHUNK_SIZE;
        let chunk_start = chunk_idx * CHUNK_SIZE;
        let chunk_end = (chunk_start + CHUNK_SIZE - 1).min(preview_size - 1);
        let chunk_len = (chunk_end - chunk_start + 1) as usize;
        let clamped_end = end.unwrap_or(preview_size - 1).min(chunk_end);
        ensure!(start <= clamped_end, "Invalid byte range");

        let part_path = self.resolve_conversation_path(message.conversation_id, &rel_path)?;
        if chunks.contains(&chunk_idx) {
            let check_path = part_path.clone();
            let slice_start = start;
            let slice_len = (clamped_end - start + 1) as usize;
            if let Ok(bytes) = tokio::task::spawn_blocking(move || {
                read_partial_slice(&check_path, chunk_end + 1, slice_start, slice_len)
            })
            .await?
            {
                return Ok(VideoStreamChunk {
                    mime,
                    total_size: preview_size,
                    start,
                    end: clamped_end,
                    bytes,
                    from_original: false,
                });
            }
            chunks.remove(&chunk_idx);
        }

        let response = self
            .api
            .send(
                self.api
                    .request(
                        Method::GET,
                        &format!("/v2/files/{}/media/preview", file.id),
                    )
                    .header(
                        reqwest::header::RANGE,
                        format!("bytes={chunk_start}-{chunk_end}"),
                    ),
            )
            .await?;
        let chunk_bytes = read_limited(response, chunk_len).await?;
        ensure!(chunk_bytes.len() == chunk_len, OnlineError::InvalidResponse);

        let write_path = part_path.clone();
        let write_bytes = chunk_bytes.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            if let Some(parent) = write_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .open(&write_path)
                .map_err(super::transfer::save_io_error)?;
            file.seek(StdSeekFrom::Start(chunk_start))?;
            file.write_all(&write_bytes)
                .map_err(super::transfer::save_io_error)?;
            file.sync_data().map_err(super::transfer::save_io_error)?;
            Ok(())
        })
        .await??;

        chunks.insert(chunk_idx);
        self.upsert_partial_video_preview(
            message.conversation_id,
            &file.sha256,
            &preview_sha,
            preview_size,
            &mime,
            &rel_path,
            &chunks,
        )
        .await?;

        let total_chunks = preview_size.div_ceil(CHUNK_SIZE);
        if chunks.len() as u64 == total_chunks {
            let desc = MediaDescriptor {
                conversation_id: message.conversation_id,
                message_seq: message.seq,
                attachment_id: Some(file.id),
                variant: MediaVariant::VideoPreview,
                content_key: &file.sha256,
                expected_sha256: &preview_sha,
                expected_size: preview_size,
                mime: &mime,
                dimensions: None,
            };
            if self.commit_media_file(&desc, &part_path).await.is_ok() {
                let _ = sqlx::query(
                    "DELETE FROM video_preview_partials
                     WHERE device_id = ?1 AND conversation_id = ?2 AND content_key = ?3",
                )
                .bind(self.device_id.to_string())
                .bind(message.conversation_id.to_string())
                .bind(&file.sha256)
                .execute(&self.db)
                .await;
                let _ = tokio::fs::remove_file(&part_path).await;
            }
        }

        let slice_offset = (start - chunk_start) as usize;
        let slice_len = (clamped_end - start + 1) as usize;
        Ok(VideoStreamChunk {
            mime,
            total_size: preview_size,
            start,
            end: clamped_end,
            bytes: chunk_bytes[slice_offset..slice_offset + slice_len].to_vec(),
            from_original: false,
        })
    }

    async fn partial_video_preview_row(
        &self,
        conversation_id: Uuid,
        content_key: &str,
    ) -> Result<Option<(String, u64, String, String, BTreeSet<u64>)>> {
        let row: Option<(String, i64, String, String, String)> = sqlx::query_as(
            "SELECT preview_sha256, preview_size, mime, rel_path, chunks_json
             FROM video_preview_partials
             WHERE device_id = ?1 AND conversation_id = ?2 AND content_key = ?3",
        )
        .bind(self.device_id.to_string())
        .bind(conversation_id.to_string())
        .bind(content_key)
        .fetch_optional(&self.db)
        .await?;
        let Some((sha, size, mime, rel_path, chunks_json)) = row else {
            return Ok(None);
        };
        let chunks: BTreeSet<u64> = serde_json::from_str(&chunks_json).unwrap_or_default();
        Ok(Some((sha, size.max(0) as u64, mime, rel_path, chunks)))
    }

    #[allow(clippy::too_many_arguments)]
    async fn upsert_partial_video_preview(
        &self,
        conversation_id: Uuid,
        content_key: &str,
        preview_sha256: &str,
        preview_size: u64,
        mime: &str,
        rel_path: &str,
        chunks: &BTreeSet<u64>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO video_preview_partials (
                device_id, conversation_id, content_key, preview_sha256, preview_size,
                mime, rel_path, chunks_json, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT (device_id, conversation_id, content_key) DO UPDATE SET
                preview_sha256 = excluded.preview_sha256,
                preview_size = excluded.preview_size,
                mime = excluded.mime,
                rel_path = excluded.rel_path,
                chunks_json = excluded.chunks_json,
                updated_at = excluded.updated_at",
        )
        .bind(self.device_id.to_string())
        .bind(conversation_id.to_string())
        .bind(content_key)
        .bind(preview_sha256)
        .bind(preview_size as i64)
        .bind(mime)
        .bind(rel_path)
        .bind(serde_json::to_string(chunks)?)
        .bind(unix_millis())
        .execute(&self.db)
        .await?;
        Ok(())
    }
}

fn read_file_slice(path: &Path, expected_size: u64, start: u64, len: usize) -> Result<Vec<u8>> {
    let meta = std::fs::symlink_metadata(path)?;
    ensure!(
        meta.is_file() && !staging::is_link(&meta) && meta.len() == expected_size,
        "Media file length verification failed"
    );
    let mut file = std::fs::File::open(path)?;
    file.seek(StdSeekFrom::Start(start))?;
    let mut buf = vec![0u8; len];
    file.read_exact(&mut buf)?;
    Ok(buf)
}

fn read_partial_slice(path: &Path, min_size: u64, start: u64, len: usize) -> Result<Vec<u8>> {
    let meta = std::fs::symlink_metadata(path)?;
    ensure!(
        meta.is_file() && !staging::is_link(&meta) && meta.len() >= min_size,
        "Partial preview chunk is incomplete"
    );
    let mut file = std::fs::File::open(path)?;
    file.seek(StdSeekFrom::Start(start))?;
    let mut buf = vec![0u8; len];
    file.read_exact(&mut buf)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Attachment, AttachmentKind, Conversation, DeviceSession, Directory, Message, User};
    use sha2::{Digest, Sha256};

    fn sha256_hex(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    #[tokio::test]
    async fn video_preview_cache_survives_clear_cache_and_releases_after_verified_original() -> Result<()> {
        let root = tempfile::tempdir()?;
        let user = User {
            id: Uuid::new_v4(),
            handle: "video_cache".into(),
        };
        let session = DeviceSession {
            user: user.clone(),
            device_id: Uuid::new_v4(),
            cursor: Uuid::new_v4(),
        };
        let client = Client::open(
            root.path(),
            crate::client::Api::new("http://127.0.0.1:7841")?,
            session,
            false,
        )
        .await?;
        let chat = Uuid::new_v4();
        let dir = Directory {
            contacts: Vec::new(),
            conversations: vec![Conversation {
                id: chat,
                title: None,
                members: vec![user.clone()],
                activity_at: 1000,
                preview: None,
                preview_seq: None,
                latest_seq: 1,
                unread: 0,
                read_through: 0,
                joined_after: None,
                pinned: false,
                manual_unread: false,
                muted: false,
            }],
            pending_contacts: Vec::new(),
            remarks: Default::default(),
            names: Default::default(),
            avatars: Default::default(),
            read_receipts: true,
        };
        sqlx::query("INSERT INTO directory (id, payload) VALUES (1, ?)")
            .bind(serde_json::to_string(&dir)?)
            .execute(&client.db)
            .await?;

        let orig_bytes = b"fake-video-original-bytes".to_vec();
        let orig_sha = sha256_hex(&orig_bytes);
        let prev_bytes = b"fake-compressed-preview".to_vec();
        let prev_sha = sha256_hex(&prev_bytes);
        let cover_bytes = b"\x89PNG\r\n\x1a\ncover".to_vec();
        let cover_sha = sha256_hex(&cover_bytes);
        let att_id = Uuid::new_v4();
        let msg = Message {
            seq: 10,
            conversation_id: chat,
            sender: user,
            created_at: 1000,
            text: None,
            attachment: Some(Attachment {
                id: att_id,
                name: "clip.mp4".into(),
                kind: AttachmentKind::File,
                size: orig_bytes.len() as u64,
                sha256: orig_sha.clone(),
            }),
            ai_provider: None,
            recalled: false,
            reply_to: None,
            nonce: None,
            expires_at: None,
            expired: None,
            pending: None,
            local_file: true,
        };
        let mut tx = client.db.begin_with("BEGIN IMMEDIATE").await?;
        local_sync::persist_message(&mut tx, msg).await?;
        tx.commit().await?;

        let cover_tmp = root.path().join("cover.png");
        let prev_tmp = root.path().join("prev.mp4");
        let orig_tmp = root.path().join("orig.mp4");
        std::fs::write(&cover_tmp, &cover_bytes)?;
        std::fs::write(&prev_tmp, &prev_bytes)?;
        std::fs::write(&orig_tmp, &orig_bytes)?;

        client
            .commit_media_file(
                &MediaDescriptor {
                    conversation_id: chat,
                    message_seq: 10,
                    attachment_id: Some(att_id),
                    variant: MediaVariant::VideoCover,
                    content_key: &orig_sha,
                    expected_sha256: &cover_sha,
                    expected_size: cover_bytes.len() as u64,
                    mime: "image/png",
                    dimensions: Some((320, 180)),
                },
                &cover_tmp,
            )
            .await?;
        client
            .commit_media_file(
                &MediaDescriptor {
                    conversation_id: chat,
                    message_seq: 10,
                    attachment_id: Some(att_id),
                    variant: MediaVariant::VideoPreview,
                    content_key: &orig_sha,
                    expected_sha256: &prev_sha,
                    expected_size: prev_bytes.len() as u64,
                    mime: "video/mp4",
                    dimensions: None,
                },
                &prev_tmp,
            )
            .await?;

        client.clear_cache().await?;
        assert!(
            client
                .has_cached_media_variant(chat, 10, Some(att_id), MediaVariant::VideoPreview, &orig_sha)
                .await?,
            "clear_cache must preserve VideoPreview"
        );
        assert!(
            client
                .has_cached_media_variant(chat, 10, Some(att_id), MediaVariant::VideoCover, &orig_sha)
                .await?,
            "clear_cache must preserve VideoCover"
        );

        client
            .commit_media_file(
                &MediaDescriptor {
                    conversation_id: chat,
                    message_seq: 10,
                    attachment_id: Some(att_id),
                    variant: MediaVariant::VideoOriginal,
                    content_key: &orig_sha,
                    expected_sha256: &orig_sha,
                    expected_size: orig_bytes.len() as u64,
                    mime: "video/mp4",
                    dimensions: None,
                },
                &orig_tmp,
            )
            .await?;
        client
            .release_video_preview_after_original(chat, &orig_sha)
            .await?;

        assert!(
            !client
                .has_cached_media_variant(chat, 10, Some(att_id), MediaVariant::VideoPreview, &orig_sha)
                .await?,
            "VideoPreview must be removed once verified original is downloaded"
        );
        assert!(
            client
                .has_cached_media_variant(chat, 10, Some(att_id), MediaVariant::VideoCover, &orig_sha)
                .await?,
            "VideoCover must be preserved after preview release"
        );
        assert!(
            client
                .has_cached_media_variant(chat, 10, Some(att_id), MediaVariant::VideoOriginal, &orig_sha)
                .await?,
            "VideoOriginal must be preserved"
        );

        client.close().await;
        Ok(())
    }
}
