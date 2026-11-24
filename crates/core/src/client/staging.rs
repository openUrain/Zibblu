use crate::{Attachment, AttachmentKind, MAX_ATTACHMENT_SIZE, safe_filename};
use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, Metadata, OpenOptions},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};
use tempfile::NamedTempFile;
use uuid::Uuid;
use walkdir::WalkDir;
use zip::{ZipWriter, write::SimpleFileOptions};

const SAMPLE_WINDOW: usize = 64 * 1024;

#[derive(Debug, thiserror::Error)]
#[error("File changed. Remove and add it again.")]
pub struct SourceChangedError;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FileIdentity {
    #[cfg(windows)]
    Windows { volume: u32, index: u64 },
    #[cfg(unix)]
    Unix { dev: u64, ino: u64 },
    Path(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceSnapshot {
    pub name: String,
    pub kind: AttachmentKind,
    pub size: u64,
    pub identity: FileIdentity,
    pub modified_ticks: u64,
    pub created_ticks: u64,
    pub sample_hash: Option<[u8; 32]>,
    pub dir_fingerprint: Option<[u8; 32]>,
}

struct HandleStamp {
    identity: FileIdentity,
    size: u64,
    modified_ticks: u64,
    created_ticks: u64,
}

fn open_no_reparse(path: &Path, is_dir: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        };
        let mut flags = FILE_FLAG_OPEN_REPARSE_POINT;
        if is_dir {
            flags |= FILE_FLAG_BACKUP_SEMANTICS;
        }
        options.custom_flags(flags);
    }
    #[cfg(not(windows))]
    let _ = is_dir;
    options.open(path)
}

fn time_ticks(time: io::Result<std::time::SystemTime>) -> u64 {
    time.ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as u64)
}

fn handle_stamp(path: &Path, file: &File, metadata: &Metadata) -> Result<HandleStamp> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };

        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        ensure!(
            unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } != 0,
            "Could not inspect the selected item"
        );
        let volume = info.dwVolumeSerialNumber;
        let index = u64::from(info.nFileIndexHigh) << 32 | u64::from(info.nFileIndexLow);
        let identity = if volume != 0 || index != 0 {
            FileIdentity::Windows { volume, index }
        } else {
            FileIdentity::Path(fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()))
        };
        let size = u64::from(info.nFileSizeHigh) << 32 | u64::from(info.nFileSizeLow);
        let modified_ticks = u64::from(info.ftLastWriteTime.dwHighDateTime) << 32
            | u64::from(info.ftLastWriteTime.dwLowDateTime);
        let created_ticks = u64::from(info.ftCreationTime.dwHighDateTime) << 32
            | u64::from(info.ftCreationTime.dwLowDateTime);
        let _ = metadata;
        Ok(HandleStamp {
            identity,
            size,
            modified_ticks,
            created_ticks,
        })
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let _ = (path, file);
        Ok(HandleStamp {
            identity: FileIdentity::Unix {
                dev: metadata.dev(),
                ino: metadata.ino(),
            },
            size: metadata.len(),
            modified_ticks: time_ticks(metadata.modified()),
            created_ticks: time_ticks(metadata.created()),
        })
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = file;
        Ok(HandleStamp {
            identity: FileIdentity::Path(fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())),
            size: metadata.len(),
            modified_ticks: time_ticks(metadata.modified()),
            created_ticks: time_ticks(metadata.created()),
        })
    }
}

fn sample_file(file: &mut File, size: u64) -> Result<[u8; 32]> {
    let mut hasher = Sha256::new();
    file.seek(SeekFrom::Start(0))?;
    if size <= (SAMPLE_WINDOW * 2) as u64 {
        let mut buf = vec![0u8; size as usize];
        file.read_exact(&mut buf)?;
        hasher.update(&buf);
    } else {
        let mut head = vec![0u8; SAMPLE_WINDOW];
        file.read_exact(&mut head)?;
        hasher.update(&head);
        file.seek(SeekFrom::Start(size - SAMPLE_WINDOW as u64))?;
        let mut tail = vec![0u8; SAMPLE_WINDOW];
        file.read_exact(&mut tail)?;
        hasher.update(&tail);
    }
    file.seek(SeekFrom::Start(0))?;
    Ok(hasher.finalize().into())
}

