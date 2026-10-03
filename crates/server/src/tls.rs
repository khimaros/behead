//! rustls backed tls endpoint for the session core

use crate::x509;
use aap::{Error, Tls};
use rustls::client::danger::HandshakeSignatureValid;
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{
    AlertDescription, CipherSuite, DigitallySignedStruct, DistinguishedName, ProtocolVersion, ServerConfig,
    ServerConnection, SignatureScheme,
};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// ClientHello layout: record header, handshake header, then the version and
/// random that precede the session id and cipher suites
const RECORD_HEADER_LEN: usize = 5;
const HANDSHAKE_HEADER_LEN: usize = 4;
const VERSION_LEN: usize = 2;
const RANDOM_LEN: usize = 32;

/// alerts a client sends when it does not accept the server's certificate
const CERTIFICATE_REJECTIONS: [AlertDescription; 6] = [
    AlertDescription::BadCertificate,
    AlertDescription::UnsupportedCertificate,
    AlertDescription::CertificateRevoked,
    AlertDescription::CertificateExpired,
    AlertDescription::CertificateUnknown,
    AlertDescription::UnknownCA,
];

/// requests the headunit certificate like a phone does, then accepts any.
/// the handshake signature is not checked either: it would only prove
/// possession of a key nobody vouches for, and checking it means parsing the
/// certificate with webpki, which rejects the x.509 v1 certificates that
/// openauto and older headunits present.
#[derive(Debug)]
struct AnyHeadunit(WebPkiSupportedAlgorithms);

impl ClientCertVerifier for AnyHeadunit {
    fn client_auth_mandatory(&self) -> bool {
        false
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_schemes()
    }
}

/// build the tls 1.2 server config from one phone certificate and key, with
/// a description of the certificate for the log
fn load_config(cert: &Path, key_path: &Path) -> Result<(String, Arc<ServerConfig>), String> {
    let certs = CertificateDer::pem_file_iter(cert)
        .and_then(Iterator::collect::<Result<Vec<_>, _>>)
        .map_err(|e| format!("reading {}: {e}", cert.display()))?;
    let key = PrivateKeyDer::from_pem_file(key_path).map_err(|e| format!("reading {}: {e}", key_path.display()))?;
    let described = certs.first().and_then(|cert| x509::describe(cert));
    let description = format!("{} ({})", described.as_deref().unwrap_or("(unreadable)"), cert.display());
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(AnyHeadunit(provider.signature_verification_algorithms));
    let config = ServerConfig::builder_with_provider(provider as Arc<CryptoProvider>)
        .with_protocol_versions(&[&rustls::version::TLS12])
        .and_then(|builder| builder.with_client_cert_verifier(verifier).with_single_cert(certs, key))
        .map_err(|e| format!("tls config for {}: {e}", cert.display()))?;
    Ok((description, Arc::new(config)))
}

/// the phone certificates on offer, most preferred first. a headunit only
/// says whether it accepts a certificate by finishing the handshake, so one
/// that refuses is offered the next on its following connection, wrapping
/// around after the last. the certificate it accepts stays in use.
pub struct Credentials {
    configs: Vec<(String, Arc<ServerConfig>)>,
    current: usize,
}

impl Credentials {
    pub fn load(pairs: &[(PathBuf, PathBuf)]) -> Result<Self, String> {
        let configs = pairs.iter().map(|(cert, key)| load_config(cert, key)).collect::<Result<Vec<_>, _>>()?;
        if configs.is_empty() {
            return Err("no phone certificate found".into());
        }
        Ok(Self { configs, current: 0 })
    }

    /// a tls endpoint for the next connection, presenting the current certificate
    pub fn endpoint(&self) -> Result<RustlsTls, String> {
        let (description, config) = &self.configs[self.current];
        RustlsTls::new(config.clone(), description.clone())
    }

    pub fn advance(&mut self) {
        self.current = (self.current + 1) % self.configs.len();
        if self.configs.len() > 1 {
            eprintln!("tls: handshake not completed, the next connection gets certificate {}", self.current + 1);
        }
    }
}

pub struct RustlsTls {
    connection: ServerConnection,
    /// the phone certificate, logged once the headunit asks for it
    certificate: String,
    /// whether the headunit has begun the handshake
    started: bool,
    /// whether the handshake outcome has been logged
    reported: bool,
}

