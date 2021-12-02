use std::fs::File;
use std::io::{self, BufReader};
use std::sync::Arc;
use std::time::SystemTime;

use rustls::client::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier, ServerName};
use rustls::internal::msgs::enums::SignatureScheme;
use rustls::internal::msgs::handshake::DigitallySignedStruct;
use rustls::internal::msgs::handshake::DistinguishedNames;
use rustls::server::{ClientCertVerified, ClientCertVerifier, ResolvesServerCertUsingSni};
use rustls::sign::{self, CertifiedKey};
use rustls::{Certificate, Error, PrivateKey};
use rustls_pemfile::{certs, pkcs8_private_keys};
use tokio_rustls::rustls;
use tokio_rustls::TlsAcceptor;

use crate::config;

pub fn tls_acceptor_conf(cfg: config::Config) -> io::Result<TlsAcceptor> {
    let resolver = load_keypair(cfg)?;
    let config = rustls::server::ServerConfig::builder()
        .with_safe_defaults()
        .with_client_cert_verifier(Arc::new(GeminiClientAuth))
        .with_cert_resolver(Arc::new(resolver));
    let acceptor = TlsAcceptor::from(Arc::new(config));

    Ok(acceptor)
}

pub fn load_certs(path: &String) -> io::Result<Vec<Certificate>> {
    certs(&mut BufReader::new(File::open(path)?))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid cert"))
        .map(|mut certs| certs.drain(..).map(Certificate).collect())
}

fn load_key(path: &String) -> io::Result<Vec<PrivateKey>> {
    pkcs8_private_keys(&mut std::io::BufReader::new(std::fs::File::open(path)?))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid key"))
        .map(|mut keys| keys.drain(..).map(PrivateKey).collect())
}

fn load_keypair(cfg: config::Config) -> io::Result<ResolvesServerCertUsingSni> {
    let mut resolver = rustls::server::ResolvesServerCertUsingSni::new();

    for server in cfg.server.iter() {
        let key = load_key(&server.key)?.remove(0);
        let certs = load_certs(&server.cert)?;
        let signing_key = sign::any_supported_type(&key).expect("error loading key");

        resolver
            .add(
                &server.hostname.clone(),
                CertifiedKey::new(certs, signing_key),
            )
            .expect("error loading key");
    }
    Ok(resolver)
}

struct GeminiClientAuth;

impl ClientCertVerifier for GeminiClientAuth {
    fn client_auth_root_subjects(&self) -> Option<DistinguishedNames> {
        Some(Vec::new())
    }

    fn verify_client_cert(
        &self,
        _end_entity: &Certificate,
        _intermidiates: &[Certificate],
        _now: SystemTime,
    ) -> Result<ClientCertVerified, Error> {
        Ok(ClientCertVerified::assertion())
    }

    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> Option<bool> {
        Some(false)
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &Certificate,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &Certificate,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        tokio_rustls::rustls::client::WebPkiVerifier::verification_schemes()
    }
}

pub struct GeminiServerAuth;

impl ServerCertVerifier for GeminiServerAuth {
    fn verify_server_cert(
        &self,
        _end_entity: &Certificate,
        _intermediates: &[Certificate],
        _server_name: &ServerName,
        _scts: &mut dyn Iterator<Item = &[u8]>,
        _ocsp_response: &[u8],
        _now: SystemTime,
    ) -> Result<ServerCertVerified, Error> {
        Ok(ServerCertVerified::assertion())
    }
}
// This was pull out of the depths of git.
// Rustls won't let self signed certs be used with sni which gemini requires.
// At 1.4.2 in https://gemini.circumlunar.space/docs/spec-spec.txt
/*
pub struct CertResolver {
    map: HashMap<String, Box<CertifiedKey>>,
}

impl CertResolver {
    pub fn from_config(cfg: config::Config) -> errors::Result<Self> {
        let mut map = HashMap::new();

        for server in cfg.server.iter() {
            let key = load_key(&server.key)?;
            let certs = load_certs(&server.cert)?;
            let signing_key = RsaSigningKey::new(&key).unwrap();

            //let signing_key_boxed: Arc<Box<dyn SigningKey>> = Arc::new(Box::new(signing_key));
            let signing_key_boxed = Arc::new(signing_key);
            map.insert(
                server.hostname.clone(),
                Box::new(CertifiedKey::new(certs, signing_key_boxed)),
            );
        }

        Ok(CertResolver { map })
    }
}

impl ResolvesServerCert for CertResolver {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        if let Some(hostname) = client_hello.server_name() {
            if let Some(cert) = self.map.get(hostname.into()) {
                let cert_box = Arc::new(cert);
                return Some(&cert_box);
            }
        }

        None
    }
}
*/
