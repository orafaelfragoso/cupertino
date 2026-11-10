//! Starting a Cupertino binary from a test with a cleared environment.
//!
//! A package build compiles the installed theme directory into every binary (`CUPERTINO_THEME_DIR`), and that directory
//! does not exist while the tests run. A binary started with a cleared environment finds the theme files only through
//! `CUPERTINO_SPEC_DIR`, so every such spawn starts from [`cleared`], which always sets it.
use std::ffi::OsStr;
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use cupertino_diagnostic_events::journal::SOCKET_ENV;

/// The theme files of the checkout.
pub fn spec_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/theme")
}

/// `binary` with an empty environment but `PATH`, `HOME` (the folder `home`), `CUPERTINO_SPEC_DIR` and the empty asset
/// search path (`CUPERTINO_ASSET_SEARCH_PATH`), so it draws with the built-in assets whatever packs the machine has
/// installed, and with the diagnostic event store under `home` (`XDG_STATE_HOME`) and the test's journal socket
/// ([`diagnostic_journal`]), so what it records reaches neither the user's store nor the system journal; the caller adds
/// the rest.
pub fn cleared(binary: impl AsRef<OsStr>, home: &Path) -> Command {
    let mut command = Command::new(binary);
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", home)
        .env("XDG_STATE_HOME", home.join(".local/state"))
        .env(SOCKET_ENV, diagnostic_journal())
        .env("CUPERTINO_SPEC_DIR", spec_dir())
        .env("CUPERTINO_ASSET_SEARCH_PATH", "");
    command
}

/// Where the diagnostic events of the processes this test process starts go when the test reads none back.
struct Diagnostics {
    /// A state directory (`XDG_STATE_HOME`) under the target's temporary directory, shared by the test processes: the
    /// store bounds itself and takes many writers.
    state: PathBuf,
    /// `journal-<pid>` beside it: a socket this process binds once and keeps open, which no one reads.
    journal: PathBuf,
    socket: UnixDatagram,
}

static DIAGNOSTICS: OnceLock<Diagnostics> = OnceLock::new();

fn diagnostics() -> &'static Diagnostics {
    DIAGNOSTICS.get_or_init(|| {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("diagnostic-events");
        let state = dir.join("state");
        std::fs::create_dir_all(&state).unwrap();
        remove_stale_journals(&dir);
        // Linux pathname sockets need the terminating NUL within sun_path's 108 bytes.
        const SOCKET_PATH_BYTES: usize = 108;
        let candidate = dir.join(format!("journal-{}", std::process::id()));
        // Only the socket uses /tmp when a worktree path is long; diagnostic data stays in the managed target.
        let journal = if candidate.as_os_str().len() >= SOCKET_PATH_BYTES {
            use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
            // The fixture lease owns this short directory, so it survives individual test children and is
            // removed with the run. Dead-process sockets are swept just as they are under a short target path.
            let session = std::env::var_os("CUPERTINO_TEST_FIXTURE_SESSION")
                .expect("long worktree paths require ci/build-test-fixtures.py");
            let short = PathBuf::from(session).join("diagnostic-journals");
            match std::fs::DirBuilder::new().mode(0o700).create(&short) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => panic!("create diagnostic socket directory: {e}"),
            }
            let meta = std::fs::symlink_metadata(&short).unwrap();
            assert!(
                meta.is_dir() && meta.permissions().mode() & 0o077 == 0,
                "diagnostic socket directory must be private"
            );
            remove_stale_journals(&short);
            short.join(format!("journal-{}", std::process::id()))
        } else {
            candidate
        };
        // a socket of an earlier process with the same id
        let _ = std::fs::remove_file(&journal);
        let socket = UnixDatagram::bind(&journal).unwrap();
        extern "C" {
            fn atexit(callback: extern "C" fn()) -> std::ffi::c_int;
        }
        extern "C" fn remove_journal() {
            if let Some(diagnostics) = DIAGNOSTICS.get() {
                let _ = std::fs::remove_file(&diagnostics.journal);
            }
        }
        // SAFETY: `atexit` keeps a plain function pointer; `remove_journal` only removes a file.
        unsafe {
            atexit(remove_journal);
        }
        Diagnostics {
            state,
            journal,
            socket,
        }
    })
}

/// Removes the journal sockets in `dir` whose test process is gone: one killed before it could remove its own.
fn remove_stale_journals(dir: &Path) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let pid = name
            .to_str()
            .and_then(|name| name.strip_prefix("journal-"))
            .and_then(|pid| pid.parse::<u32>().ok());
        if pid.is_some_and(|pid| !Path::new(&format!("/proc/{pid}")).exists()) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The shared diagnostic state directory of [`Diagnostics`].
pub fn diagnostic_state() -> &'static Path {
    &diagnostics().state
}

/// The journal socket of this test process ([`Diagnostics`]), removed when the process exits.
pub fn diagnostic_journal() -> &'static Path {
    &diagnostics().journal
}

/// The receiving end of [`diagnostic_journal`], for a test that reads what reached it.
pub fn diagnostic_journal_socket() -> &'static UnixDatagram {
    &diagnostics().socket
}

/// How long a child of [`wait_after`] may take to get to what is timed. Its own limit is for a child that hangs
/// on the way; how fast a loaded machine starts it is not what the tests measure.
const SETUP_LIMIT: Duration = Duration::from_secs(60);

/// Runs the test `name` of this test binary in a child with `env` set, and returns how the child ended and when. A
/// child that still runs after `limit` is killed and the calling test fails.
pub fn run_child_test(
    name: &str,
    env: &[(&str, &OsStr)],
    limit: Duration,
) -> (ExitStatus, Duration) {
    run_child_test_after(name, env, || true, limit)
}

/// [`run_child_test`], timed by [`wait_after`].
pub fn run_child_test_after(
    name: &str,
    env: &[(&str, &OsStr)],
    started: impl Fn() -> bool,
    limit: Duration,
) -> (ExitStatus, Duration) {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", name, "--nocapture"])
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (key, value) in env {
        command.env(key, value);
    }
    wait_after(command.spawn().unwrap(), started, limit)
}

/// Waits for `child`, timed from the first moment `started` holds, not from the spawn: how the child ended and how long
/// after `started`. A child still running `limit` after it, or [`SETUP_LIMIT`] after the call without it, is killed and
/// the calling test fails.
pub fn wait_after(
    mut child: Child,
    started: impl Fn() -> bool,
    limit: Duration,
) -> (ExitStatus, Duration) {
    let spawned = Instant::now();
    let mut since = None;
    loop {
        if since.is_none() && started() {
            since = Some(Instant::now());
        }
        if let Some(status) = child.try_wait().unwrap() {
            return (status, since.unwrap_or(spawned).elapsed());
        }
        let late = match since {
            Some(at) => (at.elapsed() > limit).then(|| format!("{limit:?} after it started")),
            None => (spawned.elapsed() > SETUP_LIMIT)
                .then(|| format!("{SETUP_LIMIT:?} without starting")),
        };
        if let Some(late) = late {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the child still ran {late}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
