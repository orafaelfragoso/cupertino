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

/// Why [`take_turn`] gave no turn.
#[derive(Debug)]
pub enum NoTurn {
    /// The turn cannot be held at all: the directory or its turn file cannot be made or opened (a read-only or full
    /// cache), or the lock failed for another reason than another process holding it. Waiting gains nothing.
    Unavailable(std::io::Error),
    /// Another process held the turn for all of the patience.
    TimedOut,
}

impl std::fmt::Display for NoTurn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NoTurn::Unavailable(e) => write!(f, "the turn cannot be held: {e}"),
            NoTurn::TimedOut => f.write_str("another process kept the turn"),
        }
    }
}

impl std::error::Error for NoTurn {}

/// Waits up to `patience` for this process's turn to make programs in `dir`, a driver's directory
/// ([`crate::Device::program_cache_dir`]): processes that start together then make their programs one after the other,
/// so on a cold cache the first compiles and saves them and the rest load what it saved. Loading the driver's compiler
/// costs a process some 50 MiB that it keeps until it exits; taking turns leaves that to one process. A turn that
/// cannot be held ([`NoTurn::Unavailable`]) is reported at once, without waiting; on either error the caller makes its
/// programs anyway.
pub fn take_turn(dir: &Path, patience: std::time::Duration) -> Result<Turn, NoTurn> {
    std::fs::create_dir_all(dir).map_err(NoTurn::Unavailable)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(".turn"))
        .map_err(NoTurn::Unavailable)?;
    let deadline = std::time::Instant::now() + patience;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Turn { _held: file }),
            Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                std::thread::sleep(TURN_POLL);
            }
            Err(std::fs::TryLockError::WouldBlock) => return Err(NoTurn::TimedOut),
            Err(std::fs::TryLockError::Error(e)) => return Err(NoTurn::Unavailable(e)),
        }
    }
}

/// [`take_turn`] waited on in steps of at most [`STOP_POLL`], so a thread waiting while another process holds the
/// turn sees `stop`: `None` once `stop` is set, when the patience ran out, or at once when the turn cannot be held.
pub fn wait_turn(
    dir: &Path,
    patience: std::time::Duration,
    stop: &std::sync::atomic::AtomicBool,
) -> Option<Turn> {
    use std::sync::atomic::Ordering;
    let deadline = std::time::Instant::now() + patience;
    loop {
        if stop.load(Ordering::Relaxed) {
            return None;
        }
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match take_turn(dir, left.min(STOP_POLL)) {
            Ok(turn) => return Some(turn),
            Err(NoTurn::TimedOut) if left > STOP_POLL => {}
            Err(_) => return None,
        }
    }
}

/// Makes `programs` into the cache under `base` for the driver of a device on `platform`, as a process that later makes
/// them on that device does, so that process loads every one instead of compiling: a device of its own, the driver's
/// directory's turn ([`wait_turn`], waited on up to `patience`, and not at all when it cannot be held), each program loaded where a saved binary the driver
/// takes is there, else compiled and saved (a refused binary is replaced). The directory and the file names are the
/// ones that process looks up, keyed by the driver's strings and the programs' sources, so a binary of another
/// driver is never loaded. The device is gone when it returns. It takes the compiles' time and the driver's compiler:
/// run it on a thread of its own, off any frame path. `None` when the driver keeps no program binaries. Once `stop` is
/// set it makes no further program and returns: a process must not exit while a thread of it is inside the driver, so
/// one that ends sets it and joins the thread, which then waits at most for the program being made.
pub fn make_ahead(
    platform: crate::Platform,
    base: &Path,
    programs: &[crate::shaders::ShaderSource],
    patience: std::time::Duration,
    stop: &std::sync::atomic::AtomicBool,
) -> Result<Option<ProgramCacheStats>, crate::Error> {
    use std::sync::atomic::Ordering;
    let mut device = crate::Device::new(&crate::DeviceDesc { platform })?;
    device.set_program_cache(Some(base));
    let Some(dir) = device.program_cache_dir().map(Path::to_path_buf) else {
        return Ok(None);
    };
    // waited on in short steps so a stop is seen while another process holds it; a turn that cannot be held at all
    // is not waited for
    let turn = wait_turn(&dir, patience, stop);
    if stop.load(Ordering::Relaxed) {
        return Ok(Some(device.program_cache_stats()));
    }
    for program in programs {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        device.prepare_programs(std::slice::from_ref(program))?;
    }
    drop(turn);
    Ok(Some(device.program_cache_stats()))
}

/// How often [`wait_turn`] looks at its stop while it waits for the turn.
const STOP_POLL: std::time::Duration = std::time::Duration::from_millis(50);

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
            matches!(
                take_turn(&dir, Duration::from_millis(30)),
                Err(NoTurn::TimedOut)
            ),
            "taken"
        );
        assert!(waited.elapsed() >= Duration::from_millis(30), "it waited");
        let later = std::thread::spawn({
            let dir = dir.clone();
            move || take_turn(&dir, Duration::from_secs(5)).is_ok()
        });
        std::thread::sleep(Duration::from_millis(20));
        drop(first);
        assert!(later.join().unwrap(), "the turn passes when given up");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cache_that_cannot_hold_the_turn_is_given_up_at_once_not_after_the_patience() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::atomic::AtomicBool;
        use std::time::{Duration, Instant};
        let root =
            std::env::temp_dir().join(format!("cupertino-gpu-noturn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // a cache path under a file (no directory can be made there, whoever runs the test), and a cache directory
        // without write permission (its turn file cannot be made, unless the test runs with every permission)
        let file = root.join("file");
        std::fs::write(&file, b"").unwrap();
        let read_only = root.join("read-only");
        std::fs::create_dir_all(&read_only).unwrap();
        std::fs::set_permissions(&read_only, std::fs::Permissions::from_mode(0o555)).unwrap();
        let everything_allowed = std::fs::write(read_only.join("probe"), b"").is_ok();
        let mut dirs = vec![file.join("driver")];
        if !everything_allowed {
            dirs.push(read_only.clone());
        }
        let patience = Duration::from_secs(10);
        for dir in &dirs {
            let started = Instant::now();
            let r = take_turn(dir, patience);
            assert!(matches!(r, Err(NoTurn::Unavailable(_))), "{dir:?}: {r:?}");
            assert!(
                started.elapsed() < STOP_POLL,
                "{dir:?}: {:?}",
                started.elapsed()
            );
            let started = Instant::now();
            assert!(wait_turn(dir, patience, &AtomicBool::new(false)).is_none());
            assert!(
                started.elapsed() < STOP_POLL,
                "{dir:?}: {:?}",
                started.elapsed()
            );
        }
        std::fs::set_permissions(&read_only, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }
}
