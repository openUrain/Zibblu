use super::{Client, MediaVariant, media_store, staging};
use crate::Attachment;
use anyhow::Result;
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
};
use uuid::Uuid;
use walkdir::WalkDir;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ConversationStorage {
    pub conversation_id: Uuid,
    pub media_bytes: u64,
    pub chat_data_bytes: u64,
    pub chat_data_estimated: bool,
    pub files_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct StorageStats {
    pub app_bytes: u64,
    pub conversation_media_bytes: u64,
    pub database_bytes: u64,
    pub public_cache_and_temp_bytes: u64,
    pub outbox_bytes: u64,
    pub total_bytes: u64,
    pub conversations: Vec<ConversationStorage>,
}

#[derive(Default)]
struct ConvDiskBytes {
    media_bytes: u64,
    files_bytes: u64,
}

struct FsScanResult {
    app_bytes: u64,
    database_bytes: u64,
    conversation_media_bytes: u64,
    public_cache_and_temp_bytes: u64,
    outbox_bytes: u64,
    conv_dir_uuids: HashSet<Uuid>,
    conv_disk: HashMap<Uuid, ConvDiskBytes>,
}

impl Client {
    pub async fn storage_stats(&self, app_dir: Option<&Path>) -> Result<StorageStats> {
        self.ensure_open()?;

        let directory = self.membership_directory().await?;
        let membership_ids: HashSet<Uuid> =
            directory.conversations.iter().map(|c| c.id).collect();

        let deleted_chats: HashSet<Uuid> = sqlx::query_scalar::<_, String>(
            "SELECT conversation_id FROM local_deleted_chats",
        )
        .fetch_all(&self.db)
        .await?
        .into_iter()
        .filter_map(|s| Uuid::parse_str(&s).ok())
        .collect();

        let download_rows: Vec<(String, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT id,
                    json_extract(payload, '$.conversation_id'),
                    json_extract(payload, '$.attachment')
             FROM transfers
             WHERE direction = 'download'",
        )
        .fetch_all(&self.db)
        .await?;
        let mut download_transfers: HashMap<Uuid, (Uuid, bool)> = HashMap::new();
        for (id_str, conv_opt, att_opt) in download_rows {
            if let (Ok(id), Some(conv_str), Some(att_json)) =
                (Uuid::parse_str(&id_str), conv_opt, att_opt)
                && let (Ok(conv_id), Ok(att)) = (
                    Uuid::parse_str(&conv_str),
                    serde_json::from_str::<Attachment>(&att_json),
                )
            {
                let is_media = matches!(
                    MediaVariant::for_attachment(&att),
                    MediaVariant::ImageOriginal | MediaVariant::VideoOriginal
                );
                download_transfers.insert(id, (conv_id, is_media));
            }
        }

        let local_file_rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT l.id, i.conversation_id
             FROM local_files l
             JOIN inbox i ON i.seq = l.message_seq",
        )
        .fetch_all(&self.db)
        .await?;
        let mut local_file_convs: HashMap<String, Uuid> = HashMap::new();
        for (id, conv_str) in local_file_rows {
            if let Ok(conv_id) = Uuid::parse_str(&conv_str) {
                local_file_convs.insert(id, conv_id);
            }
        }

        let mut db_payload_by_conv: HashMap<Uuid, u64> = HashMap::new();
        for query in [
            "SELECT conversation_id, CAST(coalesce(sum(length(CAST(payload AS BLOB))), 0) AS INTEGER)
             FROM inbox GROUP BY conversation_id",
            "SELECT conversation_id,
                    CAST(coalesce(sum(length(CAST(work AS BLOB)) + coalesce(length(CAST(draft AS BLOB)), 0)), 0) AS INTEGER)
             FROM outbox WHERE conversation_id IS NOT NULL GROUP BY conversation_id",
            "SELECT conversation_id, CAST(coalesce(sum(length(CAST(prompt_text AS BLOB))), 0) AS INTEGER)
             FROM ai_local_prompts GROUP BY conversation_id",
            "SELECT p.conversation_id, CAST(sum(length(CAST(r.text AS BLOB)) + length(CAST(r.sources AS BLOB))) AS INTEGER)
             FROM ai_local_replies r JOIN ai_local_prompts p ON p.id = r.prompt_id GROUP BY p.conversation_id",
            "SELECT i.conversation_id, CAST(coalesce(sum(length(p.bytes)), 0) AS INTEGER)
             FROM media_previews p JOIN inbox i ON i.seq = p.message_seq
             GROUP BY i.conversation_id",
        ] {
            let rows: Vec<(String, i64)> = sqlx::query_as(query).fetch_all(&self.db).await?;
            for (conv_str, bytes) in rows {
                if bytes > 0
                    && let Ok(conv_id) = Uuid::parse_str(&conv_str)
                {
                    let entry = db_payload_by_conv.entry(conv_id).or_default();
                    *entry = entry.saturating_add(bytes as u64);
                }
            }
        }

        let app_dir_buf = app_dir.map(Path::to_path_buf);
        let profile_dir = self.profile_dir().to_path_buf();
        let media_root = self.media_root.clone();
        let staging_dir = self.staging.clone();
        let managed_files_dir = self.managed_files.clone();

        let scan = tokio::task::spawn_blocking(move || {
            scan_filesystem(
                app_dir_buf.as_deref(),
                &profile_dir,
                &media_root,
                &staging_dir,
                &managed_files_dir,
                &download_transfers,
                &local_file_convs,
            )
        })
        .await?;

        let mut candidate_ids: HashSet<Uuid> = membership_ids.clone();
        candidate_ids.extend(scan.conv_dir_uuids.iter().copied());
        candidate_ids.extend(scan.conv_disk.keys().copied());
        candidate_ids.extend(db_payload_by_conv.keys().copied());

        let kept_ids: Vec<Uuid> = candidate_ids
            .into_iter()
            .filter(|id| {
                let in_membership = membership_ids.contains(id);
                let is_deleted = deleted_chats.contains(id);
                let disk_bytes = scan
                    .conv_disk
                    .get(id)
                    .map_or(0, |d| d.media_bytes.saturating_add(d.files_bytes));
                let db_payload = db_payload_by_conv.get(id).copied().unwrap_or(0);
                (in_membership && !is_deleted) || disk_bytes > 0 || db_payload > 0
            })
            .collect();

        let chat_data_alloc = allocate_database_bytes(
            scan.database_bytes,
            &kept_ids,
            &db_payload_by_conv,
        );

        let mut conversations: Vec<ConversationStorage> = kept_ids
            .into_iter()
            .map(|conversation_id| {
                let disk = scan.conv_disk.get(&conversation_id);
                let media_bytes = disk.map_or(0, |d| d.media_bytes);
                let files_bytes = disk.map_or(0, |d| d.files_bytes);
                let chat_data_bytes = chat_data_alloc.get(&conversation_id).copied().unwrap_or(0);
                let total_bytes = media_bytes
                    .saturating_add(chat_data_bytes)
                    .saturating_add(files_bytes);
                ConversationStorage {
                    conversation_id,
                    media_bytes,
                    chat_data_bytes,
                    chat_data_estimated: true,
                    files_bytes,
                    total_bytes,
                }
            })
            .collect();

        conversations.sort_by_key(|c| (std::cmp::Reverse(c.total_bytes), c.conversation_id));

        let total_bytes = scan
            .app_bytes
            .saturating_add(scan.conversation_media_bytes)
            .saturating_add(scan.database_bytes)
            .saturating_add(scan.public_cache_and_temp_bytes)
            .saturating_add(scan.outbox_bytes);

        Ok(StorageStats {
            app_bytes: scan.app_bytes,
            conversation_media_bytes: scan.conversation_media_bytes,
            database_bytes: scan.database_bytes,
            public_cache_and_temp_bytes: scan.public_cache_and_temp_bytes,
            outbox_bytes: scan.outbox_bytes,
            total_bytes,
            conversations,
        })
    }
}

