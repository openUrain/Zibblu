use super::{Client, staging, unix_millis};
use crate::{Attachment, MAX_ATTACHMENT_SIZE, MAX_PROFILE_IMAGE_BYTES, Message};
use anyhow::{Result, ensure};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, SqliteConnection};
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    sync::Arc,
};
use tempfile::Builder;
use uuid::Uuid;

pub const MEDIA_MIGRATION_KEY: &str = "migration_v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaVariant {
    ImageOriginal,
    ImageThumbnail,
    VideoCover,
    VideoPreview,
    VideoOriginal,
    Attachment,
}

impl MediaVariant {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ImageOriginal => "image_original",
            Self::ImageThumbnail => "image_thumbnail",
            Self::VideoCover => "video_cover",
            Self::VideoPreview => "video_preview",
            Self::VideoOriginal => "video_original",
            Self::Attachment => "attachment",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "image_original" => Some(Self::ImageOriginal),
            "image_thumbnail" => Some(Self::ImageThumbnail),
            "video_cover" => Some(Self::VideoCover),
            "video_preview" => Some(Self::VideoPreview),
            "video_original" => Some(Self::VideoOriginal),
            "attachment" => Some(Self::Attachment),
            _ => None,
        }
    }

    pub fn dir_name(self) -> &'static str {
        match self {
            Self::ImageOriginal | Self::ImageThumbnail => "images",
            Self::VideoCover => "video_covers",
            Self::VideoPreview => "video_previews",
            Self::VideoOriginal => "videos",
            Self::Attachment => "attachments",
        }
    }

    fn file_prefix(self) -> &'static str {
        match self {
            Self::ImageOriginal => "orig",
            Self::ImageThumbnail => "thumb",
            Self::VideoCover => "cover",
            Self::VideoPreview => "preview",
            Self::VideoOriginal => "orig",
            Self::Attachment => "file",
        }
    }

    pub fn is_original(self) -> bool {
        matches!(
            self,
            Self::ImageOriginal | Self::VideoOriginal | Self::Attachment
        )
    }

    pub fn for_attachment(attachment: &Attachment) -> Self {
        match attachment.preview_mime() {
            Some(mime) if mime.starts_with("image/") => Self::ImageOriginal,
            Some(mime) if mime.starts_with("video/") => Self::VideoOriginal,
            _ => Self::Attachment,
        }
    }

    pub fn is_compatible_with_attachment(self, attachment: &Attachment) -> bool {
        let base = Self::for_attachment(attachment);
        match self {
            Self::ImageOriginal | Self::ImageThumbnail => base == Self::ImageOriginal,
            Self::VideoCover | Self::VideoPreview | Self::VideoOriginal => {
                base == Self::VideoOriginal
            }
            Self::Attachment => base == Self::Attachment,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicResourceKind {
    Avatar,
    Cover,
}

impl PublicResourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Avatar => "avatar",
            Self::Cover => "cover",
        }
    }

    pub fn dir_name(self) -> &'static str {
        match self {
            Self::Avatar => "avatars",
            Self::Cover => "covers",
        }
    }
}

#[derive(Clone, Debug)]
pub struct MediaDescriptor<'a> {
    pub conversation_id: Uuid,
    pub message_seq: i64,
    pub attachment_id: Option<Uuid>,
    pub variant: MediaVariant,
    pub content_key: &'a str,
    pub expected_sha256: &'a str,
    pub expected_size: u64,
    pub mime: &'a str,
    pub dimensions: Option<(u32, u32)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct CacheCleanupReport {
    pub removed_public_files: usize,
    pub removed_temp_files: usize,
    pub reclaimed_bytes: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ConsistencyReport {
    pub marked_missing: usize,
    pub marked_corrupted: usize,
    pub quarantined_unknown: usize,
    pub removed_orphan_files: usize,
    pub removed_stale_tmp: usize,
}

#[derive(FromRow)]
struct MediaFileRow {
    id: i64,
    sha256: String,
    size: i64,
    mime: String,
    rel_path: String,
    status: String,
}

pub(super) struct ActiveTempGuard<'a> {
    active: &'a std::sync::Mutex<HashSet<String>>,
    name: String,
}

impl<'a> ActiveTempGuard<'a> {
    pub(super) fn track(active: &'a std::sync::Mutex<HashSet<String>>, name: String) -> Self {
        active.lock().expect("active temp files").insert(name.clone());
        Self { active, name }
    }
}

impl Drop for ActiveTempGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.active.lock() {
            guard.remove(&self.name);
        }
    }
}

pub struct TempFileGuard {
    active: Arc<std::sync::Mutex<HashSet<String>>>,
    name: String,
}

impl TempFileGuard {
    pub(super) fn track(
        active: Arc<std::sync::Mutex<HashSet<String>>>,
        name: String,
    ) -> Self {
        active.lock().expect("active temp files").insert(name.clone());
        Self { active, name }
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.active.lock() {
            guard.remove(&self.name);
        }
    }
}

pub(super) fn is_valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(super) fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    (width != 0 && height != 0).then_some((width, height))
}

fn canonical_extension(variant: MediaVariant, mime: &str) -> &'static str {
    match variant {
        MediaVariant::ImageThumbnail => "png",
        MediaVariant::ImageOriginal | MediaVariant::VideoCover => match mime {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            "image/gif" => "gif",
            "image/webp" => "webp",
            "image/bmp" => "bmp",
            _ => "img",
        },
        MediaVariant::VideoPreview | MediaVariant::VideoOriginal => match mime {
            "video/mp4" => "mp4",
            "video/webm" => "webm",
            "video/quicktime" => "mov",
            _ => "vid",
        },
        MediaVariant::Attachment => "bin",
    }
}

fn verify_file_integrity(path: &Path, expected_size: u64, expected_sha256: &str) -> Result<(), VerifyFailure> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(VerifyFailure::Missing),
        Err(_) => return Err(VerifyFailure::Corrupted),
    };
    if !metadata.is_file() || staging::is_link(&metadata) || metadata.len() != expected_size {
        return Err(VerifyFailure::Corrupted);
    }
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(VerifyFailure::Missing),
        Err(_) => return Err(VerifyFailure::Corrupted),
    };
    let actual = staging::hash_copy(file, io::sink()).map_err(|_| VerifyFailure::Corrupted)?;
    if actual != expected_sha256 {
        return Err(VerifyFailure::Corrupted);
    }
    Ok(())
}

fn read_verified_bytes(path: &Path, expected_size: u64, expected_sha256: &str) -> Result<Vec<u8>, VerifyFailure> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(VerifyFailure::Missing),
        Err(_) => return Err(VerifyFailure::Corrupted),
    };
    if !metadata.is_file() || staging::is_link(&metadata) || metadata.len() != expected_size {
        return Err(VerifyFailure::Corrupted);
    }
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(VerifyFailure::Missing),
        Err(_) => return Err(VerifyFailure::Corrupted),
    };
    let mut bytes = Vec::with_capacity(expected_size as usize);
    let mut hasher = Sha256::new();
    let mut reader = io::BufReader::with_capacity(64 * 1024, file);
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(|_| VerifyFailure::Corrupted)?;
        if n == 0 {
            break;
        }
        if bytes.len() as u64 + n as u64 > expected_size {
            return Err(VerifyFailure::Corrupted);
        }
        hasher.update(&buf[..n]);
        bytes.extend_from_slice(&buf[..n]);
    }
    if bytes.len() as u64 != expected_size {
        return Err(VerifyFailure::Corrupted);
    }
    if format!("{:x}", hasher.finalize()) != expected_sha256 {
        return Err(VerifyFailure::Corrupted);
    }
    Ok(bytes)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VerifyFailure {
    Missing,
    Corrupted,
}

impl VerifyFailure {
    fn status_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Corrupted => "corrupted",
        }
    }
}

#[cfg(windows)]
pub(super) fn fs_path(path: &Path) -> PathBuf {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    if !path.is_absolute() {
        return path.to_path_buf();
    }
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .map(|c| if c == b'/' as u16 { b'\\' as u16 } else { c })
        .collect();
    let bs = b'\\' as u16;
    if wide.starts_with(&[bs, bs, b'?' as u16, bs]) || wide.starts_with(&[bs, bs, b'.' as u16, bs]) {
        return PathBuf::from(OsString::from_wide(&wide));
    }
    let mut extended = Vec::with_capacity(wide.len() + 8);
    if wide.starts_with(&[bs, bs]) {
        // UNC path \\server\share -> \\?\UNC\server\share
        extended.extend_from_slice(&[
            bs, bs, b'?' as u16, bs, b'U' as u16, b'N' as u16, b'C' as u16, bs,
        ]);
        extended.extend_from_slice(&wide[2..]);
    } else {
        extended.extend_from_slice(&[bs, bs, b'?' as u16, bs]);
        extended.extend_from_slice(&wide);
    }
    PathBuf::from(OsString::from_wide(&extended))
}

#[cfg(not(windows))]
pub(super) fn fs_path(path: &Path) -> PathBuf {
    path.to_path_buf()
}

fn safe_rel_path(profile: &Path, rel: &str, required_prefix: &str) -> Result<PathBuf> {
    ensure!(
        rel.starts_with(required_prefix) && !rel.contains('\\') && !rel.contains('\0'),
        "Invalid media relative path"
    );
    let path = Path::new(rel);
    for component in path.components() {
        ensure!(
            matches!(component, Component::Normal(_)),
            "Unsafe media relative path"
        );
    }
    Ok(fs_path(&profile.join(path)))
}

pub(super) fn lock_slot(seed: &str) -> usize {
    let mut acc: usize = 5381;
    for byte in seed.bytes() {
        acc = acc.wrapping_mul(33).wrapping_add(usize::from(byte));
    }
    acc % 16
}

fn remove_file_if_exists(path: &Path) -> u64 {
    let size = fs::symlink_metadata(path).ok().map_or(0, |meta| meta.len());
    if fs::remove_file(path).is_ok() {
        size
    } else {
        0
    }
}

fn move_to_quarantine(source: &Path, quarantine_root: &Path, bucket: &str, relative_hint: &str) -> Result<PathBuf> {
    let sanitized: String = relative_hint
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') { ch } else { '_' })
        .collect();
    // Keep the filename component well within NTFS's 255-character component limit.
    let safe_hint = if sanitized.len() > 80 {
        &sanitized[sanitized.len() - 80..]
    } else {
        sanitized.as_str()
    };
    let dir = quarantine_root.join(bucket);
    fs::create_dir_all(&dir)?;
    let target = dir.join(format!("{}-{safe_hint}", Uuid::new_v4().simple()));
    if fs::rename(source, &target).is_err() {
        fs::copy(source, &target)?;
        let _ = fs::remove_file(source);
    }
    Ok(target)
}

impl Client {
    pub fn media_root(&self) -> &Path {
        &self.media_root
    }

    pub fn track_temp_prefix(&self, prefix: impl Into<String>) -> TempFileGuard {
        TempFileGuard::track(Arc::clone(&self.active_tmp), prefix.into())
    }

    pub fn device_id(&self) -> Uuid {
        self.device_id
    }

    pub(super) fn device_media_prefix(&self) -> String {
        format!("media/{}/", self.device_id)
    }

    pub(super) fn conversation_media_prefix(&self, conversation_id: Uuid) -> String {
        format!("media/{}/conversations/{conversation_id}/", self.device_id)
    }

    pub(super) fn public_media_prefix(&self) -> String {
        format!("media/{}/public/", self.device_id)
    }

    fn tmp_dir(&self) -> PathBuf {
        fs_path(&self.media_root.join("tmp"))
    }

    fn quarantine_dir(&self) -> PathBuf {
        fs_path(&self.media_root.join("quarantine"))
    }

    fn conversation_rel_path(
        &self,
        conversation_id: Uuid,
        variant: MediaVariant,
        content_key: &str,
        mime: &str,
    ) -> String {
        let ext = canonical_extension(variant, mime);
        format!(
            "media/{}/conversations/{conversation_id}/{}/{}_{content_key}.{ext}",
            self.device_id,
            variant.dir_name(),
            variant.file_prefix(),
        )
    }

    fn public_rel_path(&self, kind: PublicResourceKind, version: Uuid) -> String {
        format!("media/{}/public/{}/{version}.png", self.device_id, kind.dir_name())
    }

    pub(super) fn resolve_conversation_path(&self, conversation_id: Uuid, rel_path: &str) -> Result<PathBuf> {
        safe_rel_path(
            self.profile_dir(),
            rel_path,
            &self.conversation_media_prefix(conversation_id),
        )
    }

    pub(super) fn resolve_public_path(&self, rel_path: &str) -> Result<PathBuf> {
        safe_rel_path(self.profile_dir(), rel_path, &self.public_media_prefix())
    }

    pub(super) fn resolve_device_media_path(&self, rel_path: &str) -> Result<PathBuf> {
        safe_rel_path(self.profile_dir(), rel_path, &self.device_media_prefix())
    }

