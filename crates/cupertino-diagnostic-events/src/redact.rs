//! The redaction policy: what an event may keep of each value, and the scrubber for free text.
//!
//! Every value is reduced here before it is serialised, so nothing private reaches the memory ring, the store or the
//! journal. The README's table "What diagnostic events never contain" is the policy in words; `tests/redaction.rs`
//! holds one row per class.

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path};

use crate::event::{AddressKind, Identity, Value};

/// What a dropped value is replaced by; its key stays.
pub const REDACTED: &str = "[redacted]";
/// What a user name is replaced by.
pub const USER: &str = "[user]";
/// What a host name is replaced by.
pub const HOST: &str = "[host]";
/// What a URL in free text is replaced by.
pub const URL: &str = "[url]";
/// What an e-mail address in free text is replaced by.
pub const EMAIL: &str = "[email]";
/// What a probable token (a long run of hexadecimal or base64 characters) in free text is replaced by.
pub const TOKEN: &str = "[token]";

/// The shortest run of hexadecimal or base64 characters the scrubber takes for a token.
pub const TOKEN_RUN: usize = 32;

/// The parts of a key whose `key=value` value the scrubber drops.
const SECRET_KEYS: [&str; 6] = ["pass", "secret", "token", "key", "cookie", "auth"];

/// The directories under the home directory that are the desktop's own, kept in paths.
const OWN_HOME_DIRS: [&str; 2] = [".local/state/cupertino", ".config/cupertino"];

/// The system directories whose paths are kept (their components still lose user and host names).
const SYSTEM_DIRS: [&str; 6] = ["/usr", "/etc", "/run", "/dev", "/sys", "/proc"];

/// The key of the per-store hash: 16 random bytes, kept in `events/salt`, so a hash correlates values on one machine and
/// never across machines.
#[derive(Clone, PartialEq, Eq)]
pub struct Salt([u8; 16]);

impl std::fmt::Debug for Salt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Salt(..)")
    }
}

impl Salt {
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Salt(bytes)
    }

    /// 16 bytes of `/dev/urandom`; the process id and the time when it cannot be read.
    pub fn random() -> Self {
        let mut bytes = [0u8; 16];
        if fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut bytes))
            .is_err()
        {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default();
            bytes[..8].copy_from_slice(&now.as_nanos().to_le_bytes()[..8]);
            bytes[8..12].copy_from_slice(&std::process::id().to_le_bytes());
        }
        Salt(bytes)
    }

    /// The salt in `path`, made (mode 0600) when there is none. A file of another size is replaced.
    ///
    /// A new salt is written whole under another name and then linked to `path`, so a reader never sees part of one,
    /// and of processes that make one at the same time every one keeps the salt linked first.
    pub fn load_or_create(path: &Path) -> io::Result<Self> {
        if let Some(salt) = Salt::read(path) {
            return Ok(salt);
        }
        let salt = Salt::random();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let fresh = path.with_file_name(format!(
            ".{name}.{}.{}",
            std::process::id(),
            salt.hash(&name)
        ));
        let written = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&fresh)
            .and_then(|mut file| file.write_all(&salt.0));
        if let Err(e) = written {
            let _ = fs::remove_file(&fresh);
            return Err(e);
        }
        let linked = fs::hard_link(&fresh, path);
        let result = match linked {
            Ok(()) => Ok(salt),
            // another process made one first: keep it, unless it is broken
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => match Salt::read(path) {
                Some(theirs) => Ok(theirs),
                None => fs::rename(&fresh, path).map(|()| salt),
            },
            // a file system without links: replace whatever is there
            Err(_) => fs::rename(&fresh, path).map(|()| salt),
        };
        let _ = fs::remove_file(&fresh);
        result
    }

    /// The salt in `path`, when it holds one.
    fn read(path: &Path) -> Option<Self> {
        let bytes = fs::read(path).ok()?;
        <[u8; 16]>::try_from(bytes.as_slice()).ok().map(Salt)
    }

    /// The stable hash of `text` under this salt: 8 hexadecimal digits.
    pub fn hash(&self, text: &str) -> String {
        let (k0, k1) = (
            u64::from_le_bytes(self.0[..8].try_into().unwrap_or_default()),
            u64::from_le_bytes(self.0[8..].try_into().unwrap_or_default()),
        );
        format!("{:08x}", siphash24(k0, k1, text.as_bytes()) as u32)
    }
}

