//! A password in flight: zeroed when dropped, never printed, and sent as hex so the JSON parser never copies it into its
//! unzeroed scratch buffer (it only does for strings with escapes).

use std::fmt;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zeroize::Zeroizing;

use crate::hex;

/// The longest secret in bytes: `PAM_MAX_RESP_SIZE` (`pam_conv(3)).
pub const MAX_SECRET_BYTES: usize = 512;

/// A secret text. `Debug` prints `<secret>`.
pub struct Secret(Zeroizing<String>);

/// The text is longer than [`MAX_SECRET_BYTES`].
#[derive(Debug, PartialEq, Eq)]
pub struct SecretTooLong;

impl fmt::Display for SecretTooLong {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a secret is at most {MAX_SECRET_BYTES} bytes")
    }
}

impl std::error::Error for SecretTooLong {}

impl Secret {
    /// Take ownership of `text`; it is zeroed when the secret is dropped.
    pub fn new(text: Zeroizing<String>) -> Result<Self, SecretTooLong> {
        if text.len() > MAX_SECRET_BYTES {
            return Err(SecretTooLong);
        }
        Ok(Secret(text))
    }

    /// The text, for the one place that hands it to PAM.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<secret>")
    }
}

impl Serialize for Secret {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(self.0.as_bytes()))
    }
}

struct SecretVisitor;

impl Visitor<'_> for SecretVisitor {
    type Value = Secret;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a hex-encoded secret")
    }

    // The messages never carry the offending text: serde's own wording would.
    fn visit_str<E: de::Error>(self, text: &str) -> Result<Secret, E> {
        let bytes = hex::decode(text).ok_or_else(|| E::custom("secret is not lower-case hex"))?;
        let text = std::str::from_utf8(&bytes).map_err(|_| E::custom("secret is not UTF-8"))?;
        Secret::new(Zeroizing::new(text.to_owned())).map_err(|_| E::custom("secret is too long"))
    }
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_str(SecretVisitor)
    }
}
