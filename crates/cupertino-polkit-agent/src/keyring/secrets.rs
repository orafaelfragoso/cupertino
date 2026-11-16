//! The Secret Service on the session bus, as far as the offer needs it: which collection is the default, whether the
//! login collection is unlocked, a collection unlocked through the service's own prompt, and the [`Store`] the move
//! works on. Secrets cross the bus encrypted under a session key: each side sends a Diffie-Hellman public key in the
//! 1024-bit MODP group (RFC 2409, generator 2), the shared value goes through HKDF with SHA256 (no salt, no info) to
//! a 16-byte AES key, and a secret travels under 128-bit AES in CBC mode with PKCS#7 padding and a fresh IV. An error
//! carries the call and the service's error name only.

use std::collections::{BTreeMap, HashMap};

use aes::Aes128;
use cbc::cipher::block_padding::Pkcs7;
use cbc::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use crypto_bigint::modular::{FixedMontyForm, FixedMontyParams};
use crypto_bigint::{Odd, U1024};
use hkdf::Hkdf;
use sha2::Sha256;
use zbus::blocking::{Connection, MessageIterator};
use zbus::export::serde::de::DeserializeOwned;
use zbus::export::serde::Serialize;
use zbus::zvariant::{DynamicType, ObjectPath, OwnedObjectPath, OwnedValue, Type, Value};
use zeroize::Zeroizing;

use crate::keyring::exchange::random;
use crate::keyring::offer::{Observed, Record, Store, StoreError, Unlocked, LOGIN};

pub const NAME: &str = "org.freedesktop.secrets";
const SERVICE_PATH: &str = "/org/freedesktop/secrets";
const SERVICE: &str = "org.freedesktop.Secret.Service";
const COLLECTION: &str = "org.freedesktop.Secret.Collection";
const ITEM: &str = "org.freedesktop.Secret.Item";
const SESSION: &str = "org.freedesktop.Secret.Session";
const PROMPT: &str = "org.freedesktop.Secret.Prompt";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
/// The session algorithm that encrypts secrets on the bus.
const ALGORITHM: &str = "dh-ietf1024-sha256-aes128-cbc-pkcs7";
/// What "no object" is on the service's API.
const NONE: &str = "/";
/// The alias of the collection applications store in when they name none.
const DEFAULT: &str = "default";

/// The bytes of the group's prime, of a private key and of the shared value.
const GROUP_BYTES: usize = 128;
/// The 1024-bit MODP group's prime, big-endian (RFC 2409, section 6.2).
const PRIME: &str = "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74020BBEA63B139B22514A08798E3404DD\
                     EF9519B3CD3A431B302B0A6DF25F14374FE1356D6D51C245E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7ED\
                     EE386BFB5A899FA5AE9F24117C4B1FE649286651ECE65381FFFFFFFFFFFFFFFF";
const GENERATOR: u64 = 2;
const KEY_BYTES: usize = 16;
const IV_BYTES: usize = 16;
/// Sessions tried for a shared value without a leading zero byte.
const SESSION_TRIES: usize = 8;

/// A secret on the bus: the session, the IV, the encrypted value and its content type.
type Sealed = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);
/// The same, borrowed, on its way to the service.
type Sending<'a> = (ObjectPath<'a>, Vec<u8>, Vec<u8>, &'a str);

fn prime() -> U1024 {
    U1024::from_be_hex(PRIME)
}

fn params() -> FixedMontyParams<{ U1024::LIMBS }> {
    // the group's prime is odd, as every modulus of the Montgomery form must be
    FixedMontyParams::new_vartime(Odd::new(prime()).expect("the group's prime is odd"))
}

/// `base` to the power `exponent` in the group.
fn power(base: &U1024, exponent: &U1024) -> U1024 {
    FixedMontyForm::new(base, &params())
        .pow(exponent)
        .retrieve()
}

/// The service's error name; any other failure of a call, without its text.
fn failed(call: &'static str) -> impl Fn(zbus::Error) -> StoreError {
    move |error| match error {
        zbus::Error::MethodError(name, _, _) => StoreError::new(call, name.as_str()),
        _ => StoreError::new(call, "no usable reply"),
    }
}

