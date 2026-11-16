use super::{Api, Client, OnlineError};
use crate::{Device, DeviceLogin, DeviceSession, Session, valid_handle};
use anyhow::{Context, Result, ensure};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{ErrorKind, Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

#[derive(Serialize, Deserialize)]
pub(super) struct Installation {
    pub key: String,
    pub account: Option<Uuid>,
}

impl Drop for Installation {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

const SCOPE: &[u8] = b"Zibblu Local-First installation v2";

#[cfg(windows)]
fn protect(bytes: &[u8], encrypt: bool, scope: &[u8]) -> Result<Vec<u8>> {
    crate::dpapi::transform(bytes, scope, encrypt)
}

#[cfg(not(windows))]
fn protect(bytes: &[u8], _: bool, _: &[u8]) -> Result<Vec<u8>> {
    Ok(bytes.to_vec())
}

impl Installation {
    fn new() -> Self {
        Self {
            key: format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()),
            account: None,
        }
    }

    fn load(path: &Path, scope: &[u8]) -> Result<Option<Self>> {
        let file = match fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let mut sealed = Zeroizing::new(Vec::new());
        file.take(4097).read_to_end(&mut sealed)?;
        ensure!(sealed.len() <= 4096, "Invalid device credential");
        let plain = Zeroizing::new(protect(&sealed, false, scope)?);
        Ok(Some(serde_json::from_slice(&plain)?))
    }

    fn save(&self, path: &Path, scope: &[u8]) -> Result<()> {
        let plain = Zeroizing::new(serde_json::to_vec(self)?);
        let sealed = Zeroizing::new(protect(&plain, true, scope)?);
        let mut file =
            tempfile::NamedTempFile::new_in(path.parent().context("Missing device directory")?)?;
        file.write_all(&sealed)?;
        file.as_file().sync_all()?;
        file.persist(path)?;
        Ok(())
    }
}

impl Client {
    pub(super) fn profile_at(data: &Path, api: &Api, user: Uuid) -> PathBuf {
        let id = format!("{:x}", Sha256::digest(format!("{}\n{}", api.base, user)));
        data.join("profiles-v2").join(id)
    }

    pub(super) async fn sign_in(
        data: &Path,
        mut api: Api,
        credentials: crate::Credentials,
        register: bool,
    ) -> Result<Arc<Self>> {
        let handle = credentials.handle.trim().to_ascii_lowercase();
        ensure!(valid_handle(&handle), "Enter a valid account handle");
        let id = Sha256::digest(format!("{}\n{handle}", api.base));
        let mut scope = SCOPE.to_vec();
        scope.extend_from_slice(&id);
        let dir = data.join("devices-v2");
        fs::create_dir_all(&dir)?;
        let lock = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join(format!("{id:x}.lock")))?;
        lock.try_lock()
            .context("A sign-in is already in progress for this account")?;
        let path = dir.join(format!("{id:x}.bin"));
        let mut installation = match Installation::load(&path, &scope)? {
            Some(saved)
                if saved.account.is_none_or(|account| {
                    Self::profile_at(data, &api, account)
                        .join("chat.sqlite3")
                        .is_file()
                }) =>
            {
                saved
            }
            _ => {
                let fresh = Installation::new();
                fresh.save(&path, &scope)?;
                fresh
            }
        };
        let route = if register {
            "/v2/register"
        } else {
            "/v2/login"
        };
        let mut login = DeviceLogin {
            credentials,
            device_key: installation.key.clone(),
        };
        let request = api.request(Method::POST, route).json(&login);
        let session = match api.json::<Session>(request).await {
            Err(OnlineError::Rejected {
                status: StatusCode::GONE,
                ..
            }) if !register => {
                installation = Installation::new();
                installation.save(&path, &scope)?;
                login.device_key.zeroize();
                login.device_key.clone_from(&installation.key);
                let request = api.request(Method::POST, route).json(&login);
                login.device_key.zeroize();
                login.credentials.password.zeroize();
                api.json(request).await?
            }
            result => {
                login.device_key.zeroize();
                login.credentials.password.zeroize();
                result?
            }
        };
        api.token = Some(session.token);
        let client = Self::open(
            data,
            api,
            DeviceSession {
                user: session.user,
                device_id: session.device_id,
                cursor: session.cursor,
            },
            false,
        )
        .await?;
        installation.account = Some(client.user.id);
        installation.save(&path, &scope)?;
        client.reconcile_history().await?;
        Ok(client)
    }

    pub async fn devices(&self) -> Result<Vec<Device>> {
        self.ensure_open()?;
        Ok(self
            .api
            .json(self.api.request(Method::GET, "/v2/devices"))
            .await?)
    }

    pub async fn retire_device(&self, id: Uuid) -> Result<()> {
        self.ensure_open()?;
        self.api
            .send(
                self.api
                    .request(Method::DELETE, &format!("/v2/devices/{id}")),
            )
            .await?;
        Ok(())
    }
}