fn allocate_database_bytes(
    database_bytes: u64,
    kept_ids: &[Uuid],
    db_payload_by_conv: &HashMap<Uuid, u64>,
) -> HashMap<Uuid, u64> {
    let mut alloc = HashMap::with_capacity(kept_ids.len());
    let total_payload: u128 = kept_ids
        .iter()
        .map(|id| u128::from(db_payload_by_conv.get(id).copied().unwrap_or(0)))
        .sum();
    if database_bytes == 0 || total_payload == 0 {
        return alloc;
    }
    let total_db = u128::from(database_bytes);
    let mut remainders: Vec<(u128, Uuid)> = Vec::new();
    let mut allocated_sum = 0u64;
    for &id in kept_ids {
        let payload = u128::from(db_payload_by_conv.get(&id).copied().unwrap_or(0));
        if payload == 0 {
            continue;
        }
        let product = total_db * payload;
        let share = (product / total_payload) as u64;
        let rem = product % total_payload;
        alloc.insert(id, share);
        allocated_sum = allocated_sum.saturating_add(share);
        remainders.push((rem, id));
    }
    let leftover = database_bytes.saturating_sub(allocated_sum);
    if leftover > 0 {
        remainders.sort_by_key(|&(rem, id)| (std::cmp::Reverse(rem), id));
        for &(_, id) in remainders.iter().take(leftover as usize) {
            if let Some(val) = alloc.get_mut(&id) {
                *val = val.saturating_add(1);
            }
        }
    }
    alloc
}

