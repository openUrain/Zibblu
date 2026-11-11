use crate::{Attachment, AttachmentKind, MAX_ATTACHMENT_SIZE, safe_filename};
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, Metadata},
    io::{self, BufRead, BufReader, Read, Write},
    path::Path,
};
use tempfile::NamedTempFile;
use uuid::Uuid;
use walkdir::WalkDir;
use zip::{ZipWriter, write::SimpleFileOptions};

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

fn is_link(metadata: &Metadata) -> bool {
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
