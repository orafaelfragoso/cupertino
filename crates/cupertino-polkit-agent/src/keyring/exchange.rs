//! The secret exchange of the prompt protocol: how a typed password reaches the keyring daemon without crossing the
//! bus in clear. Each side sends its Diffie-Hellman public key in the 1536-bit MODP group (RFC 3526, generator 2); the
//! shared value, padded to the prime's length, goes through HKDF with SHA256 (no salt, no info) to a 16-byte AES key, and a
//! secret travels encrypted with 128-bit AES in CBC mode with PKCS#7 padding under a fresh 16-byte IV.
//!
//! A message is a key file of one group, `[sx-aes-1]`, with base64 values: `public` always, and `secret` and `iv` when
//! it carries a secret. A side must receive the other's public key before it can send a secret.

use std::fmt;

use aes::Aes128;
use cbc::cipher::block_padding::Pkcs7;
use cbc::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use crypto_bigint::modular::runtime_mod::{DynResidue, DynResidueParams};
use crypto_bigint::{Encoding, U1536};
use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroizing;

/// The protocol's name, the key file's only group.
pub const PROTOCOL: &str = "sx-aes-1";
/// The bytes of the group's prime, of a private key and of the shared value.
const GROUP_BYTES: usize = 192;
/// The 1536-bit MODP group's prime, big-endian (RFC 3526, section 2).
const PRIME: [u8; GROUP_BYTES] = hex_bytes(
    "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74020BBEA63B139B22514A08798E3404DD\
     EF9519B3CD3A431B302B0A6DF25F14374FE1356D6D51C245E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7ED\
     EE386BFB5A899FA5AE9F24117C4B1FE649286651ECE45B3DC2007CB8A163BF0598DA48361C55D39A69163FA8FD24CF5F\
     83655D23DCA3AD961C62F356208552BB9ED529077096966D670C354E4ABC9804F1746C08CA237327FFFFFFFFFFFFFFFF",
);
/// The group's generator.
const GENERATOR: u64 = 2;
/// The AES key the shared value derives.
const KEY_BYTES: usize = 16;
const IV_BYTES: usize = 16;
/// The longest secret a message may carry, in bytes; the prompt's field takes far less.
const MAX_SECRET: usize = 64 * 1024;

const fn hex_bytes(text: &str) -> [u8; GROUP_BYTES] {
    let text = text.as_bytes();
    let mut out = [0u8; GROUP_BYTES];
    let (mut i, mut n) = (0, 0);
    while i < text.len() {
        let c = text[i];
        let v = match c {
            b'0'..=b'9' => c - b'0',
            b'A'..=b'F' => c - b'A' + 10,
            _ => {
                i += 1;
                continue;
            }
        };
        out[n / 2] |= if n % 2 == 0 { v << 4 } else { v };
        n += 1;
        i += 1;
    }
    assert!(n == 2 * GROUP_BYTES, "the prime has 192 bytes");
    out
}

/// Why a message of the exchange was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExchangeError {
    /// Not a key file of the protocol's group, or a value is not base64.
    Malformed,
    /// The peer's public key is not an element of the group other than 1 and p - 1.
    BadPublicKey,
    /// A secret before the peer's public key was received.
    NoPeer,
    /// The secret does not decrypt under the shared key.
    Decrypt,
    /// The system gave no random bytes.
    Random,
}

impl fmt::Display for ExchangeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ExchangeError::Malformed => "the exchange message is malformed",
            ExchangeError::BadPublicKey => "the peer's public key is not in the group",
            ExchangeError::NoPeer => "no public key was received yet",
            ExchangeError::Decrypt => "the secret does not decrypt",
            ExchangeError::Random => "no random bytes",
        })
    }
}

impl std::error::Error for ExchangeError {}

