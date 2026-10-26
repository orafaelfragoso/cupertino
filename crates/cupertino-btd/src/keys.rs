//! Persistent pairing keys: what the daemon feeds the kernel with Load Link Keys and Load Long
//! Term Keys at startup.
//!
//! One file per adapter, `<dir>/<ADAPTER-ADDRESS>.keys` with the colons of the address written
//! as dashes. The file is plain text, versioned by its first line:
//!
//! ```text
//! cupertino-bluetooth-keys 1
//! device 11:22:33:44:55:66 bredr
//! name 4d6f757365
//! link 05 04 00112233445566778899aabbccddeeff
//! device 11:22:33:44:55:77 le-random
//! ltk 03 1 16 1234 0102030405060708 00112233445566778899aabbccddeeff
//! ```
//!
//! - `device <address> <type>` starts a record; each device appears once.
//! - `name <hex>`: the device's name, UTF-8 in hex, at most once.
//! - `link <key type, hex> <PIN length, decimal> <value, 32 hex digits>`: a BR/EDR link key, at
//!   most once, only for a `bredr` device.
//! - `ltk <key type, hex> <central, 0 or 1> <encryption size, decimal> <diversifier, 4 hex
//!   digits> <random number, 16 hex digits in wire order> <value, 32 hex digits>`: a long term key,
//!   at most one per value of `central`, only for an LE device.
//!
//! The file holds secrets: it is created with mode 0600 in a directory with mode 0700, written
//! to a temporary file and renamed into place, and refused when it is readable by anyone else
//! or does not parse. A file that is refused is never overwritten.

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};
use std::fs;
use std::io::{self, Write as _};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use cupertino_bluetooth::{
    Address, AddressType, DeviceId, KeyValue, LinkKey, LinkKeyType, LongTermKey, LongTermKeyType,
};

const HEADER: &str = "cupertino-bluetooth-keys 1";

/// What the daemon knows about one paired device.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bond {
    pub name: Option<String>,
    pub link_key: Option<LinkKey>,
    pub long_term_keys: Vec<LongTermKey>,
}

/// The bonds of one adapter.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeySet {
    bonds: BTreeMap<DeviceId, Bond>,
}

impl KeySet {
    pub fn bonds(&self) -> impl Iterator<Item = (&DeviceId, &Bond)> {
        self.bonds.iter()
    }

    pub fn bond(&self, device: &DeviceId) -> Option<&Bond> {
        self.bonds.get(device)
    }

    pub fn is_empty(&self) -> bool {
        self.bonds.is_empty()
    }

    pub fn link_keys(&self) -> Vec<LinkKey> {
        self.bonds
            .values()
            .filter_map(|bond| bond.link_key)
            .collect()
    }

    pub fn long_term_keys(&self) -> Vec<LongTermKey> {
        self.bonds
            .values()
            .flat_map(|bond| bond.long_term_keys.iter().copied())
            .collect()
    }

    /// Records a link key, replacing the device's previous one.
    pub fn set_link_key(&mut self, key: LinkKey) {
        self.bonds.entry(key.device).or_default().link_key = Some(key);
    }

    /// Records a long term key, replacing the device's previous key for the same role.
    pub fn set_long_term_key(&mut self, key: LongTermKey) {
        let keys = &mut self.bonds.entry(key.device).or_default().long_term_keys;
        keys.retain(|existing| existing.central != key.central);
        keys.push(key);
    }

    /// Remembers a name for a device that has keys. Returns false, and records nothing, for a
    /// device without a bond.
    pub fn set_name(&mut self, device: &DeviceId, name: &str) -> bool {
        match self.bonds.get_mut(device) {
            Some(bond) if bond.name.as_deref() != Some(name) => {
                bond.name = Some(name.to_owned());
                true
            }
            _ => false,
        }
    }

    /// Forgets a device. Returns whether it had a bond.
    pub fn remove(&mut self, device: &DeviceId) -> bool {
        self.bonds.remove(device).is_some()
    }

    /// The file's text.
    pub fn render(&self) -> String {
        let mut out = format!("{HEADER}\n");
        for (device, bond) in &self.bonds {
            writeln!(out, "device {} {}", device.address, device.kind)
                .expect("writing to a String");
            if let Some(name) = &bond.name {
                writeln!(out, "name {}", to_hex(name.as_bytes())).expect("writing to a String");
            }
            if let Some(key) = &bond.link_key {
                writeln!(
                    out,
                    "link {:02x} {:02} {}",
                    key.key_type.to_wire(),
                    key.pin_length,
                    to_hex(&key.value.0)
                )
                .expect("writing to a String");
            }
            for key in &bond.long_term_keys {
                writeln!(
                    out,
                    "ltk {:02x} {} {} {:04x} {} {}",
                    key.key_type.to_wire(),
                    u8::from(key.central),
                    key.encryption_size,
                    key.diversifier,
                    to_hex(&key.random),
                    to_hex(&key.value.0)
                )
                .expect("writing to a String");
            }
        }
        out
    }