    pub async fn commit_media_bytes(&self, desc: &MediaDescriptor<'_>, bytes: &[u8]) -> Result<PathBuf> {
        self.ensure_open()?;
        ensure!(
            is_valid_sha256(desc.content_key) && is_valid_sha256(desc.expected_sha256),
            "Invalid media digest"
        );
        ensure!(
            bytes.len() as u64 == desc.expected_size && desc.expected_size <= MAX_ATTACHMENT_SIZE,
            "Media length verification failed"
        );
        let rel_path = self.conversation_rel_path(
            desc.conversation_id,
            desc.variant,
            desc.content_key,
            desc.mime,
        );
        let _lock = self.media_locks[lock_slot(&rel_path)].lock().await;
        let final_path = self.resolve_conversation_path(desc.conversation_id, &rel_path)?;
        let tmp_dir = self.tmp_dir();
        let expected_sha = desc.expected_sha256.to_owned();
        let expected_size = desc.expected_size;
        let payload = bytes.to_vec();
        let target_clone = final_path.clone();

        let created_fresh = {
            let tmp_name = format!("media-{}.tmp", Uuid::new_v4().simple());
            let _guard = ActiveTempGuard::track(&self.active_tmp, tmp_name.clone());
            tokio::task::spawn_blocking(move || -> Result<bool> {
                if let Some(parent) = target_clone.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::create_dir_all(&tmp_dir)?;
                if verify_file_integrity(&target_clone, expected_size, &expected_sha).is_ok() {
                    return Ok(false);
                }
                let actual_sha = format!("{:x}", Sha256::digest(&payload));
                ensure!(actual_sha == expected_sha, "Media SHA-256 verification failed");
                let mut temp = Builder::new()
                    .prefix(&tmp_name)
                    .suffix(".part")
                    .tempfile_in(&tmp_dir)
                    .map_err(super::transfer::save_io_error)?;
                temp.write_all(&payload).map_err(super::transfer::save_io_error)?;
                temp.as_file().sync_all().map_err(super::transfer::save_io_error)?;
                let existed = target_clone.symlink_metadata().is_ok();
                if existed {
                    temp.persist(&target_clone)
                        .map_err(|error| super::transfer::save_io_error(error.error))?;
                } else {
                    temp.persist_noclobber(&target_clone)
                        .map_err(|error| super::transfer::save_io_error(error.error))?;
                }
                Ok(!existed)
            })
            .await??
        };

        let commit = self.commit_media_index(desc, &rel_path).await;
        if let Err(error) = commit {
            if created_fresh {
                let referenced: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM media_files WHERE device_id = ? AND rel_path = ?)",
                )
                .bind(self.device_id.to_string())
                .bind(&rel_path)
                .fetch_one(&self.db)
                .await
                .unwrap_or(true);
                if !referenced {
                    let _ = tokio::fs::remove_file(&final_path).await;
                }
            }
            return Err(error);
        }
        Ok(final_path)
    }

    pub async fn commit_media_file(&self, desc: &MediaDescriptor<'_>, source: &Path) -> Result<PathBuf> {
        self.ensure_open()?;
        ensure!(
            is_valid_sha256(desc.content_key) && is_valid_sha256(desc.expected_sha256),
            "Invalid media digest"
        );
        ensure!(desc.expected_size <= MAX_ATTACHMENT_SIZE, "Media exceeds size limit");
        let rel_path = self.conversation_rel_path(
            desc.conversation_id,
            desc.variant,
            desc.content_key,
            desc.mime,
        );
        let _lock = self.media_locks[lock_slot(&rel_path)].lock().await;
        let final_path = self.resolve_conversation_path(desc.conversation_id, &rel_path)?;
        let tmp_dir = self.tmp_dir();
        let expected_sha = desc.expected_sha256.to_owned();
        let expected_size = desc.expected_size;
        let source_path = source.to_path_buf();
        let target_clone = final_path.clone();

        let created_fresh = {
            let tmp_name = format!("media-{}.tmp", Uuid::new_v4().simple());
            let _guard = ActiveTempGuard::track(&self.active_tmp, tmp_name.clone());
            tokio::task::spawn_blocking(move || -> Result<bool> {
                if let Some(parent) = target_clone.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::create_dir_all(&tmp_dir)?;
                if verify_file_integrity(&target_clone, expected_size, &expected_sha).is_ok() {
                    return Ok(false);
                }
                let src_meta = fs::symlink_metadata(&source_path)?;
                ensure!(
                    src_meta.is_file() && !staging::is_link(&src_meta) && src_meta.len() == expected_size,
                    "File length or type verification failed. Retry the download."
                );
                let input = File::open(&source_path)?;
                let mut temp = Builder::new()
                    .prefix(&tmp_name)
                    .suffix(".part")
                    .tempfile_in(&tmp_dir)
                    .map_err(super::transfer::save_io_error)?;
                let actual_sha = staging::hash_copy(input, temp.as_file_mut()).map_err(|error| {
                    error
                        .downcast::<io::Error>()
                        .map_or_else(|error| error, super::transfer::save_io_error)
                })?;
                ensure!(
                    actual_sha == expected_sha,
                    "File verification failed. Retry the download."
                );
                ensure!(
                    temp.as_file().metadata()?.len() == expected_size,
                    "File length verification failed. Retry the download."
                );
                temp.as_file().sync_all().map_err(super::transfer::save_io_error)?;
                let existed = target_clone.symlink_metadata().is_ok();
                if existed {
                    temp.persist(&target_clone)
                        .map_err(|error| super::transfer::save_io_error(error.error))?;
                } else {
                    temp.persist_noclobber(&target_clone)
                        .map_err(|error| super::transfer::save_io_error(error.error))?;
                }
                Ok(!existed)
            })
            .await??
        };

        let commit = self.commit_media_index(desc, &rel_path).await;
        if let Err(error) = commit {
            if created_fresh {
                let referenced: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM media_files WHERE device_id = ? AND rel_path = ?)",
                )
                .bind(self.device_id.to_string())
                .bind(&rel_path)
                .fetch_one(&self.db)
                .await
                .unwrap_or(true);
                if !referenced {
                    let _ = tokio::fs::remove_file(&final_path).await;
                }
            }
            return Err(error);
        }
        Ok(final_path)
    }

    async fn commit_media_index(&self, desc: &MediaDescriptor<'_>, rel_path: &str) -> Result<()> {
        let now = unix_millis();
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        if desc.variant == MediaVariant::VideoCover {
            let allowed: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM preview_eligible WHERE seq = ? AND conversation_id = ?)",
            ).bind(desc.message_seq).bind(desc.conversation_id.to_string()).fetch_one(&mut *tx).await?;
            ensure!(allowed, "This video is no longer available");
        }
        let blocked: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM recall_tombstones WHERE message_seq = ?1)
                 OR EXISTS(SELECT 1 FROM hidden_messages WHERE seq = ?1)
                 OR EXISTS(SELECT 1 FROM local_deleted_messages WHERE seq = ?1)
                 OR EXISTS(SELECT 1 FROM local_suppressed_sends WHERE kind = 'upload' AND id = ?2)
                 OR EXISTS(SELECT 1 FROM inbox WHERE seq = ?1 AND json_extract(payload, '$.recalled') = 1)",
        )
        .bind(desc.message_seq)
        .bind(desc.attachment_id.map(|id| id.to_string()))
        .fetch_one(&mut *tx)
        .await?;
        ensure!(!blocked, "This attachment is no longer available");
        let (width, height) = desc
            .dimensions
            .map_or((None, None), |(w, h)| (Some(i64::from(w)), Some(i64::from(h))));
        let file_id: i64 = sqlx::query_scalar(
            "INSERT INTO media_files (
                device_id, conversation_id, variant, content_key, sha256, size, mime,
                rel_path, status, width, height, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'available', ?9, ?10, ?11, ?11)
             ON CONFLICT (device_id, conversation_id, variant, content_key) DO UPDATE SET
                sha256 = excluded.sha256,
                size = excluded.size,
                mime = excluded.mime,
                rel_path = excluded.rel_path,
                status = 'available',
                width = coalesce(excluded.width, media_files.width),
                height = coalesce(excluded.height, media_files.height),
                updated_at = excluded.updated_at
             RETURNING id",
        )
        .bind(self.device_id.to_string())
        .bind(desc.conversation_id.to_string())
        .bind(desc.variant.as_str())
        .bind(desc.content_key)
        .bind(desc.expected_sha256)
        .bind(desc.expected_size as i64)
        .bind(desc.mime)
        .bind(rel_path)
        .bind(width)
        .bind(height)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO media_refs (file_id, ref_kind, ref_id, conversation_id, attachment_id, variant, created_at)
             VALUES (?1, 'message', ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (ref_kind, ref_id, variant) DO UPDATE SET
                file_id = excluded.file_id,
                conversation_id = excluded.conversation_id,
                attachment_id = excluded.attachment_id",
        )
        .bind(file_id)
        .bind(desc.message_seq.to_string())
        .bind(desc.conversation_id.to_string())
        .bind(desc.attachment_id.map(|id| id.to_string()))
        .bind(desc.variant.as_str())
        .bind(now)
        .execute(&mut *tx)
        .await?;
        bind_same_conversation_duplicates_in_tx(
            &mut tx,
            file_id,
            desc.conversation_id,
            desc.variant,
            desc.content_key,
            Some(desc.message_seq),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub(super) async fn ensure_message_authorized_for_cache(
        &self,
        conversation_id: Uuid,
        message_seq: i64,
    ) -> Result<bool> {
        let allowed: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM preview_eligible
                WHERE seq = ?1 AND conversation_id = ?2
            )",
        )
        .bind(message_seq)
        .bind(conversation_id.to_string())
        .fetch_one(&self.db)
        .await?;
        Ok(allowed)
    }

    pub async fn read_media_bytes(
        &self,
        conversation_id: Uuid,
        message_seq: i64,
        attachment_id: Option<Uuid>,
        variant: MediaVariant,
        content_key: &str,
    ) -> Result<Option<(String, Vec<u8>)>> {
        self.ensure_open()?;
        if !is_valid_sha256(content_key)
            || !self
                .ensure_message_authorized_for_cache(conversation_id, message_seq)
                .await?
        {
            return Ok(None);
        }
        let Some(row) = self
            .lookup_or_bind_media_row(conversation_id, message_seq, attachment_id, variant, content_key)
            .await?
        else {
            return Ok(None);
        };
        if row.status != "available" {
            return Ok(None);
        }
        let path = match self.resolve_conversation_path(conversation_id, &row.rel_path) {
            Ok(path) => path,
            Err(_) => {
                self.mark_media_status(row.id, "corrupted").await;
                return Ok(None);
            }
        };
        let expected_size = row.size.max(0) as u64;
        let expected_sha = row.sha256.clone();
        match tokio::task::spawn_blocking(move || read_verified_bytes(&path, expected_size, &expected_sha))
            .await?
        {
            Ok(bytes) => Ok(Some((row.mime, bytes))),
            Err(failure) => {
                self.mark_media_status(row.id, failure.status_str()).await;
                Ok(None)
            }
        }
    }

    pub async fn verified_media_path(
        &self,
        conversation_id: Uuid,
        message_seq: i64,
        attachment_id: Option<Uuid>,
        variant: MediaVariant,
        content_key: &str,
        verify_full_hash: bool,
    ) -> Result<Option<(i64, PathBuf, u64)>> {
        self.ensure_open()?;
        if !is_valid_sha256(content_key) {
            return Ok(None);
        }
        let Some(row) = self
            .lookup_or_bind_media_row(conversation_id, message_seq, attachment_id, variant, content_key)
            .await?
        else {
            return Ok(None);
        };
        if row.status != "available" {
            return Ok(None);
        }
        let path = match self.resolve_conversation_path(conversation_id, &row.rel_path) {
            Ok(path) => path,
            Err(_) => {
                self.mark_media_status(row.id, "corrupted").await;
                return Ok(None);
            }
        };
        let expected_size = row.size.max(0) as u64;
        let expected_sha = row.sha256.clone();
        let check_path = path.clone();
        let valid = tokio::task::spawn_blocking(move || -> Result<(), VerifyFailure> {
            if verify_full_hash {
                verify_file_integrity(&check_path, expected_size, &expected_sha)
            } else {
                let meta = match fs::symlink_metadata(&check_path) {
                    Ok(meta) => meta,
                    Err(err) if err.kind() == io::ErrorKind::NotFound => {
                        return Err(VerifyFailure::Missing);
                    }
                    Err(_) => return Err(VerifyFailure::Corrupted),
                };
                if !meta.is_file() || staging::is_link(&meta) || meta.len() != expected_size {
                    return Err(VerifyFailure::Corrupted);
                }
                Ok(())
            }
        })
        .await?;
        match valid {
            Ok(()) => Ok(Some((row.id, path, expected_size))),
            Err(failure) => {
                self.mark_media_status(row.id, failure.status_str()).await;
                Ok(None)
            }
        }
    }

    async fn lookup_or_bind_media_row(
        &self,
        conversation_id: Uuid,
        message_seq: i64,
        attachment_id: Option<Uuid>,
        variant: MediaVariant,
        content_key: &str,
    ) -> Result<Option<MediaFileRow>> {
        let device = self.device_id.to_string();
        let conv = conversation_id.to_string();
        let seq_str = message_seq.to_string();
        let row: Option<MediaFileRow> = sqlx::query_as(
            "SELECT f.id, f.sha256, f.size, f.mime, f.rel_path, f.status
             FROM media_refs r
             JOIN media_files f ON f.id = r.file_id
             WHERE r.ref_kind = 'message' AND r.ref_id = ?1 AND r.variant = ?2
               AND f.device_id = ?3 AND f.conversation_id = ?4 AND f.content_key = ?5",
        )
        .bind(&seq_str)
        .bind(variant.as_str())
        .bind(&device)
        .bind(&conv)
        .bind(content_key)
        .fetch_optional(&self.db)
        .await?;
        if row.is_some() {
            return Ok(row);
        }
        // Within-conversation dedup: bind to an existing available file in the SAME conversation.
        let existing: Option<MediaFileRow> = sqlx::query_as(
            "SELECT id, sha256, size, mime, rel_path, status
             FROM media_files
             WHERE device_id = ?1 AND conversation_id = ?2 AND variant = ?3
               AND content_key = ?4 AND status = 'available'",
        )
        .bind(&device)
        .bind(&conv)
        .bind(variant.as_str())
        .bind(content_key)
        .fetch_optional(&self.db)
        .await?;
        if let Some(ref found) = existing {
            let _ = sqlx::query(
                "INSERT INTO media_refs (file_id, ref_kind, ref_id, conversation_id, attachment_id, variant, created_at)
                 VALUES (?1, 'message', ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT (ref_kind, ref_id, variant) DO UPDATE SET
                    file_id = excluded.file_id,
                    conversation_id = excluded.conversation_id,
                    attachment_id = excluded.attachment_id",
            )
            .bind(found.id)
            .bind(&seq_str)
            .bind(&conv)
            .bind(attachment_id.map(|id| id.to_string()))
            .bind(variant.as_str())
            .bind(unix_millis())
            .execute(&self.db)
            .await;
        }
        Ok(existing)
    }

    pub(super) async fn pin_task_media_ref(
        &self,
        file_id: i64,
        ref_kind: &str,
        ref_id: &str,
        conversation_id: Uuid,
        attachment_id: Option<Uuid>,
        variant: MediaVariant,
    ) -> Result<()> {
        let inserted = sqlx::query(
            "INSERT INTO media_refs (file_id, ref_kind, ref_id, conversation_id, attachment_id, variant, created_at)
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7
             WHERE ?2 <> 'download' OR NOT EXISTS (SELECT 1 FROM cancelled_downloads WHERE id = ?3)
             ON CONFLICT (ref_kind, ref_id, variant) DO UPDATE SET file_id = excluded.file_id",
        )
        .bind(file_id)
        .bind(ref_kind)
        .bind(ref_id)
        .bind(conversation_id.to_string())
        .bind(attachment_id.map(|id| id.to_string()))
        .bind(variant.as_str())
        .bind(unix_millis())
        .execute(&self.db)
        .await?;
        ensure!(inserted.rows_affected() != 0, "This download was cancelled");
        Ok(())
    }

    pub(super) async fn release_task_media_ref(&self, ref_kind: &str, ref_id: &str) -> Result<()> {
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        let removed = release_media_refs_in_tx(&mut tx, ref_kind, ref_id).await?;
        tx.commit().await?;
        self.remove_rel_paths(removed).await;
        Ok(())
    }

    pub(super) fn partial_video_preview_rel_path(&self, conversation_id: Uuid, content_key: &str) -> String {
        format!(
            "media/{}/conversations/{conversation_id}/video_previews/partial_{content_key}.part",
            self.device_id
        )
    }

    pub async fn has_verified_media(
        &self,
        conversation_id: Uuid,
        message_seq: i64,
        attachment_id: Option<Uuid>,
        variant: MediaVariant,
        content_key: &str,
        verify_full_hash: bool,
    ) -> Result<bool> {
        Ok(self
            .verified_media_path(
                conversation_id,
                message_seq,
                attachment_id,
                variant,
                content_key,
                verify_full_hash,
            )
            .await?
            .is_some())
    }

    pub async fn release_video_preview_after_original(
        &self,
        conversation_id: Uuid,
        content_key: &str,
    ) -> Result<()> {
        self.ensure_open()?;
        let device = self.device_id.to_string();
        let conv = conversation_id.to_string();
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        let mut paths = Vec::new();
        let preview_ids: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM media_files
             WHERE device_id = ?1 AND conversation_id = ?2 AND variant = 'video_preview' AND content_key = ?3",
        )
        .bind(&device)
        .bind(&conv)
        .bind(content_key)
        .fetch_all(&mut *tx)
        .await?;
        for file_id in preview_ids {
            sqlx::query("DELETE FROM media_refs WHERE file_id = ?1")
                .bind(file_id)
                .execute(&mut *tx)
                .await?;
            if let Some(rel) = sqlx::query_scalar::<_, String>(
                "DELETE FROM media_files WHERE id = ?1 RETURNING rel_path",
            )
            .bind(file_id)
            .fetch_optional(&mut *tx)
            .await?
            {
                sqlx::query("INSERT OR IGNORE INTO media_reclaims (rel_path) VALUES (?)")
                    .bind(&rel)
                    .execute(&mut *tx)
                    .await?;
                paths.push(rel);
            }
        }
        let partial_paths: Vec<String> = sqlx::query_scalar(
            "DELETE FROM video_preview_partials
             WHERE device_id = ?1 AND conversation_id = ?2 AND content_key = ?3
             RETURNING rel_path",
        )
        .bind(&device)
        .bind(&conv)
        .bind(content_key)
        .fetch_all(&mut *tx)
        .await?;
        for rel in partial_paths {
            sqlx::query("INSERT OR IGNORE INTO media_reclaims (rel_path) VALUES (?)")
                .bind(&rel)
                .execute(&mut *tx)
                .await?;
            paths.push(rel);
        }
        tx.commit().await?;
        self.remove_rel_paths(paths).await;
        Ok(())
    }

    pub(super) async fn mark_media_status(&self, file_id: i64, status: &str) {
        let _ = sqlx::query("UPDATE media_files SET status = ?1, updated_at = ?2 WHERE id = ?3")
            .bind(status)
            .bind(unix_millis())
            .bind(file_id)
            .execute(&self.db)
            .await;
    }

    pub(super) async fn recover_media_reclaims(&self) -> Result<()> {
        let paths: Vec<String> = sqlx::query_scalar("SELECT rel_path FROM media_reclaims")
            .fetch_all(&self.db).await?;
        self.remove_rel_paths(paths).await;
        Ok(())
    }

    pub(super) async fn remove_rel_paths(&self, rel_paths: Vec<String>) {
        for rel in rel_paths {
            let _lock = self.media_locks[lock_slot(&rel)].lock().await;
            let Ok(mut tx) = self.db.begin_with("BEGIN IMMEDIATE").await else { continue };
            let indexed: Result<bool, _> = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM media_files WHERE rel_path = ?1)
                     OR EXISTS(SELECT 1 FROM public_cache_files WHERE rel_path = ?1)
                     OR EXISTS(SELECT 1 FROM video_preview_partials WHERE rel_path = ?1)",
            ).bind(&rel).fetch_one(&mut *tx).await;
            if matches!(indexed, Ok(true)) {
                // A replacement may be released again concurrently; clear this old intent in the same transaction.
                if sqlx::query("DELETE FROM media_reclaims WHERE rel_path = ?")
                    .bind(&rel).execute(&mut *tx).await.is_ok()
                {
                    let _ = tx.commit().await;
                }
                continue;
            }
            if tx.commit().await.is_err() { continue; }
            let completed = match indexed {
                Ok(false) => {
                    let path = rel.strip_prefix("media/").and_then(|rest| rest.split_once('/'))
                        .and_then(|(device, _)| Uuid::parse_str(device).ok()
                            .and_then(|_| safe_rel_path(self.profile_dir(), &rel, &format!("media/{device}/")).ok()));
                    match path {
                        Some(path) => match tokio::fs::remove_file(path).await {
                            Ok(()) => true,
                            Err(error) => error.kind() == io::ErrorKind::NotFound,
                        },
                        None => false,
                    }
                }
                _ => false,
            };
            if completed {
                let _ = sqlx::query("DELETE FROM media_reclaims WHERE rel_path = ?")
                    .bind(&rel).execute(&self.db).await;
            }
        }
    }

    pub async fn clear_cache(&self) -> Result<CacheCleanupReport> {
        self.clear_cache_inner(None).await
    }

    async fn clear_cache_inner(
        &self,
        after_snapshot_hook: Option<(tokio::sync::oneshot::Sender<()>, tokio::sync::oneshot::Receiver<()>)>,
    ) -> Result<CacheCleanupReport> {
        let _prepare = self.prepare_gate.lock().await;
        self.ensure_open()?;
        let now = unix_millis();
        let device = self.device_id.to_string();
        let mut report = CacheCleanupReport::default();

        // 1. Clean expired or confirmed unreferenced public cache entries ONLY.
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query("DELETE FROM cached_profiles WHERE expires_at <= ?1")
            .bind(now)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM public_cache_refs WHERE expires_at <= ?1")
            .bind(now)
            .execute(&mut *tx)
            .await?;
        let mut removed_public: Vec<String> = sqlx::query_scalar(
            "DELETE FROM public_cache_files
             WHERE device_id = ?1 AND (
                expires_at <= ?2
                OR status <> 'available'
                OR id NOT IN (
                    SELECT active_file_id FROM public_cache_refs WHERE active_file_id IS NOT NULL
                )
             )
             RETURNING rel_path",
        )
        .bind(&device)
        .bind(now)
        .fetch_all(&mut *tx)
        .await?;
        for rel in self.prune_profile_cache(&mut tx).await? {
            if !removed_public.contains(&rel) {
                removed_public.push(rel);
            }
        }
        let valid_public_rel: HashSet<String> = sqlx::query_scalar(
            "SELECT rel_path FROM public_cache_files WHERE device_id = ?1",
        )
        .bind(&device)
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .collect();
        tx.commit().await?;

        if let Some((ready, resume)) = after_snapshot_hook {
            let _ = ready.send(());
            let _ = resume.await;
        }

        for rel in removed_public {
            let _lock = self.media_locks[lock_slot(&rel)].lock().await;
            let still_indexed: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM public_cache_files WHERE device_id = ?1 AND rel_path = ?2)",
            )
            .bind(&device)
            .bind(&rel)
            .fetch_one(&self.db)
            .await
            .unwrap_or(true);
            if still_indexed {
                continue;
            }
            if let Ok(path) = self.resolve_public_path(&rel) {
                let bytes = remove_file_if_exists(&path);
                if bytes > 0 || !path.exists() {
                    report.removed_public_files += 1;
                    report.reclaimed_bytes += bytes;
                }
            }
        }

        // Collect stray public/{avatars,covers} and tmp/ candidates read-only, then re-verify under lock before deleting.
        let profile_dir = fs_path(self.profile_dir());
        let public_root = fs_path(&self.media_root.join("public"));
        let tmp_root = self.tmp_dir();

        let (public_candidates, tmp_candidates) = tokio::task::spawn_blocking(move || {
            let mut public_candidates = Vec::new();
            if public_root.is_dir() {
                for entry in walkdir::WalkDir::new(&public_root)
                    .follow_links(false)
                    .into_iter()
                    .flatten()
                {
                    let path = entry.path();
                    let Ok(meta) = fs::symlink_metadata(path) else { continue };
                    if !meta.is_file() || staging::is_link(&meta) {
                        continue;
                    }
                    if let Ok(rel) = path.strip_prefix(&profile_dir) {
                        let rel_str = rel.to_string_lossy().replace('\\', "/");
                        if !valid_public_rel.contains(&rel_str) {
                            public_candidates.push((rel_str, path.to_path_buf()));
                        }
                    }
                }
            }
            let mut tmp_candidates = Vec::new();
            if tmp_root.is_dir() {
                for entry in walkdir::WalkDir::new(&tmp_root)
                    .follow_links(false)
                    .into_iter()
                    .flatten()
                {
                    let path = entry.path();
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let Ok(meta) = fs::symlink_metadata(path) else { continue };
                    if meta.is_file() && !staging::is_link(&meta) {
                        tmp_candidates.push((name, path.to_path_buf()));
                    }
                }
            }
            (public_candidates, tmp_candidates)
        })
        .await?;

        for (rel_str, path) in public_candidates {
            let _lock = self.media_locks[lock_slot(&rel_str)].lock().await;
            let still_indexed: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM public_cache_files WHERE device_id = ?1 AND rel_path = ?2)",
            )
            .bind(&device)
            .bind(&rel_str)
            .fetch_one(&self.db)
            .await
            .unwrap_or(true);
            if still_indexed {
                continue;
            }
            let bytes = remove_file_if_exists(&path);
            if bytes > 0 || !path.exists() {
                report.removed_public_files += 1;
                report.reclaimed_bytes += bytes;
            }
        }

        if !tmp_candidates.is_empty() {
            let active_tmp = Arc::clone(&self.active_tmp);
            let (removed_tmp, reclaimed_tmp) = tokio::task::spawn_blocking(move || {
                let mut removed = 0usize;
                let mut reclaimed = 0u64;
                for (name, path) in tmp_candidates {
                    if let Some(bytes) = delete_if_inactive_tmp(&active_tmp, &name, &path) {
                        removed += 1;
                        reclaimed += bytes;
                    }
                }
                (removed, reclaimed)
            })
            .await?;
            report.removed_temp_files += removed_tmp;
            report.reclaimed_bytes += reclaimed_tmp;
        }
        Ok(report)
    }

    pub async fn run_consistency_check(self: &Arc<Self>) -> Result<ConsistencyReport> {
        self.run_consistency_check_inner(None).await
    }

    async fn run_consistency_check_inner(
        self: &Arc<Self>,
        after_snapshot_hook: Option<(tokio::sync::oneshot::Sender<()>, tokio::sync::oneshot::Receiver<()>)>,
    ) -> Result<ConsistencyReport> {
        self.ensure_open()?;
        let mut report = ConsistencyReport::default();
        let device = self.device_id.to_string();

        self.recover_media_reclaims().await?;
        // 1. Reclaim any DB media_files rows that have 0 references (e.g. crash after unref).
        {
            let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
            let orphan_media: Vec<String> = sqlx::query_scalar(
                "DELETE FROM media_files
                 WHERE device_id = ?1
                   AND id NOT IN (SELECT file_id FROM media_refs)
                 RETURNING rel_path",
            )
            .bind(&device)
            .fetch_all(&mut *tx)
            .await?;
            let orphan_public: Vec<String> = sqlx::query_scalar(
                "DELETE FROM public_cache_files
                 WHERE device_id = ?1
                   AND id NOT IN (
                       SELECT active_file_id FROM public_cache_refs WHERE active_file_id IS NOT NULL
                   )
                 RETURNING rel_path",
            )
            .bind(&device)
            .fetch_all(&mut *tx)
            .await?;
            tx.commit().await?;
            for rel in orphan_media.into_iter().chain(orphan_public) {
                let _lock = self.media_locks[lock_slot(&rel)].lock().await;
                let still_indexed: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM media_files WHERE device_id = ?1 AND rel_path = ?2)
                         OR EXISTS(SELECT 1 FROM public_cache_files WHERE device_id = ?1 AND rel_path = ?2)",
                )
                .bind(&device)
                .bind(&rel)
                .fetch_one(&self.db)
                .await
                .unwrap_or(true);
                if still_indexed {
                    continue;
                }
                if let Ok(path) = self.resolve_device_media_path(&rel) {
                    if remove_file_if_exists(&path) > 0 || !path.exists() {
                        report.removed_orphan_files += 1;
                    }
                }
            }
        }

        // 2. Verify every 'available' media_files row exists and matches size + SHA-256.
        let rows: Vec<MediaFileRow> = sqlx::query_as(
            "SELECT id, sha256, size, mime, rel_path, status
             FROM media_files WHERE device_id = ?1 AND status = 'available'",
        )
        .bind(&device)
        .fetch_all(&self.db)
        .await?;

        let mut indexed_paths = HashSet::new();
        for row in rows {
            self.ensure_open()?;
            indexed_paths.insert(row.rel_path.clone());
            let Ok(path) = self.resolve_device_media_path(&row.rel_path) else {
                self.mark_media_status(row.id, "corrupted").await;
                report.marked_corrupted += 1;
                continue;
            };
            let expected_size = row.size.max(0) as u64;
            let expected_sha = row.sha256.clone();
            let check = tokio::task::spawn_blocking(move || {
                verify_file_integrity(&path, expected_size, &expected_sha)
            })
            .await?;
            if let Err(failure) = check {
                self.mark_media_status(row.id, failure.status_str()).await;
                match failure {
                    VerifyFailure::Missing => report.marked_missing += 1,
                    VerifyFailure::Corrupted => report.marked_corrupted += 1,
                }
            }
            tokio::task::yield_now().await;
        }

        // Also include non-available rows in indexed_paths so we don't quarantine known-corrupt rows mid-repair.
        let non_available: Vec<String> = sqlx::query_scalar(
            "SELECT rel_path FROM media_files WHERE device_id = ?1 AND status <> 'available'
             UNION ALL
             SELECT rel_path FROM video_preview_partials WHERE device_id = ?1",
        )
        .bind(&device)
        .fetch_all(&self.db)
        .await?;
        indexed_paths.extend(non_available);

        // 3. Verify 'available' public_cache_files rows.
        let pub_rows: Vec<(i64, String, i64, String)> = sqlx::query_as(
            "SELECT id, sha256, size, rel_path FROM public_cache_files
             WHERE device_id = ?1 AND status = 'available'",
        )
        .bind(&device)
        .fetch_all(&self.db)
        .await?;
        let mut indexed_public = HashSet::new();
        for (id, sha, size, rel) in pub_rows {
            self.ensure_open()?;
            indexed_public.insert(rel.clone());
            let Ok(path) = self.resolve_public_path(&rel) else {
                let _ = sqlx::query("UPDATE public_cache_files SET status = 'corrupted' WHERE id = ?")
                    .bind(id)
                    .execute(&self.db)
                    .await;
                report.marked_corrupted += 1;
                continue;
            };
            let check = tokio::task::spawn_blocking(move || {
                verify_file_integrity(&path, size.max(0) as u64, &sha)
            })
            .await?;
            if let Err(failure) = check {
                let _ = sqlx::query("UPDATE public_cache_files SET status = ?1 WHERE id = ?2")
                    .bind(failure.status_str())
                    .bind(id)
                    .execute(&self.db)
                    .await;
                match failure {
                    VerifyFailure::Missing => report.marked_missing += 1,
                    VerifyFailure::Corrupted => report.marked_corrupted += 1,
                }
            }
            tokio::task::yield_now().await;
        }
        let non_available_public: Vec<String> = sqlx::query_scalar(
            "SELECT rel_path FROM public_cache_files WHERE device_id = ?1 AND status <> 'available'",
        )
        .bind(&device)
        .fetch_all(&self.db)
        .await?;
        indexed_public.extend(non_available_public);

        if let Some((ready, resume)) = after_snapshot_hook {
            let _ = ready.send(());
            let _ = resume.await;
        }

        // 4. Scan disk read-only to collect unindexed candidates; re-verify under lock before quarantine/delete.
        let profile_dir = fs_path(self.profile_dir());
        let conversations_root = fs_path(&self.media_root.join("conversations"));
        let public_root = fs_path(&self.media_root.join("public"));
        let tmp_root = self.tmp_dir();

        let (conv_candidates, public_candidates, tmp_candidates) =
            tokio::task::spawn_blocking(move || {
                let mut conv_candidates = Vec::new();
                let mut public_candidates = Vec::new();
                let mut tmp_candidates = Vec::new();

                if conversations_root.is_dir() {
                    for entry in walkdir::WalkDir::new(&conversations_root)
                        .follow_links(false)
                        .into_iter()
                        .flatten()
                    {
                        let path = entry.path();
                        let Ok(meta) = fs::symlink_metadata(path) else { continue };
                        if !meta.is_file() || staging::is_link(&meta) {
                            continue;
                        }
                        let Ok(rel) = path.strip_prefix(&profile_dir) else { continue };
                        let rel_str = rel.to_string_lossy().replace('\\', "/");
                        if !indexed_paths.contains(&rel_str) {
                            conv_candidates.push((rel_str, path.to_path_buf()));
                        }
                    }
                }

                if public_root.is_dir() {
                    for entry in walkdir::WalkDir::new(&public_root)
                        .follow_links(false)
                        .into_iter()
                        .flatten()
                    {
                        let path = entry.path();
                        let Ok(meta) = fs::symlink_metadata(path) else { continue };
                        if !meta.is_file() || staging::is_link(&meta) {
                            continue;
                        }
                        let Ok(rel) = path.strip_prefix(&profile_dir) else { continue };
                        let rel_str = rel.to_string_lossy().replace('\\', "/");
                        if !indexed_public.contains(&rel_str) {
                            public_candidates.push((rel_str, path.to_path_buf()));
                        }
                    }
                }

                if let Ok(entries) = fs::read_dir(&tmp_root) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        let name = entry.file_name().to_string_lossy().into_owned();
                        let Ok(meta) = fs::symlink_metadata(&path) else { continue };
                        if meta.is_file() && !staging::is_link(&meta) {
                            tmp_candidates.push((name, path));
                        }
                    }
                }
                (conv_candidates, public_candidates, tmp_candidates)
            })
            .await?;

        let quarantine_root = self.quarantine_dir();
        for (rel_str, path) in conv_candidates {
            self.ensure_open()?;
            let _lock = self.media_locks[lock_slot(&rel_str)].lock().await;
            let indexed_now: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM media_files WHERE device_id = ?1 AND rel_path = ?2)
                     OR EXISTS(SELECT 1 FROM video_preview_partials WHERE device_id = ?1 AND rel_path = ?2)",
            )
            .bind(&device)
            .bind(&rel_str)
            .fetch_one(&self.db)
            .await
            .unwrap_or(true);
            if indexed_now {
                continue;
            }
            let reclaim: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM media_reclaims WHERE rel_path = ?)")
                .bind(&rel_str).fetch_one(&self.db).await?;
            if reclaim {
                drop(_lock);
                self.remove_rel_paths(vec![rel_str]).await;
                continue;
            }
            let qroot = quarantine_root.clone();
            let rel_for_q = rel_str.clone();
            if tokio::task::spawn_blocking(move || {
                move_to_quarantine(&path, &qroot, "unindexed", &rel_for_q)
            })
            .await?
            .is_ok()
            {
                report.quarantined_unknown += 1;
            }
            tokio::task::yield_now().await;
        }

        for (rel_str, path) in public_candidates {
            self.ensure_open()?;
            let _lock = self.media_locks[lock_slot(&rel_str)].lock().await;
            let indexed_now: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM public_cache_files WHERE device_id = ?1 AND rel_path = ?2)",
            )
            .bind(&device)
            .bind(&rel_str)
            .fetch_one(&self.db)
            .await
            .unwrap_or(true);
            if indexed_now {
                continue;
            }
            if remove_file_if_exists(&path) > 0 || !path.exists() {
                report.removed_orphan_files += 1;
            }
            tokio::task::yield_now().await;
        }

        if !tmp_candidates.is_empty() {
            let active_tmp = Arc::clone(&self.active_tmp);
            let stale_tmp = tokio::task::spawn_blocking(move || {
                let mut removed = 0usize;
                for (name, path) in tmp_candidates {
                    if delete_if_inactive_tmp(&active_tmp, &name, &path).is_some() {
                        removed += 1;
                    }
                }
                removed
            })
            .await?;
            report.removed_stale_tmp += stale_tmp;
        }
        Ok(report)
    }

    pub(super) async fn migrate_legacy_media_if_needed(&self, previous_device_id: Option<String>) -> Result<()> {
        // Handle device re-registration on the same profile directory so local history keeps its media.
        if let Some(old_dev) = previous_device_id
            && old_dev != self.device_id.to_string()
        {
            self.migrate_device_media_scope(&old_dev).await?;
        }

        let done: Option<String> = sqlx::query_scalar("SELECT value FROM media_meta WHERE key = ?")
            .bind(MEDIA_MIGRATION_KEY)
            .fetch_optional(&self.db)
            .await?;
        if done.as_deref() == Some("done") {
            return Ok(());
        }

        let mut any_pending_retry = false;

        // 1. Migrate legacy <profile>/files/<attachment_id> entries.
        let legacy_local: Vec<(String, i64, String, i64)> = sqlx::query_as(
            "SELECT id, message_seq, sha256, size FROM local_files",
        )
        .fetch_all(&self.db)
        .await?;

        let mut handled_legacy_files = HashSet::new();
        for (att_id, seq, sha256, size) in legacy_local {
            handled_legacy_files.insert(att_id.clone());
            let inbox_row: Option<(String, String)> = sqlx::query_as(
                "SELECT conversation_id, payload FROM inbox
                 WHERE seq = ?1 AND attachment_id = ?2
                   AND json_extract(payload, '$.recalled') IS NOT 1
                   AND seq NOT IN (SELECT seq FROM hidden_messages)",
            )
            .bind(seq)
            .bind(&att_id)
            .fetch_optional(&self.db)
            .await?;
            let old_path = self.managed_files.join(&att_id);
            let Some((conv_str, payload)) = inbox_row else {
                continue;
            };
            let Ok(conv_id) = Uuid::parse_str(&conv_str) else { continue };
            let Ok(message) = serde_json::from_str::<Message>(&payload) else { continue };
            let Some(attachment) = message.attachment else { continue };
            if !old_path.exists() {
                continue;
            }
            let variant = MediaVariant::for_attachment(&attachment);
            let mime = attachment
                .preview_mime()
                .unwrap_or("application/octet-stream");
            let desc = MediaDescriptor {
                conversation_id: conv_id,
                message_seq: seq,
                attachment_id: Some(attachment.id),
                variant,
                content_key: &sha256,
                expected_sha256: &sha256,
                expected_size: size.max(0) as u64,
                mime,
                dimensions: None,
            };
            match self.commit_media_file(&desc, &old_path).await {
                Ok(_) => {
                    let _ = tokio::fs::remove_file(&old_path).await;
                    let _ = sqlx::query("DELETE FROM local_files WHERE id = ?1")
                        .bind(&att_id)
                        .execute(&self.db)
                        .await;
                }
                Err(_) => {
                    // Distinguish genuine corrupt source from transient write/index failure.
                    let check_path = old_path.clone();
                    let check_sha = sha256.clone();
                    let check_size = size.max(0) as u64;
                    let source_valid = tokio::task::spawn_blocking(move || {
                        verify_file_integrity(&check_path, check_size, &check_sha).is_ok()
                    })
                    .await
                    .unwrap_or(false);
                    if source_valid {
                        // Valid source failed to commit (e.g. disk full / DB failure): keep source and retry on next open.
                        any_pending_retry = true;
                    } else {
                        // Corrupt or unverified legacy file: protect in quarantine instead of deleting.
                        let quarantine = self.quarantine_dir();
                        let src = old_path.clone();
                        let hint = att_id.clone();
                        let _ = tokio::task::spawn_blocking(move || {
                            move_to_quarantine(&src, &quarantine, "legacy-files", &hint)
                        })
                        .await;
                    }
                }
            }
        }

        // Check any remaining files inside legacy <profile>/files/.
        let legacy_dir = fs_path(&self.managed_files);
        if legacy_dir.is_dir() {
            let leftovers: Vec<(String, PathBuf)> = tokio::task::spawn_blocking(move || {
                let mut found = Vec::new();
                if let Ok(entries) = fs::read_dir(&legacy_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if let Ok(meta) = fs::symlink_metadata(&path)
                            && meta.is_file()
                            && !staging::is_link(&meta)
                        {
                            found.push((entry.file_name().to_string_lossy().into_owned(), path));
                        }
                    }
                }
                found
            })
            .await?;
            for (name, path) in leftovers {
                if handled_legacy_files.contains(&name) && any_pending_retry {
                    continue;
                }
                let mut migrated = false;
                let mut keep_for_retry = false;
                if let Ok(att_uuid) = Uuid::parse_str(&name) {
                    let inbox_row: Option<(i64, String, String)> = sqlx::query_as(
                        "SELECT seq, conversation_id, payload FROM inbox
                         WHERE attachment_id = ?1
                           AND json_extract(payload, '$.recalled') IS NOT 1
                           AND seq NOT IN (SELECT seq FROM hidden_messages)",
                    )
                    .bind(att_uuid.to_string())
                    .fetch_optional(&self.db)
                    .await?;
                    if let Some((seq, conv_str, payload)) = inbox_row
                        && let Ok(conv_id) = Uuid::parse_str(&conv_str)
                        && let Ok(message) = serde_json::from_str::<Message>(&payload)
                        && let Some(attachment) = message.attachment
                    {
                        let variant = MediaVariant::for_attachment(&attachment);
                        let mime = attachment.preview_mime().unwrap_or("application/octet-stream");
                        let desc = MediaDescriptor {
                            conversation_id: conv_id,
                            message_seq: seq,
                            attachment_id: Some(attachment.id),
                            variant,
                            content_key: &attachment.sha256,
                            expected_sha256: &attachment.sha256,
                            expected_size: attachment.size,
                            mime,
                            dimensions: None,
                        };
                        match self.commit_media_file(&desc, &path).await {
                            Ok(_) => {
                                let _ = tokio::fs::remove_file(&path).await;
                                migrated = true;
                            }
                            Err(_) => {
                                let check_path = path.clone();
                                let check_sha = attachment.sha256.clone();
                                let check_size = attachment.size;
                                let source_valid = tokio::task::spawn_blocking(move || {
                                    verify_file_integrity(&check_path, check_size, &check_sha).is_ok()
                                })
                                .await
                                .unwrap_or(false);
                                if source_valid {
                                    keep_for_retry = true;
                                    any_pending_retry = true;
                                }
                            }
                        }
                    }
                }
                if !migrated && !keep_for_retry {
                    let quarantine = self.quarantine_dir();
                    let _ = tokio::task::spawn_blocking(move || {
                        move_to_quarantine(&path, &quarantine, "legacy-files", &name)
                    })
                    .await;
                }
            }
        }

        // 2. Migrate legacy SQLite media_previews (image original = -1, thumbnail = -2).
        // Delete each legacy row ONLY after its replacement file & index commit succeeds (or if message was recalled/hidden).
        let legacy_previews: Vec<(i64, String, i64, String, Vec<u8>)> = sqlx::query_as(
            "SELECT message_seq, sha256, variant, mime, bytes FROM media_previews WHERE variant IN (-1, -2)",
        )
        .fetch_all(&self.db)
        .await
        .unwrap_or_default();

        for (seq, sha256, variant_id, mime, bytes) in legacy_previews {
            let dead: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM recall_tombstones WHERE message_seq = ?1)
                     OR EXISTS(SELECT 1 FROM hidden_messages WHERE seq = ?1)
                     OR EXISTS(SELECT 1 FROM inbox WHERE seq = ?1 AND json_extract(payload, '$.recalled') = 1)",
            )
            .bind(seq)
            .fetch_one(&self.db)
            .await
            .unwrap_or(false);
            if dead {
                let _ = sqlx::query("DELETE FROM media_previews WHERE message_seq = ?1 AND variant = ?2")
                    .bind(seq)
                    .bind(variant_id)
                    .execute(&self.db)
                    .await;
                continue;
            }
            let inbox_row: Option<(String, Option<String>, String)> = sqlx::query_as(
                "SELECT conversation_id, attachment_id, payload FROM inbox
                 WHERE seq = ?1
                   AND json_extract(payload, '$.recalled') IS NOT 1
                   AND seq NOT IN (SELECT seq FROM hidden_messages)",
            )
            .bind(seq)
            .fetch_optional(&self.db)
            .await?;
            let Some((conv_str, att_str, payload)) = inbox_row else {
                // Unknown attribution: preserve legacy row without deleting.
                continue;
            };
            let Ok(conv_id) = Uuid::parse_str(&conv_str) else { continue };
            let Ok(message) = serde_json::from_str::<Message>(&payload) else { continue };
            let Some(attachment) = message.attachment else { continue };
            if attachment.sha256 != sha256 {
                continue;
            }
            let att_id = att_str.and_then(|s| Uuid::parse_str(&s).ok()).or(Some(attachment.id));
            if variant_id == -1 {
                if bytes.len() as u64 == attachment.size
                    && format!("{:x}", Sha256::digest(&bytes)) == attachment.sha256
                {
                    let desc = MediaDescriptor {
                        conversation_id: conv_id,
                        message_seq: seq,
                        attachment_id: att_id,
                        variant: MediaVariant::ImageOriginal,
                        content_key: &attachment.sha256,
                        expected_sha256: &attachment.sha256,
                        expected_size: attachment.size,
                        mime: &mime,
                        dimensions: None,
                    };
                    if self.commit_media_bytes(&desc, &bytes).await.is_ok() {
                        let _ = sqlx::query(
                            "DELETE FROM media_previews WHERE message_seq = ?1 AND variant = ?2",
                        )
                        .bind(seq)
                        .bind(variant_id)
                        .execute(&self.db)
                        .await;
                    } else {
                        any_pending_retry = true;
                    }
                }
            } else if variant_id == -2
                && let Some(dims) = png_dimensions(&bytes)
            {
                let thumb_sha = format!("{:x}", Sha256::digest(&bytes));
                let desc = MediaDescriptor {
                    conversation_id: conv_id,
                    message_seq: seq,
                    attachment_id: att_id,
                    variant: MediaVariant::ImageThumbnail,
                    content_key: &attachment.sha256,
                    expected_sha256: &thumb_sha,
                    expected_size: bytes.len() as u64,
                    mime: "image/png",
                    dimensions: Some(dims),
                };
                if self.commit_media_bytes(&desc, &bytes).await.is_ok() {
                    let _ = sqlx::query(
                        "DELETE FROM media_previews WHERE message_seq = ?1 AND variant = ?2",
                    )
                    .bind(seq)
                    .bind(variant_id)
                    .execute(&self.db)
                    .await;
                } else {
                    any_pending_retry = true;
                }
            }
        }

        // 3. Migrate legacy SQLite cached_profile_images to public cache files.
        let now = unix_millis();
        let _ = sqlx::query("DELETE FROM cached_profile_images WHERE expires_at <= ?1")
            .bind(now)
            .execute(&self.db)
            .await;
        let legacy_pub: Vec<(String, String, Vec<u8>, i64)> = sqlx::query_as(
            "SELECT user_id, version, data, expires_at FROM cached_profile_images WHERE expires_at > ?1",
        )
        .bind(now)
        .fetch_all(&self.db)
        .await
        .unwrap_or_default();

        for (user_str, ver_str, data, expires_at) in legacy_pub {
            if let (Ok(user_id), Ok(version)) = (Uuid::parse_str(&user_str), Uuid::parse_str(&ver_str)) {
                let is_avatar: bool = sqlx::query_scalar(
                    "SELECT EXISTS(
                        SELECT 1 FROM directory WHERE json_extract(payload, '$.avatars.\"' || ?1 || '\"') = ?2
                        UNION ALL
                        SELECT 1 FROM cached_profiles WHERE user_id = ?1 AND json_extract(payload, '$.avatar') = ?2
                    )",
                )
                .bind(&user_str)
                .bind(&ver_str)
                .fetch_one(&self.db)
                .await
                .unwrap_or(false);
                let is_cover: bool = sqlx::query_scalar(
                    "SELECT EXISTS(
                        SELECT 1 FROM cached_profiles WHERE user_id = ?1 AND json_extract(payload, '$.cover') = ?2
                    )",
                )
                .bind(&user_str)
                .bind(&ver_str)
                .fetch_one(&self.db)
                .await
                .unwrap_or(false);
                if !is_avatar && !is_cover {
                    // Unknown attribution: preserve legacy row without deleting.
                    continue;
                }
                let mut ok = true;
                if is_avatar
                    && self
                        .commit_public_image(user_id, PublicResourceKind::Avatar, version, &data, expires_at)
                        .await
                        .is_err()
                {
                    ok = false;
                }
                if is_cover
                    && self
                        .commit_public_image(user_id, PublicResourceKind::Cover, version, &data, expires_at)
                        .await
                        .is_err()
                {
                    ok = false;
                }
                if ok {
                    let _ = sqlx::query(
                        "DELETE FROM cached_profile_images WHERE user_id = ?1 AND version = ?2",
                    )
                    .bind(&user_str)
                    .bind(&ver_str)
                    .execute(&self.db)
                    .await;
                } else {
                    any_pending_retry = true;
                }
            }
        }

        if !any_pending_retry {
            sqlx::query("INSERT INTO media_meta (key, value) VALUES (?1, 'done') ON CONFLICT(key) DO UPDATE SET value = 'done'")
                .bind(MEDIA_MIGRATION_KEY)
                .execute(&self.db)
                .await?;
        }
        Ok(())
    }

    async fn migrate_device_media_scope(&self, old_device_id: &str) -> Result<()> {
        let new_device_id = self.device_id.to_string();
        let old_prefix = format!("media/{old_device_id}/");
        let new_prefix = self.device_media_prefix();
        let rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT id, rel_path FROM media_files WHERE device_id = ?1",
        )
        .bind(old_device_id)
        .fetch_all(&self.db)
        .await?;
        for (id, old_rel) in rows {
            let Some(suffix) = old_rel.strip_prefix(&old_prefix) else { continue };
            let new_rel = format!("{new_prefix}{suffix}");
            let Ok(src) = safe_rel_path(self.profile_dir(), &old_rel, &old_prefix) else { continue };
            let Ok(dst) = self.resolve_device_media_path(&new_rel) else { continue };
            let moved = tokio::task::spawn_blocking(move || -> Result<()> {
                if src.is_file() {
                    if let Some(parent) = dst.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    if !dst.exists() && fs::rename(&src, &dst).is_err() {
                        fs::copy(&src, &dst)?;
                        let _ = fs::remove_file(&src);
                    }
                }
                Ok(())
            })
            .await?;
            if moved.is_ok() {
                let _ = sqlx::query(
                    "UPDATE OR IGNORE media_files SET device_id = ?1, rel_path = ?2, updated_at = ?3 WHERE id = ?4",
                )
                .bind(&new_device_id)
                .bind(&new_rel)
                .bind(unix_millis())
                .bind(id)
                .execute(&self.db)
                .await;
            }
        }
        Ok(())
    }

    pub(super) async fn commit_public_image(
        &self,
        user_id: Uuid,
        kind: PublicResourceKind,
        version: Uuid,
        bytes: &[u8],
        expires_at: i64,
    ) -> Result<()> {
        self.ensure_open()?;
        ensure!(
            !bytes.is_empty() && bytes.len() <= MAX_PROFILE_IMAGE_BYTES,
            "Invalid profile image size"
        );
        let rel_path = self.public_rel_path(kind, version);
        let lock = self.media_locks[lock_slot(&rel_path)].lock().await;
        let final_path = self.resolve_public_path(&rel_path)?;
        let tmp_dir = self.tmp_dir();
        let payload = bytes.to_vec();
        let expected_sha = format!("{:x}", Sha256::digest(&payload));
        let expected_size = payload.len() as u64;
        let sha_for_io = expected_sha.clone();
        let target_clone = final_path.clone();

        let created_fresh = {
            let tmp_name = format!("public-{}.tmp", Uuid::new_v4().simple());
            let _guard = ActiveTempGuard::track(&self.active_tmp, tmp_name.clone());
            tokio::task::spawn_blocking(move || -> Result<bool> {
                if let Some(parent) = target_clone.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::create_dir_all(&tmp_dir)?;
                if verify_file_integrity(&target_clone, expected_size, &sha_for_io).is_ok() {
                    return Ok(false);
                }
                let mut temp = Builder::new()
                    .prefix(&tmp_name)
                    .suffix(".part")
                    .tempfile_in(&tmp_dir)
                    .map_err(super::transfer::save_io_error)?;
                temp.write_all(&payload).map_err(super::transfer::save_io_error)?;
                temp.as_file().sync_all().map_err(super::transfer::save_io_error)?;
                let existed = target_clone.symlink_metadata().is_ok();
                if existed {
                    temp.persist(&target_clone)
                        .map_err(|error| super::transfer::save_io_error(error.error))?;
                } else {
                    temp.persist_noclobber(&target_clone)
                        .map_err(|error| super::transfer::save_io_error(error.error))?;
                }
                Ok(!existed)
            })
            .await??
        };

        let now = unix_millis();
        let device = self.device_id.to_string();
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        let upsert = sqlx::query_scalar::<_, i64>(
            "INSERT INTO public_cache_files (
                device_id, kind, version, sha256, size, mime, rel_path, status,
                created_at, touched_at, expires_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'image/png', ?6, 'available', ?7, ?7, ?8)
             ON CONFLICT (device_id, kind, version) DO UPDATE SET
                sha256 = excluded.sha256,
                size = excluded.size,
                rel_path = excluded.rel_path,
                status = 'available',
                touched_at = excluded.touched_at,
                expires_at = max(public_cache_files.expires_at, excluded.expires_at)
             RETURNING id",
        )
        .bind(&device)
        .bind(kind.as_str())
        .bind(version.to_string())
        .bind(&expected_sha)
        .bind(expected_size as i64)
        .bind(&rel_path)
        .bind(now)
        .bind(expires_at)
        .fetch_one(&mut *tx)
        .await;

        let file_id = match upsert {
            Ok(id) => id,
            Err(error) => {
                if created_fresh {
                    let _ = tokio::fs::remove_file(&final_path).await;
                }
                return Err(error.into());
            }
        };

        // Atomically switch active reference to the verified new version.
        sqlx::query(
            "INSERT INTO public_cache_refs (user_id, kind, active_file_id, active_version, desired_version, updated_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6)
             ON CONFLICT (user_id, kind) DO UPDATE SET
                active_file_id = excluded.active_file_id,
                active_version = excluded.active_version,
                desired_version = excluded.desired_version,
                updated_at = excluded.updated_at,
                expires_at = max(public_cache_refs.expires_at, excluded.expires_at)",
        )
        .bind(user_id.to_string())
        .bind(kind.as_str())
        .bind(file_id)
        .bind(version.to_string())
        .bind(now)
        .bind(expires_at)
        .execute(&mut *tx)
        .await?;

        // Automatically reclaim confirmed unreferenced old public cache files.
        let unreferenced: Vec<String> = sqlx::query_scalar(
            "DELETE FROM public_cache_files
             WHERE device_id = ?1
               AND id <> ?2
               AND id NOT IN (
                   SELECT active_file_id FROM public_cache_refs WHERE active_file_id IS NOT NULL
               )
             RETURNING rel_path",
        )
        .bind(&device)
        .bind(file_id)
        .fetch_all(&mut *tx)
        .await?;

        if let Err(error) = tx.commit().await {
            if created_fresh {
                let _ = tokio::fs::remove_file(&final_path).await;
            }
            return Err(error.into());
        }
        drop(lock);
        self.remove_rel_paths(unreferenced).await;
        Ok(())
    }

    pub(super) async fn read_public_image_file(&self, file_id: i64, rel_path: &str, size: i64, sha256: &str) -> Option<Vec<u8>> {
        let path = match self.resolve_public_path(rel_path) {
            Ok(path) => path,
            Err(_) => {
                let _ = sqlx::query("UPDATE public_cache_files SET status = 'corrupted' WHERE id = ?")
                    .bind(file_id)
                    .execute(&self.db)
                    .await;
                return None;
            }
        };
        let sha = sha256.to_owned();
        let expected_size = size.max(0) as u64;
        match tokio::task::spawn_blocking(move || read_verified_bytes(&path, expected_size, &sha))
            .await
            .ok()?
        {
            Ok(bytes) => {
                let _ = sqlx::query("UPDATE public_cache_files SET touched_at = ?1 WHERE id = ?2")
                    .bind(unix_millis())
                    .bind(file_id)
                    .execute(&self.db)
                    .await;
                Some(bytes)
            }
            Err(failure) => {
                let _ = sqlx::query("UPDATE public_cache_files SET status = ?1 WHERE id = ?2")
                    .bind(failure.status_str())
                    .bind(file_id)
                    .execute(&self.db)
                    .await;
                None
            }
        }
    }
}

