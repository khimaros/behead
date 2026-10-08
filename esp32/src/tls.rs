//! mbedtls backed tls endpoint for the session core. mbedtls comes with
//! esp-idf and uses the chip's crypto accelerators; ring, which the linux
//! server's rustls builds on, has no 32 bit risc-v support.

use aap::{Error, Tls};
use esp_idf_svc::sys;
use std::collections::VecDeque;
use std::ffi::{c_int, c_uchar, c_void};
use std::ptr::{addr_of_mut, null};

const READ_CHUNK: usize = 4096;
const ERROR_TEXT: usize = 128;

/// the records waiting to be read by mbedtls, and those it wrote
#[derive(Default)]
struct Bio {
    incoming: VecDeque<u8>,
    outgoing: Vec<u8>,
}

/// everything mbedtls keeps for one connection. the contexts point at each
/// other, so they live in a box that never moves
struct Contexts {
    ssl: sys::mbedtls_ssl_context,
    config: sys::mbedtls_ssl_config,
    certificate: sys::mbedtls_x509_crt,
    key: sys::mbedtls_pk_context,
    entropy: sys::mbedtls_entropy_context,
    random: sys::mbedtls_ctr_drbg_context,
    bio: Bio,
}

pub struct MbedTls(Box<Contexts>);

// SAFETY: the contexts are only reached through the box, by whichever one
// thread holds the link's lock
unsafe impl Send for MbedTls {}

fn check(what: &str, status: c_int) -> Result<(), String> {
    if status == 0 {
        return Ok(());
    }
    let mut text = [0u8; ERROR_TEXT];
    // SAFETY: the buffer is as long as the length given, and comes back nul terminated
    unsafe { sys::mbedtls_strerror(status, text.as_mut_ptr().cast(), text.len()) };
    let length = text.iter().position(|&byte| byte == 0).unwrap_or(text.len());
    Err(format!("tls: {what}: {}", String::from_utf8_lossy(&text[..length])))
}

/// mbedtls takes pem with its terminating nul counted in
fn terminated(pem: &str) -> Vec<u8> {
    [pem.as_bytes(), &[0]].concat()
}

unsafe extern "C" fn send(bio: *mut c_void, data: *const c_uchar, length: usize) -> c_int {
    // SAFETY: mbedtls hands back the bio pointer it was given, and `length` readable bytes
    let (bio, data) = unsafe { (&mut *bio.cast::<Bio>(), std::slice::from_raw_parts(data, length)) };
    bio.outgoing.extend_from_slice(data);
    length as c_int
}

unsafe extern "C" fn receive(bio: *mut c_void, buffer: *mut c_uchar, length: usize) -> c_int {
    // SAFETY: mbedtls hands back the bio pointer it was given, and `length` writable bytes
    let (bio, buffer) = unsafe { (&mut *bio.cast::<Bio>(), std::slice::from_raw_parts_mut(buffer, length)) };
    if bio.incoming.is_empty() {
        return sys::MBEDTLS_ERR_SSL_WANT_READ;
    }
    let count = length.min(bio.incoming.len());
    bio.incoming.drain(..count).zip(buffer).for_each(|(byte, slot)| *slot = byte);
    count as c_int
}