    /// Parses the file's text. Every deviation is an error that names the line.
    pub fn parse(text: &str) -> Result<Self, KeyFileError> {
        let mut lines = text
            .split_inclusive('\n')
            .enumerate()
            .map(|(number, line)| (number + 1, line));
        match lines.next() {
            Some((_, line)) if line.strip_suffix('\n') == Some(HEADER) => {}
            Some((_, line)) if line.starts_with("cupertino-bluetooth-keys ") => {
                return Err(KeyFileError {
                    line: 1,
                    kind: KeyFileErrorKind::UnsupportedVersion(line.trim_end().to_owned()),
                })
            }
            _ => {
                return Err(KeyFileError {
                    line: 1,
                    kind: KeyFileErrorKind::MissingHeader,
                })
            }
        }
        let mut set = Self::default();
        let mut current: Option<DeviceId> = None;
        for (number, line) in lines {
            let at = |kind| KeyFileError { line: number, kind };
            let line = line
                .strip_suffix('\n')
                .ok_or_else(|| at(KeyFileErrorKind::MissingNewline))?;
            let mut fields = line.split(' ');
            let keyword = fields.next().unwrap_or_default();
            let fields: Vec<&str> = fields.collect();
            match (keyword, fields.as_slice()) {
                ("device", [address, kind]) => {
                    let id = DeviceId {
                        address: address
                            .parse()
                            .map_err(|_| at(KeyFileErrorKind::Field("address")))?,
                        kind: kind
                            .parse()
                            .map_err(|_| at(KeyFileErrorKind::Field("address type")))?,
                    };
                    if set.bonds.insert(id, Bond::default()).is_some() {
                        return Err(at(KeyFileErrorKind::DuplicateDevice));
                    }
                    current = Some(id);
                }
                ("name", [name]) => {
                    let bond = current_bond(&mut set, current)
                        .ok_or_else(|| at(KeyFileErrorKind::OutsideDevice))?;
                    if bond.name.is_some() {
                        return Err(at(KeyFileErrorKind::Repeated("name")));
                    }
                    let bytes =
                        from_hex(name).ok_or_else(|| at(KeyFileErrorKind::Field("name")))?;
                    bond.name = Some(
                        String::from_utf8(bytes)
                            .map_err(|_| at(KeyFileErrorKind::Field("name")))?,
                    );
                }
                ("link", [key_type, pin_length, value]) => {
                    let device = current.ok_or_else(|| at(KeyFileErrorKind::OutsideDevice))?;
                    if device.kind != AddressType::BrEdr {
                        return Err(at(KeyFileErrorKind::WrongTransport("link")));
                    }
                    let bond = current_bond(&mut set, current)
                        .ok_or_else(|| at(KeyFileErrorKind::OutsideDevice))?;
                    if bond.link_key.is_some() {
                        return Err(at(KeyFileErrorKind::Repeated("link")));
                    }
                    bond.link_key = Some(LinkKey {
                        device,
                        key_type: LinkKeyType::from_wire(
                            hex_u8(key_type)
                                .ok_or_else(|| at(KeyFileErrorKind::Field("key type")))?,
                        ),
                        value: KeyValue(
                            hex_array(value).ok_or_else(|| at(KeyFileErrorKind::Field("value")))?,
                        ),
                        pin_length: pin_length
                            .parse()
                            .map_err(|_| at(KeyFileErrorKind::Field("PIN length")))?,
                    });
                }
                ("ltk", [key_type, central, size, diversifier, random, value]) => {
                    let device = current.ok_or_else(|| at(KeyFileErrorKind::OutsideDevice))?;
                    if !device.kind.is_le() {
                        return Err(at(KeyFileErrorKind::WrongTransport("ltk")));
                    }
                    let central = match *central {
                        "0" => false,
                        "1" => true,
                        _ => return Err(at(KeyFileErrorKind::Field("central"))),
                    };
                    let key = LongTermKey {
                        device,
                        key_type: LongTermKeyType::from_wire(
                            hex_u8(key_type)
                                .ok_or_else(|| at(KeyFileErrorKind::Field("key type")))?,
                        ),
                        central,
                        encryption_size: size
                            .parse()
                            .map_err(|_| at(KeyFileErrorKind::Field("encryption size")))?,
                        diversifier: if diversifier.len() == 4 {
                            u16::from_str_radix(diversifier, 16).ok()
                        } else {
                            None
                        }
                        .ok_or_else(|| at(KeyFileErrorKind::Field("diversifier")))?,
                        random: hex_array(random)
                            .ok_or_else(|| at(KeyFileErrorKind::Field("random number")))?,
                        value: KeyValue(
                            hex_array(value).ok_or_else(|| at(KeyFileErrorKind::Field("value")))?,
                        ),
                    };
                    let bond = current_bond(&mut set, current)
                        .ok_or_else(|| at(KeyFileErrorKind::OutsideDevice))?;
                    if bond
                        .long_term_keys
                        .iter()
                        .any(|existing| existing.central == central)
                    {
                        return Err(at(KeyFileErrorKind::Repeated("ltk")));
                    }
                    bond.long_term_keys.push(key);
                }
                _ => return Err(at(KeyFileErrorKind::UnknownLine(line.to_owned()))),
            }
        }
        Ok(set)
    }
}