/// `buf` filled from the kernel's random source.
fn random(buf: &mut [u8]) -> Result<(), ExchangeError> {
    let mut filled = 0;
    while filled < buf.len() {
        // Safety: the pointer and length name the unfilled rest of `buf`, which outlives the call.
        let n =
            unsafe { libc::getrandom(buf[filled..].as_mut_ptr().cast(), buf.len() - filled, 0) };
        if n < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(ExchangeError::Random);
        }
        filled += n as usize;
    }
    Ok(())
}

/// One side of an exchange: its key pair and, once the peer's public key arrived, the key they share.
pub struct Exchange {
    private: Zeroizing<U1536>,
    public: Vec<u8>,
    key: Option<Zeroizing<[u8; KEY_BYTES]>>,
}

impl fmt::Debug for Exchange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Exchange")
            .field("shared", &self.key.is_some())
            .finish_non_exhaustive()
    }
}

fn params() -> DynResidueParams<{ U1536::LIMBS }> {
    DynResidueParams::new(&U1536::from_be_bytes(PRIME))
}

/// The big-endian bytes of `value` without leading zeros, as the protocol writes a public key.
fn minimal(value: &U1536) -> Vec<u8> {
    let bytes = value.to_be_bytes();
    let first = bytes.iter().position(|b| *b != 0).unwrap_or(bytes.len());
    bytes[first..].to_vec()
}

impl Exchange {
    /// A side with a fresh random private key.
    pub fn new() -> Result<Exchange, ExchangeError> {
        let mut bytes = Zeroizing::new([0u8; GROUP_BYTES]);
        random(&mut bytes[..])?;
        // below the prime: its top byte is 0xff, so a clear top bit keeps the key smaller
        bytes[0] &= 0x7f;
        Ok(Exchange::with_private(&bytes[..]))
    }

    /// A side whose private key is `private` (big-endian, at most 192 bytes), for tests.
    pub fn with_private(private: &[u8]) -> Exchange {
        let mut padded = Zeroizing::new([0u8; GROUP_BYTES]);
        padded[GROUP_BYTES - private.len()..].copy_from_slice(private);
        let private = Zeroizing::new(U1536::from_be_bytes(*padded));
        let public = DynResidue::new(&U1536::from_u64(GENERATOR), params())
            .pow(&private)
            .retrieve();
        Exchange {
            private,
            public: minimal(&public),
            key: None,
        }
    }

    /// Whether the peer's public key arrived, so a secret may be sent.
    pub fn shared(&self) -> bool {
        self.key.is_some()
    }

    /// The first message: this side's public key alone.
    pub fn begin(&self) -> String {
        write_message(&[("public", &self.public)])
    }

    /// Take the peer's message: its public key (the shared key follows from it) and the secret it carries, if any.
    pub fn receive(&mut self, message: &str) -> Result<Option<Zeroizing<Vec<u8>>>, ExchangeError> {
        let fields = read_message(message)?;
        let field = |name: &str| fields.iter().find(|(k, _)| k == name).map(|(_, v)| v);
        let peer = field("public").ok_or(ExchangeError::Malformed)?;
        self.key = Some(self.derive(peer)?);
        match (field("secret"), field("iv")) {
            (Some(secret), Some(iv)) => self.decrypt(secret, iv).map(Some),
            (None, None) => Ok(None),
            _ => Err(ExchangeError::Malformed),
        }
    }

    /// A message with this side's public key and, when given, `secret` encrypted under the shared key.
    pub fn send(&self, secret: Option<&[u8]>) -> Result<String, ExchangeError> {
        let Some(secret) = secret else {
            return Ok(self.begin());
        };
        let key = self.key.as_ref().ok_or(ExchangeError::NoPeer)?;
        let mut iv = [0u8; IV_BYTES];
        random(&mut iv)?;
        let cipher = cbc::Encryptor::<Aes128>::new(key.as_ref().into(), &iv.into())
            .encrypt_padded_vec_mut::<Pkcs7>(secret);
        Ok(write_message(&[
            ("public", &self.public),
            ("secret", &cipher),
            ("iv", &iv),
        ]))
    }

