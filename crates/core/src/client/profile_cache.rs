use super::{Client, unix_millis};
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
    format!("({DIRECTORY_AVATAR} OR ((?1 = ?4 OR ?1 IN ({DIRECTORY_CONTACTS})) AND EXISTS (
        SELECT 1 FROM cached_profiles WHERE user_id = ?1 AND expires_at > ?3
          AND ?2 IN (json_extract(payload, '$.avatar'), json_extract(payload, '$.cover'))
    )))")
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
        CachedProfile { profile, expires_at: unix_millis() + PROFILE_CACHE_AGE_MS }
    }

    pub(super) async fn remember_profile(&self, profile: &Profile) {
        let id = profile.user.id;
        if !matches!(profile.relation, ProfileRelation::Own | ProfileRelation::Contact) {
            self.forget_profile(id).await;
            return;
        }
        let Ok(payload) = serde_json::to_string(profile) else {
            return;
        };
        let Ok(mut tx) = self.db.begin().await else { return };
        if sqlx::query(&format!(
            "INSERT INTO cached_profiles (user_id, payload, expires_at)
             SELECT ?1, ?2, ?3 WHERE ?1 = ?4 OR ?1 IN ({DIRECTORY_CONTACTS})
             ON CONFLICT (user_id) DO UPDATE SET payload = excluded.payload, expires_at = excluded.expires_at
             WHERE cached_profiles.payload <> excluded.payload
                OR cached_profiles.expires_at <= excluded.expires_at - 3600000",
        ))
        .bind(id.to_string())
        .bind(payload)
        .bind(unix_millis() + PROFILE_CACHE_AGE_MS)
        .bind(self.user.id.to_string())
        .execute(&mut *tx)
        .await.is_err() {
            return;
        }
        if sqlx::query(
            "DELETE FROM cached_profiles WHERE user_id <> ?1 AND user_id IN (
                SELECT user_id FROM cached_profiles WHERE user_id <> ?1
                ORDER BY expires_at DESC, user_id DESC LIMIT -1 OFFSET 255
             )",
        )
        .bind(self.user.id.to_string())
        .execute(&mut *tx)
        .await.is_ok() && self.prune_profile_cache(&mut tx).await.is_ok() {
            let _ = tx.commit().await;
        }
    }

    pub(super) async fn forget_profile(&self, id: Uuid) {
        let Ok(mut tx) = self.db.begin().await else { return };
        if sqlx::query("DELETE FROM cached_profiles WHERE user_id = ?")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await.is_ok() && self.prune_profile_cache(&mut tx).await.is_ok() {
            let _ = tx.commit().await;
        }
    }

    pub async fn cached_profile_image(&self, id: Uuid, version: Uuid) -> Option<Vec<u8>> {
        self.ensure_open().ok()?;
        sqlx::query_scalar(&format!(
            "SELECT data FROM cached_profile_images
             WHERE user_id = ?1 AND version = ?2 AND expires_at > ?3 AND {}",
            profile_image_scope(),
        ))
        .bind(id.to_string())
        .bind(version.to_string())
        .bind(unix_millis())
        .bind(self.user.id.to_string())
        .fetch_optional(&self.db)
        .await
        .ok()?
    }

    pub(super) async fn forget_profile_image(&self, id: Uuid, version: Uuid) {
        let _ = sqlx::query("DELETE FROM cached_profile_images WHERE user_id = ? AND version = ?")
            .bind(id.to_string())
            .bind(version.to_string())
            .execute(&self.db)
            .await;
    }

    pub(super) async fn remember_profile_image(&self, id: Uuid, version: Uuid, bytes: &[u8]) {
        let now = unix_millis();
        let Ok(mut tx) = self.db.begin().await else { return };
        if sqlx::query(&format!(
            "INSERT INTO cached_profile_images
             SELECT ?1, ?2, ?5, ?6 WHERE {}
             ON CONFLICT (user_id, version) DO UPDATE SET expires_at = excluded.expires_at
             WHERE cached_profile_images.expires_at <= excluded.expires_at - 3600000",
            profile_image_scope(),
        ))
        .bind(id.to_string())
        .bind(version.to_string())
        .bind(now)
        .bind(self.user.id.to_string())
        .bind(bytes)
        .bind(now + PROFILE_CACHE_AGE_MS)
        .execute(&mut *tx)
        .await
        .is_ok()
            && sqlx::query(
                "DELETE FROM cached_profile_images WHERE rowid IN (
                    SELECT entry FROM (
                      SELECT rowid AS entry, row_number() OVER recent AS count, sum(length(data)) OVER recent AS budget
                      FROM cached_profile_images WINDOW recent AS (ORDER BY expires_at DESC, rowid DESC)
                    ) WHERE count > 96 OR budget > 33554432
                 )",
            )
            .execute(&mut *tx)
            .await
            .is_ok()
        {
            let _ = tx.commit().await;
        }
    }

    pub(super) async fn remember_profile_edit(
        &self, id: Uuid, field: &str, value: impl Serialize, directory_field: Option<&str>,
    ) {
        let Ok(value) = serde_json::to_string(&value) else { return };
        let _sync = self.sync_gate.lock().await;
        let Ok(mut tx) = self.db.begin().await else { return };
        if sqlx::query("UPDATE cached_profiles SET payload = json_set(payload, ?, json(?)) WHERE user_id = ?")
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

    pub(super) async fn prune_profile_cache(&self, db: &mut SqliteConnection) -> anyhow::Result<()> {
        prune(db, self.user.id, unix_millis()).await
    }
}

