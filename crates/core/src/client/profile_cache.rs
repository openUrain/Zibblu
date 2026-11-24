use super::{Client, PublicResourceKind, unix_millis};
use crate::{Profile, ProfileRelation};
use serde::Serialize;
use sqlx::SqliteConnection;
use uuid::Uuid;

pub(super) const PROFILE_CACHE_AGE_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Serialize)]
pub struct CachedProfile {
    pub profile: Profile,
    pub expires_at: i64,
}

const DIRECTORY_CONTACTS: &str = "SELECT json_extract(contact.value, '$.id')
    FROM directory, json_each(directory.payload, '$.contacts') contact";
const DIRECTORY_AVATAR: &str = "EXISTS (SELECT 1 FROM directory
    WHERE json_extract(payload, '$.avatars.\"' || ?1 || '\"') = ?2)";

fn profile_image_scope() -> String {
    format!(
        "({DIRECTORY_AVATAR} OR ((?1 = ?4 OR ?1 IN ({DIRECTORY_CONTACTS})) AND EXISTS (
        SELECT 1 FROM cached_profiles WHERE user_id = ?1 AND expires_at > ?3
          AND ?2 IN (json_extract(payload, '$.avatar'), json_extract(payload, '$.cover'))
    )))"
    )
}

impl Client {
    pub async fn cached_profile(&self, id: Uuid) -> Option<CachedProfile> {
        self.ensure_open().ok()?;
        let (payload, expires_at): (String, i64) = sqlx::query_as(&format!(
            "SELECT payload, expires_at FROM cached_profiles
             WHERE user_id = ?1 AND expires_at > ?2
               AND (?1 = ?3 OR ?1 IN ({DIRECTORY_CONTACTS}))",
        ))
        .bind(id.to_string())
        .bind(unix_millis())
        .bind(self.user.id.to_string())
        .fetch_optional(&self.db)
        .await
        .ok()??;
        Some(CachedProfile {
            profile: serde_json::from_str(&payload).ok()?,
            expires_at,
        })
    }

    pub fn fresh_profile(profile: Profile) -> CachedProfile {
        CachedProfile {
            profile,
            expires_at: unix_millis() + PROFILE_CACHE_AGE_MS,
        }
    }