    fn derive(&self, peer: &[u8]) -> Result<Zeroizing<[u8; KEY_BYTES]>, ExchangeError> {
        if peer.is_empty() || peer.len() > GROUP_BYTES {
            return Err(ExchangeError::BadPublicKey);
        }
        let mut padded = [0u8; GROUP_BYTES];
        padded[GROUP_BYTES - peer.len()..].copy_from_slice(peer);
        let peer = U1536::from_be_bytes(padded);
        let prime = U1536::from_be_bytes(PRIME);
        let last = prime.wrapping_sub(&U1536::ONE);
        // 1 < y < p - 1: anything else gives a shared value the peer's choice alone decides
        if peer <= U1536::ONE || peer >= last {
            return Err(ExchangeError::BadPublicKey);
        }
        let shared = Zeroizing::new(
            DynResidue::new(&peer, params())
                .pow(&self.private)
                .retrieve()
                .to_be_bytes(),
        );
        let mut key = Zeroizing::new([0u8; KEY_BYTES]);
        Hkdf::<Sha256>::new(None, &shared[..])
            .expand(&[], &mut key[..])
            .expect("16 bytes is a valid HKDF with SHA256 length");
        Ok(key)
    }

    fn decrypt(&self, secret: &[u8], iv: &[u8]) -> Result<Zeroizing<Vec<u8>>, ExchangeError> {
        let key = self.key.as_ref().ok_or(ExchangeError::NoPeer)?;
        let iv: [u8; IV_BYTES] = iv.try_into().map_err(|_| ExchangeError::Malformed)?;
        if secret.is_empty() || !secret.len().is_multiple_of(16) || secret.len() > MAX_SECRET {
            return Err(ExchangeError::Malformed);
        }
        let mut buf = Zeroizing::new(secret.to_vec());
        let plain = cbc::Decryptor::<Aes128>::new(key.as_ref().into(), &iv.into())
            .decrypt_padded_mut::<Pkcs7>(&mut buf)
            .map_err(|_| ExchangeError::Decrypt)?;
        Ok(Zeroizing::new(plain.to_vec()))
    }
}

fn write_message(fields: &[(&str, &[u8])]) -> String {
    let mut out = format!("[{PROTOCOL}]\n");
    for (name, value) in fields {
        out.push_str(name);
        out.push('=');
        out.push_str(&base64_encode(value));
        out.push('\n');
    }
    out
}

/// A field of a message: its name and its decoded value.
type Field = (String, Zeroizing<Vec<u8>>);

/// The fields of the protocol's group in `message`, decoded; comments, blank lines and other groups are skipped.
fn read_message(message: &str) -> Result<Vec<Field>, ExchangeError> {
    let mut group = None;
    let mut fields = Vec::new();
    for line in message.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            group = Some(name == PROTOCOL);
            continue;
        }
        if group != Some(true) {
            continue;
        }
        let (name, value) = line.split_once('=').ok_or(ExchangeError::Malformed)?;
        fields.push((name.trim().to_owned(), base64_decode(value.trim())?));
    }
    if group.is_none() {
        return Err(ExchangeError::Malformed);
    }
    Ok(fields)
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding.
pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Standard base64, padded or not; anything else is refused.
pub fn base64_decode(text: &str) -> Result<Zeroizing<Vec<u8>>, ExchangeError> {
    let text = text.trim_end_matches('=').as_bytes();
    let mut out = Zeroizing::new(Vec::with_capacity(text.len() * 3 / 4));
    let (mut acc, mut bits) = (0u32, 0);
    for c in text {
        let v = ALPHABET
            .iter()
            .position(|a| a == c)
            .ok_or(ExchangeError::Malformed)? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    if bits >= 6 {
        return Err(ExchangeError::Malformed);
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