fn delete_if_inactive_tmp(
    active_tmp: &std::sync::Mutex<HashSet<String>>,
    name: &str,
    path: &Path,
) -> Option<u64> {
    let active = active_tmp.lock().expect("active temp files");
    if active.iter().any(|prefix| name.starts_with(prefix)) {
        return None;
    }
    let bytes = remove_file_if_exists(path);
    if bytes > 0 || !path.exists() {
        Some(bytes)
    } else {
        None
    }
}

async fn conversation_joined_after_in_tx(
    db: &mut SqliteConnection,
    conversation_id: Uuid,
) -> Result<Option<Option<i64>>> {
    let has_directory: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM directory WHERE id = 1)")
            .fetch_one(&mut *db)
            .await?;
    if !has_directory {
        return Ok(None);
    }
    let row: Option<(Option<i64>,)> = sqlx::query_as(
        "SELECT json_extract(c.value, '$.joined_after')
         FROM directory d, json_each(d.payload, '$.conversations') c
         WHERE d.id = 1 AND json_extract(c.value, '$.id') = ?1",
    )
    .bind(conversation_id.to_string())
    .fetch_optional(&mut *db)
    .await?;
    Ok(Some(row.and_then(|(cutoff,)| cutoff)))
}

async fn bind_same_conversation_duplicates_in_tx(
    db: &mut SqliteConnection,
    file_id: i64,
    conversation_id: Uuid,
    variant: MediaVariant,
    content_key: &str,
    exclude_seq: Option<i64>,
) -> Result<()> {
    let cutoff = match conversation_joined_after_in_tx(&mut *db, conversation_id).await? {
        None => None,
        Some(Some(joined_after)) => Some(joined_after),
        Some(None) => return Ok(()),
    };
    let conv_str = conversation_id.to_string();
    let rows: Vec<(i64, Option<String>, String)> = sqlx::query_as(
        "SELECT seq, attachment_id, payload FROM inbox
         WHERE conversation_id = ?1
           AND attachment_id IS NOT NULL
           AND json_extract(payload, '$.attachment.sha256') = ?2
           AND json_extract(payload, '$.recalled') IS NOT 1
           AND seq NOT IN (SELECT seq FROM hidden_messages)
           AND seq NOT IN (SELECT message_seq FROM recall_tombstones)",
    )
    .bind(&conv_str)
    .bind(content_key)
    .fetch_all(&mut *db)
    .await?;
    let now = unix_millis();
    for (seq, att_str, payload) in rows {
        if exclude_seq == Some(seq) {
            continue;
        }
        if let Some(joined_after) = cutoff
            && seq <= joined_after
        {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Message>(&payload) else { continue };
        if message.recalled {
            continue;
        }
        let Some(ref attachment) = message.attachment else { continue };
        if attachment.sha256 != content_key || !variant.is_compatible_with_attachment(attachment) {
            continue;
        }
        sqlx::query(
            "INSERT OR IGNORE INTO media_refs (file_id, ref_kind, ref_id, conversation_id, attachment_id, variant, created_at)
             VALUES (?1, 'message', ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(file_id)
        .bind(seq.to_string())
        .bind(&conv_str)
        .bind(att_str.or_else(|| Some(attachment.id.to_string())))
        .bind(variant.as_str())
        .bind(now)
        .execute(&mut *db)
        .await?;
    }
    Ok(())
}

pub(super) async fn bind_message_to_existing_media_in_tx(
    db: &mut SqliteConnection,
    conversation_id: Uuid,
    message_seq: i64,
    attachment: &Attachment,
) -> Result<()> {
    let blocked: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM hidden_messages WHERE seq = ?1)
             OR EXISTS(SELECT 1 FROM recall_tombstones WHERE message_seq = ?1)",
    )
    .bind(message_seq)
    .fetch_one(&mut *db)
    .await?;
    if blocked {
        return Ok(());
    }
    match conversation_joined_after_in_tx(&mut *db, conversation_id).await? {
        None => {}
        Some(Some(joined_after)) if message_seq > joined_after => {}
        Some(_) => return Ok(()),
    }
    let conv_str = conversation_id.to_string();
    let files: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, variant FROM media_files
         WHERE conversation_id = ?1 AND content_key = ?2 AND status = 'available'",
    )
    .bind(&conv_str)
    .bind(&attachment.sha256)
    .fetch_all(&mut *db)
    .await?;
    let now = unix_millis();
    for (file_id, variant_str) in files {
        let Some(variant) = MediaVariant::from_str(&variant_str) else { continue };
        if !variant.is_compatible_with_attachment(attachment) {
            continue;
        }
        sqlx::query(
            "INSERT OR IGNORE INTO media_refs (file_id, ref_kind, ref_id, conversation_id, attachment_id, variant, created_at)
             VALUES (?1, 'message', ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(file_id)
        .bind(message_seq.to_string())
        .bind(&conv_str)
        .bind(attachment.id.to_string())
        .bind(variant.as_str())
        .bind(now)
        .execute(&mut *db)
        .await?;
    }
    Ok(())
}

pub(super) async fn release_media_refs_in_tx(
    db: &mut SqliteConnection,
    ref_kind: &str,
    ref_id: &str,
) -> Result<Vec<String>> {
    let candidate_files: Vec<i64> = sqlx::query_scalar(
        "DELETE FROM media_refs WHERE ref_kind = ?1 AND ref_id = ?2 RETURNING file_id",
    )
    .bind(ref_kind)
    .bind(ref_id)
    .fetch_all(&mut *db)
    .await?;
    if candidate_files.is_empty() {
        return Ok(Vec::new());
    }
    let excluded_seq = if ref_kind == "message" {
        ref_id.parse::<i64>().ok()
    } else {
        None
    };
    let mut removed_paths = Vec::new();
    let unique: HashSet<i64> = candidate_files.into_iter().collect();
    for file_id in unique {
        let mut remaining: i64 =
            sqlx::query_scalar("SELECT count(*) FROM media_refs WHERE file_id = ?1")
                .bind(file_id)
                .fetch_one(&mut *db)
                .await?;
        if remaining == 0 {
            let meta: Option<(String, String, String, String)> = sqlx::query_as(
                "SELECT conversation_id, variant, content_key, status FROM media_files WHERE id = ?1",
            )
            .bind(file_id)
            .fetch_optional(&mut *db)
            .await?;
            if let Some((conv_str, variant_str, content_key, status)) = meta
                && status == "available"
                && let Ok(conv_id) = Uuid::parse_str(&conv_str)
                && let Some(variant) = MediaVariant::from_str(&variant_str)
            {
                bind_same_conversation_duplicates_in_tx(
                    &mut *db,
                    file_id,
                    conv_id,
                    variant,
                    &content_key,
                    excluded_seq,
                )
                .await?;
                remaining =
                    sqlx::query_scalar("SELECT count(*) FROM media_refs WHERE file_id = ?1")
                        .bind(file_id)
                        .fetch_one(&mut *db)
                        .await?;
            }
        }
        if remaining == 0
            && let Some(rel_path) = sqlx::query_scalar::<_, String>(
                "DELETE FROM media_files WHERE id = ?1 RETURNING rel_path",
            )
            .bind(file_id)
            .fetch_optional(&mut *db)
            .await?
        {
            sqlx::query("INSERT OR IGNORE INTO media_reclaims (rel_path) VALUES (?)")
                .bind(&rel_path).execute(&mut *db).await?;
            removed_paths.push(rel_path);
        }
    }
    Ok(removed_paths)
}

pub(super) async fn cleanup_unreferenced_video_partials_in_tx(
    db: &mut SqliteConnection,
    conversation_id: &str,
    media_paths: &mut Vec<String>,
) -> Result<()> {
    let stale: Vec<(String, String)> = sqlx::query_as(
        "SELECT content_key, rel_path FROM video_preview_partials p
         WHERE p.conversation_id = ?1
           AND NOT EXISTS (
               SELECT 1 FROM preview_eligible e
               WHERE e.conversation_id = p.conversation_id AND e.sha256 = p.content_key
           )",
    )
    .bind(conversation_id)
    .fetch_all(&mut *db)
    .await?;
    for (content_key, rel_path) in stale {
        sqlx::query(
            "DELETE FROM video_preview_partials WHERE conversation_id = ?1 AND content_key = ?2",
        )
        .bind(conversation_id)
        .bind(&content_key)
        .execute(&mut *db)
        .await?;
        sqlx::query("INSERT OR IGNORE INTO media_reclaims (rel_path) VALUES (?)")
            .bind(&rel_path)
            .execute(&mut *db)
            .await?;
        media_paths.push(rel_path);
    }
    Ok(())
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Api;
    use crate::{AttachmentKind, DeviceSession, Direction, TransferStatus, User};

    fn sha256_hex(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn sample_png(width: u32, height: u32, salt: u8) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(32);
        bytes.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        bytes.extend_from_slice(&13u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0, salt, salt.wrapping_add(1)]);
        bytes
    }

    async fn setup_directory(client: &Client, conversations: &[(Uuid, Option<i64>)]) -> Result<()> {
        let chats: Vec<_> = conversations
            .iter()
            .map(|(id, cutoff)| {
                serde_json::json!({
                    "id": id,
                    "members": [&client.user],
                    "activity_at": 1,
                    "joined_after": cutoff
                })
            })
            .collect();
        sqlx::query("INSERT INTO directory (id, payload) VALUES (1, ?) ON CONFLICT(id) DO UPDATE SET payload = excluded.payload")
            .bind(serde_json::json!({ "contacts": [], "conversations": chats }).to_string())
            .execute(&client.db)
            .await?;
        Ok(())
    }

    async fn insert_attachment_message(
        client: &Client,
        seq: i64,
        conversation: Uuid,
        name: &str,
        bytes: &[u8],
        expires_at: Option<i64>,
    ) -> Result<Message> {
        let att_id = Uuid::new_v4();
        let message = Message {
            seq,
            conversation_id: conversation,
            sender: client.user.clone(),
            created_at: seq * 1000,
            text: None,
            attachment: Some(Attachment {
                id: att_id,
                name: name.into(),
                kind: AttachmentKind::File,
                size: bytes.len() as u64,
                sha256: sha256_hex(bytes),
            }),
            ai_provider: None,
            published_at: None,
            ai_sources: Vec::new(),
            recalled: false,
            reply_to: None,
            nonce: None,
            expires_at,
            expired: None,
            pending: None,
            local_file: false,
        };
        sqlx::query(
            "INSERT INTO inbox (seq, conversation_id, attachment_id, payload) VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(seq)
        .bind(conversation.to_string())
        .bind(att_id.to_string())
        .bind(serde_json::to_string(&message)?)
        .execute(&client.db)
        .await?;
        Ok(message)
    }

    #[tokio::test]
    async fn local_deletion_preserves_unreceived_sequence_gaps_after_restart() -> Result<()> {
        let root = tempfile::tempdir()?;
        let user = User { id: Uuid::new_v4(), handle: "gap".into() };
        let device_id = Uuid::new_v4();
        let cursor = Uuid::new_v4();
        let session = || DeviceSession { user: user.clone(), device_id, cursor };
        let api = || Api::new("http://127.0.0.1:7841");
        let client = Client::open(root.path(), api()?, session(), false).await?;
        let chat = Uuid::new_v4();
        setup_directory(&client, &[(chat, Some(0))]).await?;
        let first = insert_attachment_message(&client, 101, chat, "first.txt", b"first", None).await?;
        let third = insert_attachment_message(&client, 103, chat, "third.txt", b"third", None).await?;
        let mut missing = first.clone();
        missing.seq = 102;
        missing.attachment.as_mut().unwrap().id = Uuid::new_v4();
        client.clear_received_history(chat).await?;
        assert!(client.messages(chat, None).await?.items.is_empty());
        let legacy = insert_attachment_message(&client, 104, chat, "legacy.txt", b"legacy", None).await?;
        sqlx::query("INSERT INTO hidden_messages (seq, attachment_id) VALUES (?,?)")
            .bind(legacy.seq).bind(legacy.attachment.as_ref().unwrap().id.to_string())
            .execute(&client.db).await?;
        client.close().await;
        drop(client);
        let client = Client::open(root.path(), api()?, session(), false).await?;
        let mut tx = client.db.begin_with("BEGIN IMMEDIATE").await?;
        assert!(!super::super::local_sync::persist_message(&mut tx, first).await?);
        assert!(!super::super::local_sync::persist_message(&mut tx, third).await?);
        assert!(!super::super::local_sync::persist_message(&mut tx, legacy).await?);
        assert!(super::super::local_sync::persist_message(&mut tx, missing).await?);
        tx.commit().await?;
        let items = client.messages(chat, None).await?.items;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].seq, 102);
        client.hide_message(chat, 102).await?;
        client.hide_message(chat, 102).await?;
        assert!(client.messages(chat, None).await?.items.is_empty());
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM local_deleted_messages")
            .fetch_one(&client.db).await?;
        assert_eq!(count, 4);
        let legacy_count: i64 = sqlx::query_scalar("SELECT count(*) FROM hidden_messages")
            .fetch_one(&client.db).await?;
        assert_eq!(legacy_count, 0);
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn local_deletion_recovers_committed_media_reclaim_on_restart() -> Result<()> {
        let root = tempfile::tempdir()?;
        let user = User { id: Uuid::new_v4(), handle: "reclaim".into() };
        let device_id = Uuid::new_v4();
        let cursor = Uuid::new_v4();
        let session = || DeviceSession { user: user.clone(), device_id, cursor };
        let api = || Api::new("http://127.0.0.1:7841");
        let client = Client::open(root.path(), api()?, session(), false).await?;
        let chat = Uuid::new_v4();
        setup_directory(&client, &[(chat, Some(0))]).await?;
        let bytes = sample_png(96, 96, 5);
        let msg = insert_attachment_message(&client, 101, chat, "reclaim.png", &bytes, None).await?;
        let file = msg.attachment.as_ref().unwrap();
        let sha = sha256_hex(&bytes);
        client.remember_image_variant(chat, msg.seq, Some(file.id), &sha, false, "image/png", &bytes).await?;
        let path = client.media_root().join("conversations").join(chat.to_string()).join("images").join(format!("orig_{sha}.png"));
        assert!(path.is_file());
        let mut tx = client.db.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query("INSERT INTO local_deleted_messages (seq, conversation_id, attachment_id) VALUES (?,?,?)")
            .bind(msg.seq).bind(chat.to_string()).bind(file.id.to_string()).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM inbox WHERE seq = ?").bind(msg.seq).execute(&mut *tx).await?;
        let paths = release_media_refs_in_tx(&mut tx, "message", &msg.seq.to_string()).await?;
        assert_eq!(paths.len(), 1);
        tx.commit().await?;
        // Simulate termination between the committed deletion and physical unlink.
        assert!(path.is_file());
        client.close().await;
        drop(client);
        let client = Client::open(root.path(), api()?, session(), false).await?;
        assert!(!path.exists());
        let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM media_reclaims").fetch_one(&client.db).await?;
        assert_eq!(pending, 0);
        assert!(client.messages(chat, None).await?.items.is_empty());
        client.close().await;
        Ok(())
    }

    #[cfg(windows)]
    fn windows_file_info(path: &Path) -> Result<(u32, u64, u32)> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };
        let file = File::open(path)?;
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        ensure!(unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } != 0);
        let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
        Ok((info.dwVolumeSerialNumber, index, info.nNumberOfLinks))
    }

    #[tokio::test]
    async fn cross_conversation_same_sha256_has_independent_physical_files_and_delete_isolation() -> Result<()> {
        let root = tempfile::tempdir()?;
        let session = DeviceSession {
            user: User { id: Uuid::new_v4(), handle: "alice".into() },
            device_id: Uuid::new_v4(),
            cursor: Uuid::new_v4(),
        };
        let client = Client::open(root.path(), Api::new("http://127.0.0.1:7841")?, session, false).await?;
        let conv_a = Uuid::new_v4();
        let conv_b = Uuid::new_v4();
        setup_directory(&client, &[(conv_a, Some(0)), (conv_b, Some(0))]).await?;

        let img = sample_png(200, 150, 7);
        let thumb = sample_png(120, 90, 9);
        let msg_a = insert_attachment_message(&client, 1, conv_a, "photo.png", &img, Some(i64::MAX)).await?;
        let msg_b = insert_attachment_message(&client, 2, conv_b, "photo.png", &img, Some(i64::MAX)).await?;
        let sha = sha256_hex(&img);

        client.remember_image_variant(conv_a, 1, Some(msg_a.attachment.as_ref().unwrap().id), &sha, false, "image/png", &img).await?;
        client.remember_image_variant(conv_a, 1, Some(msg_a.attachment.as_ref().unwrap().id), &sha, true, "image/png", &thumb).await?;
        client.remember_image_variant(conv_b, 2, Some(msg_b.attachment.as_ref().unwrap().id), &sha, false, "image/png", &img).await?;
        client.remember_image_variant(conv_b, 2, Some(msg_b.attachment.as_ref().unwrap().id), &sha, true, "image/png", &thumb).await?;

        let path_a = fs_path(&client.media_root().join("conversations").join(conv_a.to_string()).join("images").join(format!("orig_{sha}.png")));
        let path_b = fs_path(&client.media_root().join("conversations").join(conv_b.to_string()).join("images").join(format!("orig_{sha}.png")));
        assert!(path_a.is_file());
        assert!(path_b.is_file());
        assert_ne!(path_a, path_b);

        #[cfg(windows)]
        {
            let info_a = windows_file_info(&path_a)?;
            let info_b = windows_file_info(&path_b)?;
            assert_eq!(info_a.2, 1, "no hard links allowed");
            assert_eq!(info_b.2, 1, "no hard links allowed");
            assert_ne!((info_a.0, info_a.1), (info_b.0, info_b.1), "must be independent physical files");
        }

        // Delete message in conversation A -> A's physical files removed, B stays readable offline.
        client.hide_message(conv_a, 1).await?;
        assert!(!path_a.exists());
        assert!(path_b.is_file());
        assert!(client.cached_image_variant(conv_a, 1, Some(msg_a.attachment.as_ref().unwrap().id), &sha, false).await?.is_none());
        assert_eq!(
            client.cached_image_variant(conv_b, 2, Some(msg_b.attachment.as_ref().unwrap().id), &sha, false).await?.map(|(_, b)| b),
            Some(img)
        );
        assert_eq!(
            client.cached_thumbnail_sizes(std::slice::from_ref(&msg_b)).await?.get(&2),
            Some(&(120, 90))
        );
        Ok(())
    }

    #[tokio::test]
    async fn same_conversation_dedup_keeps_one_physical_file_until_last_ref_released() -> Result<()> {
        let root = tempfile::tempdir()?;
        let session = DeviceSession {
            user: User { id: Uuid::new_v4(), handle: "bob".into() },
            device_id: Uuid::new_v4(),
            cursor: Uuid::new_v4(),
        };
        let client = Client::open(root.path(), Api::new("http://127.0.0.1:7841")?, session, false).await?;
        let conv = Uuid::new_v4();
        setup_directory(&client, &[(conv, Some(0))]).await?;

        let img = sample_png(300, 200, 11);
        let thumb = sample_png(150, 100, 12);
        let sha = sha256_hex(&img);
        let msg1 = insert_attachment_message(&client, 10, conv, "a.png", &img, None).await?;
        let msg2 = insert_attachment_message(&client, 20, conv, "b.png", &img, None).await?;
        let msg3 = insert_attachment_message(&client, 30, conv, "c.png", &img, None).await?;

        client.remember_image_variant(conv, 10, Some(msg1.attachment.as_ref().unwrap().id), &sha, false, "image/png", &img).await?;
        client.remember_image_variant(conv, 10, Some(msg1.attachment.as_ref().unwrap().id), &sha, true, "image/png", &thumb).await?;
        client.remember_image_variant(conv, 20, Some(msg2.attachment.as_ref().unwrap().id), &sha, false, "image/png", &img).await?;
        client.remember_image_variant(conv, 20, Some(msg2.attachment.as_ref().unwrap().id), &sha, true, "image/png", &thumb).await?;

        // msg3 auto-binds via same-conversation dedup on read!
        assert_eq!(
            client.cached_image_variant(conv, 30, Some(msg3.attachment.as_ref().unwrap().id), &sha, false).await?.map(|(_, b)| b),
            Some(img.clone())
        );
        assert_eq!(
            client.cached_image_variant(conv, 30, Some(msg3.attachment.as_ref().unwrap().id), &sha, true).await?.map(|(_, b)| b),
            Some(thumb.clone())
        );

        let files_count: i64 = sqlx::query_scalar("SELECT count(*) FROM media_files WHERE variant = 'image_original'")
            .fetch_one(&client.db).await?;
        let refs_count: i64 = sqlx::query_scalar("SELECT count(*) FROM media_refs WHERE variant = 'image_original'")
            .fetch_one(&client.db).await?;
        assert_eq!(files_count, 1);
        assert_eq!(refs_count, 3);

        let orig_path = fs_path(&client.media_root().join("conversations").join(conv.to_string()).join("images").join(format!("orig_{sha}.png")));
        let thumb_path = fs_path(&client.media_root().join("conversations").join(conv.to_string()).join("images").join(format!("thumb_{sha}.png")));
        assert!(orig_path.is_file());
        assert!(thumb_path.is_file());

        // Delete 1st and 2nd message -> physical file stays for 3rd message.
        client.hide_message(conv, 10).await?;
        assert!(orig_path.is_file());
        assert!(thumb_path.is_file());
        client.hide_message(conv, 20).await?;
        assert!(orig_path.is_file());
        assert!(thumb_path.is_file());
        assert_eq!(
            client.cached_image_variant(conv, 30, Some(msg3.attachment.as_ref().unwrap().id), &sha, false).await?.map(|(_, b)| b),
            Some(img)
        );

        // Delete final reference -> physical files & DB rows reclaimed.
        client.hide_message(conv, 30).await?;
        assert!(!orig_path.exists());
        assert!(!thumb_path.exists());
        let remaining_files: i64 = sqlx::query_scalar("SELECT count(*) FROM media_files").fetch_one(&client.db).await?;
        assert_eq!(remaining_files, 0);
        Ok(())
    }

    #[tokio::test]
    async fn restart_reuses_cache_survives_server_ttl_enforces_scope_and_separates_variants() -> Result<()> {
        let root = tempfile::tempdir()?;
        let user_id = Uuid::new_v4();
        let device_id = Uuid::new_v4();
        let session = |account| DeviceSession {
            user: User { id: account, handle: "carol".into() },
            device_id,
            cursor: Uuid::new_v4(),
        };
        let service = || Api::new("http://127.0.0.1:7841");
        let client = Client::open(root.path(), service()?, session(user_id), false).await?;
        let conv = Uuid::new_v4();
        setup_directory(&client, &[(conv, Some(0))]).await?;

        let img = sample_png(160, 120, 3);
        let thumb = sample_png(80, 60, 4);
        let sha = sha256_hex(&img);
        let img_msg = insert_attachment_message(&client, 1, conv, "pic.png", &img, Some(i64::MAX)).await?;
        let img_att_id = img_msg.attachment.as_ref().unwrap().id;
        client.remember_image_variant(conv, 1, Some(img_att_id), &sha, false, "image/png", &img).await?;
        client.remember_image_variant(conv, 1, Some(img_att_id), &sha, true, "image/png", &thumb).await?;

        // Video preview and cover must NOT impersonate VideoOriginal (local_file stays false!).
        let video_orig = b"original-full-video-bytes";
        let video_preview = b"compressed-preview-bytes";
        let video_cover = sample_png(320, 180, 5);
        let vid_msg = insert_attachment_message(&client, 2, conv, "clip.mp4", video_orig, Some(i64::MAX)).await?;
        let vid_att_id = vid_msg.attachment.as_ref().unwrap().id;
        let preview_sha = sha256_hex(video_preview);
        let cover_sha = sha256_hex(&video_cover);
        client.commit_media_bytes(&MediaDescriptor {
            conversation_id: conv,
            message_seq: 2,
            attachment_id: Some(vid_att_id),
            variant: MediaVariant::VideoCover,
            content_key: &cover_sha,
            expected_sha256: &cover_sha,
            expected_size: video_cover.len() as u64,
            mime: "image/png",
            dimensions: Some((320, 180)),
        }, &video_cover).await?;
        client.commit_media_bytes(&MediaDescriptor {
            conversation_id: conv,
            message_seq: 2,
            attachment_id: Some(vid_att_id),
            variant: MediaVariant::VideoPreview,
            content_key: &preview_sha,
            expected_sha256: &preview_sha,
            expected_size: video_preview.len() as u64,
            mime: "video/mp4",
            dimensions: None,
        }, video_preview).await?;
        assert!(!client.attachment(vid_att_id).await?.local_file, "video preview must never impersonate original");
        assert!(client.attachment(img_att_id).await?.local_file);

        // Close and reopen -> persistent cache hit without re-download.
        client.close().await;
        drop(client);
        let client = Client::open(root.path(), service()?, session(user_id), true).await?;
        assert_eq!(client.cached_thumbnail_sizes(std::slice::from_ref(&img_msg)).await?.get(&1), Some(&(80, 60)));
        assert_eq!(client.cached_image_variant(conv, 1, Some(img_att_id), &sha, false).await?.map(|(_, b)| b), Some(img.clone()));
        assert_eq!(client.read_media_bytes(conv, 2, Some(vid_att_id), MediaVariant::VideoPreview, &preview_sha).await?.map(|(_, b)| b), Some(video_preview.to_vec()));

        // Server TTL expiry does NOT delete or block verified local image cache!
        sqlx::query("UPDATE inbox SET payload = json_set(payload, '$.expires_at', 0) WHERE seq = 1")
            .execute(&client.db).await?;
        assert!(client.preview_attachment(img_att_id).await.is_ok());
        assert_eq!(client.cached_image_variant(conv, 1, Some(img_att_id), &sha, false).await?.map(|(_, b)| b), Some(img));

        // Account & server isolation.
        let other_acc = Client::open(root.path(), service()?, session(Uuid::new_v4()), false).await?;
        let other_srv = Client::open(root.path(), Api::new("http://127.0.0.1:7842")?, session(user_id), false).await?;
        assert!(other_acc.cached_image_variant(conv, 1, Some(img_att_id), &sha, false).await?.is_none());
        assert!(other_srv.cached_image_variant(conv, 1, Some(img_att_id), &sha, false).await?.is_none());

        // Group rejoin cutoff revokes access.
        setup_directory(&client, &[(conv, Some(1))]).await?;
        assert!(client.cached_image_variant(conv, 1, Some(img_att_id), &sha, false).await?.is_none());
        assert!(client.cached_thumbnail_sizes(std::slice::from_ref(&img_msg)).await?.is_empty());
        assert!(client.preview_attachment(img_att_id).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn failure_recovery_handles_bad_hash_index_error_missing_corrupted_and_quarantine() -> Result<()> {
        let root = tempfile::tempdir()?;
        let session = DeviceSession {
            user: User { id: Uuid::new_v4(), handle: "dave".into() },
            device_id: Uuid::new_v4(),
            cursor: Uuid::new_v4(),
        };
        let client = Client::open(root.path(), Api::new("http://127.0.0.1:7841")?, session, false).await?;
        let conv = Uuid::new_v4();
        setup_directory(&client, &[(conv, Some(0))]).await?;

        let good = sample_png(100, 100, 21);
        let good_sha = sha256_hex(&good);
        let msg = insert_attachment_message(&client, 1, conv, "good.png", &good, None).await?;
        let att_id = msg.attachment.as_ref().unwrap().id;

        // 1. Bad hash rejected without committing fake file or leaving tmp garbage.
        let bad_desc = MediaDescriptor {
            conversation_id: conv,
            message_seq: 1,
            attachment_id: Some(att_id),
            variant: MediaVariant::ImageOriginal,
            content_key: &good_sha,
            expected_sha256: &good_sha,
            expected_size: good.len() as u64,
            mime: "image/png",
            dimensions: None,
        };
        assert!(client.commit_media_bytes(&bad_desc, &sample_png(100, 100, 99)).await.is_err());
        assert_eq!(fs::read_dir(client.tmp_dir())?.count(), 0);

        // 2. Simulated SQLite index commit failure cleans up newly written file (no fake-valid cache).
        sqlx::query("CREATE TRIGGER fail_ref_insert BEFORE INSERT ON media_refs BEGIN SELECT RAISE(ABORT, 'simulated index failure'); END;")
            .execute(&client.db).await?;
        let orig_path = fs_path(&client.media_root().join("conversations").join(conv.to_string()).join("images").join(format!("orig_{good_sha}.png")));
        assert!(client.commit_media_bytes(&bad_desc, &good).await.is_err());
        assert!(!orig_path.exists(), "freshly written file must be cleaned on index commit failure");
        sqlx::query("DROP TRIGGER fail_ref_insert").execute(&client.db).await?;

        // 3. Normal commit succeeds; then corrupting file on disk is detected & never served.
        client.commit_media_bytes(&bad_desc, &good).await?;
        assert!(orig_path.is_file());
        fs::write(&orig_path, sample_png(100, 100, 77))?;
        assert!(client.cached_image_variant(conv, 1, Some(att_id), &good_sha, false).await?.is_none());
        let status: String = sqlx::query_scalar("SELECT status FROM media_files WHERE content_key = ?")
            .bind(&good_sha).fetch_one(&client.db).await?;
        assert_eq!(status, "corrupted");
        assert!(!client.attachment(att_id).await?.local_file);

        // Re-commit repairs corrupted file atomically.
        client.commit_media_bytes(&bad_desc, &good).await?;
        assert_eq!(client.cached_image_variant(conv, 1, Some(att_id), &good_sha, false).await?.map(|(_, b)| b), Some(good.clone()));

        // 4. Manually deleting file on disk -> consistency check marks 'missing', quarantines unknown files, cleans stale tmp.
        fs::remove_file(&orig_path)?;
        let unknown_path = fs_path(&client.media_root().join("conversations").join(conv.to_string()).join("images").join("mystery.bin"));
        fs::write(&unknown_path, b"keep-me-safe")?;
        let stale_tmp = client.tmp_dir().join("stale.part");
        fs::write(&stale_tmp, b"partial")?;
        let active_tmp_file = client.tmp_dir().join("active-job.part");
        fs::write(&active_tmp_file, b"in-flight")?;
        let _active_guard = ActiveTempGuard::track(&client.active_tmp, "active-job".into());

        let report = client.run_consistency_check().await?;
        assert_eq!(report.marked_missing, 1);
        assert_eq!(report.quarantined_unknown, 1);
        assert_eq!(report.removed_stale_tmp, 1);
        assert!(!unknown_path.exists());
        assert!(!stale_tmp.exists());
        assert!(active_tmp_file.is_file(), "active temp file must not be removed");
        let quarantined_files: Vec<_> = walkdir::WalkDir::new(client.quarantine_dir())
            .into_iter().flatten().filter(|e| e.file_type().is_file()).collect();
        assert_eq!(quarantined_files.len(), 1);
        assert_eq!(fs::read(quarantined_files[0].path())?, b"keep-me-safe");
        Ok(())
    }

    #[tokio::test]
    async fn legacy_migration_preserves_data_outbox_and_quarantines_unknown_files() -> Result<()> {
        let root = tempfile::tempdir()?;
        let user = User { id: Uuid::new_v4(), handle: "legacy".into() };
        let device_id = Uuid::new_v4();
        let session = || DeviceSession { user: user.clone(), device_id, cursor: Uuid::new_v4() };
        let api = || Api::new("http://127.0.0.1:7841").unwrap();

        // Seed a profile then roll back migration flag and inject legacy files/rows.
        let client = Client::open(root.path(), api(), session(), false).await?;
        let conv = Uuid::new_v4();
        let avatar_ver = Uuid::new_v4();
        let dir_json = serde_json::json!({
            "conversations": [{"id": conv, "members": [&user], "activity_at": 1, "joined_after": 0}],
            "avatars": { user.id.to_string(): avatar_ver }
        });
        sqlx::query("INSERT INTO directory VALUES (1, ?)").bind(dir_json.to_string()).execute(&client.db).await?;

        let doc_bytes = b"important legacy attachment";
        let doc_msg = insert_attachment_message(&client, 1, conv, "report.pdf", doc_bytes, Some(0)).await?;
        let doc_id = doc_msg.attachment.as_ref().unwrap().id;
        fs::write(client.managed_files.join(doc_id.to_string()), doc_bytes)?;
        sqlx::query("INSERT INTO local_files (id, message_seq, sha256, size, ack_pending) VALUES (?,?,?,?,0)")
            .bind(doc_id.to_string()).bind(1i64).bind(sha256_hex(doc_bytes)).bind(doc_bytes.len() as i64)
            .execute(&client.db).await?;

        let img_bytes = sample_png(240, 160, 5);
        let thumb_bytes = sample_png(120, 80, 6);
        let img_msg = insert_attachment_message(&client, 2, conv, "legacy.png", &img_bytes, Some(0)).await?;
        let img_sha = sha256_hex(&img_bytes);
        sqlx::query("INSERT INTO media_previews VALUES (2, ?, -1, 'image/png', ?, 1)")
            .bind(&img_sha).bind(&img_bytes).execute(&client.db).await?;
        sqlx::query("INSERT INTO media_previews VALUES (2, ?, -2, 'image/png', ?, 1)")
            .bind(&img_sha).bind(&thumb_bytes).execute(&client.db).await?;

        let avatar_bytes = sample_png(64, 64, 8);
        sqlx::query("INSERT INTO cached_profile_images VALUES (?, ?, ?, ?)")
            .bind(user.id.to_string()).bind(avatar_ver.to_string()).bind(&avatar_bytes).bind(i64::MAX)
            .execute(&client.db).await?;

        // Unknown file in legacy files/ + pending outbox file in transfers/
        fs::write(client.managed_files.join("unknown-leftover.bin"), b"preserve-me")?;
        let outbox_part = client.staging.join(format!("upload-{}.part", Uuid::new_v4()));
        fs::write(&outbox_part, b"pending-upload-bytes")?;

        sqlx::query("DELETE FROM media_meta WHERE key = ?").bind(MEDIA_MIGRATION_KEY).execute(&client.db).await?;
        client.close().await;
        drop(client);

        // Reopen triggers safe migration!
        let migrated = Client::open(root.path(), api(), session(), true).await?;
        assert!(migrated.attachment(doc_id).await?.local_file);
        assert_eq!(migrated.read_attachment_chunk(doc_id, 0, doc_bytes.len()).await?, doc_bytes);
        assert!(!migrated.managed_files.join(doc_id.to_string()).exists());

        assert_eq!(migrated.cached_thumbnail_sizes(std::slice::from_ref(&img_msg)).await?.get(&2), Some(&(120, 80)));
        assert_eq!(migrated.cached_image_variant(conv, 2, Some(img_msg.attachment.as_ref().unwrap().id), &img_sha, false).await?.map(|(_, b)| b), Some(img_bytes));
        assert_eq!(migrated.cached_profile_image(user.id, avatar_ver).await, Some(avatar_bytes));
        assert!(outbox_part.is_file(), "outbox staging must survive migration untouched");
        assert!(!migrated.managed_files.join("unknown-leftover.bin").exists());
        let quarantined: Vec<_> = walkdir::WalkDir::new(migrated.quarantine_dir().join("legacy-files"))
            .into_iter().flatten().filter(|e| e.file_type().is_file()).collect();
        assert_eq!(quarantined.len(), 1);
        assert_eq!(fs::read(quarantined[0].path())?, b"preserve-me");
        Ok(())
    }

    #[tokio::test]
    async fn clear_cache_protects_chat_media_outbox_and_tombstones_while_cleaning_expired_public_and_tmp() -> Result<()> {
        let root = tempfile::tempdir()?;
        let user = User { id: Uuid::new_v4(), handle: "erin".into() };
        let session = DeviceSession { user: user.clone(), device_id: Uuid::new_v4(), cursor: Uuid::new_v4() };
        let client = Client::open(root.path(), Api::new("http://127.0.0.1:7841")?, session, false).await?;
        let conv = Uuid::new_v4();
        let active_avatar = Uuid::new_v4();
        let expired_avatar = Uuid::new_v4();
        sqlx::query("INSERT INTO directory VALUES (1, ?)")
            .bind(serde_json::json!({
                "conversations": [{"id": conv, "members": [&user], "activity_at": 1, "joined_after": 0}],
                "avatars": { user.id.to_string(): active_avatar }
            }).to_string())
            .execute(&client.db).await?;

        // Commit all 6 conversation media variants.
        let img = sample_png(100, 100, 1);
        let thumb = sample_png(50, 50, 2);
        let cover = sample_png(120, 80, 3);
        let vprev = b"video-preview-payload";
        let vorig = b"video-original-payload";
        let doc = b"ordinary-attachment-payload";
        let m1 = insert_attachment_message(&client, 1, conv, "a.png", &img, None).await?;
        let m2 = insert_attachment_message(&client, 2, conv, "v.mp4", vorig, None).await?;
        let m3 = insert_attachment_message(&client, 3, conv, "d.pdf", doc, None).await?;

        let img_p = client.commit_media_bytes(&MediaDescriptor {
            conversation_id: conv, message_seq: 1, attachment_id: Some(m1.attachment.as_ref().unwrap().id),
            variant: MediaVariant::ImageOriginal, content_key: &sha256_hex(&img), expected_sha256: &sha256_hex(&img),
            expected_size: img.len() as u64, mime: "image/png", dimensions: None,
        }, &img).await?;
        let thumb_p = client.commit_media_bytes(&MediaDescriptor {
            conversation_id: conv, message_seq: 1, attachment_id: Some(m1.attachment.as_ref().unwrap().id),
            variant: MediaVariant::ImageThumbnail, content_key: &sha256_hex(&img), expected_sha256: &sha256_hex(&thumb),
            expected_size: thumb.len() as u64, mime: "image/png", dimensions: Some((50, 50)),
        }, &thumb).await?;
        let cover_p = client.commit_media_bytes(&MediaDescriptor {
            conversation_id: conv, message_seq: 2, attachment_id: Some(m2.attachment.as_ref().unwrap().id),
            variant: MediaVariant::VideoCover, content_key: &sha256_hex(&cover), expected_sha256: &sha256_hex(&cover),
            expected_size: cover.len() as u64, mime: "image/png", dimensions: Some((120, 80)),
        }, &cover).await?;
        let vprev_p = client.commit_media_bytes(&MediaDescriptor {
            conversation_id: conv, message_seq: 2, attachment_id: Some(m2.attachment.as_ref().unwrap().id),
            variant: MediaVariant::VideoPreview, content_key: &sha256_hex(vprev), expected_sha256: &sha256_hex(vprev),
            expected_size: vprev.len() as u64, mime: "video/mp4", dimensions: None,
        }, vprev).await?;
        let vorig_p = client.commit_media_bytes(&MediaDescriptor {
            conversation_id: conv, message_seq: 2, attachment_id: Some(m2.attachment.as_ref().unwrap().id),
            variant: MediaVariant::VideoOriginal, content_key: &sha256_hex(vorig), expected_sha256: &sha256_hex(vorig),
            expected_size: vorig.len() as u64, mime: "video/mp4", dimensions: None,
        }, vorig).await?;
        let doc_p = client.commit_media_bytes(&MediaDescriptor {
            conversation_id: conv, message_seq: 3, attachment_id: Some(m3.attachment.as_ref().unwrap().id),
            variant: MediaVariant::Attachment, content_key: &sha256_hex(doc), expected_sha256: &sha256_hex(doc),
            expected_size: doc.len() as u64, mime: "application/octet-stream", dimensions: None,
        }, doc).await?;

        // Outbox file + recall tombstone + active avatar + expired avatar + stale tmp + active tmp.
        let outbox_file = client.staging.join(format!("upload-{}.part", Uuid::new_v4()));
        fs::write(&outbox_file, b"active-outbox-file")?;
        sqlx::query("INSERT INTO recall_tombstones VALUES (99, 100)").execute(&client.db).await?;
        let av_bytes = sample_png(32, 32, 4);
        client.commit_public_image(user.id, PublicResourceKind::Avatar, active_avatar, &av_bytes, unix_millis() + 86_400_000).await?;
        let peer = Uuid::new_v4();
        client.commit_public_image(peer, PublicResourceKind::Avatar, expired_avatar, &av_bytes, 1).await?;
        let expired_pub_path = client.media_root().join("public").join("avatars").join(format!("{expired_avatar}.png"));
        let active_pub_path = client.media_root().join("public").join("avatars").join(format!("{active_avatar}.png"));
        let stray_pub_path = client.media_root().join("public").join("avatars").join("stray.png");
        fs::write(&stray_pub_path, b"stray-public")?;
        let stale_tmp = client.tmp_dir().join("stale-download.part");
        fs::write(&stale_tmp, b"stale-temp")?;
        let active_tmp = client.tmp_dir().join("live-write.part");
        fs::write(&active_tmp, b"live-temp")?;
        let _guard = ActiveTempGuard::track(&client.active_tmp, "live-write".into());

        let report = client.clear_cache().await?;
        assert_eq!(report.removed_public_files, 2);
        assert_eq!(report.removed_temp_files, 1);
        assert!(!expired_pub_path.exists());
        assert!(!stray_pub_path.exists());
        assert!(!stale_tmp.exists());

        // Every protected asset must survive!
        for protected in [&img_p, &thumb_p, &cover_p, &vprev_p, &vorig_p, &doc_p, &outbox_file, &active_pub_path, &active_tmp] {
            assert!(protected.is_file(), "protected path was deleted: {}", protected.display());
        }
        let tombstones: i64 = sqlx::query_scalar("SELECT count(*) FROM recall_tombstones WHERE message_seq = 99")
            .fetch_one(&client.db).await?;
        assert_eq!(tombstones, 1);
        Ok(())
    }

    #[tokio::test]
    async fn public_avatar_update_keeps_old_on_failure_and_reclaims_old_after_switch() -> Result<()> {
        use std::sync::{Arc, Mutex};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let png_v1 = sample_png(48, 48, 1);
        let png_v2 = sample_png(48, 48, 2);
        let v1 = Uuid::new_v4();
        let v2 = Uuid::new_v4();

        #[derive(Clone, Copy)]
        enum Mode { OkV1, Fail500V2, CorruptV2, OkV2 }
        let mode = Arc::new(Mutex::new(Mode::OkV1));
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let server_mode = mode.clone();
        let v1_bytes = png_v1.clone();
        let v2_bytes = png_v2.clone();
        let server = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else { break };
                let mode = *server_mode.lock().unwrap();
                let b1 = v1_bytes.clone();
                let b2 = v2_bytes.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let _ = stream.read(&mut buf).await;
                    let (status, ct, body): (u16, &str, Vec<u8>) = match mode {
                        Mode::OkV1 => (200, "image/png", b1),
                        Mode::Fail500V2 => (500, "application/json", br#"{"error":"boom"}"#.to_vec()),
                        Mode::CorruptV2 => (200, "image/png", b"not-a-png-at-all".to_vec()),
                        Mode::OkV2 => (200, "image/png", b2),
                    };
                    let head = format!("HTTP/1.1 {status} OK\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                    let _ = stream.write_all(head.as_bytes()).await;
                    let _ = stream.write_all(&body).await;
                });
            }
        });

        let root = tempfile::tempdir()?;
        let own = User { id: Uuid::new_v4(), handle: "frank".into() };
        let peer = Uuid::new_v4();
        let session = DeviceSession { user: own, device_id: Uuid::new_v4(), cursor: Uuid::new_v4() };
        let client = Client::open(root.path(), Api::new(&format!("http://{addr}"))?, session, false).await?;
        sqlx::query("INSERT INTO directory VALUES (1, ?)")
            .bind(serde_json::json!({ "avatars": { peer.to_string(): v1 } }).to_string())
            .execute(&client.db).await?;

        // Download v1 -> committed and active.
        assert_eq!(client.profile_image(peer, v1).await?, png_v1);
        let path_v1 = client.media_root().join("public").join("avatars").join(format!("{v1}.png"));
        let path_v2 = client.media_root().join("public").join("avatars").join(format!("{v2}.png"));
        assert!(path_v1.is_file());

        // Directory announces v2; prune must keep v1 until v2 succeeds!
        let mut tx = client.db.begin().await?;
        sqlx::query("UPDATE directory SET payload = ?")
            .bind(serde_json::json!({ "avatars": { peer.to_string(): v2 } }).to_string())
            .execute(&mut *tx).await?;
        let removed = client.prune_profile_cache(&mut tx).await?;
        tx.commit().await?;
        client.remove_rel_paths(removed).await;
        assert!(path_v1.is_file());

        // v2 HTTP 500 failure -> v1 preserved!
        *mode.lock().unwrap() = Mode::Fail500V2;
        assert!(client.profile_image(peer, v2).await.is_err());
        assert!(path_v1.is_file());
        assert!(!path_v2.exists());
        assert_eq!(client.active_profile_image_for_request(peer, v2).await, Some((v1, png_v1.clone())));

        // v2 corrupt PNG failure -> v1 preserved, v2 never committed!
        *mode.lock().unwrap() = Mode::CorruptV2;
        assert!(client.profile_image(peer, v2).await.is_err());
        assert!(path_v1.is_file());
        assert!(!path_v2.exists());
        assert_eq!(client.active_profile_image(peer, PublicResourceKind::Avatar).await, Some((v1, png_v1)));

        // v2 succeeds -> switches atomically and auto-reclaims unreferenced v1!
        *mode.lock().unwrap() = Mode::OkV2;
        assert_eq!(client.profile_image(peer, v2).await?, png_v2);
        assert!(path_v2.is_file());
        assert!(!path_v1.exists(), "unreferenced old avatar must be reclaimed after switch");
        assert_eq!(client.active_profile_image(peer, PublicResourceKind::Avatar).await, Some((v2, png_v2)));

        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn external_save_as_copy_is_independent_of_chat_deletion_and_clear_cache() -> Result<()> {
        let root = tempfile::tempdir()?;
        let exports = tempfile::tempdir()?;
        let user = User { id: Uuid::new_v4(), handle: "grace".into() };
        let session = DeviceSession { user: user.clone(), device_id: Uuid::new_v4(), cursor: Uuid::new_v4() };
        let client = Client::open(root.path(), Api::new("http://127.0.0.1:7841")?, session, false).await?;
        let conv = Uuid::new_v4();
        setup_directory(&client, &[(conv, Some(0))]).await?;

        let content = b"independent exported document bytes";
        let msg = insert_attachment_message(&client, 1, conv, "export.txt", content, None).await?;
        let att = msg.attachment.as_ref().unwrap();
        let internal_path = client.commit_media_bytes(&MediaDescriptor {
            conversation_id: conv,
            message_seq: 1,
            attachment_id: Some(att.id),
            variant: MediaVariant::Attachment,
            content_key: &att.sha256,
            expected_sha256: &att.sha256,
            expected_size: att.size,
            mime: "application/octet-stream",
            dimensions: None,
        }, content).await?;
        // Mark local_files so offline export needs no server /download call.
        sqlx::query("INSERT INTO local_files (id, message_seq, sha256, size, ack_pending) VALUES (?,?,?,?,0)")
            .bind(att.id.to_string()).bind(1i64).bind(&att.sha256).bind(att.size as i64)
            .execute(&client.db).await?;

        let external = exports.path().join("user-saved.txt");
        client.queue_download(att.id, external.clone()).await?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if client.transfers().await?.iter().any(|t| t.direction == Direction::Download && t.status == TransferStatus::Complete) {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline, "timed out waiting for export");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(fs::read(&external)?, content);
        #[cfg(windows)]
        {
            let info_int = windows_file_info(&internal_path)?;
            let info_ext = windows_file_info(&external)?;
            assert_eq!(info_ext.2, 1);
            assert_ne!((info_int.0, info_int.1), (info_ext.0, info_ext.1));
        }

        // Delete message in chat + clear cache + consistency check -> internal file gone, external copy intact!
        client.hide_message(conv, 1).await?;
        client.clear_cache().await?;
        client.run_consistency_check().await?;
        assert!(!internal_path.exists());
        assert_eq!(fs::read(&external)?, content);
        Ok(())
    }

    #[tokio::test]
    async fn device_reregistration_migrates_scope_and_outbox_survives_message_deletion() -> Result<()> {
        let root = tempfile::tempdir()?;
        let user = User { id: Uuid::new_v4(), handle: "heidi".into() };
        let dev1 = Uuid::new_v4();
        let dev2 = Uuid::new_v4();
        let api = || Api::new("http://127.0.0.1:7841").unwrap();
        let client = Client::open(root.path(), api(), DeviceSession { user: user.clone(), device_id: dev1, cursor: Uuid::new_v4() }, false).await?;
        let conv = Uuid::new_v4();
        setup_directory(&client, &[(conv, Some(0))]).await?;

        let img = sample_png(96, 96, 33);
        let sha = sha256_hex(&img);
        let msg = insert_attachment_message(&client, 1, conv, "keep.png", &img, None).await?;
        let att_id = msg.attachment.as_ref().unwrap().id;
        client.remember_image_variant(conv, 1, Some(att_id), &sha, false, "image/png", &img).await?;

        // Stage a pending upload in the independent outbox (transfers/).
        let src_dir = tempfile::tempdir()?;
        let src_file = src_dir.path().join("queued.txt");
        fs::write(&src_file, b"pending-outbox-data")?;
        let snap = staging::capture_source_snapshot(&src_file)?;
        let upload_id = Uuid::new_v4();
        client.queue_upload_verified(conv, upload_id, src_file, snap).await?;
        let outbox_spool = client.staging.join(format!("upload-{upload_id}.part"));
        assert!(outbox_spool.is_file());

        // Deleting/hiding chat messages in the same conversation must NOT delete active outbox files!
        client.hide_message(conv, 1).await?;
        assert!(outbox_spool.is_file());
        assert_eq!(client.transfers().await?.len(), 1);

        // Commit a second message, then simulate device re-registration (dev1 -> dev2).
        let msg2 = insert_attachment_message(&client, 2, conv, "keep2.png", &img, None).await?;
        let att2_id = msg2.attachment.as_ref().unwrap().id;
        client.remember_image_variant(conv, 2, Some(att2_id), &sha, false, "image/png", &img).await?;
        client.close().await;
        drop(client);

        let rereg = Client::open(root.path(), api(), DeviceSession { user, device_id: dev2, cursor: Uuid::new_v4() }, false).await?;
        assert!(rereg.device_re_registered());
        assert_eq!(rereg.device_id(), dev2);
        let new_path = fs_path(&rereg.media_root().join("conversations").join(conv.to_string()).join("images").join(format!("orig_{sha}.png")));
        assert!(new_path.is_file());
        assert_eq!(rereg.cached_image_variant(conv, 2, Some(att2_id), &sha, false).await?.map(|(_, b)| b), Some(img));
        assert!(outbox_spool.is_file());
        rereg.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn legacy_migration_write_failure_preserves_source_and_retries_on_reopen() -> Result<()> {
        let root = tempfile::tempdir()?;
        let user = User { id: Uuid::new_v4(), handle: "mig_fail".into() };
        let dev = Uuid::new_v4();
        let conv = Uuid::new_v4();
        let peer = Uuid::new_v4();
        let avatar_ver = Uuid::new_v4();
        let img = sample_png(24, 16, 42);
        let sha = sha256_hex(&img);
        let avatar = sample_png(16, 16, 43);

        // Open once to initialize schema, then seed legacy rows and clear migration_v1.
        let client = Client::open(
            root.path(),
            Api::new("http://127.0.0.1:1")?,
            DeviceSession { user: user.clone(), device_id: dev, cursor: Uuid::new_v4() },
            false,
        )
        .await?;
        let media_root = client.media_root().to_path_buf();
        setup_directory(&client, &[(conv, Some(0))]).await?;
        sqlx::query("UPDATE directory SET payload = json_set(payload, '$.avatars', json_object(?1, ?2)) WHERE id = 1")
            .bind(peer.to_string())
            .bind(avatar_ver.to_string())
            .execute(&client.db)
            .await?;
        insert_attachment_message(&client, 1, conv, "legacy.png", &img, None).await?;
        sqlx::query("INSERT INTO media_previews VALUES (1, ?, -1, 'image/png', ?, 1)")
            .bind(&sha)
            .bind(&img)
            .execute(&client.db)
            .await?;
        sqlx::query("INSERT INTO media_previews VALUES (1, ?, -2, 'image/png', ?, 1)")
            .bind(&sha)
            .bind(&img)
            .execute(&client.db)
            .await?;
        sqlx::query("INSERT INTO cached_profile_images (user_id, version, data, expires_at) VALUES (?, ?, ?, ?)")
            .bind(peer.to_string())
            .bind(avatar_ver.to_string())
            .bind(&avatar)
            .bind(unix_millis() + 600_000)
            .execute(&client.db)
            .await?;
        // Also seed an unknown-attribution preview row (no inbox message) to confirm it is not deleted.
        sqlx::query("INSERT INTO media_previews VALUES (999, ?, -1, 'image/png', ?, 1)")
            .bind(&sha)
            .bind(&img)
            .execute(&client.db)
            .await?;
        sqlx::query("DELETE FROM media_meta WHERE key = ?")
            .bind(MEDIA_MIGRATION_KEY)
            .execute(&client.db)
            .await?;
        client.close().await;
        drop(client);

        // Inject real migration write failures: block the conversation & public avatar target dirs with regular files
        // so Client::open succeeds while commit_media_bytes and commit_public_image fail during migration.
        let conv_blocker = fs_path(&media_root.join("conversations").join(conv.to_string()));
        let pub_blocker = fs_path(&media_root.join("public").join("avatars"));
        for blocker in [&conv_blocker, &pub_blocker] {
            if blocker.is_dir() {
                fs::remove_dir_all(blocker)?;
            }
            fs::create_dir_all(blocker.parent().unwrap())?;
            fs::write(blocker, b"blocker")?;
        }

        let failed_open = Client::open(
            root.path(),
            Api::new("http://127.0.0.1:1")?,
            DeviceSession { user: user.clone(), device_id: dev, cursor: Uuid::new_v4() },
            false,
        )
        .await?;
        let preview_count: i64 = sqlx::query_scalar("SELECT count(*) FROM media_previews")
            .fetch_one(&failed_open.db)
            .await?;
        let pub_count: i64 = sqlx::query_scalar("SELECT count(*) FROM cached_profile_images")
            .fetch_one(&failed_open.db)
            .await?;
        let mig_state: Option<String> = sqlx::query_scalar("SELECT value FROM media_meta WHERE key = ?")
            .bind(MEDIA_MIGRATION_KEY)
            .fetch_optional(&failed_open.db)
            .await?;
        assert_eq!(preview_count, 3, "legacy media_previews must be preserved on migration failure");
        assert_eq!(pub_count, 1, "legacy cached_profile_images must be preserved on migration failure");
        assert_ne!(mig_state.as_deref(), Some("done"), "migration_v1 must not be marked done after failure");
        failed_open.close().await;
        drop(failed_open);

        // Remove obstacles and reopen: migration must retry, succeed, clean migrated rows, preserve unknown row, and mark done.
        fs::remove_file(&conv_blocker)?;
        fs::remove_file(&pub_blocker)?;
        let retried = Client::open(
            root.path(),
            Api::new("http://127.0.0.1:1")?,
            DeviceSession { user, device_id: dev, cursor: Uuid::new_v4() },
            false,
        )
        .await?;
        assert_eq!(
            retried.cached_image_variant(conv, 1, None, &sha, false).await?.map(|(_, b)| b),
            Some(img.clone())
        );
        assert_eq!(
            retried.cached_image_variant(conv, 1, None, &sha, true).await?.map(|(_, b)| b),
            Some(img)
        );
        assert_eq!(retried.cached_profile_image(peer, avatar_ver).await, Some(avatar));
        let remaining_previews: Vec<i64> = sqlx::query_scalar("SELECT message_seq FROM media_previews")
            .fetch_all(&retried.db)
            .await?;
        assert_eq!(remaining_previews, vec![999], "only unknown-attribution preview row should remain");
        let remaining_pub: i64 = sqlx::query_scalar("SELECT count(*) FROM cached_profile_images")
            .fetch_one(&retried.db)
            .await?;
        assert_eq!(remaining_pub, 0);
        let mig_done: Option<String> = sqlx::query_scalar("SELECT value FROM media_meta WHERE key = ?")
            .bind(MEDIA_MIGRATION_KEY)
            .fetch_optional(&retried.db)
            .await?;
        assert_eq!(mig_done.as_deref(), Some("done"));
        retried.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn within_conversation_dedup_protects_unopened_duplicates_in_both_orderings() -> Result<()> {
        let root = tempfile::tempdir()?;
        let user = User { id: Uuid::new_v4(), handle: "dedup_unopened".into() };
        let conv = Uuid::new_v4();
        let other_conv = Uuid::new_v4();
        let client = Client::open(
            root.path(),
            Api::new("http://127.0.0.1:1")?,
            DeviceSession { user: user.clone(), device_id: Uuid::new_v4(), cursor: Uuid::new_v4() },
            false,
        )
        .await?;
        setup_directory(&client, &[(conv, Some(0)), (other_conv, Some(0))]).await?;
        let img = sample_png(30, 20, 55);
        let sha = sha256_hex(&img);

        // Timing 1: both A (seq 10) and B (seq 20) are already in inbox; also a recalled seq 21 and other_conv seq 22.
        let msg_a = insert_attachment_message(&client, 10, conv, "a.png", &img, None).await?;
        let msg_b = insert_attachment_message(&client, 20, conv, "b.png", &img, None).await?;
        let _msg_other = insert_attachment_message(&client, 22, other_conv, "other.png", &img, None).await?;
        let mut recalled_msg = insert_attachment_message(&client, 21, conv, "rec.png", &img, None).await?;
        recalled_msg.recalled = true;
        recalled_msg.attachment = None;
        sqlx::query("UPDATE inbox SET payload = ?, attachment_id = NULL WHERE seq = 21")
            .bind(serde_json::to_string(&recalled_msg)?)
            .execute(&client.db)
            .await?;

        // Open/cache ONLY A (B is never opened!).
        let att_a = msg_a.attachment.as_ref().unwrap().id;
        let att_b = msg_b.attachment.as_ref().unwrap().id;
        client.remember_image_variant(conv, 10, Some(att_a), &sha, false, "image/png", &img).await?;
        let phys = fs_path(&client.media_root().join("conversations").join(conv.to_string()).join("images").join(format!("orig_{sha}.png")));
        assert!(phys.is_file());
        // Cross-conversation message must NOT be bound to conv's file!
        let other_refs: i64 = sqlx::query_scalar("SELECT count(*) FROM media_refs WHERE conversation_id = ?")
            .bind(other_conv.to_string())
            .fetch_one(&client.db)
            .await?;
        assert_eq!(other_refs, 0);

        // Delete/hide A while B was never opened: physical file MUST survive and B reads offline!
        client.hide_message(conv, 10).await?;
        assert!(phys.is_file(), "unopened duplicate B must keep physical file alive");
        assert_eq!(
            client.cached_image_variant(conv, 20, Some(att_b), &sha, false).await?.map(|(_, b)| b),
            Some(img.clone())
        );

        // Timing 2: while B is cached, a new duplicate C (seq 30) arrives via persist_message (never opened!).
        let msg_c = Message {
            seq: 30,
            conversation_id: conv,
            sender: user.clone(),
            created_at: unix_millis(),
            text: None,
            attachment: Some(Attachment {
                id: Uuid::new_v4(),
                name: "c.png".into(),
                kind: AttachmentKind::File,
                size: img.len() as u64,
                sha256: sha.clone(),
            }),
            ai_provider: None,
            published_at: None,
            ai_sources: Vec::new(),
            recalled: false,
            reply_to: None,
            nonce: None,
            expires_at: None,
            expired: None,
            pending: None,
            local_file: false,
        };
        let att_c = msg_c.attachment.as_ref().unwrap().id;
        let mut tx = client.db.begin_with("BEGIN IMMEDIATE").await?;
        super::super::local_sync::persist_message(&mut tx, msg_c).await?;
        tx.commit().await?;

        // Delete/hide B while C was never opened: physical file MUST survive and C reads offline!
        client.hide_message(conv, 20).await?;
        assert!(phys.is_file(), "newly persisted duplicate C must keep physical file alive");
        assert_eq!(
            client.cached_image_variant(conv, 30, Some(att_c), &sha, false).await?.map(|(_, b)| b),
            Some(img)
        );

        // Finally hide C (last live ref in conv): physical file MUST be reclaimed (recalled seq 21 must NOT hold it).
        client.hide_message(conv, 30).await?;
        assert!(!phys.exists(), "physical file must be reclaimed once all live references are removed");
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn consistency_check_does_not_quarantine_or_delete_concurrent_commits() -> Result<()> {
        let root = tempfile::tempdir()?;
        let user = User { id: Uuid::new_v4(), handle: "race_scan".into() };
        let conv = Uuid::new_v4();
        let peer = Uuid::new_v4();
        let avatar_ver = Uuid::new_v4();
        let client = Arc::new(
            Client::open(
                root.path(),
                Api::new("http://127.0.0.1:1")?,
                DeviceSession { user, device_id: Uuid::new_v4(), cursor: Uuid::new_v4() },
                false,
            )
            .await?,
        );
        setup_directory(&client, &[(conv, Some(0))]).await?;
        sqlx::query("UPDATE directory SET payload = json_set(payload, '$.avatars', json_object(?1, ?2)) WHERE id = 1")
            .bind(peer.to_string())
            .bind(avatar_ver.to_string())
            .execute(&client.db)
            .await?;
        let img = sample_png(18, 18, 66);
        let sha = sha256_hex(&img);
        let avatar = sample_png(12, 12, 67);
        let msg = insert_attachment_message(&client, 1, conv, "race.png", &img, None).await?;
        let att_id = msg.attachment.as_ref().unwrap().id;

        // Place genuinely unknown/orphan files on disk so we verify they ARE still cleaned/quarantined.
        let stray_conv = fs_path(&client.media_root().join("conversations").join(conv.to_string()).join("images").join("stray.bin"));
        fs::create_dir_all(stray_conv.parent().unwrap())?;
        fs::write(&stray_conv, b"stray")?;
        let orphan_pub = fs_path(&client.media_root().join("public").join("avatars").join(format!("{}.png", Uuid::new_v4())));
        fs::create_dir_all(orphan_pub.parent().unwrap())?;
        fs::write(&orphan_pub, b"orphan")?;

        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (resume_tx, resume_rx) = tokio::sync::oneshot::channel();
        let scan_client = Arc::clone(&client);
        let scan_task = tokio::spawn(async move {
            scan_client
                .run_consistency_check_inner(Some((ready_tx, resume_rx)))
                .await
        });

        // Wait until consistency check has taken its DB snapshot, then commit new chat media + new avatar!
        ready_rx.await?;
        client.remember_image_variant(conv, 1, Some(att_id), &sha, false, "image/png", &img).await?;
        client
            .commit_public_image(peer, PublicResourceKind::Avatar, avatar_ver, &avatar, unix_millis() + 600_000)
            .await?;
        let _ = resume_tx.send(());

        let report = scan_task.await??;
        assert_eq!(report.quarantined_unknown, 1, "only stray file should be quarantined");
        assert_eq!(report.removed_orphan_files, 1, "only orphan public file should be removed");
        assert!(!stray_conv.exists());
        assert!(!orphan_pub.exists());

        // Concurrently committed chat image and avatar MUST still exist and be readable!
        let new_img_path = fs_path(&client.media_root().join("conversations").join(conv.to_string()).join("images").join(format!("orig_{sha}.png")));
        let new_avatar_path = fs_path(&client.media_root().join("public").join("avatars").join(format!("{avatar_ver}.png")));
        assert!(new_img_path.is_file(), "concurrently committed chat media must not be quarantined");
        assert!(new_avatar_path.is_file(), "concurrently committed avatar must not be deleted");
        assert_eq!(
            client.cached_image_variant(conv, 1, Some(att_id), &sha, false).await?.map(|(_, b)| b),
            Some(img)
        );
        assert_eq!(client.cached_profile_image(peer, avatar_ver).await, Some(avatar));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn clear_cache_and_remove_rel_paths_respect_concurrent_writes() -> Result<()> {
        let root = tempfile::tempdir()?;
        let user = User { id: Uuid::new_v4(), handle: "race_clear".into() };
        let conv = Uuid::new_v4();
        let peer = Uuid::new_v4();
        let avatar_ver = Uuid::new_v4();
        let client = Arc::new(
            Client::open(
                root.path(),
                Api::new("http://127.0.0.1:1")?,
                DeviceSession { user, device_id: Uuid::new_v4(), cursor: Uuid::new_v4() },
                false,
            )
            .await?,
        );
        setup_directory(&client, &[(conv, Some(0))]).await?;
        sqlx::query("UPDATE directory SET payload = json_set(payload, '$.avatars', json_object(?1, ?2)) WHERE id = 1")
            .bind(peer.to_string())
            .bind(avatar_ver.to_string())
            .execute(&client.db)
            .await?;
        let img = sample_png(14, 14, 77);
        let sha = sha256_hex(&img);
        let avatar = sample_png(10, 10, 78);
        let msg = insert_attachment_message(&client, 1, conv, "keep.png", &img, None).await?;
        let att_id = msg.attachment.as_ref().unwrap().id;
        client.remember_image_variant(conv, 1, Some(att_id), &sha, false, "image/png", &img).await?;

        // Seed one stale tmp file.
        let stale_tmp = client.tmp_dir().join("stale-old.part");
        fs::create_dir_all(client.tmp_dir())?;
        fs::write(&stale_tmp, b"stale")?;

        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (resume_tx, resume_rx) = tokio::sync::oneshot::channel();
        let clear_client = Arc::clone(&client);
        let clear_task = tokio::spawn(async move {
            clear_client.clear_cache_inner(Some((ready_tx, resume_rx))).await
        });

        // After clear_cache commits its DB snapshot, concurrently commit a new avatar AND create an active tmp file!
        ready_rx.await?;
        client
            .commit_public_image(peer, PublicResourceKind::Avatar, avatar_ver, &avatar, unix_millis() + 600_000)
            .await?;
        let active_prefix = "concurrent-active.tmp".to_owned();
        let _active_guard = ActiveTempGuard::track(&client.active_tmp, active_prefix.clone());
        let active_tmp_file = client.tmp_dir().join(format!("{active_prefix}1234.part"));
        fs::write(&active_tmp_file, b"in-progress")?;
        let _ = resume_tx.send(());

        let report = clear_task.await??;
        assert_eq!(report.removed_temp_files, 1, "only stale tmp file should be removed");
        assert!(!stale_tmp.exists());
        assert!(active_tmp_file.is_file(), "active tmp file created after snapshot must survive");
        assert_eq!(
            client.cached_profile_image(peer, avatar_ver).await,
            Some(avatar),
            "avatar committed after snapshot must survive clear_cache"
        );

        // Stale remove_rel_paths must re-check SQLite and NOT unlink a currently indexed file!
        let rel_path: String = sqlx::query_scalar(
            "SELECT rel_path FROM media_files WHERE conversation_id = ? AND variant = 'image_original'",
        )
        .bind(conv.to_string())
        .fetch_one(&client.db)
        .await?;
        client.remove_rel_paths(vec![rel_path]).await;
        assert_eq!(
            client.cached_image_variant(conv, 1, Some(att_id), &sha, false).await?.map(|(_, b)| b),
            Some(img),
            "remove_rel_paths must not delete a file that is still indexed"
        );
        client.close().await;
        Ok(())
    }
}
