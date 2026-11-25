use crate::{LanCredential, User, lan_peer_name, valid_handle};
use anyhow::{Context, Result, ensure};
use quinn::{
    ClientConfig, Connection, ServerConfig,
    crypto::rustls::{QuicClientConfig, QuicServerConfig},
    rustls::{
        self, RootCertStore,
        pki_types::{CertificateDer, PrivatePkcs8KeyDer, UnixTime},
        server::{WebPkiClientVerifier, danger::ClientCertVerifier},
    },
};
use rcgen::{KeyPair, PublicKeyData, SigningKey};
use ring::signature::{ECDSA_P256_SHA256_ASN1, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};
use uuid::Uuid;
use x509_parser::{extensions::GeneralName, parse_x509_certificate};

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Consent {
    certificate: Vec<u8>,
    signed_at: u64,
    signature: Vec<u8>,
}

impl Consent {
    pub fn device(&self) -> Result<[u8; 32]> {
        Ok(parse(&self.certificate)?.2)
    }
}

#[derive(Clone)]
pub struct DeviceKey(Arc<KeyPair>);

impl DeviceKey {
    pub fn generate() -> Result<Self> {
        Ok(Self(Arc::new(KeyPair::generate()?)))
    }

    pub fn public_key(&self) -> Vec<u8> {
        self.0.subject_public_key_info()
    }

    #[cfg(test)]
    pub(super) fn key_pair(&self) -> &KeyPair {
        &self.0
    }
}

pub(super) struct Peer {
    pub user: User,
    pub device: [u8; 32],
}

pub(super) struct Identity {
    key: Arc<KeyPair>,
    certificate: CertificateDer<'static>,
    verifier: Arc<dyn ClientCertVerifier>,
    pub client: ClientConfig,
    pub server: ServerConfig,
    pub device: [u8; 32],
}

impl Identity {
    pub fn new(own: Uuid, key: DeviceKey, credential: LanCredential) -> Result<Self> {
        let key = key.0;
        let certificate = CertificateDer::from(credential.certificate);
        let (user, _, fingerprint) = parse(&certificate)?;
        ensure!(
            user.id == own,
            "The device credential belongs to a different account."
        );
        let own_key: [u8; 32] = Sha256::digest(key.subject_public_key_info()).into();
        ensure!(
            fingerprint == own_key,
            "The device credential belongs to a different key."
        );
        let mut roots = RootCertStore::empty();
        roots.add(credential.authority.into())?;
        let roots = Arc::new(roots);
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier =
            WebPkiClientVerifier::builder_with_provider(roots.clone(), provider.clone()).build()?;
        verifier.verify_client_cert(&certificate, &[], UnixTime::now())?;

        let private_key = PrivatePkcs8KeyDer::from(key.serialize_der());
        let server_crypto = rustls::ServerConfig::builder_with_provider(provider.clone())
            .with_protocol_versions(&[&rustls::version::TLS13])?
            .with_client_cert_verifier(verifier.clone())
            .with_single_cert(vec![certificate.clone()], private_key.clone_key().into())?;
        let client_crypto = rustls::ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])?
            .with_root_certificates(roots)
            .with_client_auth_cert(vec![certificate.clone()], private_key.into())?;
        Ok(Self {
            key,
            certificate,
            verifier,
            client: ClientConfig::new(Arc::new(QuicClientConfig::try_from(client_crypto)?)),
            server: ServerConfig::with_crypto(Arc::new(QuicServerConfig::try_from(server_crypto)?)),
            device: fingerprint,
        })
    }

    pub fn sign(&self, message: &[u8]) -> Result<Consent> {
        let signed_at = UnixTime::now().as_secs();
        Ok(Consent {
            certificate: self.certificate.to_vec(),
            signed_at,
            signature: self.key.sign(&proof_bytes(message, signed_at))?,
        })
    }

    pub fn verify(
        &self,
        consent: &Consent,
        message: &[u8],
        expected: Uuid,
        fresh: bool,
    ) -> Result<()> {
        let now = UnixTime::now().as_secs();
        ensure!(
            consent.signed_at <= now.saturating_add(60)
                && (!fresh || now.saturating_sub(consent.signed_at) <= 60),
            "Invalid approval timestamp."
        );
        let cert = CertificateDer::from(consent.certificate.as_slice());
        let signed_at = UnixTime::since_unix_epoch(Duration::from_secs(consent.signed_at));
        self.verifier.verify_client_cert(&cert, &[], signed_at)?;
        let (user, public_key, _) = parse(&cert)?;
        ensure!(
            user.id == expected,
            "This approval belongs to another account."
        );
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, public_key)
            .verify(&proof_bytes(message, consent.signed_at), &consent.signature)
            .map_err(|_| anyhow::anyhow!("Could not verify account approval."))
    }
}

fn proof_bytes(message: &[u8], signed_at: u64) -> Vec<u8> {
    [message, &signed_at.to_be_bytes()].concat()
}

pub(super) fn peer(connection: &Connection) -> Result<Peer> {
    let certificates = connection
        .peer_identity()
        .and_then(|certificates| certificates.downcast::<Vec<CertificateDer<'static>>>().ok())
        .context("Peer credentials are missing.")?;
    let (user, _, device) = parse(
        certificates
            .first()
            .context("Peer credentials are missing.")?,
    )?;
    Ok(Peer { user, device })
}

fn parse(der: &[u8]) -> Result<(User, Vec<u8>, [u8; 32])> {
    let (_, certificate) =
        parse_x509_certificate(der).map_err(|_| anyhow::anyhow!("Invalid device certificate."))?;
    let name = certificate
        .subject_alternative_name()?
        .and_then(|names| {
            names
                .value
                .general_names
                .iter()
                .find_map(|name| match name {
                    GeneralName::DNSName(name) => Some(*name),
                    _ => None,
                })
        })
        .context("Device account is missing.")?;
    let id = Uuid::parse_str(
        name.strip_suffix(".zibblu.invalid")
            .context("Invalid device account.")?,
    )?;
    ensure!(name == lan_peer_name(id), "Invalid device account.");
    let handle = certificate
        .subject()
        .iter_common_name()
        .next()
        .context("Device handle is missing.")?
        .as_str()?
        .to_owned();
    ensure!(valid_handle(&handle), "Invalid device handle.");
    let key = certificate.public_key();
    let device = Sha256::digest(key.raw).into();
    Ok((
        User { id, handle },
        key.subject_public_key.data.to_vec(),
        device,
    ))
}
