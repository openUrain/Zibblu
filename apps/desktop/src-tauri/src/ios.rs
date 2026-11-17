use crate::ai::MAX_FILE_BYTES;
use anyhow::{Context, Result, ensure};
use std::{
    ffi::CString,
    io::Read,
    os::unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    path::{Path, PathBuf},
};
use tokio::sync::watch;
use zibblu_core::assistant::{AiFile, AiProvider};
use zibblu_core::keychain::{delete_secret, read_secret, store_secret};

unsafe extern "C" {
    fn zibblu_ios_install(network_changed: extern "C" fn(bool));
    fn zibblu_ios_share(path: *const std::ffi::c_char) -> bool;
    fn zibblu_ios_data_dir(buffer: *mut u8, capacity: usize) -> usize;
}

#[derive(Clone, Copy, Default)]
pub(crate) struct Network {
    pub generation: u64,
    pub available: bool,
}

static NETWORK: std::sync::LazyLock<watch::Sender<Network>> =
    std::sync::LazyLock::new(|| watch::channel(Network::default()).0);

extern "C" fn network_changed(available: bool) {
    NETWORK.send_modify(|state| {
        state.generation += 1;
        state.available = available;
    });
}

pub(crate) fn install() {
    unsafe { zibblu_ios_install(network_changed) };
}

pub(crate) fn network() -> watch::Receiver<Network> {
    NETWORK.subscribe()
}

pub(crate) fn allows_private(generation: u64) -> bool {
    let current = NETWORK.borrow();
    current.available && current.generation == generation
}

pub(crate) fn share_file(path: &Path) -> Result<()> {
    let path = CString::new(path.as_os_str().as_bytes())?;
    ensure!(
        unsafe { zibblu_ios_share(path.as_ptr()) },
        "Could not open the iOS share sheet"
    );
    Ok(())
}

pub(crate) fn data_directory() -> Result<PathBuf> {
    let mut path = [0u8; 4096];
    let size = unsafe { zibblu_ios_data_dir(path.as_mut_ptr(), path.len()) };
    ensure!(
        size > 0 && size <= path.len(),
        "The iOS app data directory is unavailable"
    );
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(&path[..size])))
}

fn ai_account(provider: AiProvider, reference: &[u8]) -> Result<String> {
    let id = uuid::Uuid::from_slice(reference).context("Replace the stored API key in Settings")?;
    Ok(format!("assistant/{}/{id}", provider.as_str()))
}

pub(crate) fn protect_key(provider: AiProvider, bytes: &[u8], seal: bool) -> Result<Vec<u8>> {
    if seal {
        let reference = uuid::Uuid::new_v4().into_bytes();
        store_secret(&ai_account(provider, &reference)?, bytes)?;
        return Ok(reference.to_vec());
    }
    let mut secret = read_secret(&ai_account(provider, bytes)?, 512)?
        .context("Replace the stored API key in Settings")?;
    Ok(std::mem::take(&mut *secret))
}

pub(crate) fn delete_key(provider: AiProvider, reference: &[u8]) {
    if let Ok(account) = ai_account(provider, reference) {
        let _ = delete_secret(&account);
    }
}

pub(crate) fn read_text_file(path: &Path) -> Result<AiFile> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .context("Could not open the selected file for reading")?;
    let metadata = file.metadata()?;
    ensure!(metadata.is_file(), "Choose a regular file");
    ensure!(
        metadata.len() <= MAX_FILE_BYTES,
        "Select a text file of up to 32 KiB"
    );
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_FILE_BYTES && !bytes.contains(&0),
        "Select a UTF-8 text file of up to 32 KiB"
    );
    Ok(AiFile {
        name: path
            .file_name()
            .context("Choose a regular file")?
            .to_string_lossy()
            .into_owned(),
        text: String::from_utf8(bytes).context("Select a UTF-8 text file")?,
    })
}