/// SipHash-2-4, a keyed hash: without the salt the hash of a value cannot be computed, so a short value (an SSID) cannot
/// be found by trying candidates.
fn siphash24(k0: u64, k1: u64, data: &[u8]) -> u64 {
    let mut v = [
        k0 ^ 0x736f_6d65_7073_6575,
        k1 ^ 0x646f_7261_6e64_6f6d,
        k0 ^ 0x6c79_6765_6e65_7261,
        k1 ^ 0x7465_6462_7974_6573,
    ];
    fn round(v: &mut [u64; 4]) {
        v[0] = v[0].wrapping_add(v[1]);
        v[1] = v[1].rotate_left(13) ^ v[0];
        v[0] = v[0].rotate_left(32);
        v[2] = v[2].wrapping_add(v[3]);
        v[3] = v[3].rotate_left(16) ^ v[2];
        v[0] = v[0].wrapping_add(v[3]);
        v[3] = v[3].rotate_left(21) ^ v[0];
        v[2] = v[2].wrapping_add(v[1]);
        v[1] = v[1].rotate_left(17) ^ v[2];
        v[2] = v[2].rotate_left(32);
    }
    let (chunks, remainder) = data.as_chunks::<8>();
    for chunk in chunks {
        let m = u64::from_le_bytes(*chunk);
        v[3] ^= m;
        round(&mut v);
        round(&mut v);
        v[0] ^= m;
    }
    let mut last = [0u8; 8];
    last[..remainder.len()].copy_from_slice(remainder);
    let m = u64::from_le_bytes(last) | ((data.len() as u64) << 56);
    v[3] ^= m;
    round(&mut v);
    round(&mut v);
    v[0] ^= m;
    v[2] ^= 0xff;
    for _ in 0..4 {
        round(&mut v);
    }
    v[0] ^ v[1] ^ v[2] ^ v[3]
}

/// Who and where the process runs: what the policy removes from paths and free text, and the salt of its hashes.
#[derive(Clone, Debug)]
pub struct Policy {
    home: Option<String>,
    user: Option<String>,
    host: Option<String>,
    salt: Salt,
}

/// A value after the policy, and whether the policy removed or rewrote anything of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Reduced {
    pub value: serde_json::Value,
    pub rewritten: bool,
}