fn path(call: &'static str, path: &str) -> Result<ObjectPath<'static>, StoreError> {
    ObjectPath::try_from(path.to_owned()).map_err(|_| StoreError::new(call, "not an object path"))
}

fn call<R: DeserializeOwned + Type>(
    bus: &Connection,
    what: &'static str,
    object: &str,
    interface: &str,
    method: &str,
    body: &(impl Serialize + DynamicType),
) -> Result<R, StoreError> {
    bus.call_method(Some(NAME), object, Some(interface), method, body)
        .and_then(|reply| reply.body().deserialize())
        .map_err(failed(what))
}

fn property<R: TryFrom<OwnedValue>>(
    bus: &Connection,
    what: &'static str,
    object: &str,
    interface: &str,
    name: &str,
) -> Result<R, StoreError> {
    let value: OwnedValue = call(bus, what, object, PROPERTIES, "Get", &(interface, name))?;
    R::try_from(value).map_err(|_| StoreError::new(what, "a property of another type"))
}

/// Whether a Secret Service runs on `bus`; asking never starts one.
pub fn present(bus: &Connection) -> bool {
    zbus::blocking::fdo::DBusProxy::new(bus)
        .and_then(|dbus| Ok(dbus.name_has_owner(NAME.try_into()?)?))
        .unwrap_or(false)
}

/// The default collection and the login collection's lock, as the service says now.
pub fn observe(bus: &Connection) -> Result<Observed, StoreError> {
    let default: OwnedObjectPath = call(
        bus,
        "reading the default keyring",
        SERVICE_PATH,
        SERVICE,
        "ReadAlias",
        &(DEFAULT,),
    )?;
    let default = (default.as_str() != NONE).then(|| {
        let label = property(
            bus,
            "reading a keyring's name",
            default.as_str(),
            COLLECTION,
            "Label",
        )
        .unwrap_or_default();
        (default.to_string(), label)
    });
    // a login collection that does not answer does not exist
    let login_locked =
        property::<bool>(bus, "reading a keyring's lock", LOGIN, COLLECTION, "Locked").ok();
    Ok(Observed {
        default,
        login_locked,
    })
}

/// Unlock `collection` with the service's own prompt, which asks the session's prompter for the keyring's password.
/// Returns when the person answered it.
pub fn unlock(bus: &Connection, collection: &str) -> Unlocked {
    match try_unlock(bus, collection) {
        Ok(unlocked) => unlocked,
        Err(error) => {
            eprintln!("cupertino-keyring-prompter: {error}");
            Unlocked::Failed
        }
    }
}

fn try_unlock(bus: &Connection, collection: &str) -> Result<Unlocked, StoreError> {
    const WHAT: &str = "unlocking the keyring";
    let is_unlocked = || {
        property::<bool>(bus, WHAT, collection, COLLECTION, "Locked").map(|locked| {
            if locked {
                Unlocked::Failed
            } else {
                Unlocked::Yes
            }
        })
    };
    let objects = vec![path(WHAT, collection)?];
    let (_, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) =
        call(bus, WHAT, SERVICE_PATH, SERVICE, "Unlock", &(objects,))?;
    if prompt.as_str() == NONE {
        return is_unlocked();
    }
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender(NAME)
        .and_then(|rule| rule.interface(PROMPT))
        .and_then(|rule| rule.member("Completed"))
        .and_then(|rule| rule.path(prompt.as_str().to_owned()))
        .map_err(failed(WHAT))?
        .build();
    let mut completions =
        MessageIterator::for_match_rule(rule, bus, Some(4)).map_err(failed(WHAT))?;
    let () = call(bus, WHAT, prompt.as_str(), PROMPT, "Prompt", &("",))?;
    let completed = completions
        .next()
        .ok_or(StoreError::new(WHAT, "the bus closed"))?
        .map_err(failed(WHAT))?;
    let (dismissed, _): (bool, OwnedValue) =
        completed.body().deserialize().map_err(failed(WHAT))?;
    if dismissed {
        return Ok(Unlocked::Cancelled);
    }
    is_unlocked()
}

/// A session with the service: the key its secrets travel under.
pub struct Secrets {
    bus: Connection,
    session: OwnedObjectPath,
    key: Zeroizing<[u8; KEY_BYTES]>,
}

impl Secrets {
    /// Open a session whose secrets are encrypted on the bus.
    pub fn open(bus: &Connection) -> Result<Secrets, StoreError> {
        const WHAT: &str = "opening a session";
        for _ in 0..SESSION_TRIES {
            let mut bytes = Zeroizing::new([0u8; GROUP_BYTES]);
            random(&mut bytes[..]).map_err(|_| StoreError::new(WHAT, "no random bytes"))?;
            // below the prime: its top byte is 0xff, so a clear top bit keeps the key smaller
            bytes[0] &= 0x7f;
            let private = Zeroizing::new(U1024::from_be_slice(&bytes[..]));
            let public = power(&U1024::from_u64(GENERATOR), &private).to_be_bytes();
            let (theirs, session): (OwnedValue, OwnedObjectPath) = call(
                bus,
                WHAT,
                SERVICE_PATH,
                SERVICE,
                "OpenSession",
                &(ALGORITHM, Value::from(public.to_vec())),
            )?;
            let theirs = Vec::<u8>::try_from(theirs)
                .map_err(|_| StoreError::new(WHAT, "no public key in the reply"))?;
            let shared = shared_value(&private, &theirs)
                .ok_or(StoreError::new(WHAT, "a public key outside the group"))?;
            // implementations differ in whether a shared value keeps its leading zero bytes when the key is derived
            // from it; one value in 256 has one, and then another session is opened instead
            if shared[0] == 0 {
                let _: Result<(), _> = call(bus, WHAT, session.as_str(), SESSION, "Close", &());
                continue;
            }
            let mut key = Zeroizing::new([0u8; KEY_BYTES]);
            Hkdf::<Sha256>::new(None, &shared[..])
                .expand(&[], &mut key[..])
                .expect("16 bytes is a valid HKDF with SHA256 length");
            return Ok(Secrets {
                bus: bus.clone(),
                session,
                key,
            });
        }
        Err(StoreError::new(WHAT, "no usable session key"))
    }

    fn seal<'a>(&'a self, record: &'a Record) -> Result<Sending<'a>, StoreError> {
        let mut iv = [0u8; IV_BYTES];
        random(&mut iv).map_err(|_| StoreError::new("saving an item", "no random bytes"))?;
        let cipher = cbc::Encryptor::<Aes128>::new(self.key.as_ref().into(), &iv.into())
            .encrypt_padded_vec_mut::<Pkcs7>(&record.secret);
        Ok((
            ObjectPath::from(&self.session),
            iv.to_vec(),
            cipher,
            record.content_type.as_str(),
        ))
    }

    fn open_sealed(&self, iv: &[u8], cipher: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
        let iv: [u8; IV_BYTES] = iv.try_into().ok()?;
        if cipher.is_empty() || !cipher.len().is_multiple_of(16) {
            return None;
        }
        let mut buf = Zeroizing::new(cipher.to_vec());
        let plain = cbc::Decryptor::<Aes128>::new(self.key.as_ref().into(), &iv.into())
            .decrypt_padded_mut::<Pkcs7>(&mut buf)
            .ok()?;
        Some(Zeroizing::new(plain.to_vec()))
    }
}

impl Drop for Secrets {
    fn drop(&mut self) {
        let _: Result<(), _> = call(
            &self.bus,
            "closing a session",
            self.session.as_str(),
            SESSION,
            "Close",
            &(),
        );
    }
}

/// The value both sides share, as big-endian bytes of the group's size; `None` for a public key that is not an
/// element of the group other than 1 and p - 1.
fn shared_value(private: &U1024, theirs: &[u8]) -> Option<Zeroizing<[u8; GROUP_BYTES]>> {
    if theirs.is_empty() || theirs.len() > GROUP_BYTES {
        return None;
    }
    let mut padded = [0u8; GROUP_BYTES];
    padded[GROUP_BYTES - theirs.len()..].copy_from_slice(theirs);
    let theirs = U1024::from_be_slice(&padded);
    // 1 < y < p - 1: anything else gives a shared value the peer's choice alone decides
    if theirs <= U1024::ONE || theirs >= prime().wrapping_sub(&U1024::ONE) {
        return None;
    }
    let mut out = Zeroizing::new([0u8; GROUP_BYTES]);
    out.copy_from_slice(&power(&theirs, private).to_be_bytes()[..]);
    Some(out)
}

/// The attribute that names an item's schema, and the schema of an item that names none.
const SCHEMA: &str = "xdg:schema";
const GENERIC_SCHEMA: &str = "org.freedesktop.Secret.Generic";

/// An item's attributes as the service reports them once it has read the item from its file: an item stored without
/// a schema has the generic one from then on, so the same item compares equal before and after.
fn with_schema(attributes: HashMap<String, String>) -> BTreeMap<String, String> {
    let mut attributes: BTreeMap<String, String> = attributes.into_iter().collect();
    attributes
        .entry(SCHEMA.to_owned())
        .or_insert_with(|| GENERIC_SCHEMA.to_owned());
    attributes
}

impl Store for Secrets {
    fn default_collection(&mut self) -> Result<String, StoreError> {
        let default: OwnedObjectPath = call(
            &self.bus,
            "reading the default keyring",
            SERVICE_PATH,
            SERVICE,
            "ReadAlias",
            &(DEFAULT,),
        )?;
        Ok(default.to_string())
    }

    fn set_default(&mut self, collection: &str) -> Result<(), StoreError> {
        const WHAT: &str = "changing the default keyring";
        let collection = path(WHAT, collection)?;
        call(
            &self.bus,
            WHAT,
            SERVICE_PATH,
            SERVICE,
            "SetAlias",
            &(DEFAULT, collection),
        )
    }

    fn items(&mut self, collection: &str) -> Result<Vec<String>, StoreError> {
        let items: Vec<OwnedObjectPath> = property(
            &self.bus,
            "listing a keyring",
            collection,
            COLLECTION,
            "Items",
        )?;
        Ok(items.iter().map(|item| item.to_string()).collect())
    }

    fn attributes(&mut self, item: &str) -> Result<BTreeMap<String, String>, StoreError> {
        let attributes: HashMap<String, String> =
            property(&self.bus, "reading an item", item, ITEM, "Attributes")?;
        Ok(with_schema(attributes))
    }

    fn read(&mut self, item: &str) -> Result<Record, StoreError> {
        const WHAT: &str = "reading an item";
        let label: String = property(&self.bus, WHAT, item, ITEM, "Label")?;
        let attributes: HashMap<String, String> =
            property(&self.bus, WHAT, item, ITEM, "Attributes")?;
        let (_, iv, cipher, content_type): Sealed = call(
            &self.bus,
            WHAT,
            item,
            ITEM,
            "GetSecret",
            &(ObjectPath::from(&self.session),),
        )?;
        let secret = self
            .open_sealed(&iv, &cipher)
            .ok_or(StoreError::new(WHAT, "the secret does not decrypt"))?;
        Ok(Record {
            label,
            attributes: with_schema(attributes),
            secret,
            content_type,
        })
    }

    fn create(&mut self, collection: &str, record: &Record) -> Result<String, StoreError> {
        const WHAT: &str = "saving an item";
        let attributes: HashMap<&str, &str> = record
            .attributes
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let properties: HashMap<&str, Value<'_>> = HashMap::from([
            (
                "org.freedesktop.Secret.Item.Label",
                Value::from(record.label.as_str()),
            ),
            (
                "org.freedesktop.Secret.Item.Attributes",
                Value::from(attributes),
            ),
        ]);
        // not replacing: an item of the keyring with the same attributes stays as it is
        let (item, prompt): (OwnedObjectPath, OwnedObjectPath) = call(
            &self.bus,
            WHAT,
            collection,
            COLLECTION,
            "CreateItem",
            &(properties, self.seal(record)?, false),
        )?;
        if prompt.as_str() != NONE || item.as_str() == NONE {
            return Err(StoreError::new(WHAT, "the keyring asked for a prompt"));
        }
        Ok(item.to_string())
    }

    fn delete(&mut self, item: &str) -> Result<(), StoreError> {
        const WHAT: &str = "removing a copy";
        let prompt: OwnedObjectPath = call(&self.bus, WHAT, item, ITEM, "Delete", &())?;
        if prompt.as_str() != NONE {
            return Err(StoreError::new(WHAT, "the keyring asked for a prompt"));
        }
        Ok(())
    }
}
