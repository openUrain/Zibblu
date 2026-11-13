use crate::{App, Auth, Error, Result};
use axum::{Json, extract::State};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose, SubjectPublicKeyInfo,
};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use zibblu_core::{LAN_NEARBY_NAME, LanCredential, LanCredentialRequest, lan_peer_name};

pub(crate) struct Authority {
    issuer: Issuer<'static, KeyPair>,
    der: Vec<u8>,
}

impl Authority {
    pub(crate) async fn load(db: &PgPool) -> anyhow::Result<Self> {
        let existing: Option<(Vec<u8>, Vec<u8>)> =
            sqlx::query_as("SELECT certificate, signing_key FROM lan_authority WHERE singleton")
                .fetch_optional(db)
                .await?;
        let (der, key) = match existing {
            Some(existing) => existing,
            None => {
                let mut params = CertificateParams::default();
                params.distinguished_name = name("Zibblu LAN device authority");
                params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
                params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
                let key = KeyPair::generate()?;
                let certificate = params.self_signed(&key)?;
                sqlx::query(
                    "INSERT INTO lan_authority (singleton, certificate, signing_key)
                     VALUES (true, $1, $2) ON CONFLICT (singleton) DO NOTHING",
                )
                .bind(certificate.der().as_ref())
                .bind(key.serialize_der())
                .execute(db)
                .await?;
                sqlx::query_as("SELECT certificate, signing_key FROM lan_authority WHERE singleton")
                    .fetch_one(db)
                    .await?
            }
        };
        let issuer =
            Issuer::from_ca_cert_der(&der.clone().into(), KeyPair::try_from(key.as_slice())?)?;
        Ok(Self { issuer, der })
    }
}

fn name(common_name: &str) -> DistinguishedName {
    let mut name = DistinguishedName::new();
    name.push(DnType::CommonName, common_name);
    name
}

pub(crate) async fn issue(
    State(app): State<App>,
    auth: Auth,
    Json(input): Json<LanCredentialRequest>,
) -> Result<Json<LanCredential>> {
    if input.public_key.len() > 256 {
        return Err(Error::bad("Invalid device public key."));
    }
    let key = SubjectPublicKeyInfo::from_der(&input.public_key)
        .map_err(|_| Error::bad("Invalid device public key."))?;
    let handle: String = sqlx::query_scalar("SELECT handle FROM accounts WHERE id = $1")
        .bind(auth.user_id)
        .fetch_one(&app.db)
        .await?;
    let now = OffsetDateTime::now_utc();
    let expiry =
        OffsetDateTime::from_unix_timestamp(auth.expires_at).map_err(|_| Error::internal())?;
    let mut params =
        CertificateParams::new(vec![lan_peer_name(auth.user_id), LAN_NEARBY_NAME.into()])
            .map_err(|_| Error::internal())?;
    // Older LAN clients validate the certificate CN as a lowercase handle.
    params.distinguished_name = name(&handle.to_ascii_lowercase());
    params.not_before = now - Duration::minutes(1);
    params.not_after = (now + Duration::hours(1)).min(expiry);
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![
        ExtendedKeyUsagePurpose::ServerAuth,
        ExtendedKeyUsagePurpose::ClientAuth,
    ];
    let expires_at = params.not_after.unix_timestamp();
    let certificate = params
        .signed_by(&key, &app.lan_authority.issuer)
        .map_err(|_| Error::internal())?;
    Ok(Json(LanCredential {
        certificate: certificate.der().to_vec(),
        authority: app.lan_authority.der.clone(),
        expires_at,
    }))
}