fn current_bond(set: &mut KeySet, current: Option<DeviceId>) -> Option<&mut Bond> {
    set.bonds.get_mut(&current?)
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

fn hex_array<const N: usize>(text: &str) -> Option<[u8; N]> {
    from_hex(text)?.try_into().ok()
}

fn hex_u8(text: &str) -> Option<u8> {
    if text.len() == 2 {
        u8::from_str_radix(text, 16).ok()
    } else {
        None
    }
}

/// Why a key file did not parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyFileError {
    pub line: usize,
    pub kind: KeyFileErrorKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyFileErrorKind {
    MissingHeader,
    UnsupportedVersion(String),
    MissingNewline,
    UnknownLine(String),
    Field(&'static str),
    DuplicateDevice,
    Repeated(&'static str),
    OutsideDevice,
    WrongTransport(&'static str),
}

impl fmt::Display for KeyFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: ", self.line)?;
        match &self.kind {
            KeyFileErrorKind::MissingHeader => {
                write!(f, "not a key file (the first line must be \"{HEADER}\")")
            }
            KeyFileErrorKind::UnsupportedVersion(found) => {
                write!(f, "unsupported version: {found:?}")
            }
            KeyFileErrorKind::MissingNewline => write!(f, "the file ends in the middle of a line"),
            KeyFileErrorKind::UnknownLine(line) => write!(f, "cannot read {line:?}"),
            KeyFileErrorKind::Field(name) => write!(f, "invalid {name}"),
            KeyFileErrorKind::DuplicateDevice => write!(f, "the device appears twice"),
            KeyFileErrorKind::Repeated(keyword) => {
                write!(f, "{keyword} appears twice for one device")
            }
            KeyFileErrorKind::OutsideDevice => write!(f, "the line belongs to no device"),
            KeyFileErrorKind::WrongTransport(keyword) => {
                write!(f, "{keyword} does not fit this device's address type")
            }
        }
    }
}

impl std::error::Error for KeyFileError {}

/// Why keys could not be loaded or saved.
#[derive(Debug)]
pub enum KeyStoreError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    /// The file is readable by its group or by everyone.
    Permissions {
        path: PathBuf,
        mode: u32,
    },
    Corrupt {
        path: PathBuf,
        source: KeyFileError,
    },
}

impl fmt::Display for KeyStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Permissions { path, mode } => write!(
                f,
                "{}: mode {mode:04o} lets others read the keys; fix it to 0600",
                path.display()
            ),
            Self::Corrupt { path, source } => write!(
                f,
                "{}: {source}; the file was left as it is",
                path.display()
            ),
        }
    }
}

impl std::error::Error for KeyStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Permissions { .. } => None,
            Self::Corrupt { source, .. } => Some(source),
        }
    }
}

/// The directory that holds the key files.
#[derive(Clone, Debug)]
pub struct KeyStore {
    dir: PathBuf,
}

impl KeyStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn path(&self, adapter: Address) -> PathBuf {
        self.dir
            .join(format!("{}.keys", adapter.to_string().replace(':', "-")))
    }

    /// Reads the keys of `adapter`. A file that does not exist is an adapter with no bonds; a file
    /// that cannot be read, is readable by others or does not parse is an error.
    pub fn load(&self, adapter: Address) -> Result<KeySet, KeyStoreError> {
        let path = self.path(adapter);
        let io = |source| KeyStoreError::Io {
            path: path.clone(),
            source,
        };
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(KeySet::default()),
            Err(source) => return Err(io(source)),
        };
        let mode = metadata.mode() & 0o777;
        if mode & 0o077 != 0 {
            return Err(KeyStoreError::Permissions { path, mode });
        }
        let text = fs::read_to_string(&path).map_err(io)?;
        KeySet::parse(&text).map_err(|source| KeyStoreError::Corrupt { path, source })
    }

    /// Writes the keys of `adapter` atomically: a temporary file with mode 0600 in the same
    /// directory, flushed to disk, then renamed over the old file.
    pub fn save(&self, adapter: Address, keys: &KeySet) -> Result<(), KeyStoreError> {
        let path = self.path(adapter);
        let io = |at: &Path| {
            let at = at.to_owned();
            move |source| KeyStoreError::Io { path: at, source }
        };
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.dir)
            .map_err(io(&self.dir))?;
        let temporary = path.with_extension("keys.tmp");
        match fs::remove_file(&temporary) {
            Ok(()) => {}
            Err(source) if source.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(io(&temporary)(source)),
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(io(&temporary))?;
        file.write_all(keys.render().as_bytes())
            .map_err(io(&temporary))?;
        file.sync_all().map_err(io(&temporary))?;
        fs::rename(&temporary, &path).map_err(io(&path))?;
        fs::File::open(&self.dir)
            .and_then(|dir| dir.sync_all())
            .map_err(io(&self.dir))
    }
}