fn scan_filesystem(
    app_dir: Option<&Path>,
    profile_dir: &Path,
    media_root: &Path,
    staging_dir: &Path,
    managed_files_dir: &Path,
    download_transfers: &HashMap<Uuid, (Uuid, bool)>,
    local_file_convs: &HashMap<String, Uuid>,
) -> FsScanResult {
    let app_bytes = app_dir.map_or(0, |dir| scan_app_bytes(dir, profile_dir));

    let database_bytes = ["chat.sqlite3", "chat.sqlite3-wal", "chat.sqlite3-shm"]
        .iter()
        .map(|name| file_size_no_link(&profile_dir.join(name)))
        .fold(0u64, u64::saturating_add);

    let assistants_bytes = file_size_no_link(&profile_dir.join("assistants.json"));
    let public_bytes = dir_size_no_links(&media_root.join("public"));
    let tmp_bytes = dir_size_no_links(&media_root.join("tmp"));
    let quarantine_bytes = dir_size_no_links(&media_root.join("quarantine"));

    let mut conv_dir_uuids = HashSet::new();
    let mut conv_disk: HashMap<Uuid, ConvDiskBytes> = HashMap::new();
    let mut conversation_media_bytes = 0u64;
    let mut outbox_bytes = 0u64;
    let mut seen_outbox_identities: HashSet<staging::FileIdentity> = HashSet::new();
    let mut orphan_staging_bytes = 0u64;
    let mut orphan_managed_bytes = 0u64;

    let conv_root = media_store::fs_path(&media_root.join("conversations"));
    if let Ok(entries) = fs::read_dir(&conv_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if staging::is_link(&meta) {
                continue;
            }
            if meta.is_file() {
                conversation_media_bytes = conversation_media_bytes.saturating_add(meta.len());
                continue;
            }
            if !meta.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Ok(conv_id) = Uuid::parse_str(&name) {
                conv_dir_uuids.insert(conv_id);
                let mut media_acc = 0u64;
                let mut files_acc = 0u64;
                if let Ok(sub_entries) = fs::read_dir(&path) {
                    for sub in sub_entries.flatten() {
                        let sub_path = sub.path();
                        let Ok(sub_meta) = fs::symlink_metadata(&sub_path) else {
                            continue;
                        };
                        if staging::is_link(&sub_meta) {
                            continue;
                        }
                        if sub_meta.is_file() {
                            files_acc = files_acc.saturating_add(sub_meta.len());
                        } else if sub_meta.is_dir() {
                            let sub_size = dir_size_no_links(&sub_path);
                            let sub_name = sub.file_name().to_string_lossy().into_owned();
                            match sub_name.as_str() {
                                "images"
                                | "video_covers"
                                | "video_previews"
                                | "videos"
                                | "video_preview_partials" => {
                                    media_acc = media_acc.saturating_add(sub_size);
                                }
                                "attachments" => {
                                    files_acc = files_acc.saturating_add(sub_size);
                                }
                                _ => {
                                    files_acc = files_acc.saturating_add(sub_size);
                                }
                            }
                        }
                    }
                }
                let entry_disk = conv_disk.entry(conv_id).or_default();
                entry_disk.media_bytes = entry_disk.media_bytes.saturating_add(media_acc);
                entry_disk.files_bytes = entry_disk.files_bytes.saturating_add(files_acc);
                conversation_media_bytes = conversation_media_bytes
                    .saturating_add(media_acc)
                    .saturating_add(files_acc);
            } else {
                conversation_media_bytes =
                    conversation_media_bytes.saturating_add(dir_size_no_links(&path));
            }
        }
    }

    let staging_path = media_store::fs_path(staging_dir);
    if let Ok(entries) = fs::read_dir(&staging_path) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if staging::is_link(&meta) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if meta.is_dir() {
                let dir_size = dir_size_no_links(&path);
                if name.starts_with("draft-") {
                    outbox_bytes = outbox_bytes.saturating_add(dir_size);
                } else {
                    orphan_staging_bytes = orphan_staging_bytes.saturating_add(dir_size);
                }
                continue;
            }
            if !meta.is_file() {
                continue;
            }
            let len = meta.len();
            if (name.starts_with("upload-") && name.ends_with(".part")) || name.starts_with("draft-")
            {
                if seen_outbox_identities.insert(file_identity(&path, &meta)) {
                    outbox_bytes = outbox_bytes.saturating_add(len);
                }
            } else if let Some(id) = name
                .strip_prefix("download-")
                .and_then(|s| s.strip_suffix(".part"))
                .and_then(|s| Uuid::parse_str(s).ok())
                && let Some(&(conv_id, is_media)) = download_transfers.get(&id)
            {
                conversation_media_bytes = conversation_media_bytes.saturating_add(len);
                let entry_disk = conv_disk.entry(conv_id).or_default();
                if is_media {
                    entry_disk.media_bytes = entry_disk.media_bytes.saturating_add(len);
                } else {
                    entry_disk.files_bytes = entry_disk.files_bytes.saturating_add(len);
                }
            } else {
                orphan_staging_bytes = orphan_staging_bytes.saturating_add(len);
            }
        }
    }

    let managed_path = media_store::fs_path(managed_files_dir);
    if let Ok(entries) = fs::read_dir(&managed_path) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if staging::is_link(&meta) {
                continue;
            }
            if meta.is_dir() {
                orphan_managed_bytes = orphan_managed_bytes.saturating_add(dir_size_no_links(&path));
                continue;
            }
            if !meta.is_file() {
                continue;
            }
            let len = meta.len();
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(&conv_id) = local_file_convs.get(&name) {
                conversation_media_bytes = conversation_media_bytes.saturating_add(len);
                let entry_disk = conv_disk.entry(conv_id).or_default();
                entry_disk.files_bytes = entry_disk.files_bytes.saturating_add(len);
            } else {
                orphan_managed_bytes = orphan_managed_bytes.saturating_add(len);
            }
        }
    }

    let public_cache_and_temp_bytes = public_bytes
        .saturating_add(tmp_bytes)
        .saturating_add(quarantine_bytes)
        .saturating_add(assistants_bytes)
        .saturating_add(orphan_staging_bytes)
        .saturating_add(orphan_managed_bytes);

    FsScanResult {
        app_bytes,
        database_bytes,
        conversation_media_bytes,
        public_cache_and_temp_bytes,
        outbox_bytes,
        conv_dir_uuids,
        conv_disk,
    }
}

