//! Saved program binaries: a linked program is written to a per-user cache directory and loaded from it by the next
//! process, which then skips the driver's compiler. The directory has one subdirectory per driver, named by a hash of
//! the driver's vendor, renderer and version strings, so a driver update starts a fresh one and never loads a binary
//! another driver wrote. Each file holds one program, named by the program's name and a hash of its sources; a binary
//! the driver refuses is compiled again and replaced.

use std::cell::Cell;
use std::path::{Path, PathBuf};

/// What the cache did since the device was made.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProgramCacheStats {
    /// Programs loaded from a saved binary.
    pub loaded: u32,
    /// Programs compiled because no usable binary was saved.
    pub compiled: u32,
    /// Binaries written after a compile.
    pub stored: u32,
    /// Saved binaries the driver refused (compiled again and replaced).
    pub refused: u32,
}

/// The cache of one device: its driver's directory.
#[derive(Debug)]
pub(crate) struct ProgramCache {
    dir: PathBuf,
    pub(crate) stats: Cell<ProgramCacheStats>,
}

/// Bytes ahead of a binary in its file: a magic word and the driver's binary format.
const HEADER: usize = 8;
/// The first four bytes of every saved binary.
const MAGIC: [u8; 4] = *b"CPB1";

/// 64-bit FNV-1a of `parts`, each followed by a zero byte so that `("ab", "c")` and `("a", "bc")` differ.
pub(crate) fn fnv(parts: &[&[u8]]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for &b in part.iter().chain(std::iter::once(&0u8)) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

/// The directory name of a driver, from its vendor, renderer and version strings.
pub fn driver_key(vendor: &str, renderer: &str, version: &str) -> String {
    format!(
        "{:016x}",
        fnv(&[vendor.as_bytes(), renderer.as_bytes(), version.as_bytes()])
    )
}

/// The file name of a program, from its name and sources.
pub fn program_key(name: &str, vertex: &str, fragment: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!(
        "{clean}-{:016x}.bin",
        fnv(&[name.as_bytes(), vertex.as_bytes(), fragment.as_bytes()])
    )
}

/// The per-user cache directory programs are saved under: `$XDG_CACHE_HOME/cupertino/programs`, else
/// `~/.cache/cupertino/programs`; `None` without either.
pub fn default_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .map(|h| h.join(".cache"))
        })?;
    Some(base.join("cupertino").join("programs"))
}

/// The turn of one process to make its programs in a driver's directory, taken by [`take_turn`] and given up when
/// dropped.
#[derive(Debug)]
pub struct Turn {
    _held: std::fs::File,
}

/// How often a process waiting for its turn looks again.
const TURN_POLL: std::time::Duration = std::time::Duration::from_millis(5);

/// Waits up to `patience` for this process's turn to make programs in `dir`, a driver's directory
/// ([`crate::Device::program_cache_dir`]): processes that start together then make their programs one after the other,
/// so on a cold cache the first compiles and saves them and the rest load what it saved. Loading the driver's compiler
/// costs a process some 50 MiB that it keeps until it exits; taking turns leaves that to one process. `None` when the
/// directory cannot hold the turn's file or the turn did not come in time: the caller makes its programs anyway.
pub fn take_turn(dir: &Path, patience: std::time::Duration) -> Option<Turn> {
    std::fs::create_dir_all(dir).ok()?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(".turn"))
        .ok()?;
    let deadline = std::time::Instant::now() + patience;
    loop {
        match file.try_lock() {
            Ok(()) => return Some(Turn { _held: file }),
            Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                std::thread::sleep(TURN_POLL);
            }
            Err(_) => return None,
        }
    }
}

impl ProgramCache {
    pub(crate) fn new(base: &Path, driver: &str) -> ProgramCache {
        ProgramCache {
            dir: base.join(driver),
            stats: Cell::new(ProgramCacheStats::default()),
        }
    }

    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    fn bump(&self, f: impl FnOnce(&mut ProgramCacheStats)) {
        let mut s = self.stats.get();
        f(&mut s);
        self.stats.set(s);
    }