impl Policy {
    /// `home` is the user's home directory, `user` and `host` the names free text loses.
    pub fn new(home: Option<&Path>, user: Option<&str>, host: Option<&str>, salt: Salt) -> Self {
        let clean = |s: Option<&str>| {
            s.map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        Policy {
            home: home
                .and_then(|h| h.to_str())
                .map(|h| h.trim_end_matches('/').to_owned())
                .filter(|h| h.len() > 1),
            user: clean(user),
            host: clean(host),
            salt,
        }
    }

    /// The calling process's: `HOME`, `USER` (or `LOGNAME`) and the kernel's host name.
    pub fn of_process(salt: Salt) -> Self {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let home = var("HOME");
        let user = var("USER").or_else(|| var("LOGNAME"));
        let host = fs::read_to_string("/proc/sys/kernel/hostname").ok();
        Policy::new(
            home.as_deref().map(Path::new),
            user.as_deref(),
            host.as_deref(),
            salt,
        )
    }

    pub fn salt(&self) -> &Salt {
        &self.salt
    }

    /// `value` as an event may keep it.
    pub fn value(&self, value: &Value<'_>) -> Reduced {
        let text = |s: String, rewritten: bool| Reduced {
            value: serde_json::Value::String(s),
            rewritten,
        };
        match *value {
            Value::Public(s) => {
                let scrubbed = self.text(s);
                let rewritten = scrubbed != s;
                text(scrubbed, rewritten)
            }
            Value::Count(n) => Reduced {
                value: n.into(),
                rewritten: false,
            },
            Value::Int(n) => Reduced {
                value: n.into(),
                rewritten: false,
            },
            Value::Duration(d) => Reduced {
                value: u64::try_from(d.as_millis()).unwrap_or(u64::MAX).into(),
                rewritten: false,
            },
            Value::Bool(b) => Reduced {
                value: b.into(),
                rewritten: false,
            },
            Value::Path(p) => {
                let shown = p.to_string_lossy();
                let kept = self.path(p);
                let rewritten = kept != shown;
                text(kept, rewritten)
            }
            Value::UserText(_) | Value::Secret(_) => text(REDACTED.to_owned(), true),
            Value::Address(kind, s) => text(self.address(kind, s), true),
            Value::Identifier(Identity::User, _) => text(USER.to_owned(), true),
            Value::Identifier(Identity::Host, _) => text(HOST.to_owned(), true),
        }
    }

    /// An address or a device name: its kind and its salted hash, `mac#1a2b3c4d`.
    pub fn address(&self, kind: AddressKind, text: &str) -> String {
        format!(
            "{}#{}",
            kind.name(),
            self.salt.hash(&text.trim().to_ascii_lowercase())
        )
    }

    /// A path as an event may keep it:
    /// - under the home directory, `~/` and the desktop's own state and configuration paths, or `~/…#<hash>` with the
    ///   salted hash of the basename for anything else;
    /// - under a system directory (`/usr`, `/etc`, `/run`, `/dev`, `/sys`, `/proc`), the path, less user and host names;
    /// - anything else (another home, `/tmp`, removable media, a relative path), `…#<hash>` after its first directory
    ///   when it is a home (`/home/…#<hash>`), or alone.
    pub fn path(&self, path: &Path) -> String {
        let text = path.to_string_lossy();
        if let Some(rest) = self.under_home(&text) {
            let rest = rest.trim_start_matches('/');
            if rest.is_empty() {
                return "~".to_owned();
            }
            if OWN_HOME_DIRS
                .iter()
                .any(|own| rest == *own || rest.starts_with(&format!("{own}/")))
            {
                let kept: Vec<String> = rest.split('/').map(|c| self.word(c)).collect();
                return format!("~/{}", kept.join("/"));
            }
            return format!("~/…#{}", self.basename_hash(path));
        }
        if path.is_absolute()
            && SYSTEM_DIRS
                .iter()
                .any(|dir| text == *dir || text.starts_with(&format!("{dir}/")))
            && !path.components().any(|c| c == Component::ParentDir)
        {
            let kept: Vec<String> = text.split('/').map(|c| self.word(c)).collect();
            return kept.join("/");
        }
        if text.starts_with("/home/") || text.starts_with("/root/") || text == "/root" {
            let first = if text.starts_with("/home/") {
                "/home"
            } else {
                "/root"
            };
            return format!("{first}/…#{}", self.basename_hash(path));
        }
        format!("…#{}", self.basename_hash(path))
    }

    fn basename_hash(&self, path: &Path) -> String {
        let base = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        self.salt.hash(&base)
    }

    /// What follows the home directory in `text` (or `~`), when `text` is under it.
    fn under_home<'t>(&self, text: &'t str) -> Option<&'t str> {
        if text == "~" {
            return Some("");
        }
        if let Some(rest) = text.strip_prefix("~/") {
            return Some(rest);
        }
        let home = self.home.as_deref()?;
        let rest = text.strip_prefix(home)?;
        (rest.is_empty() || rest.starts_with('/')).then_some(rest)
    }

    /// Free text (a panic message, an error's text) without home paths, user and host names, e-mail addresses, URLs,
    /// network and hardware addresses, probable tokens and the values of `key=value` pairs whose key names a secret.
    pub fn text(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for (i, line) in text.split('\n').enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&self.line(line));
        }
        out
    }

    fn line(&self, line: &str) -> String {
        let line = self.home_spans(line);
        let mut out = String::with_capacity(line.len());
        // the value after `Authorization:`, `password:` and the like; `Bearer` and `Basic` are followed by the secret
        let mut drop_next = 0u8;
        let mut rest = line.as_str();
        while !rest.is_empty() {
            let start = rest
                .find(|c: char| !c.is_whitespace())
                .unwrap_or(rest.len());
            out.push_str(&rest[..start]);
            rest = &rest[start..];
            if rest.is_empty() {
                break;
            }
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let word = &rest[..end];
            rest = &rest[end..];
            if drop_next > 0 {
                let (lead, core, trail) = split_punctuation(word);
                let scheme = matches!(
                    core.to_ascii_lowercase().as_str(),
                    "bearer" | "basic" | "token"
                );
                out.push_str(lead);
                out.push_str(if scheme { core } else { REDACTED });
                out.push_str(trail);
                drop_next = if scheme { drop_next } else { drop_next - 1 };
                continue;
            }
            if let Some(key) = word.strip_suffix(':').or_else(|| word.strip_suffix('=')) {
                if secret_key(key) {
                    out.push_str(&self.word(key));
                    out.push_str(&word[key.len()..]);
                    drop_next = 1;
                    continue;
                }
            }
            out.push_str(&self.word(word));
        }
        out
    }

    /// Rewrites every path under a home directory (`/home/…`, `/root/…`, `~/…` and the user's own home) as one path,
    /// spaces included, up to a quote, a bracket, a comma, a newline, or a colon or semicolon followed by a space.
    fn home_spans(&self, line: &str) -> String {
        let mut out = String::with_capacity(line.len());
        let mut rest = line;
        loop {
            let found = self.next_home(rest);
            let Some(at) = found else {
                out.push_str(rest);
                return out;
            };
            out.push_str(&rest[..at]);
            let span = &rest[at..];
            let len = span_end(span);
            out.push_str(&self.path(Path::new(&span[..len])));
            rest = &span[len..];
        }
    }

    /// Where the next home path starts in `text`.
    fn next_home(&self, text: &str) -> Option<usize> {
        let mut best: Option<usize> = None;
        let mut take = |at: Option<usize>| {
            if let Some(at) = at {
                best = Some(best.map_or(at, |b| b.min(at)));
            }
        };
        if let Some(home) = &self.home {
            take(find_path_start(text, home));
        }
        take(text.find("/home/"));
        take(find_path_start(text, "/root"));
        // `~/` at the start of a word
        take(text.match_indices("~/").map(|(i, _)| i).find(|&i| {
            i == 0
                || !text[..i]
                    .chars()
                    .last()
                    .is_some_and(|c| c.is_alphanumeric())
        }));
        best
    }

    /// One whitespace-free word of free text.
    fn word(&self, word: &str) -> String {
        if word.is_empty() {
            return String::new();
        }
        let (lead, core, trail) = split_punctuation(word);
        if core.is_empty() {
            return word.to_owned();
        }
        let mut out = String::from(lead);
        out.push_str(&self.core(core));
        out.push_str(trail);
        out
    }

    fn core(&self, core: &str) -> String {
        if core.contains("://") || core.starts_with("mailto:") || core.starts_with("www.") {
            return URL.to_owned();
        }
        if core.starts_with('/') {
            return self.path(Path::new(core));
        }
        if let Some(at) = core.find('@') {
            let (local, domain) = (&core[..at], &core[at + 1..]);
            if !local.is_empty() && !domain.is_empty() {
                return if domain.contains('.') {
                    EMAIL.to_owned()
                } else {
                    format!("{USER}@{HOST}")
                };
            }
        }
        if let Some(eq) = core.find('=') {
            let (key, value) = (&core[..eq], &core[eq + 1..]);
            // `=` at the end only is base64 padding, not a pair
            if !key.is_empty() && !value.bytes().all(|b| b == b'=') {
                if secret_key(key) {
                    return format!("{}={REDACTED}", self.core(key));
                }
                if !value.is_empty() {
                    return format!("{}={}", self.core(key), self.word(value));
                }
            }
        }
        if let Some(kind) = address_kind(core) {
            return self.address(kind, core);
        }
        let core = tokens(core);
        let core = match &self.user {
            Some(user) if user.len() >= 2 => replace_word(&core, user, USER),
            _ => core,
        };
        match &self.host {
            Some(host) if host.len() >= 2 => replace_word(&core, host, HOST),
            _ => core,
        }
    }
}