    pub(super) async fn remember_profile(&self, profile: &Profile) {
        let id = profile.user.id;
        if !matches!(
            profile.relation,
            ProfileRelation::Own | ProfileRelation::Contact
        ) {
            self.forget_profile(id).await;
            return;
        }
        let Ok(payload) = serde_json::to_string(profile) else {
            return;
        };
        let now = unix_millis();
        let expires_at = now + PROFILE_CACHE_AGE_MS;
        let Ok(mut tx) = self.db.begin_with("BEGIN IMMEDIATE").await else {
            return;
        };
        if sqlx::query(&format!(
            "INSERT INTO cached_profiles (user_id, payload, expires_at)
             SELECT ?1, ?2, ?3 WHERE ?1 = ?4 OR ?1 IN ({DIRECTORY_CONTACTS})
             ON CONFLICT (user_id) DO UPDATE SET payload = excluded.payload, expires_at = excluded.expires_at
             WHERE cached_profiles.payload <> excluded.payload
                OR cached_profiles.expires_at <= excluded.expires_at - 3600000",
        ))
        .bind(id.to_string())
        .bind(payload)
        .bind(expires_at)
        .bind(self.user.id.to_string())
        .execute(&mut *tx)
        .await
        .is_err()
        {
            return;
        }
        if let Some(avatar) = profile.avatar {
            let _ = sqlx::query(
                "UPDATE public_cache_refs SET desired_version = ?1, updated_at = ?2, expires_at = max(expires_at, ?3)
                 WHERE user_id = ?4 AND kind = 'avatar'",
            )
            .bind(avatar.to_string())
            .bind(now)
            .bind(expires_at)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await;
        }
        if let Some(cover) = profile.cover {
            let _ = sqlx::query(
                "UPDATE public_cache_refs SET desired_version = ?1, updated_at = ?2, expires_at = max(expires_at, ?3)
                 WHERE user_id = ?4 AND kind = 'cover'",
            )
            .bind(cover.to_string())
            .bind(now)
            .bind(expires_at)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await;
        }
        let evicted = sqlx::query(
            "DELETE FROM cached_profiles WHERE user_id <> ?1 AND user_id IN (
                SELECT user_id FROM cached_profiles WHERE user_id <> ?1
                ORDER BY expires_at DESC, user_id DESC LIMIT -1 OFFSET 255
             )",
        )
        .bind(self.user.id.to_string())
        .execute(&mut *tx)
        .await
        .is_ok();
        if evicted
            && let Ok(removed) = self.prune_profile_cache(&mut tx).await
            && tx.commit().await.is_ok()
        {
            self.remove_rel_paths(removed).await;
        }
    }

    pub(super) async fn forget_profile(&self, id: Uuid) {
        let Ok(mut tx) = self.db.begin_with("BEGIN IMMEDIATE").await else {
            return;
        };
        let deleted = sqlx::query("DELETE FROM cached_profiles WHERE user_id = ?")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await
            .is_ok();
        if deleted
            && let Ok(removed) = self.prune_profile_cache(&mut tx).await
            && tx.commit().await.is_ok()
        {
            self.remove_rel_paths(removed).await;
        }
    }

    pub async fn cached_profile_image(&self, id: Uuid, version: Uuid) -> Option<Vec<u8>> {
        self.ensure_open().ok()?;
        let now = unix_millis();
        let row: (i64, String, i64, String) = sqlx::query_as(&format!(
            "SELECT f.id, f.rel_path, f.size, f.sha256
             FROM public_cache_files f
             WHERE f.device_id = ?5
               AND f.version = ?2
               AND f.status = 'available'
               AND f.expires_at > ?3
               AND (
                   (f.kind = 'avatar' AND (
                       {DIRECTORY_AVATAR}
                       OR ((?1 = ?4 OR ?1 IN ({DIRECTORY_CONTACTS})) AND EXISTS (
                           SELECT 1 FROM cached_profiles
                           WHERE user_id = ?1 AND expires_at > ?3
                             AND json_extract(payload, '$.avatar') = ?2
                       ))
                   ))
                   OR (f.kind = 'cover' AND (?1 = ?4 OR ?1 IN ({DIRECTORY_CONTACTS})) AND EXISTS (
                       SELECT 1 FROM cached_profiles
                       WHERE user_id = ?1 AND expires_at > ?3
                         AND json_extract(payload, '$.cover') = ?2
                   ))
               )
             LIMIT 1",
        ))
        .bind(id.to_string())
        .bind(version.to_string())
        .bind(now)
        .bind(self.user.id.to_string())
        .bind(self.device_id.to_string())
        .fetch_optional(&self.db)
        .await
        .ok()??;
        self.read_public_image_file(row.0, &row.1, row.2, &row.3)
            .await
    }

    pub async fn active_profile_image(
        &self,
        id: Uuid,
        kind: PublicResourceKind,
    ) -> Option<(Uuid, Vec<u8>)> {
        self.ensure_open().ok()?;
        let now = unix_millis();
        let allowed: bool = match kind {
            PublicResourceKind::Avatar => sqlx::query_scalar(&format!(
                "SELECT ?1 = ?2
                 OR EXISTS (SELECT 1 FROM directory WHERE json_extract(payload, '$.avatars.\"' || ?1 || '\"') IS NOT NULL)
                 OR (?1 IN ({DIRECTORY_CONTACTS}))",
            ))
            .bind(id.to_string())
            .bind(self.user.id.to_string())
            .fetch_one(&self.db)
            .await
            .unwrap_or(false),
            PublicResourceKind::Cover => sqlx::query_scalar(&format!(
                "SELECT (?1 = ?2 OR ?1 IN ({DIRECTORY_CONTACTS}))
                 AND EXISTS (SELECT 1 FROM cached_profiles WHERE user_id = ?1 AND expires_at > ?3
                             AND json_extract(payload, '$.cover') IS NOT NULL)",
            ))
            .bind(id.to_string())
            .bind(self.user.id.to_string())
            .bind(now)
            .fetch_one(&self.db)
            .await
            .unwrap_or(false),
        };
        if !allowed {
            return None;
        }
        let row: (i64, String, String, i64, String) = sqlx::query_as(
            "SELECT f.id, f.version, f.rel_path, f.size, f.sha256
             FROM public_cache_refs r
             JOIN public_cache_files f ON f.id = r.active_file_id
             WHERE r.user_id = ?1 AND r.kind = ?2
               AND r.expires_at > ?3
               AND f.device_id = ?4 AND f.status = 'available' AND f.expires_at > ?3",
        )
        .bind(id.to_string())
        .bind(kind.as_str())
        .bind(now)
        .bind(self.device_id.to_string())
        .fetch_optional(&self.db)
        .await
        .ok()??;
        let active_version = Uuid::parse_str(&row.1).ok()?;
        let bytes = self
            .read_public_image_file(row.0, &row.2, row.3, &row.4)
            .await?;
        Some((active_version, bytes))
    }

    pub async fn active_profile_image_for_request(
        &self,
        id: Uuid,
        requested_version: Uuid,
    ) -> Option<(Uuid, Vec<u8>)> {
        self.ensure_open().ok()?;
        let user_str = id.to_string();
        let ver_str = requested_version.to_string();
        let is_avatar: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM directory WHERE json_extract(payload, '$.avatars.\"' || ?1 || '\"') = ?2)
                 OR EXISTS(SELECT 1 FROM cached_profiles WHERE user_id = ?1 AND json_extract(payload, '$.avatar') = ?2)",
        )
        .bind(&user_str)
        .bind(&ver_str)
        .fetch_one(&self.db)
        .await
        .unwrap_or(false);
        if is_avatar {
            return self.active_profile_image(id, PublicResourceKind::Avatar).await;
        }
        let is_cover: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM cached_profiles WHERE user_id = ?1 AND json_extract(payload, '$.cover') = ?2)",
        )
        .bind(&user_str)
        .bind(&ver_str)
        .fetch_one(&self.db)
        .await
        .unwrap_or(false);
        if is_cover {
            return self.active_profile_image(id, PublicResourceKind::Cover).await;
        }
        None
    }

    pub(super) async fn forget_profile_image(&self, id: Uuid, version: Uuid) {
        let Ok(mut tx) = self.db.begin_with("BEGIN IMMEDIATE").await else {
            return;
        };
        let _ = sqlx::query(
            "DELETE FROM public_cache_refs WHERE user_id = ?1 AND active_version = ?2",
        )
        .bind(id.to_string())
        .bind(version.to_string())
        .execute(&mut *tx)
        .await;
        let removed: Vec<String> = sqlx::query_scalar(
            "DELETE FROM public_cache_files
             WHERE device_id = ?1 AND version = ?2
               AND id NOT IN (SELECT active_file_id FROM public_cache_refs WHERE active_file_id IS NOT NULL)
             RETURNING rel_path",
        )
        .bind(self.device_id.to_string())
        .bind(version.to_string())
        .fetch_all(&mut *tx)
        .await
        .unwrap_or_default();
        if tx.commit().await.is_ok() {
            self.remove_rel_paths(removed).await;
        }
    }

    pub(super) async fn remember_profile_image(&self, id: Uuid, version: Uuid, bytes: &[u8]) {
        let now = unix_millis();
        let user_str = id.to_string();
        let ver_str = version.to_string();
        let own_str = self.user.id.to_string();
        let allowed: bool = sqlx::query_scalar(&format!(
            "SELECT {}",
            profile_image_scope()
        ))
        .bind(&user_str)
        .bind(&ver_str)
        .bind(now)
        .bind(&own_str)
        .fetch_one(&self.db)
        .await
        .unwrap_or(false);
        if !allowed {
            return;
        }
        let is_avatar: bool = sqlx::query_scalar(&format!(
            "SELECT {DIRECTORY_AVATAR} OR ((?1 = ?4 OR ?1 IN ({DIRECTORY_CONTACTS})) AND EXISTS (
                SELECT 1 FROM cached_profiles WHERE user_id = ?1 AND expires_at > ?3
                  AND json_extract(payload, '$.avatar') = ?2
            ))",
        ))
        .bind(&user_str)
        .bind(&ver_str)
        .bind(now)
        .bind(&own_str)
        .fetch_one(&self.db)
        .await
        .unwrap_or(false);
        let is_cover: bool = sqlx::query_scalar(&format!(
            "SELECT (?1 = ?4 OR ?1 IN ({DIRECTORY_CONTACTS})) AND EXISTS (
                SELECT 1 FROM cached_profiles WHERE user_id = ?1 AND expires_at > ?3
                  AND json_extract(payload, '$.cover') = ?2
            )",
        ))
        .bind(&user_str)
        .bind(&ver_str)
        .bind(now)
        .bind(&own_str)
        .fetch_one(&self.db)
        .await
        .unwrap_or(false);

        let expires_at = now + PROFILE_CACHE_AGE_MS;
        if is_avatar {
            let _ = self
                .commit_public_image(id, PublicResourceKind::Avatar, version, bytes, expires_at)
                .await;
        }
        if is_cover {
            let _ = self
                .commit_public_image(id, PublicResourceKind::Cover, version, bytes, expires_at)
                .await;
        }
    }

    pub(super) async fn remember_profile_edit(
        &self,
        id: Uuid,
        field: &str,
        value: impl Serialize,
        directory_field: Option<&str>,
    ) {
        let Ok(value) = serde_json::to_string(&value) else {
            return;
        };
        let _sync = self.sync_gate.lock().await;
        let Ok(mut tx) = self.db.begin().await else {
            return;
        };
        if sqlx::query(
            "UPDATE cached_profiles SET payload = json_set(payload, ?, json(?)) WHERE user_id = ?",
        )
        .bind(format!("$.{field}"))
        .bind(&value)
        .bind(id.to_string())
        .execute(&mut *tx)
        .await
        .is_err()
        {
            return;
        }
        if let Some(field) = directory_field
            && sqlx::query(
                "UPDATE directory SET payload = CASE WHEN ?2 = 'null' THEN json_remove(payload, ?1)
                 ELSE json_set(payload, ?1, json(?2)) END WHERE id = 1",
            )
            .bind(format!("$.{field}.\"{id}\""))
            .bind(&value)
            .execute(&mut *tx)
            .await
            .is_err()
        {
            return;
        }
        if tx.commit().await.is_ok() && directory_field.is_some() {
            self.history_changed();
        }
    }

    pub(super) async fn prune_profile_cache(
        &self,
        db: &mut SqliteConnection,
    ) -> anyhow::Result<Vec<String>> {
        prune(db, self.user.id, self.device_id, unix_millis()).await
    }
}