fn scan_app_bytes(dir: &Path, profile_dir: &Path) -> u64 {
    let dir_fs = media_store::fs_path(dir);
    let Ok(dir_meta) = fs::symlink_metadata(&dir_fs) else {
        return 0;
    };
    if staging::is_link(&dir_meta) || !dir_meta.is_dir() {
        return 0;
    }

    let cargo_markers = ["deps", "build", "incremental", ".fingerprint"]
        .iter()
        .filter(|name| {
            let p = dir_fs.join(name);
            fs::symlink_metadata(&p).is_ok_and(|m| !staging::is_link(&m) && m.is_dir())
        })
        .count();

    if cargo_markers < 2 {
        return dir_size_excluding(&dir_fs, Some(profile_dir));
    }

    // In Cargo target/debug, count only the app binary, *.dll, and ffmpeg/ resources.
    let current_exe_canon = std::env::current_exe()
        .ok()
        .and_then(|p| fs::canonicalize(&p).ok());
    let mut total = 0u64;

    if let Ok(entries) = fs::read_dir(&dir_fs) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if staging::is_link(&meta) || !meta.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dll = name
                .rsplit_once('.')
                .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("dll"));
            let is_current_exe = current_exe_canon.as_ref().is_some_and(|exe| {
                fs::canonicalize(&path).ok().as_ref() == Some(exe)
            });
            let is_desktop_bin = matches!(
                name.to_ascii_lowercase().as_str(),
                "zibblu.exe" | "zibblu-desktop.exe" | "zibblu" | "zibblu-desktop"
            );
            if is_dll || is_current_exe || is_desktop_bin {
                total = total.saturating_add(meta.len());
            }
        }
    }

    total.saturating_add(dir_size_no_links(&dir_fs.join("ffmpeg")))
}

fn file_identity(path: &Path, meta: &fs::Metadata) -> staging::FileIdentity {
    #[cfg(windows)]
    {
        use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_FLAG_OPEN_REPARSE_POINT, GetFileInformationByHandle,
        };

        let _ = meta;
        if let Ok(file) = fs::OpenOptions::new()
            .access_mode(0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
        {
            let mut info = BY_HANDLE_FILE_INFORMATION::default();
            if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } != 0 {
                let volume = info.dwVolumeSerialNumber;
                let index = u64::from(info.nFileIndexHigh) << 32 | u64::from(info.nFileIndexLow);
                if volume != 0 || index != 0 {
                    return staging::FileIdentity::Windows { volume, index };
                }
            }
        }
        staging::FileIdentity::Path(path.to_path_buf())
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let _ = path;
        staging::FileIdentity::Unix {
            dev: meta.dev(),
            ino: meta.ino(),
        }
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = meta;
        staging::FileIdentity::Path(path.to_path_buf())
    }
}

fn file_size_no_link(path: &Path) -> u64 {
    let p = media_store::fs_path(path);
    match fs::symlink_metadata(&p) {
        Ok(meta) if !staging::is_link(&meta) && meta.is_file() => meta.len(),
        _ => 0,
    }
}

fn dir_size_no_links(root: &Path) -> u64 {
    dir_size_excluding(root, None)
}