/// the headunit's tls version and cipher suites, from its ClientHello record
fn describe_client_hello(record: &[u8]) -> Option<String> {
    let hello = record.get(RECORD_HEADER_LEN + HANDSHAKE_HEADER_LEN..)?;
    let version = u16::from_be_bytes([*hello.first()?, *hello.get(1)?]);
    let session_id_at = VERSION_LEN + RANDOM_LEN;
    let suites_at = session_id_at + 1 + *hello.get(session_id_at)? as usize;
    let suites_len = u16::from_be_bytes([*hello.get(suites_at)?, *hello.get(suites_at + 1)?]) as usize;
    let suites = hello.get(suites_at + 2..suites_at + 2 + suites_len)?;
    let names: Vec<String> = suites
        .chunks_exact(2)
        .map(|pair| format!("{:?}", CipherSuite::from(u16::from_be_bytes([pair[0], pair[1]]))))
        .collect();
    Some(format!("{:?} with cipher suites {}", ProtocolVersion::from(version), names.join(" ")))
}

/// a log line for a tls failure, naming the case that matters most: a
/// headunit that checks the phone certificate and does not accept ours
fn describe_failure(error: &rustls::Error) -> String {
    match error {
        rustls::Error::AlertReceived(alert) if CERTIFICATE_REJECTIONS.contains(alert) => {
            format!("tls: the headunit rejected the phone certificate (alert {alert:?})")
        }
        _ => format!("tls: {error}"),
    }
}

impl RustlsTls {
    pub fn new(config: Arc<ServerConfig>, certificate: String) -> Result<Self, String> {
        let connection = ServerConnection::new(config).map_err(|e| format!("tls: {e}"))?;
        Ok(Self { connection, certificate, started: false, reported: false })
    }

    /// whether the headunit began the handshake and it never finished, the
    /// sign of a headunit that will not accept the phone certificate
    pub fn refused(&self) -> bool {
        self.started && self.connection.is_handshaking()
    }

    fn feed(&mut self, mut input: &[u8]) -> Result<(), Error> {
        while !input.is_empty() {
            self.connection.read_tls(&mut input).map_err(tls_error)?;
            if let Err(error) = self.connection.process_new_packets() {
                eprintln!("{}", describe_failure(&error));
                return Err(tls_error(error));
            }
        }
        Ok(())
    }

    /// log the negotiated parameters and the headunit's certificate, once
    fn report_handshake(&mut self) {
        if self.reported || self.connection.is_handshaking() {
            return;
        }
        self.reported = true;
        let version = self.connection.protocol_version().map_or("unknown version".into(), |v| format!("{v:?}"));
        let suite =
            self.connection.negotiated_cipher_suite().map_or("unknown suite".into(), |s| format!("{:?}", s.suite()));
        let certificate = self.connection.peer_certificates().and_then(|certs| certs.first());
        let headunit = match certificate.map(|cert| x509::describe(cert)) {
            Some(Some(description)) => format!("headunit certificate {description}"),
            Some(None) => "headunit certificate (unreadable)".to_string(),
            None => "headunit sent no certificate".to_string(),
        };
        eprintln!("tls: handshake complete: {version} {suite}, {headunit}");
    }

    fn drain(&mut self) -> Result<Vec<u8>, Error> {
        let mut out = Vec::new();
        while self.connection.wants_write() {
            self.connection.write_tls(&mut out).map_err(tls_error)?;
        }
        Ok(out)
    }
}

fn tls_error(error: impl std::fmt::Display) -> Error {
    Error::Tls(error.to_string())
}

impl Tls for RustlsTls {
    fn handshake(&mut self, input: &[u8]) -> Result<Vec<u8>, Error> {
        if !self.started {
            self.started = true;
            eprintln!("tls: presenting certificate {}", self.certificate);
            if let Some(offer) = describe_client_hello(input) {
                eprintln!("tls: headunit offers {offer}");
            }
        }
        self.feed(input)?;
        let reply = self.drain();
        self.report_handshake();
        reply
    }

    fn encrypt(&mut self, plain: &[u8]) -> Result<Vec<u8>, Error> {
        self.connection.writer().write_all(plain).map_err(tls_error)?;
        self.drain()
    }

    fn decrypt(&mut self, cipher: &[u8]) -> Result<Vec<u8>, Error> {
        self.feed(cipher)?;
        let mut plain = Vec::new();
        match self.connection.reader().read_to_end(&mut plain) {
            Err(e) if e.kind() != std::io::ErrorKind::WouldBlock => Err(tls_error(e)),
            _ => Ok(plain),
        }
    }
}