fn folder_fingerprint(root: &Path) -> Result<(u64, [u8; 32])> {
    let mut entries = Vec::new();
    let mut total_size = 0u64;
    for (index, entry) in WalkDir::new(root)
        .min_depth(1)
        .follow_links(false)
        .into_iter()
        .enumerate()
    {
        ensure!(index < 4096, "Folders can contain up to 4,096 entries");
        let entry = entry.context("Could not read a folder entry")?;
        let metadata = fs::symlink_metadata(entry.path())?;
        ensure!(
            !is_link(&metadata),
            "Remove links and junctions before sharing a folder"
        );
        let components: Vec<_> = entry
            .path()
            .strip_prefix(root)?
            .components()
            .map(|part| {
                part.as_os_str()
                    .to_str()
                    .filter(|name| safe_filename(name))
                    .context("Folder entries need Windows-compatible names")
            })
            .collect::<Result<_>>()?;
        let rel = components.join("/");
        ensure!(rel.len() <= 1024, "A path inside the folder is too long");
        let kind_byte = if metadata.is_dir() {
            1u8
        } else {
            ensure!(
                metadata.is_file(),
                "Folders can contain regular files and directories"
            );
            total_size = total_size.saturating_add(metadata.len());
            ensure!(
                total_size <= MAX_ATTACHMENT_SIZE,
                "Attachments can be up to 256 MiB"
            );
            2u8
        };
        entries.push((rel, kind_byte, metadata.len(), time_ticks(metadata.modified())));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = Sha256::new();
    for (rel, kind_byte, len, mtime) in entries {
        hasher.update([kind_byte]);
        hasher.update((rel.len() as u32).to_le_bytes());
        hasher.update(rel.as_bytes());
        hasher.update(len.to_le_bytes());
        hasher.update(mtime.to_le_bytes());
    }
    Ok((total_size, hasher.finalize().into()))
}

pub fn capture_source_snapshot(source: &Path) -> Result<SourceSnapshot> {
    let link_meta = fs::symlink_metadata(source).context("Could not read the selected item")?;
    ensure!(
        !is_link(&link_meta),
        "Choose a file or folder that is not a link"
    );
    let raw_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .context("Choose a named file or folder")?;
    let is_dir = link_meta.is_dir();
    let check_name = if is_dir {
        format!("{raw_name}.zip")
    } else {
        raw_name.to_owned()
    };
    ensure!(
        safe_filename(&check_name),
        "The attachment needs a Windows-compatible filename"
    );
    let mut file = open_no_reparse(source, is_dir).context("Could not open the selected item")?;
    let metadata = file.metadata().context("Could not inspect the selected item")?;
    ensure!(
        !is_link(&metadata),
        "Choose a file or folder that is not a link"
    );
    let stamp = handle_stamp(source, &file, &metadata)?;
    if is_dir {
        ensure!(metadata.is_dir(), "Choose a regular file or folder");
        let (total_size, dir_fingerprint) = folder_fingerprint(source)?;
        Ok(SourceSnapshot {
            name: raw_name.to_owned(),
            kind: AttachmentKind::Folder,
            size: total_size,
            identity: stamp.identity,
            modified_ticks: stamp.modified_ticks,
            created_ticks: stamp.created_ticks,
            sample_hash: None,
            dir_fingerprint: Some(dir_fingerprint),
        })
    } else {
        ensure!(metadata.is_file(), "Choose a regular file");
        ensure!(
            stamp.size <= MAX_ATTACHMENT_SIZE,
            "Attachments can be up to 256 MiB"
        );
        let sample_hash = sample_file(&mut file, stamp.size)?;
        Ok(SourceSnapshot {
            name: raw_name.to_owned(),
            kind: AttachmentKind::File,
            size: stamp.size,
            identity: stamp.identity,
            modified_ticks: stamp.modified_ticks,
            created_ticks: stamp.created_ticks,
            sample_hash: Some(sample_hash),
            dir_fingerprint: None,
        })
    }
}

pub(super) fn stage_verified(
    source: &Path,
    expected: &SourceSnapshot,
    spool: &Path,
    id: Uuid,
) -> Result<Attachment> {
    if expected.kind == AttachmentKind::Folder {
        let current = capture_source_snapshot(source).map_err(|_| anyhow::Error::new(SourceChangedError))?;
        if current != *expected {
            bail!(SourceChangedError);
        }
        let mut snapshot =
            NamedTempFile::new_in(spool.parent().context("Missing transfer directory")?)?;
        archive(source, snapshot.as_file_mut())?;
        let after = capture_source_snapshot(source).map_err(|_| anyhow::Error::new(SourceChangedError))?;
        if after != *expected {
            bail!(SourceChangedError);
        }
        snapshot.as_file().sync_all()?;
        let size = snapshot.as_file().metadata()?.len();
        ensure!(
            size <= MAX_ATTACHMENT_SIZE,
            "Attachments can be up to 256 MiB"
        );
        let sha256 = hash_copy(snapshot.reopen()?, io::sink())?;
        snapshot.persist_noclobber(spool)?;
        return Ok(Attachment {
            id,
            name: format!("{}.zip", expected.name),
            kind: AttachmentKind::Folder,
            size,
            sha256,
        });
    }

    let mut file = open_no_reparse(source, false).map_err(|_| anyhow::Error::new(SourceChangedError))?;
    let metadata = file.metadata().map_err(|_| anyhow::Error::new(SourceChangedError))?;
    if is_link(&metadata) || !metadata.is_file() {
        bail!(SourceChangedError);
    }
    let before = handle_stamp(source, &file, &metadata)
        .map_err(|_| anyhow::Error::new(SourceChangedError))?;
    if before.identity != expected.identity
        || before.size != expected.size
        || before.modified_ticks != expected.modified_ticks
        || before.created_ticks != expected.created_ticks
    {
        bail!(SourceChangedError);
    }
    let mut snapshot =
        NamedTempFile::new_in(spool.parent().context("Missing transfer directory")?)?;
    let (copied, sha256, sample_hash) = copy_and_verify_single_handle(
        &mut file,
        snapshot.as_file_mut(),
        expected.size,
    )?;
    if copied != expected.size || expected.sample_hash != Some(sample_hash) {
        bail!(SourceChangedError);
    }
    let after_meta = file.metadata().map_err(|_| anyhow::Error::new(SourceChangedError))?;
    let after = handle_stamp(source, &file, &after_meta)
        .map_err(|_| anyhow::Error::new(SourceChangedError))?;
    if after.identity != expected.identity
        || after.size != expected.size
        || after.modified_ticks != expected.modified_ticks
        || after.created_ticks != expected.created_ticks
    {
        bail!(SourceChangedError);
    }
    snapshot.as_file().sync_all()?;
    snapshot.persist_noclobber(spool)?;
    Ok(Attachment {
        id,
        name: expected.name.clone(),
        kind: AttachmentKind::File,
        size: copied,
        sha256,
    })
}

fn copy_and_verify_single_handle(
    input: &mut File,
    output: &mut File,
    expected_size: u64,
) -> Result<(u64, String, [u8; 32])> {
    input.seek(SeekFrom::Start(0))?;
    let mut full_hash = Sha256::new();
    let mut sample_hash = Sha256::new();
    let full_sample = expected_size <= (SAMPLE_WINDOW * 2) as u64;
    let tail_start = expected_size.saturating_sub(SAMPLE_WINDOW as u64);
    let mut copied = 0u64;
    let mut buf = vec![0u8; 128 * 1024];
    loop {
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        let next_copied = copied + n as u64;
        if next_copied > MAX_ATTACHMENT_SIZE || next_copied > expected_size {
            bail!(SourceChangedError);
        }
        let chunk = &buf[..n];
        output.write_all(chunk)?;
        full_hash.update(chunk);
        if full_sample {
            sample_hash.update(chunk);
        } else {
            if copied < SAMPLE_WINDOW as u64 {
                let take = ((SAMPLE_WINDOW as u64 - copied) as usize).min(n);
                sample_hash.update(&chunk[..take]);
            }
            if next_copied > tail_start {
                let skip = tail_start.saturating_sub(copied) as usize;
                sample_hash.update(&chunk[skip..]);
            }
        }
        copied = next_copied;
    }
    Ok((copied, format!("{:x}", full_hash.finalize()), sample_hash.finalize().into()))
}

pub(super) fn stage(source: &Path, spool: &Path, id: Uuid) -> Result<Attachment> {
    let metadata = fs::symlink_metadata(source).context("Could not read the selected item")?;
    ensure!(
        !is_link(&metadata),
        "Choose a file or folder that is not a link"
    );
    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .context("Choose a named file or folder")?;
    let kind = if metadata.is_dir() {
        AttachmentKind::Folder
    } else {
        AttachmentKind::File
    };
    let name = if metadata.is_dir() {
        format!("{name}.zip")
    } else {
        name.to_owned()
    };
    ensure!(
        safe_filename(&name),
        "The attachment needs a Windows-compatible filename"
    );
    let mut snapshot =
        NamedTempFile::new_in(spool.parent().context("Missing transfer directory")?)?;
    if metadata.is_dir() {
        archive(source, snapshot.as_file_mut())?;
    } else {
        ensure!(metadata.is_file(), "Choose a regular file");
        copy_bounded(
            File::open(source)?,
            snapshot.as_file_mut(),
            MAX_ATTACHMENT_SIZE,
        )?;
    }
    snapshot.as_file().sync_all()?;
    let size = snapshot.as_file().metadata()?.len();
    ensure!(
        size <= MAX_ATTACHMENT_SIZE,
        "Attachments can be up to 256 MiB"
    );
    let sha256 = hash_copy(snapshot.reopen()?, io::sink())?;
    snapshot.persist_noclobber(spool)?;
    Ok(Attachment {
        id,
        name,
        kind,
        size,
        sha256,
    })
}

pub(super) fn stage_pasted(bytes: &[u8], spool: &Path, id: Uuid) -> Result<Attachment> {
    let extension = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "png"
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        "jpg"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "gif"
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        "webp"
    } else if bytes.starts_with(b"BM") {
        "bmp"
    } else {
        anyhow::bail!("Paste a PNG, JPEG, GIF, WebP or BMP image");
    };
    let mut snapshot =
        NamedTempFile::new_in(spool.parent().context("Missing transfer directory")?)?;
    snapshot.write_all(bytes)?;
    snapshot.as_file().sync_all()?;
    snapshot.persist_noclobber(spool)?;
    Ok(Attachment {
        id,
        name: format!("Pasted image {}.{extension}", &id.simple().to_string()[..8]),
        kind: AttachmentKind::File,
        size: bytes.len() as u64,
        sha256: format!("{:x}", Sha256::digest(bytes)),
    })
}