fn dir_size_excluding(root: &Path, excluded: Option<&Path>) -> u64 {
    let root_fs = media_store::fs_path(root);
    let excluded = excluded.map(media_store::fs_path);
    let Ok(root_meta) = fs::symlink_metadata(&root_fs) else {
        return 0;
    };
    if staging::is_link(&root_meta) || !root_meta.is_dir() {
        return 0;
    }

    let mut total = 0u64;
    let mut count = 0usize;
    let walker = WalkDir::new(&root_fs)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            !excluded.as_deref().is_some_and(|path| entry.path() == path)
                && fs::symlink_metadata(entry.path())
                .is_ok_and(|meta| !staging::is_link(&meta))
        });

    for entry in walker.flatten() {
        count += 1;
        if count.is_multiple_of(512) {
            std::thread::yield_now();
        }
        let Ok(meta) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if !staging::is_link(&meta) && meta.is_file() {
            total = total.saturating_add(meta.len());
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{Api, MediaDescriptor, PublicResourceKind};
    use crate::{Attachment, AttachmentKind, DeviceSession, Directory, Message, User};
    use sha2::{Digest, Sha256};

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

    async fn setup_directory(client: &Client, conversations: &[Uuid]) -> Result<()> {
        let chats: Vec<_> = conversations
            .iter()
            .map(|id| {
                serde_json::json!({
                    "id": id,
                    "members": [&client.user],
                    "activity_at": 1,
                    "joined_after": 0
                })
            })
            .collect();
        sqlx::query(
            "INSERT INTO directory (id, payload) VALUES (1, ?)
             ON CONFLICT(id) DO UPDATE SET payload = excluded.payload",
        )
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
            ai_sources: Vec::new(),
            recalled: false,
            reply_to: None,
            nonce: None,
            expires_at: None,
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

    fn assert_global_sum(stats: &StorageStats) {
        assert_eq!(
            stats.total_bytes,
            stats.app_bytes
                + stats.conversation_media_bytes
                + stats.database_bytes
                + stats.public_cache_and_temp_bytes
                + stats.outbox_bytes
        );
        let conv_media_and_files: u64 = stats
            .conversations
            .iter()
            .map(|c| c.media_bytes + c.files_bytes)
            .sum();
        assert_eq!(conv_media_and_files, stats.conversation_media_bytes);
    }

    #[tokio::test]
    async fn cross_conversation_same_file_counts_per_conversation_and_isolates_deletion() -> Result<()> {
        let root = tempfile::tempdir()?;
        let session = DeviceSession {
            user: User {
                id: Uuid::new_v4(),
                handle: "stats_a".into(),
            },
            device_id: Uuid::new_v4(),
            cursor: Uuid::new_v4(),
        };
        let client =
            Client::open(root.path(), Api::new("http://127.0.0.1:1")?, session, false).await?;
        let conv_a = Uuid::new_v4();
        let conv_b = Uuid::new_v4();
        setup_directory(&client, &[conv_a, conv_b]).await?;

        let img = sample_png(120, 80, 1);
        let sha = sha256_hex(&img);
        let msg_a = insert_attachment_message(&client, 1, conv_a, "photo.png", &img).await?;
        let msg_b = insert_attachment_message(&client, 2, conv_b, "photo.png", &img).await?;
        let att_a = msg_a.attachment.as_ref().unwrap().id;
        let att_b = msg_b.attachment.as_ref().unwrap().id;
        client
            .remember_image_variant(conv_a, 1, Some(att_a), &sha, false, "image/png", &img)
            .await?;
        client
            .remember_image_variant(conv_b, 2, Some(att_b), &sha, false, "image/png", &img)
            .await?;

        let before = client.storage_stats(None).await?;
        assert_global_sum(&before);
        let size = img.len() as u64;
        assert_eq!(before.conversation_media_bytes, size * 2);
        let stat_a = before
            .conversations
            .iter()
            .find(|c| c.conversation_id == conv_a)
            .unwrap();
        let stat_b = before
            .conversations
            .iter()
            .find(|c| c.conversation_id == conv_b)
            .unwrap();
        assert_eq!(stat_a.media_bytes, size);
        assert_eq!(stat_b.media_bytes, size);

        // Delete conversation A completely: B's media_bytes and physical file stay intact.
        client.delete_conversation(conv_a).await?;
        let after = client.storage_stats(None).await?;
        assert_global_sum(&after);
        assert!(
            !after
                .conversations
                .iter()
                .any(|c| c.conversation_id == conv_a),
            "deleted conversation with no residual must be excluded"
        );
        let stat_b_after = after
            .conversations
            .iter()
            .find(|c| c.conversation_id == conv_b)
            .unwrap();
        assert_eq!(stat_b_after.media_bytes, size);
        assert_eq!(after.conversation_media_bytes, size);
        assert_eq!(
            client
                .cached_image_variant(conv_b, 2, Some(att_b), &sha, false)
                .await?
                .map(|(_, b)| b),
            Some(img)
        );
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn same_conversation_dedup_counts_once_until_last_ref_deleted() -> Result<()> {
        let root = tempfile::tempdir()?;
        let session = DeviceSession {
            user: User {
                id: Uuid::new_v4(),
                handle: "stats_dedup".into(),
            },
            device_id: Uuid::new_v4(),
            cursor: Uuid::new_v4(),
        };
        let client =
            Client::open(root.path(), Api::new("http://127.0.0.1:1")?, session, false).await?;
        let conv = Uuid::new_v4();
        setup_directory(&client, &[conv]).await?;

        let img = sample_png(160, 100, 9);
        let sha = sha256_hex(&img);
        let m1 = insert_attachment_message(&client, 1, conv, "dup1.png", &img).await?;
        let m2 = insert_attachment_message(&client, 2, conv, "dup2.png", &img).await?;
        client
            .remember_image_variant(
                conv,
                1,
                Some(m1.attachment.as_ref().unwrap().id),
                &sha,
                false,
                "image/png",
                &img,
            )
            .await?;
        client
            .remember_image_variant(
                conv,
                2,
                Some(m2.attachment.as_ref().unwrap().id),
                &sha,
                false,
                "image/png",
                &img,
            )
            .await?;

        let size = img.len() as u64;
        let s1 = client.storage_stats(None).await?;
        assert_global_sum(&s1);
        assert_eq!(s1.conversations[0].media_bytes, size);

        // Delete first duplicate -> physical file and media_bytes stay unchanged.
        client.hide_message(conv, 1).await?;
        let s2 = client.storage_stats(None).await?;
        assert_global_sum(&s2);
        assert_eq!(s2.conversations[0].media_bytes, size);

        // Delete last reference -> media_bytes returns to 0.
        client.hide_message(conv, 2).await?;
        let s3 = client.storage_stats(None).await?;
        assert_global_sum(&s3);
        assert_eq!(s3.conversations[0].media_bytes, 0);
        assert_eq!(s3.conversation_media_bytes, 0);
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn ranking_three_categories_estimated_chat_data_and_restart_consistency() -> Result<()> {
        let root = tempfile::tempdir()?;
        let app_dir = tempfile::tempdir()?;
        fs::write(app_dir.path().join("zibblu-desktop.exe"), vec![0u8; 500])?;
        fs::write(app_dir.path().join("helper.dll"), vec![0u8; 200])?;
        fs::create_dir_all(app_dir.path().join("ffmpeg"))?;
        fs::write(app_dir.path().join("ffmpeg").join("ffmpeg.exe"), vec![0u8; 300])?;
        // Simulate Cargo dev target/debug artifacts that must be ignored.
        for cargo_dir in ["deps", "build", "incremental", ".fingerprint"] {
            let p = app_dir.path().join(cargo_dir);
            fs::create_dir_all(&p)?;
            fs::write(p.join("huge.rlib"), vec![0u8; 4096])?;
        }

        let user = User {
            id: Uuid::new_v4(),
            handle: "stats_rank".into(),
        };
        let dev = Uuid::new_v4();
        let session = || DeviceSession {
            user: user.clone(),
            device_id: dev,
            cursor: Uuid::new_v4(),
        };
        let client =
            Client::open(root.path(), Api::new("http://127.0.0.1:1")?, session(), false).await?;
        let c_small = Uuid::new_v4();
        let c_big = Uuid::new_v4();
        let c_hidden = Uuid::new_v4();
        setup_directory(&client, &[c_small, c_big, c_hidden]).await?;

        // c_hidden is hidden via conversation_prefs (not deleted) and must appear in stats.
        client.hide_conversation(c_hidden).await?;

        // c_big has large attachment + text; c_small has smaller media + text.
        let doc = vec![b'D'; 5000];
        let m_big = insert_attachment_message(&client, 1, c_big, "archive.zip", &doc).await?;
        client
            .commit_media_bytes(
                &MediaDescriptor {
                    conversation_id: c_big,
                    message_seq: 1,
                    attachment_id: Some(m_big.attachment.as_ref().unwrap().id),
                    variant: MediaVariant::Attachment,
                    content_key: &sha256_hex(&doc),
                    expected_sha256: &sha256_hex(&doc),
                    expected_size: doc.len() as u64,
                    mime: "application/octet-stream",
                    dimensions: None,
                },
                &doc,
            )
            .await?;

        let small_img = sample_png(64, 64, 3);
        let m_small = insert_attachment_message(&client, 2, c_small, "pic.png", &small_img).await?;
        client
            .remember_image_variant(
                c_small,
                2,
                Some(m_small.attachment.as_ref().unwrap().id),
                &sha256_hex(&small_img),
                false,
                "image/png",
                &small_img,
            )
            .await?;

        let stats = client.storage_stats(Some(app_dir.path())).await?;
        assert_global_sum(&stats);
        assert_eq!(stats.app_bytes, 1000);
        assert_eq!(stats.conversations.len(), 3);
        assert!(stats.conversations.iter().all(|c| c.chat_data_estimated));
        assert!(stats.conversations[0].total_bytes >= stats.conversations[1].total_bytes);
        assert!(stats.conversations[1].total_bytes >= stats.conversations[2].total_bytes);
        assert_eq!(stats.conversations[0].conversation_id, c_big);
        assert_eq!(stats.conversations[0].files_bytes, 5000);
        assert_eq!(stats.conversations[0].media_bytes, 0);
        let small_stat = stats
            .conversations
            .iter()
            .find(|c| c.conversation_id == c_small)
            .unwrap();
        assert_eq!(small_stat.media_bytes, small_img.len() as u64);
        assert_eq!(small_stat.files_bytes, 0);

        // Sum of allocated chat_data_bytes matches database_bytes when payload > 0.
        let sum_chat: u64 = stats.conversations.iter().map(|c| c.chat_data_bytes).sum();
        assert_eq!(sum_chat, stats.database_bytes);

        client.close().await;
        drop(client);
        let reopened =
            Client::open(root.path(), Api::new("http://127.0.0.1:1")?, session(), true).await?;
        let stats_reopened = reopened.storage_stats(Some(app_dir.path())).await?;
        assert_global_sum(&stats_reopened);
        assert_eq!(stats_reopened.app_bytes, stats.app_bytes);
        assert_eq!(
            stats_reopened.conversation_media_bytes,
            stats.conversation_media_bytes
        );
        assert_eq!(
            stats_reopened.public_cache_and_temp_bytes,
            stats.public_cache_and_temp_bytes
        );
        assert_eq!(stats_reopened.outbox_bytes, stats.outbox_bytes);
        assert_eq!(stats_reopened.conversations.len(), stats.conversations.len());
        for (after, before) in stats_reopened
            .conversations
            .iter()
            .zip(stats.conversations.iter())
        {
            assert_eq!(after.conversation_id, before.conversation_id);
            assert_eq!(after.media_bytes, before.media_bytes);
            assert_eq!(after.files_bytes, before.files_bytes);
            assert_eq!(after.chat_data_estimated, before.chat_data_estimated);
        }
        assert_eq!(
            stats_reopened
                .conversations
                .iter()
                .map(|c| c.chat_data_bytes)
                .sum::<u64>(),
            stats_reopened.database_bytes
        );
        // After a clean close (WAL already checkpointed), a second reopen yields identical database_bytes too.
        reopened.close().await;
        drop(reopened);
        let reopened_again =
            Client::open(root.path(), Api::new("http://127.0.0.1:1")?, session(), true).await?;
        let stats_reopened_again = reopened_again.storage_stats(Some(app_dir.path())).await?;
        assert_eq!(stats_reopened_again, stats_reopened);
        reopened_again.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn partial_downloads_video_original_replacement_and_outbox_are_accurately_measured()
    -> Result<()> {
        let root = tempfile::tempdir()?;
        let session = DeviceSession {
            user: User {
                id: Uuid::new_v4(),
                handle: "stats_transfers".into(),
            },
            device_id: Uuid::new_v4(),
            cursor: Uuid::new_v4(),
        };
        let client =
            Client::open(root.path(), Api::new("http://127.0.0.1:1")?, session, false).await?;
        let conv = Uuid::new_v4();
        setup_directory(&client, &[conv]).await?;

        // 1. Video preview + partial preview -> replaced by original video.
        let v_orig = vec![b'V'; 2400];
        let v_prev = vec![b'P'; 600];
        let v_msg = insert_attachment_message(&client, 1, conv, "clip.mp4", &v_orig).await?;
        let v_att = v_msg.attachment.as_ref().unwrap();
        client
            .commit_media_bytes(
                &MediaDescriptor {
                    conversation_id: conv,
                    message_seq: 1,
                    attachment_id: Some(v_att.id),
                    variant: MediaVariant::VideoPreview,
                    content_key: &v_att.sha256,
                    expected_sha256: &sha256_hex(&v_prev),
                    expected_size: v_prev.len() as u64,
                    mime: "video/mp4",
                    dimensions: None,
                },
                &v_prev,
            )
            .await?;

        // 2. Partial download-*.part in staging for an attachment transfer.
        let dl_att_id = Uuid::new_v4();
        let dl_att = Attachment {
            id: dl_att_id,
            name: "manual.pdf".into(),
            kind: AttachmentKind::File,
            size: 1000,
            sha256: sha256_hex(b"manual"),
        };
        let dl_payload = serde_json::json!({
            "attachment": dl_att,
            "conversation_id": conv,
            "direction": "download",
            "status": "paused",
            "received": 350,
            "error": null
        });
        sqlx::query("INSERT INTO transfers (id, direction, payload) VALUES (?, 'download', ?)")
            .bind(dl_att_id.to_string())
            .bind(dl_payload.to_string())
            .execute(&client.db)
            .await?;
        fs::write(
            client.staging.join(format!("download-{dl_att_id}.part")),
            vec![0u8; 350],
        )?;

        // 3. Outbox upload-*.part + draft-* in staging + assistants.json in profile_dir.
        let upload_id = Uuid::new_v4();
        fs::write(
            client.staging.join(format!("upload-{upload_id}.part")),
            vec![0u8; 777],
        )?;
        fs::write(client.staging.join("draft-temp.part"), vec![0u8; 123])?;
        fs::write(client.profile_dir().join("assistants.json"), b"{}")?;

        let mid = client.storage_stats(None).await?;
        assert_global_sum(&mid);
        assert_eq!(mid.outbox_bytes, 777 + 123);
        assert_eq!(mid.public_cache_and_temp_bytes, 2);
        assert_eq!(mid.conversations[0].media_bytes, 600);
        assert_eq!(mid.conversations[0].files_bytes, 350);

        // Replace video preview with full original video.
        client
            .commit_media_bytes(
                &MediaDescriptor {
                    conversation_id: conv,
                    message_seq: 1,
                    attachment_id: Some(v_att.id),
                    variant: MediaVariant::VideoOriginal,
                    content_key: &v_att.sha256,
                    expected_sha256: &v_att.sha256,
                    expected_size: v_orig.len() as u64,
                    mime: "video/mp4",
                    dimensions: None,
                },
                &v_orig,
            )
            .await?;
        client
            .release_video_preview_after_original(conv, &v_att.sha256)
            .await?;

        let after = client.storage_stats(None).await?;
        assert_global_sum(&after);
        assert_eq!(after.conversations[0].media_bytes, 2400);
        assert_eq!(after.conversations[0].files_bytes, 350);
        assert_eq!(after.outbox_bytes, 777 + 123);
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn clear_cache_reclaims_expired_public_and_tmp_while_keeping_protected_data() -> Result<()> {
        let root = tempfile::tempdir()?;
        let user = User {
            id: Uuid::new_v4(),
            handle: "stats_clear".into(),
        };
        let session = DeviceSession {
            user: user.clone(),
            device_id: Uuid::new_v4(),
            cursor: Uuid::new_v4(),
        };
        let client =
            Client::open(root.path(), Api::new("http://127.0.0.1:1")?, session, false).await?;
        let conv = Uuid::new_v4();
        let active_avatar = Uuid::new_v4();
        let expired_avatar = Uuid::new_v4();
        let dir = Directory {
            contacts: Vec::new(),
            conversations: vec![crate::Conversation {
                id: conv,
                title: None,
                members: vec![user.clone()],
                activity_at: 1,
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
            remarks: HashMap::new(),
            names: HashMap::new(),
            avatars: [(user.id, active_avatar)].into_iter().collect(),
            read_receipts: false,
        };
        sqlx::query("INSERT INTO directory (id, payload) VALUES (1, ?)")
            .bind(serde_json::to_string(&dir)?)
            .execute(&client.db)
            .await?;

        let img = sample_png(80, 80, 4);
        let msg = insert_attachment_message(&client, 1, conv, "keep.png", &img).await?;
        client
            .remember_image_variant(
                conv,
                1,
                Some(msg.attachment.as_ref().unwrap().id),
                &sha256_hex(&img),
                false,
                "image/png",
                &img,
            )
            .await?;

        let outbox_part = client.staging.join(format!("upload-{}.part", Uuid::new_v4()));
        fs::write(&outbox_part, vec![0u8; 400])?;
        sqlx::query("INSERT INTO recall_tombstones VALUES (99, 100)")
            .execute(&client.db)
            .await?;

        let av_bytes = sample_png(32, 32, 8);
        client
            .commit_public_image(
                user.id,
                PublicResourceKind::Avatar,
                active_avatar,
                &av_bytes,
                i64::MAX / 2,
            )
            .await?;
        client
            .commit_public_image(
                Uuid::new_v4(),
                PublicResourceKind::Avatar,
                expired_avatar,
                &av_bytes,
                1,
            )
            .await?;
        fs::write(client.media_root().join("tmp").join("stale.part"), vec![0u8; 250])?;

        let before = client.storage_stats(None).await?;
        assert_global_sum(&before);
        assert_eq!(
            before.public_cache_and_temp_bytes,
            (av_bytes.len() as u64) * 2 + 250
        );

        let report = client.clear_cache().await?;
        assert_eq!(report.removed_public_files, 1);
        assert_eq!(report.removed_temp_files, 1);
        assert_eq!(report.reclaimed_bytes, (av_bytes.len() as u64) + 250);

        let after = client.storage_stats(None).await?;
        assert_global_sum(&after);
        assert_eq!(after.public_cache_and_temp_bytes, av_bytes.len() as u64);
        assert_eq!(after.conversation_media_bytes, img.len() as u64);
        assert_eq!(after.outbox_bytes, 400);
        assert!(outbox_part.is_file());
        let tombstones: i64 =
            sqlx::query_scalar("SELECT count(*) FROM recall_tombstones WHERE message_seq = 99")
                .fetch_one(&client.db)
                .await?;
        assert_eq!(tombstones, 1);
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn edited_batch_hardlinked_spools_count_once_until_last_link_removed() -> Result<()> {
        let root = tempfile::tempdir()?;
        let src_dir = tempfile::tempdir()?;
        let user = User {
            id: Uuid::new_v4(),
            handle: "batch_hardlink".into(),
        };
        let device_id = Uuid::new_v4();
        let cursor = Uuid::new_v4();
        let make_session = || DeviceSession {
            user: user.clone(),
            device_id,
            cursor,
        };
        let client =
            Client::open(root.path(), Api::new("http://127.0.0.1:1")?, make_session(), false)
                .await?;

        let chats: Vec<Uuid> = (0..9).map(|_| Uuid::new_v4()).collect();
        setup_directory(&client, &chats).await?;

        let src = src_dir.path().join("edited_photo.png");
        let payload = vec![b'X'; 128 * 1024];
        fs::write(&src, &payload)?;
        let expected_size = payload.len() as u64;

        let targets: Vec<crate::ForwardTarget> = chats
            .iter()
            .map(|&conversation_id| crate::ForwardTarget {
                conversation_id,
                attachment_id: Uuid::new_v4(),
            })
            .collect();
        let queued = client
            .queue_edited_media_batch(
                "edited_photo.png".into(),
                src,
                Uuid::new_v4(),
                targets.clone(),
            )
            .await?;
        assert_eq!(queued.len(), 9);

        let stats_all = client.storage_stats(None).await?;
        assert_global_sum(&stats_all);
        assert_eq!(
            stats_all.outbox_bytes, expected_size,
            "nine hard-linked spools must count as one physical file"
        );

        client.close().await;
        drop(client);
        let client =
            Client::open(root.path(), Api::new("http://127.0.0.1:1")?, make_session(), true).await?;

        // Remove 1st link -> 8 links still reference the physical file, so outbox_bytes stays unchanged.
        fs::remove_file(client.staging.join(format!("upload-{}.part", targets[0].attachment_id)))?;
        let stats_eight = client.storage_stats(None).await?;
        assert_global_sum(&stats_eight);
        assert_eq!(
            stats_eight.outbox_bytes, expected_size,
            "removing one hard link must not report space reclaimed"
        );

        // Remove links 2..=8 -> 1 link remains, outbox_bytes still unchanged.
        for target in &targets[1..8] {
            fs::remove_file(client.staging.join(format!("upload-{}.part", target.attachment_id)))?;
        }
        let stats_one = client.storage_stats(None).await?;
        assert_global_sum(&stats_one);
        assert_eq!(stats_one.outbox_bytes, expected_size);

        // Remove final 9th link -> outbox_bytes drops to 0.
        fs::remove_file(client.staging.join(format!("upload-{}.part", targets[8].attachment_id)))?;
        let stats_zero = client.storage_stats(None).await?;
        assert_global_sum(&stats_zero);
        assert_eq!(stats_zero.outbox_bytes, 0);

        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn outbox_copy_fallback_counts_each_independent_physical_copy() -> Result<()> {
        let root = tempfile::tempdir()?;
        let session = DeviceSession {
            user: User {
                id: Uuid::new_v4(),
                handle: "copy_fallback".into(),
            },
            device_id: Uuid::new_v4(),
            cursor: Uuid::new_v4(),
        };
        let client =
            Client::open(root.path(), Api::new("http://127.0.0.1:1")?, session, false).await?;

        let payload = vec![b'C'; 4096];
        let unit = payload.len() as u64;
        let first = client.staging.join(format!("upload-{}.part", Uuid::new_v4()));
        let second = client.staging.join(format!("upload-{}.part", Uuid::new_v4()));
        let third = client.staging.join(format!("upload-{}.part", Uuid::new_v4()));
        fs::write(&first, &payload)?;
        fs::copy(&first, &second)?;
        fs::copy(&first, &third)?;

        let stats_three = client.storage_stats(None).await?;
        assert_global_sum(&stats_three);
        assert_eq!(
            stats_three.outbox_bytes,
            unit * 3,
            "independent copy-fallback files must each be counted"
        );

        fs::remove_file(&second)?;
        let stats_two = client.storage_stats(None).await?;
        assert_global_sum(&stats_two);
        assert_eq!(stats_two.outbox_bytes, unit * 2);

        client.close().await;
        Ok(())
    }
}