impl MbedTls {
    /// a tls 1.2 server presenting one phone certificate, both given as pem.
    /// like the linux server it asks for the headunit's certificate and
    /// accepts any
    pub fn new(certificate: &str, key: &str) -> Result<Self, String> {
        // SAFETY: every mbedtls context is plain data that its init function fills in
        let mut contexts: Box<Contexts> = Box::new(Contexts {
            ssl: unsafe { std::mem::zeroed() },
            config: unsafe { std::mem::zeroed() },
            certificate: unsafe { std::mem::zeroed() },
            key: unsafe { std::mem::zeroed() },
            entropy: unsafe { std::mem::zeroed() },
            random: unsafe { std::mem::zeroed() },
            bio: Bio::default(),
        });
        let c = &mut *contexts;
        let (certificate, key) = (terminated(certificate), terminated(key));
        let random: *mut c_void = addr_of_mut!(c.random).cast();
        // SAFETY: the contexts outlive every pointer handed to mbedtls here,
        // since they are freed together in drop, and the pem buffers are
        // copied by the parsers
        unsafe {
            sys::mbedtls_ssl_init(&mut c.ssl);
            sys::mbedtls_ssl_config_init(&mut c.config);
            sys::mbedtls_x509_crt_init(&mut c.certificate);
            sys::mbedtls_pk_init(&mut c.key);
            sys::mbedtls_entropy_init(&mut c.entropy);
            sys::mbedtls_ctr_drbg_init(&mut c.random);
            let entropy: *mut c_void = addr_of_mut!(c.entropy).cast();
            check(
                "seeding",
                sys::mbedtls_ctr_drbg_seed(&mut c.random, Some(sys::mbedtls_entropy_func), entropy, null(), 0),
            )?;
            check(
                "certificate",
                sys::mbedtls_x509_crt_parse(&mut c.certificate, certificate.as_ptr(), certificate.len()),
            )?;
            let parsed = sys::mbedtls_pk_parse_key(
                &mut c.key,
                key.as_ptr(),
                key.len(),
                null(),
                0,
                Some(sys::mbedtls_ctr_drbg_random),
                random,
            );
            check("key", parsed)?;
            let defaults = sys::mbedtls_ssl_config_defaults(
                &mut c.config,
                sys::MBEDTLS_SSL_IS_SERVER as c_int,
                sys::MBEDTLS_SSL_TRANSPORT_STREAM as c_int,
                sys::MBEDTLS_SSL_PRESET_DEFAULT as c_int,
            );
            check("config", defaults)?;
            sys::mbedtls_ssl_conf_rng(&mut c.config, Some(sys::mbedtls_ctr_drbg_random), random);
            sys::mbedtls_ssl_conf_authmode(&mut c.config, sys::MBEDTLS_SSL_VERIFY_OPTIONAL as c_int);
            check("own certificate", sys::mbedtls_ssl_conf_own_cert(&mut c.config, &mut c.certificate, &mut c.key))?;
            check("setup", sys::mbedtls_ssl_setup(&mut c.ssl, &c.config))?;
            sys::mbedtls_ssl_set_bio(&mut c.ssl, addr_of_mut!(c.bio).cast(), Some(send), Some(receive), None);
        }
        Ok(Self(contexts))
    }

    fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.0.bio.outgoing)
    }
}

impl Drop for MbedTls {
    fn drop(&mut self) {
        let c = &mut *self.0;
        // SAFETY: each context was initialised in new, and is freed once
        unsafe {
            sys::mbedtls_ssl_free(&mut c.ssl);
            sys::mbedtls_ssl_config_free(&mut c.config);
            sys::mbedtls_x509_crt_free(&mut c.certificate);
            sys::mbedtls_pk_free(&mut c.key);
            sys::mbedtls_ctr_drbg_free(&mut c.random);
            sys::mbedtls_entropy_free(&mut c.entropy);
        }
    }
}

fn tls_error(what: &str, status: c_int) -> Error {
    Error::Tls(check(what, status).err().unwrap_or_default())
}

impl Tls for MbedTls {
    fn handshake(&mut self, input: &[u8]) -> Result<Vec<u8>, Error> {
        self.0.bio.incoming.extend(input);
        // SAFETY: the context was set up in new
        match unsafe { sys::mbedtls_ssl_handshake(&mut self.0.ssl) } {
            0 | sys::MBEDTLS_ERR_SSL_WANT_READ => Ok(self.take_output()),
            status => Err(tls_error("handshake", status)),
        }
    }

    fn encrypt(&mut self, plain: &[u8]) -> Result<Vec<u8>, Error> {
        let mut rest = plain;
        while !rest.is_empty() {
            // SAFETY: the context was set up in new, and `rest` is readable for its length
            match unsafe { sys::mbedtls_ssl_write(&mut self.0.ssl, rest.as_ptr(), rest.len()) } {
                written if written > 0 => rest = &rest[written as usize..],
                status => return Err(tls_error("write", status)),
            }
        }
        Ok(self.take_output())
    }

    fn decrypt(&mut self, cipher: &[u8]) -> Result<Vec<u8>, Error> {
        self.0.bio.incoming.extend(cipher);
        let (mut plain, mut chunk) = (Vec::new(), [0u8; READ_CHUNK]);
        loop {
            // SAFETY: the context was set up in new, and the chunk is writable for its length
            match unsafe { sys::mbedtls_ssl_read(&mut self.0.ssl, chunk.as_mut_ptr(), chunk.len()) } {
                read if read > 0 => plain.extend_from_slice(&chunk[..read as usize]),
                0 | sys::MBEDTLS_ERR_SSL_WANT_READ | sys::MBEDTLS_ERR_SSL_PEER_CLOSE_NOTIFY => return Ok(plain),
                status => return Err(tls_error("read", status)),
            }
        }
    }
}