async fn prune(
    db: &mut SqliteConnection,
    own: Uuid,
    device_id: Uuid,
    now: i64,
) -> anyhow::Result<Vec<String>> {
    let own_str = own.to_string();
    let device_str = device_id.to_string();
    sqlx::query(&format!(
        "DELETE FROM cached_profiles WHERE expires_at <= ?1 OR (user_id <> ?2 AND user_id NOT IN ({DIRECTORY_CONTACTS}))",
    ))
    .bind(now)
    .bind(&own_str)
    .execute(&mut *db)
    .await?;

    // Update desired_version for avatars announced in directory without dropping active_file_id yet.
    sqlx::query(
        "UPDATE public_cache_refs
         SET desired_version = (
             SELECT json_extract(d.payload, '$.avatars.\"' || public_cache_refs.user_id || '\"')
             FROM directory d WHERE d.id = 1
         )
         WHERE kind = 'avatar'
           AND EXISTS (
               SELECT 1 FROM directory d
               WHERE d.id = 1
                 AND json_extract(d.payload, '$.avatars.\"' || public_cache_refs.user_id || '\"') IS NOT NULL
           )",
    )
    .execute(&mut *db)
    .await?;

    // Revoke expired refs, revoked contact covers, or explicitly cleared avatars/covers.
    sqlx::query(&format!(
        "DELETE FROM public_cache_refs
         WHERE expires_at <= ?1
            OR (kind = 'cover' AND (
                (user_id <> ?2 AND user_id NOT IN ({DIRECTORY_CONTACTS}))
                OR NOT EXISTS (
                    SELECT 1 FROM cached_profiles p
                    WHERE p.user_id = public_cache_refs.user_id
                      AND json_extract(p.payload, '$.cover') IS NOT NULL
                )
            ))
            OR (kind = 'avatar' AND
                NOT EXISTS (
                    SELECT 1 FROM directory d
                    WHERE d.id = 1
                      AND json_extract(d.payload, '$.avatars.\"' || public_cache_refs.user_id || '\"') IS NOT NULL
                )
                AND NOT EXISTS (
                    SELECT 1 FROM cached_profiles p
                    WHERE p.user_id = public_cache_refs.user_id
                      AND json_extract(p.payload, '$.avatar') IS NOT NULL
                )
            )",
    ))
    .bind(now)
    .bind(&own_str)
    .execute(&mut *db)
    .await?;

    // Reclaim expired or confirmed unreferenced public cache files.
    let removed: Vec<String> = sqlx::query_scalar(
        "DELETE FROM public_cache_files
         WHERE device_id = ?1 AND (
             expires_at <= ?2
             OR id NOT IN (
                 SELECT active_file_id FROM public_cache_refs WHERE active_file_id IS NOT NULL
             )
         )
         RETURNING rel_path",
    )
    .bind(&device_str)
    .bind(now)
    .fetch_all(&mut *db)
    .await?;
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Api;
    use crate::{DeviceSession, User};

    #[tokio::test]
    async fn a_reopened_profile_does_not_share_cached_covers_or_keep_revoked_access()
    -> anyhow::Result<()> {
        let data = tempfile::tempdir()?;
        let own = Uuid::new_v4();
        let peer = Uuid::new_v4();
        let version = Uuid::new_v4();
        let session = |id| DeviceSession {
            user: User {
                id,
                handle: "fixture".into(),
            },
            device_id: Uuid::new_v4(),
            cursor: Uuid::new_v4(),
        };
        let service = || Api::new("http://127.0.0.1:7841");
        let client = Client::open(data.path(), service()?, session(own), false).await?;
        let directory = serde_json::json!({ "contacts": [{ "id": peer, "handle": "peer" }] });
        sqlx::query("INSERT INTO directory VALUES (1, ?)")
            .bind(directory.to_string())
            .execute(&client.db)
            .await?;
        client
            .remember_profile(&Profile {
                user: User {
                    id: peer,
                    handle: "peer".into(),
                },
                display_name: None,
                relation: ProfileRelation::Contact,
                avatar: None,
                cover: Some(version),
                bio: Some("contact only".into()),
                notes: None,
            })
            .await;
        client.remember_profile_image(peer, version, &[1, 2, 3]).await;
        let device = sqlx::query_scalar::<_, String>("SELECT device_id FROM sync_checkpoint")
            .fetch_one(&client.db)
            .await?;
        client.close().await;
        drop(client);

        let other = Client::open(data.path(), service()?, session(Uuid::new_v4()), false).await?;
        let other_server = Client::open(
            data.path(),
            Api::new("http://127.0.0.1:7842")?,
            session(own),
            false,
        )
        .await?;
        for unrelated in [other, other_server] {
            assert!(unrelated.cached_profile(peer).await.is_none());
            assert!(unrelated.cached_profile_image(peer, version).await.is_none());
        }

        let client = Client::open(
            data.path(),
            service()?,
            DeviceSession {
                device_id: device.parse()?,
                ..session(own)
            },
            true,
        )
        .await?;
        assert_eq!(
            client.cached_profile(peer).await.unwrap().profile.bio.as_deref(),
            Some("contact only")
        );
        assert_eq!(
            client.cached_profile_image(peer, version).await,
            Some(vec![1, 2, 3])
        );
        sqlx::query("UPDATE cached_profiles SET expires_at = 0")
            .execute(&client.db)
            .await?;
        assert!(client.cached_profile(peer).await.is_none());
        assert!(client.cached_profile_image(peer, version).await.is_none());
        sqlx::query("UPDATE cached_profiles SET expires_at = ?")
            .bind(i64::MAX)
            .execute(&client.db)
            .await?;

        let mut tx = client.db.begin().await?;
        sqlx::query("UPDATE directory SET payload = '{\"contacts\":[]}'")
            .execute(&mut *tx)
            .await?;
        let removed = client.prune_profile_cache(&mut tx).await?;
        tx.commit().await?;
        client.remove_rel_paths(removed).await;
        assert!(client.cached_profile(peer).await.is_none());
        assert!(client.cached_profile_image(peer, version).await.is_none());
        let retained: i64 = sqlx::query_scalar("SELECT count(*) FROM public_cache_files")
            .fetch_one(&client.db)
            .await?;
        assert_eq!(retained, 0);
        Ok(())
    }
}