/// Where `prefix` starts a path in `text`: followed by `/` or the end, and not inside a longer name.
fn find_path_start(text: &str, prefix: &str) -> Option<usize> {
    text.match_indices(prefix).map(|(i, _)| i).find(|&i| {
        let after = &text[i + prefix.len()..];
        let before_ok = i == 0
            || !text[..i]
                .chars()
                .last()
                .is_some_and(|c| c.is_alphanumeric() || c == '/');
        before_ok
            && (after.is_empty()
                || after.starts_with('/')
                || after.starts_with(char::is_whitespace))
    })
}

/// The length of the path at the start of `span`.
fn span_end(span: &str) -> usize {
    let bytes = span.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        let next_space = bytes.get(i + 1).is_none_or(|n| n.is_ascii_whitespace());
        match c {
            b'"' | b'\'' | b'`' | b'\n' | b'\r' | b',' | b')' | b']' | b'>' | b'(' | b'['
            | b'<' => return i,
            b':' | b';' if next_space => return i,
            b'.' if next_space && i + 1 == bytes.len() => return i,
            b' ' | b'\t'
                if bytes
                    .get(i + 1)
                    .is_none_or(|n| !n.is_ascii_alphanumeric() && *n != b'.' && *n != b'_') =>
            {
                return i
            }
            _ => i += 1,
        }
    }
    i
}