async fn prune(db: &mut SqliteConnection, own: Uuid, now: i64) -> anyhow::Result<()> {
    sqlx::query(&format!(
        "DELETE FROM cached_profiles WHERE expires_at <= ?1 OR (user_id <> ?2 AND user_id NOT IN ({DIRECTORY_CONTACTS}))",
    ))
    .bind(now)
    .bind(own.to_string())
    .execute(&mut *db)
    .await?;
    sqlx::query(
        "DELETE FROM cached_profile_images WHERE expires_at <= ?1 OR (
           AND NOT EXISTS (SELECT 1 FROM directory
               WHERE json_extract(payload, '$.avatars.\"' || cached_profile_images.user_id || '\"') = version)
           AND NOT EXISTS (SELECT 1 FROM cached_profiles p WHERE p.user_id = cached_profile_images.user_id
               AND version IN (json_extract(p.payload, '$.avatar'), json_extract(p.payload, '$.cover'))))",
    )
    .bind(now)
    .execute(db)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DeviceSession, User};
    use crate::client::Api;

    #[tokio::test]
    async fn a_reopened_profile_does_not_share_cached_covers_or_keep_revoked_access() -> anyhow::Result<()> {
        let data = tempfile::tempdir()?;
        let own = Uuid::new_v4();
        let peer = Uuid::new_v4();
        let version = Uuid::new_v4();
        let session = |id| DeviceSession {
            user: User { id, handle: "fixture".into() },
            device_id: Uuid::new_v4(),
            cursor: Uuid::new_v4(),
        };
        let service = || Api::new("http://127.0.0.1:7841");
        let client = Client::open(data.path(), service()?, session(own), false).await?;
        let directory = serde_json::json!({ "contacts": [{ "id": peer, "handle": "peer" }] });
        sqlx::query("INSERT INTO directory VALUES (1, ?)")
            .bind(directory.to_string()).execute(&client.db).await?;
        client.remember_profile(&Profile {
            user: User { id: peer, handle: "peer".into() },
            display_name: None, relation: ProfileRelation::Contact, avatar: None,
            cover: Some(version), bio: Some("contact only".into()), notes: None,
        }).await;
        client.remember_profile_image(peer, version, &[1, 2, 3]).await;
        let device = sqlx::query_scalar::<_, String>("SELECT device_id FROM sync_checkpoint")
            .fetch_one(&client.db).await?;
        client.close().await;
        drop(client);

        let other = Client::open(data.path(), service()?, session(Uuid::new_v4()), false).await?;
        let other_server = Client::open(data.path(), Api::new("http://127.0.0.1:7842")?, session(own), false).await?;
        for unrelated in [other, other_server] {
            assert!(unrelated.cached_profile(peer).await.is_none());
            assert!(unrelated.cached_profile_image(peer, version).await.is_none());
        }

        let client = Client::open(data.path(), service()?, DeviceSession {
            device_id: device.parse()?, ..session(own)
        }, true).await?;
        assert_eq!(client.cached_profile(peer).await.unwrap().profile.bio.as_deref(), Some("contact only"));
        assert_eq!(client.cached_profile_image(peer, version).await, Some(vec![1, 2, 3]));
        sqlx::query("UPDATE cached_profiles SET expires_at = 0").execute(&client.db).await?;
        assert!(client.cached_profile(peer).await.is_none());
        assert!(client.cached_profile_image(peer, version).await.is_none());
        sqlx::query("UPDATE cached_profiles SET expires_at = ?").bind(i64::MAX).execute(&client.db).await?;

        let mut tx = client.db.begin().await?;
        sqlx::query("UPDATE directory SET payload = '{\"contacts\":[]}'").execute(&mut *tx).await?;
        client.prune_profile_cache(&mut tx).await?;
        tx.commit().await?;
        assert!(client.cached_profile(peer).await.is_none());
        assert!(client.cached_profile_image(peer, version).await.is_none());
        let retained: i64 = sqlx::query_scalar("SELECT count(*) FROM cached_profile_images").fetch_one(&client.db).await?;
        assert_eq!(retained, 0);
        Ok(())
    }
}