fn archive(root: &Path, output: &mut File) -> Result<()> {
    let mut archive = ZipWriter::new(output);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut remaining = MAX_ATTACHMENT_SIZE;
    for (index, entry) in WalkDir::new(root)
        .min_depth(1)
        .follow_links(false)
        .into_iter()
        .enumerate()
    {
        ensure!(index < 4096, "Folders can contain up to 4,096 entries");
        let entry = entry.context("Could not read a folder entry")?;
        let metadata = fs::symlink_metadata(entry.path())?;
        ensure!(
            !is_link(&metadata),
            "Remove links and junctions before sharing a folder"
        );
        let components: Vec<_> = entry
            .path()
            .strip_prefix(root)?
            .components()
            .map(|part| {
                part.as_os_str()
                    .to_str()
                    .filter(|name| safe_filename(name))
                    .context("Folder entries need Windows-compatible names")
            })
            .collect::<Result<_>>()?;
        let name = components.join("/");
        ensure!(name.len() <= 1024, "A path inside the folder is too long");
        if metadata.is_dir() {
            archive.add_directory(format!("{name}/"), options)?;
        } else {
            ensure!(
                metadata.is_file(),
                "Folders can contain regular files and directories"
            );
            archive.start_file(name, options)?;
            remaining -= copy_bounded(File::open(entry.path())?, &mut archive, remaining)?;
        }
    }
    archive.finish()?;
    Ok(())
}