/// A word's leading and trailing punctuation, and what is between.
fn split_punctuation(word: &str) -> (&str, &str, &str) {
    let lead_len = word.len()
        - word
            .trim_start_matches(['(', '[', '{', '<', '"', '\'', '`'])
            .len();
    let rest = &word[lead_len..];
    let core = rest.trim_end_matches([
        ')', ']', '}', '>', '"', '\'', '`', ',', '.', ';', ':', '!', '?',
    ]);
    (&word[..lead_len], core, &rest[core.len()..])
}

/// Whether a key names a secret: it contains `pass`, `secret`, `token`, `key`, `cookie` or `auth`, in any case.
fn secret_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    SECRET_KEYS.iter().any(|part| key.contains(part))
}

/// What kind of address `core` is, if it is one: an IPv4 address (with a port or a prefix length), a MAC or Bluetooth
/// address (six pairs of hexadecimal digits), or an IPv6 address.
fn address_kind(core: &str) -> Option<AddressKind> {
    let core = core.trim_start_matches('[');
    if ipv4(core) {
        return Some(AddressKind::Ip);
    }
    let groups: Vec<&str> = core.split([':', '-']).collect();
    if groups.len() == 6
        && groups
            .iter()
            .all(|g| g.len() == 2 && g.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Some(AddressKind::Mac);
    }
    ipv6(core).then_some(AddressKind::Ip)
}

fn ipv4(core: &str) -> bool {
    let addr = core.split([':', '/']).next().unwrap_or("");
    let parts: Vec<&str> = addr.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.len() <= 3
                && p.bytes().all(|b| b.is_ascii_digit())
                && p.parse::<u16>().is_ok_and(|n| n <= 255)
        })
}

fn ipv6(core: &str) -> bool {
    let addr = core.split(['%', '/', ']']).next().unwrap_or("");
    let groups: Vec<&str> = addr.split(':').collect();
    if groups.len() < 3 || groups.len() > 8 || groups.iter().all(|g| g.is_empty()) {
        return false;
    }
    let compressed = addr.contains("::");
    if !compressed && groups.len() != 8 {
        return false;
    }
    let last = groups.len() - 1;
    groups.iter().enumerate().all(|(i, g)| {
        (g.len() <= 4 && g.bytes().all(|b| b.is_ascii_hexdigit())) || (i == last && ipv4(g))
    })
}

/// `text` with each probable token replaced: a run of at least [`TOKEN_RUN`] hexadecimal or base64 characters that is
/// all hexadecimal or holds both digits and letters.
fn tokens(text: &str) -> String {
    let is_run = |c: char| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '_' | '-');
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(is_run) {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest.find(|c: char| !is_run(c)).unwrap_or(rest.len());
        let run = &rest[..end];
        let hex = run.bytes().all(|b| b.is_ascii_hexdigit());
        let mixed =
            run.bytes().any(|b| b.is_ascii_digit()) && run.bytes().any(|b| b.is_ascii_alphabetic());
        if run.len() >= TOKEN_RUN && (hex || mixed) {
            out.push_str(TOKEN);
        } else {
            out.push_str(run);
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// `text` with every whole-word `needle` (not inside a longer name of letters and digits) replaced by `with`.
fn replace_word(text: &str, needle: &str, with: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut prev: Option<char> = None;
    while let Some(at) = rest.find(needle) {
        let before = rest[..at].chars().last().or(prev);
        let after = rest[at + needle.len()..].chars().next();
        let bounded =
            !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric);
        out.push_str(&rest[..at]);
        if bounded {
            out.push_str(with);
        } else {
            out.push_str(needle);
        }
        prev = needle.chars().last();
        rest = &rest[at + needle.len()..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn siphash_matches_the_reference_vector() {
        // the reference implementation's key 00..0f and message 00..0e
        let key: Vec<u8> = (0..16).collect();
        let k0 = u64::from_le_bytes(key[..8].try_into().unwrap());
        let k1 = u64::from_le_bytes(key[8..].try_into().unwrap());
        let message: Vec<u8> = (0..15).collect();
        assert_eq!(siphash24(k0, k1, &message), 0xa129ca6149be45e5);
        assert_eq!(siphash24(k0, k1, &[]), 0x726fdb47dd0e0e31);
    }

    #[test]
    fn spans_end_at_separators() {
        assert_eq!(
            span_end("/home/a/My Files/x.txt: not found"),
            "/home/a/My Files/x.txt".len()
        );
        assert_eq!(span_end("/home/a/x\" and"), "/home/a/x".len());
        assert_eq!(span_end("/home/a/x, then"), "/home/a/x".len());
        assert_eq!(span_end("/home/a/x - gone"), "/home/a/x".len());
    }
}
