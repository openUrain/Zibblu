use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;
use zeroize::Zeroize;
use zibblu_core::User;

#[cfg(windows)]
use {
    anyhow::ensure,
    std::{
        fs,
        io::{ErrorKind, Read, Write},
    },
    zeroize::Zeroizing,
};

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct SavedAccount {
    pub user: User,
    pub server: String,
    pub avatar: Option<Vec<u8>>,
}

#[derive(Deserialize, Serialize)]
pub(crate) struct SavedSession {
    pub account: SavedAccount,
    pub token: String,
}

impl Drop for SavedSession {
    fn drop(&mut self) {
        self.token.zeroize();
    }
}

#[cfg(windows)]
const FILE: &str = "saved-session.bin";
#[cfg(windows)]
const SCOPE: &[u8] = b"Zibblu saved sign-in v1";
#[cfg(windows)]
const MAX_BYTES: usize = 256 * 1024;

#[cfg(windows)]
pub(crate) fn load(data: &Path) -> Result<Option<SavedSession>> {
    let file = match fs::File::open(data.join(FILE)) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut sealed = Vec::new();
    file.take(MAX_BYTES as u64 + 1).read_to_end(&mut sealed)?;
    ensure!(
        !sealed.is_empty() && sealed.len() <= MAX_BYTES,
        "Invalid saved sign-in"
    );
    let plain = Zeroizing::new(crate::dpapi::transform(&sealed, SCOPE, false)?);
    Ok(Some(serde_json::from_slice(&plain)?))
}

#[cfg(windows)]
pub(crate) fn save(data: &Path, session: &SavedSession) -> Result<()> {
    let plain = Zeroizing::new(serde_json::to_vec(session)?);
    let sealed = crate::dpapi::transform(&plain, SCOPE, true)?;
    ensure!(sealed.len() <= MAX_BYTES, "Saved sign-in is too large");
    let mut temporary = tempfile::NamedTempFile::new_in(data)?;
    temporary.write_all(&sealed)?;
    temporary.as_file().sync_all()?;
    temporary.persist(data.join(FILE))?;
    Ok(())
}

#[cfg(windows)]
pub(crate) fn clear(data: &Path) -> Result<()> {
    match fs::remove_file(data.join(FILE)) {
        Err(error) if error.kind() != ErrorKind::NotFound => Err(error.into()),
        _ => Ok(()),
    }
}

#[cfg(not(windows))]
pub(crate) fn load(_: &Path) -> Result<Option<SavedSession>> {
    Ok(None)
}
#[cfg(not(windows))]
pub(crate) fn save(_: &Path, _: &SavedSession) -> Result<()> {
    Ok(())
}
#[cfg(not(windows))]
pub(crate) fn clear(_: &Path) -> Result<()> {
    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn saved_sign_in_is_sealed_and_replaced_with_the_latest_account() -> Result<()> {
        let data = tempfile::tempdir()?;
        let mut saved = SavedSession {
            account: SavedAccount {
                user: User {
                    id: uuid::Uuid::new_v4(),
                    handle: "FixtureAccount".into(),
                },
                server: "https://example.com/".into(),
                avatar: Some(vec![1, 2, 3]),
            },
            token: "0123456789abcdef".repeat(4),
        };
        save(data.path(), &saved)?;
        let raw = fs::read(data.path().join(FILE))?;
        assert!(
            !raw.windows(saved.token.len())
                .any(|part| part == saved.token.as_bytes())
        );
        assert!(
            !raw.windows(saved.account.user.handle.len())
                .any(|part| part == saved.account.user.handle.as_bytes())
        );
        let opened = load(data.path())?.unwrap();
        assert_eq!(opened.account.user, saved.account.user);
        assert_eq!(opened.account.server, saved.account.server);
        assert_eq!(opened.account.avatar, saved.account.avatar);
        assert!(opened.token == saved.token);
        assert!(crate::dpapi::transform(&raw, b"other scope", false).is_err());

        saved.account.user.id = uuid::Uuid::new_v4();
        saved.account.server = "https://other.example.com/".into();
        saved.account.avatar = None;
        save(data.path(), &saved)?;
        assert_eq!(load(data.path())?.unwrap().account.user, saved.account.user);
        fs::write(data.path().join(FILE), b"invalid file")?;
        assert!(load(data.path()).is_err());
        clear(data.path())?;
        assert!(load(data.path())?.is_none());
        clear(data.path())?;
        Ok(())
    }
}