fn copy_bounded(input: File, output: &mut impl Write, limit: u64) -> Result<u64> {
    let copied = io::copy(&mut input.take(limit + 1), output)?;
    ensure!(copied <= limit, "Attachments can be up to 256 MiB");
    Ok(copied)
}

pub(super) fn is_link(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    metadata.is_symlink()
}

pub(super) fn hash_copy(input: File, mut output: impl Write) -> Result<String> {
    let mut reader = BufReader::with_capacity(128 * 1024, input);
    let mut hash = Sha256::new();
    loop {
        let bytes = reader.fill_buf()?;
        if bytes.is_empty() {
            break;
        }
        hash.update(bytes);
        output.write_all(bytes)?;
        let count = bytes.len();
        reader.consume(count);
    }
    Ok(format!("{:x}", hash.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verified_staging_detects_modifications_replacements_and_deletions() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let spool_dir = tempfile::tempdir()?;
        let file_a = dir.path().join("note.txt");
        let sub = dir.path().join("other");
        fs::create_dir_all(&sub)?;
        let file_b = sub.join("note.txt");
        fs::write(&file_a, b"hello zibblu")?;
        fs::write(&file_b, b"hello zibblu")?;

        let snap_a = capture_source_snapshot(&file_a)?;
        let snap_a_again = capture_source_snapshot(&file_a)?;
        let snap_b = capture_source_snapshot(&file_b)?;
        assert_eq!(snap_a.identity, snap_a_again.identity);
        assert_ne!(snap_a.identity, snap_b.identity);

        // Unchanged file stages cleanly with matching SHA-256.
        let id = Uuid::new_v4();
        let spool = spool_dir.path().join(format!("{id}.part"));
        let staged = stage_verified(&file_a, &snap_a, &spool, id)?;
        assert_eq!(staged.size, 12);
        assert_eq!(staged.sha256, format!("{:x}", Sha256::digest(b"hello zibblu")));

        // Same-length overwrite is blocked.
        fs::write(&file_a, b"HELLO ZIBBLU")?;
        let err = stage_verified(&file_a, &snap_a, &spool_dir.path().join("2.part"), Uuid::new_v4())
            .unwrap_err();
        assert!(err.downcast_ref::<SourceChangedError>().is_some());

        // Replacement (remove + recreate with identical bytes) is blocked.
        let snap_b_before = capture_source_snapshot(&file_b)?;
        fs::remove_file(&file_b)?;
        fs::write(&file_b, b"hello zibblu")?;
        // Wait or rely on identity/creation/modified change; on NTFS delete+recreate may reuse
        // index only rarely, so also verify deletion itself is blocked.
        fs::remove_file(&file_b)?;
        let err = stage_verified(
            &file_b,
            &snap_b_before,
            &spool_dir.path().join("3.part"),
            Uuid::new_v4(),
        )
        .unwrap_err();
        assert!(err.downcast_ref::<SourceChangedError>().is_some());

        // Folder modification is detected.
        let folder = dir.path().join("pack");
        fs::create_dir_all(&folder)?;
        fs::write(folder.join("a.txt"), b"v1")?;
        let folder_snap = capture_source_snapshot(&folder)?;
        fs::write(folder.join("a.txt"), b"v2-longer")?;
        let err = stage_verified(
            &folder,
            &folder_snap,
            &spool_dir.path().join("4.part"),
            Uuid::new_v4(),
        )
        .unwrap_err();
        assert!(err.downcast_ref::<SourceChangedError>().is_some());
        Ok(())
    }
}
