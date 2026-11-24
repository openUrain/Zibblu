use crate::{Attachment, AttachmentKind, MAX_ATTACHMENT_SIZE, safe_filename};
use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, Metadata, OpenOptions},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
#[cfg(not(windows))]
use std::time::UNIX_EPOCH;
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
pub struct FolderEntrySnapshot {
    pub rel_path: String,
    pub is_dir: bool,
    pub identity: FileIdentity,
    pub size: u64,
    pub modified_ticks: u64,
    pub created_ticks: u64,
    pub sample_hash: Option<[u8; 32]>,
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
    pub folder_entries: Option<Vec<FolderEntrySnapshot>>,
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

#[cfg(not(windows))]
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

fn folder_entries_snapshot(root: &Path) -> Result<(u64, Vec<FolderEntrySnapshot>)> {
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
        let link_meta = fs::symlink_metadata(entry.path())?;
        ensure!(
            !is_link(&link_meta),
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
        let rel_path = components.join("/");
        ensure!(rel_path.len() <= 1024, "A path inside the folder is too long");
        let is_dir = link_meta.is_dir();
        let mut file = open_no_reparse(entry.path(), is_dir)
            .context("Could not open a folder entry")?;
        let metadata = file.metadata()?;
        ensure!(
            !is_link(&metadata),
            "Remove links and junctions before sharing a folder"
        );
        let stamp = handle_stamp(entry.path(), &file, &metadata)?;
        if is_dir {
            ensure!(
                metadata.is_dir(),
                "Folders can contain regular files and directories"
            );
            entries.push(FolderEntrySnapshot {
                rel_path,
                is_dir: true,
                identity: stamp.identity,
                size: 0,
                modified_ticks: stamp.modified_ticks,
                created_ticks: stamp.created_ticks,
                sample_hash: None,
            });
        } else {
            ensure!(
                metadata.is_file(),
                "Folders can contain regular files and directories"
            );
            total_size = total_size.saturating_add(stamp.size);
            ensure!(
                total_size <= MAX_ATTACHMENT_SIZE,
                "Attachments can be up to 256 MiB"
            );
            let sample_hash = sample_file(&mut file, stamp.size)?;
            entries.push(FolderEntrySnapshot {
                rel_path,
                is_dir: false,
                identity: stamp.identity,
                size: stamp.size,
                modified_ticks: stamp.modified_ticks,
                created_ticks: stamp.created_ticks,
                sample_hash: Some(sample_hash),
            });
        }
    }
    entries.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok((total_size, entries))
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
        let (total_size, folder_entries) = folder_entries_snapshot(source)?;
        Ok(SourceSnapshot {
            name: raw_name.to_owned(),
            kind: AttachmentKind::Folder,
            size: total_size,
            identity: stamp.identity,
            modified_ticks: stamp.modified_ticks,
            created_ticks: stamp.created_ticks,
            sample_hash: None,
            folder_entries: Some(folder_entries),
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
            folder_entries: None,
        })
    }
}

pub fn open_verified_local_file(source: &Path, expected: &SourceSnapshot) -> Result<File> {
    if expected.kind != AttachmentKind::File {
        bail!(SourceChangedError);
    }
    let file = open_no_reparse(source, false).map_err(|_| anyhow::Error::new(SourceChangedError))?;
    verify_open_handle(source, &file, expected)?;
    Ok(file)
}

pub fn verify_open_handle(source: &Path, file: &File, expected: &SourceSnapshot) -> Result<()> {
    let metadata = file.metadata().map_err(|_| anyhow::Error::new(SourceChangedError))?;
    if is_link(&metadata) || !metadata.is_file() {
        bail!(SourceChangedError);
    }
    let stamp = handle_stamp(source, file, &metadata)
        .map_err(|_| anyhow::Error::new(SourceChangedError))?;
    if stamp.identity != expected.identity
        || stamp.size != expected.size
        || stamp.modified_ticks != expected.modified_ticks
        || stamp.created_ticks != expected.created_ticks
    {
        bail!(SourceChangedError);
    }
    Ok(())
}

fn verify_folder_root(root: &Path, expected: &SourceSnapshot) -> Result<File> {
    let link_meta = fs::symlink_metadata(root).map_err(|_| anyhow::Error::new(SourceChangedError))?;
    if is_link(&link_meta) || !link_meta.is_dir() {
        bail!(SourceChangedError);
    }
    let dir_handle = open_no_reparse(root, true).map_err(|_| anyhow::Error::new(SourceChangedError))?;
    let meta = dir_handle.metadata().map_err(|_| anyhow::Error::new(SourceChangedError))?;
    if is_link(&meta) || !meta.is_dir() {
        bail!(SourceChangedError);
    }
    let stamp = handle_stamp(root, &dir_handle, &meta)
        .map_err(|_| anyhow::Error::new(SourceChangedError))?;
    if stamp.identity != expected.identity
        || stamp.modified_ticks != expected.modified_ticks
        || stamp.created_ticks != expected.created_ticks
    {
        bail!(SourceChangedError);
    }
    Ok(dir_handle)
}