    pub(crate) fn note_compiled(&self) {
        self.bump(|s| s.compiled += 1);
    }

    pub(crate) fn note_loaded(&self) {
        self.bump(|s| s.loaded += 1);
    }

    pub(crate) fn note_refused(&self, file: &str) {
        self.bump(|s| s.refused += 1);
        let _ = std::fs::remove_file(self.dir.join(file));
    }

    /// The saved binary of `file` and its format, if there is a well-formed one.
    pub(crate) fn load(&self, file: &str) -> Option<(u32, Vec<u8>)> {
        let bytes = std::fs::read(self.dir.join(file)).ok()?;
        decode(&bytes)
    }

    /// Save `binary` of `format` as `file`, written whole or not at all.
    pub(crate) fn store(&self, file: &str, format: u32, binary: &[u8]) {
        if binary.is_empty() || std::fs::create_dir_all(&self.dir).is_err() {
            return;
        }
        let tmp = self.dir.join(format!(".{file}.{}.tmp", std::process::id()));
        let ok = std::fs::write(&tmp, encode(format, binary)).is_ok()
            && std::fs::rename(&tmp, self.dir.join(file)).is_ok();
        if ok {
            self.bump(|s| s.stored += 1);
        } else {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

fn encode(format: u32, binary: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER + binary.len());
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&format.to_le_bytes());
    out.extend_from_slice(binary);
    out
}

fn decode(bytes: &[u8]) -> Option<(u32, Vec<u8>)> {
    if bytes.len() <= HEADER || bytes[..4] != MAGIC {
        return None;
    }
    let format = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
    Some((format, bytes[HEADER..].to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_driver_update_gives_a_new_directory_and_a_source_change_a_new_file() {
        let a = driver_key("vendor", "renderer", "OpenGL ES 3.2 1.0");
        let b = driver_key("vendor", "renderer", "OpenGL ES 3.2 1.1");
        assert_ne!(a, b);
        assert_eq!(a.len(), 16);
        assert_ne!(
            program_key("shape", "v", "f"),
            program_key("shape", "v", "f2")
        );
        assert_ne!(fnv(&[b"ab", b"c"]), fnv(&[b"a", b"bc"]));
        assert!(program_key("a b/c", "", "").starts_with("a_b_c-"));
    }

    #[test]
    fn a_binary_round_trips_and_a_foreign_file_is_ignored() {
        let dir = std::env::temp_dir().join(format!("cupertino-gpu-cache-{}", std::process::id()));
        let cache = ProgramCache::new(&dir, "driver");
        assert_eq!(cache.load("p.bin"), None);
        cache.store("p.bin", 0x1234, &[1, 2, 3]);
        assert_eq!(cache.load("p.bin"), Some((0x1234, vec![1, 2, 3])));
        assert_eq!(cache.stats.get().stored, 1);
        std::fs::write(cache.dir().join("q.bin"), b"not a binary").unwrap();
        assert_eq!(cache.load("q.bin"), None);
        cache.note_refused("p.bin");
        assert_eq!(cache.load("p.bin"), None, "a refused binary is removed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn processes_take_turns_and_one_kept_waiting_too_long_goes_ahead_without() {
        use std::time::{Duration, Instant};
        let dir = std::env::temp_dir().join(format!("cupertino-gpu-turn-{}", std::process::id()));
        let first = take_turn(&dir, Duration::ZERO).expect("a free turn");
        // another holder of the directory (the lock is the open file's, so this thread stands for another process)
        let waited = Instant::now();
        assert!(
            take_turn(&dir, Duration::from_millis(30)).is_none(),
            "taken"
        );
        assert!(waited.elapsed() >= Duration::from_millis(30), "it waited");
        let later = std::thread::spawn({
            let dir = dir.clone();
            move || take_turn(&dir, Duration::from_secs(5)).is_some()
        });
        std::thread::sleep(Duration::from_millis(20));
        drop(first);
        assert!(later.join().unwrap(), "the turn passes when given up");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