fn archive_verified(root: &Path, expected: &SourceSnapshot, output: &mut File) -> Result<()> {
    let expected_entries = expected
        .folder_entries
        .as_ref()
        .ok_or_else(|| anyhow::Error::new(SourceChangedError))?;
    let root_handle = verify_folder_root(root, expected)?;

    // Collect current relative paths first so added/removed/reparse entries fail closed.
    let mut current_paths = Vec::new();
    for (index, entry) in WalkDir::new(root)
        .min_depth(1)
        .follow_links(false)
        .into_iter()
        .enumerate()
    {
        if index >= 4096 {
            bail!(SourceChangedError);
        }
        let entry = entry.map_err(|_| anyhow::Error::new(SourceChangedError))?;
        let link_meta =
            fs::symlink_metadata(entry.path()).map_err(|_| anyhow::Error::new(SourceChangedError))?;
        if is_link(&link_meta) {
            bail!(SourceChangedError);
        }
        let rel = entry
            .path()
            .strip_prefix(root)
            .map_err(|_| anyhow::Error::new(SourceChangedError))?
            .components()
            .map(|part| part.as_os_str().to_str())
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| anyhow::Error::new(SourceChangedError))?
            .join("/");
        current_paths.push(rel);
    }
    current_paths.sort();
    if current_paths.len() != expected_entries.len()
        || current_paths
            .iter()
            .zip(expected_entries.iter())
            .any(|(actual, expected)| actual != &expected.rel_path)
    {
        bail!(SourceChangedError);
    }

    let mut archive = ZipWriter::new(output);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut remaining = MAX_ATTACHMENT_SIZE;

    for expected_entry in expected_entries {
        let entry_path = root.join(&expected_entry.rel_path);
        let link_meta =
            fs::symlink_metadata(&entry_path).map_err(|_| anyhow::Error::new(SourceChangedError))?;
        if is_link(&link_meta) || link_meta.is_dir() != expected_entry.is_dir {
            bail!(SourceChangedError);
        }
        let mut handle = open_no_reparse(&entry_path, expected_entry.is_dir)
            .map_err(|_| anyhow::Error::new(SourceChangedError))?;
        let meta = handle
            .metadata()
            .map_err(|_| anyhow::Error::new(SourceChangedError))?;
        if is_link(&meta) || meta.is_dir() != expected_entry.is_dir {
            bail!(SourceChangedError);
        }
        let before = handle_stamp(&entry_path, &handle, &meta)
            .map_err(|_| anyhow::Error::new(SourceChangedError))?;
        if before.identity != expected_entry.identity
            || before.modified_ticks != expected_entry.modified_ticks
            || before.created_ticks != expected_entry.created_ticks
            || (!expected_entry.is_dir && before.size != expected_entry.size)
        {
            bail!(SourceChangedError);
        }
        if expected_entry.is_dir {
            archive.add_directory(format!("{}/", expected_entry.rel_path), options)?;
        } else {
            if expected_entry.size > remaining {
                bail!("Attachments can be up to 256 MiB");
            }
            archive.start_file(&expected_entry.rel_path, options)?;
            let (copied, _, sample_hash) = copy_and_verify_single_handle(
                &mut handle,
                &mut archive,
                expected_entry.size,
            )?;
            if copied != expected_entry.size || expected_entry.sample_hash != Some(sample_hash) {
                bail!(SourceChangedError);
            }
            let after_meta = handle
                .metadata()
                .map_err(|_| anyhow::Error::new(SourceChangedError))?;
            let after = handle_stamp(&entry_path, &handle, &after_meta)
                .map_err(|_| anyhow::Error::new(SourceChangedError))?;
            if after.identity != expected_entry.identity
                || after.size != expected_entry.size
                || after.modified_ticks != expected_entry.modified_ticks
                || after.created_ticks != expected_entry.created_ticks
            {
                bail!(SourceChangedError);
            }
            remaining -= copied;
        }
    }

    archive.finish()?;
    // Re-check root directory handle after archiving.
    let root_meta = root_handle
        .metadata()
        .map_err(|_| anyhow::Error::new(SourceChangedError))?;
    let root_after = handle_stamp(root, &root_handle, &root_meta)
        .map_err(|_| anyhow::Error::new(SourceChangedError))?;
    if root_after.identity != expected.identity
        || root_after.modified_ticks != expected.modified_ticks
        || root_after.created_ticks != expected.created_ticks
    {
        bail!(SourceChangedError);
    }
    Ok(())
}

pub(super) fn stage_verified(
    source: &Path,
    expected: &SourceSnapshot,
    spool: &Path,
    id: Uuid,
) -> Result<Attachment> {
    if expected.kind == AttachmentKind::Folder {
        let mut snapshot =
            NamedTempFile::new_in(spool.parent().context("Missing transfer directory")?)?;
        archive_verified(source, expected, snapshot.as_file_mut())?;
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

    let mut file = open_verified_local_file(source, expected)?;
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
    verify_open_handle(source, &file, expected)?;
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
    output: &mut impl Write,
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

        // Folder modification is detected, including same-length replacement with preserved mtime.
        let folder = dir.path().join("pack");
        fs::create_dir_all(&folder)?;
        let member = folder.join("a.txt");
        fs::write(&member, b"v1-data")?;
        let orig_mtime = fs::metadata(&member)?.modified()?;
        let folder_snap = capture_source_snapshot(&folder)?;
        // Atomic replacement of member with identical length and preserved mtime must be blocked.
        let replacement = dir.path().join("replacement.tmp");
        fs::write(&replacement, b"v2-DATA")?;
        File::options().write(true).open(&replacement)?.set_modified(orig_mtime)?;
        fs::remove_file(&member)?;
        fs::rename(&replacement, &member)?;
        let err = stage_verified(
            &folder,
            &folder_snap,
            &spool_dir.path().join("4.part"),
            Uuid::new_v4(),
        )
        .unwrap_err();
        assert!(err.downcast_ref::<SourceChangedError>().is_some());

        // Fresh snapshot after replacement stages cleanly.
        let fresh_folder_snap = capture_source_snapshot(&folder)?;
        let staged_folder = stage_verified(
            &folder,
            &fresh_folder_snap,
            &spool_dir.path().join("5.part"),
            Uuid::new_v4(),
        )?;
        assert_eq!(staged_folder.kind, AttachmentKind::Folder);
        assert_eq!(staged_folder.name, "pack.zip");
        Ok(())
    }
}
