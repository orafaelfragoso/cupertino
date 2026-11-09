//! Reopening applications at the next login, with the real login process under the server: the list a previous session
//! saved in the state directory of a fixture home is started once, through the desktop entries of the fixture's data
//! directory; the shell's own programs, removed entries, duplicates and an application a login item started are not.
//! The list is emptied before anything starts, so a restarted login process starts nothing, and a list saved in the
//! running session is dropped. Every directory the process reads is the fixture's; it finds its session through a
//! stand-in logind on a private system bus and serves its interface on a private session bus.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cupertino_server::backend::headless;
use cupertino_server::cli::start_shell_supervision;
use cupertino_server::config::{Config, ShellProcess};
use cupertino_server::Cupertino;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;

const OUTPUT: (i32, i32) = (800, 600);

struct Server {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
}

impl Server {
    /// A server with an offscreen output that supervises `login`; `None` without an EGL device.
    fn start(login: ShellProcess) -> Option<Server> {
        let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().unwrap();
        let mut state = Cupertino::new(
            Display::new().unwrap(),
            event_loop.handle(),
            event_loop.get_signal(),
            Config {
                shell_processes: vec![login],
                ..Config::default()
            },
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .unwrap();
        if let Err(e) = headless::init(&mut state, OUTPUT, 1.0) {
            common::egl_unavailable(format!("{e:#}"));
            return None;
        }
        start_shell_supervision(&state, &event_loop).expect("the supervision timer is inserted");
        Some(Server { state, event_loop })
    }

    fn step(&mut self) {
        headless::step(
            &mut self.state,
            &mut self.event_loop,
            Duration::from_millis(5),
        )
        .unwrap();
    }

    fn wait_until(&mut self, what: &str, mut condition: impl FnMut() -> bool) {
        let end = Instant::now() + common::HANG_GUARD;
        while !condition() {
            assert!(Instant::now() < end, "timed out waiting for {what}");
            self.step();
        }
    }
}

/// The fixture home and XDG directories, the stand-in programs and the files they write.
struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        for sub in [
            "home",
            "config",
            "config-dirs/autostart",
            "data/applications",
            "data-dirs",
            "state/cupertino",
            "cache",
            "bin",
        ] {
            std::fs::create_dir_all(root.join(sub)).unwrap();
        }
        let runtime = root.join("runtime");
        std::fs::create_dir(&runtime).unwrap();
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
        Fixture { _dir: dir, root }
    }

    fn path(&self, sub: &str) -> PathBuf {
        self.root.join(sub)
    }

    /// The file each stand-in program appends its name to.
    fn marker(&self) -> PathBuf {
        self.path("started")
    }

    /// A program that appends `name` to the marker file, and its path.
    fn program(&self, name: &str) -> PathBuf {
        let path = self.path("bin").join(name);
        std::fs::write(
            &path,
            format!("#!/bin/sh\necho {name} >> '{}'\n", self.marker().display()),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// An application entry `id` in the fixture's data directory that runs the stand-in `program`.
    fn application(&self, id: &str, program: &str) {
        let exec = self.program(program);
        std::fs::write(
            self.path("data/applications").join(id),
            format!(
                "[Desktop Entry]\nType=Application\nName={program}\nExec={} %U\n",
                exec.display()
            ),
        )
        .unwrap();
    }

    /// A login item `id` that runs the stand-in `program`.
    fn login_item(&self, id: &str, program: &str) {
        let exec = self.program(program);
        std::fs::write(
            self.path("config-dirs/autostart").join(id),
            format!(
                "[Desktop Entry]\nType=Application\nName={program}\nExec={}\n",
                exec.display()
            ),
        )
        .unwrap();
    }

    fn state_file(&self) -> PathBuf {
        self.path("state/cupertino/restore")
    }

    /// Whether a stand-in program is still running. The login process logs what it started only once each start
    /// returned, so after that log line a program that is no longer running has written its name to the marker.
    fn programs_running(&self) -> bool {
        let bin = self.path("bin").display().to_string();
        std::fs::read_dir("/proc").unwrap().flatten().any(|entry| {
            std::fs::read(entry.path().join("cmdline")).is_ok_and(|cmdline| {
                cmdline
                    .split(|b| *b == 0)
                    .any(|arg| arg.starts_with(bin.as_bytes()))
            })
        })
    }

    fn started(&self) -> Vec<String> {
        let mut lines: Vec<String> = std::fs::read_to_string(self.marker())
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect();
        lines.sort();
        lines
    }

    /// The real login process on the session bus `bus`, finding its session through the stand-in logind on `system`,
    /// with every XDG directory in the fixture and its standard error appended to `log`.
    fn login_process(&self, bus: &str, system: &str, log: &Path) -> ShellProcess {
        let mut login = common::shell::process("cupertino-login", bus, &[]);
        let program = login.command.pop().unwrap();
        let path = |sub: &str| self.path(sub).display().to_string();
        login.command.extend([
            format!("DBUS_SYSTEM_BUS_ADDRESS={system}"),
            format!("XDG_SESSION_ID={}", fake_logind::SESSION_ID),
            format!("HOME={}", path("home")),
            format!("XDG_CONFIG_HOME={}", path("config")),
            format!("XDG_CONFIG_DIRS={}", path("config-dirs")),
            format!("XDG_DATA_HOME={}", path("data")),
            format!("XDG_DATA_DIRS={}", path("data-dirs")),
            format!("XDG_STATE_HOME={}", path("state")),
            format!("XDG_CACHE_HOME={}", path("cache")),
            format!("XDG_RUNTIME_DIR={}", path("runtime")),
            format!("CUPERTINO_SPEC_DIR={}", common::spawn::spec_dir().display()),
            "sh".into(),
            "-c".into(),
            "exec \"$0\" 2>>\"$1\"".into(),
            program,
            log.display().to_string(),
        ]);
        login
    }
}

fn log_text(log: &Path) -> String {
    std::fs::read_to_string(log).unwrap_or_default()
}

/// The ID of this boot, as the login process names it in the saved state.
fn boot_id() -> String {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .unwrap()
        .trim()
        .to_owned()
}

#[test]
fn the_saved_applications_start_once_at_the_next_login_and_never_again() {
    let fixture = Fixture::new();
    fixture.application("org.example.Restored.desktop", "restored");
    fixture.application("org.example.Both.desktop", "both-as-application");
    fixture.application("cupertino-dock.desktop", "shell-program");
    fixture.login_item("org.example.Both.desktop", "both-as-login-item");
    // a previous session on another boot saved these, the dock's entry and a removed one among them
    std::fs::write(
        fixture.state_file(),
        "version = 1\nreopen = true\nboot = an-earlier-boot\nsession = 7\n\
         app = org.example.Restored.desktop\napp = org.example.Both.desktop\napp = cupertino-dock.desktop\n\
         app = org.example.Removed.desktop\napp = org.example.Restored.desktop\n",
    )
    .unwrap();
    let session = common::shell::private_bus();
    let system = common::shell::private_bus();
    let _logind = fake_logind::serve(&system.address);
    let log = fixture.path("login.log");

    let Some(mut server) =
        Server::start(fixture.login_process(&session.address, &system.address, &log))
    else {
        return;
    };
    server.wait_until("the saved applications to be started", || {
        let text = log_text(&log);
        text.contains("application reopened: org.example.Restored.desktop")
            && text.contains("application not reopened: org.example.Removed.desktop")
    });
    server.wait_until("the started programs to finish", || {
        !fixture.programs_running()
    });
    assert_eq!(
        fixture.started(),
        ["both-as-login-item", "restored"],
        "each application started once: the saved one through its entry, the login item's not again, the shell \
         program never\n{}",
        log_text(&log)
    );
    let text = log_text(&log);
    for line in [
        "application not reopened: org.example.Both.desktop: LoginItem",
        "application not reopened: cupertino-dock.desktop: Shell",
        "application not reopened: org.example.Removed.desktop: NoEntry",
    ] {
        assert!(text.contains(line), "{line:?} in\n{text}");
    }
    let state = std::fs::read_to_string(fixture.state_file()).unwrap();
    assert!(
        state.contains("reopen = true") && !state.contains("app ="),
        "the list is emptied and the choice kept:\n{state}"
    );
    let mode = std::fs::metadata(fixture.state_file())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "the state file is its owner's only");

    // the login process restarts in the same session: it finds no list, and a list saved in this very session (a restart
    // that was refused, then a crash) is dropped
    drop(server);
    wait_for_no_owner(&session.address, "org.cupertino.Login1");
    std::fs::write(
        fixture.state_file(),
        format!(
            "version = 1\nreopen = true\nboot = {}\nsession = {}\napp = org.example.Restored.desktop\n",
            boot_id(),
            fake_logind::SESSION_ID
        ),
    )
    .unwrap();
    let log = fixture.path("login-again.log");
    let mut server = Server::start(fixture.login_process(&session.address, &system.address, &log))
        .expect("the offscreen output worked a moment ago");
    server.wait_until("the restarted login process to look at the list", || {
        log_text(&log).contains("applications not reopened: the list was saved in this session")
    });
    server.wait_until("the started programs to finish", || {
        !fixture.programs_running()
    });
    assert_eq!(
        fixture.started(),
        ["both-as-login-item", "restored"],
        "nothing started a second time\n{}",
        log_text(&log)
    );
    assert!(log_text(&log).contains("login items ran already in this session"));
    assert!(!std::fs::read_to_string(fixture.state_file())
        .unwrap()
        .contains("app ="));
}

/// Wait until nobody owns `name` on the bus at `address`: the login process before has gone.
fn wait_for_no_owner(address: &str, name: &str) {
    let connection = zbus::blocking::connection::Builder::address(address)
        .unwrap()
        .build()
        .unwrap();
    let end = Instant::now() + common::HANG_GUARD;
    loop {
        let owned: bool = connection
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "NameHasOwner",
                &name,
            )
            .unwrap()
            .body()
            .deserialize()
            .unwrap();
        if !owned {
            return;
        }
        assert!(
            Instant::now() < end,
            "timed out waiting for {name} to have no owner"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A stand-in for logind for the login process: session [`SESSION_ID`] is active on `seat0`, every power action is
/// allowed, and each inhibitor is one end of a socket pair whose other end the stand-in keeps.
mod fake_logind {
    use std::os::fd::OwnedFd;
    use std::sync::Mutex;
    use zbus::zvariant::{self, OwnedObjectPath};

    pub const SESSION_ID: &str = "1";
    const SESSION_PATH: &str = "/org/freedesktop/login1/session/_31";

    struct Manager {
        held: Mutex<Vec<OwnedFd>>,
    }

    #[zbus::interface(name = "org.freedesktop.login1.Manager")]
    impl Manager {
        fn get_session(&self, id: &str) -> zbus::fdo::Result<OwnedObjectPath> {
            if id != SESSION_ID {
                return Err(zbus::fdo::Error::Failed(format!("no session {id}")));
            }
            Ok(OwnedObjectPath::try_from(SESSION_PATH).unwrap())
        }

        fn inhibit(
            &self,
            _what: &str,
            _who: &str,
            _why: &str,
            _mode: &str,
        ) -> zbus::fdo::Result<zvariant::OwnedFd> {
            let (kept, given) = std::os::unix::net::UnixStream::pair()
                .map_err(|e| zbus::fdo::Error::IOError(e.to_string()))?;
            self.held.lock().unwrap().push(kept.into());
            Ok(OwnedFd::from(given).into())
        }

        fn can_suspend(&self) -> String {
            "yes".to_owned()
        }

        fn can_reboot(&self) -> String {
            "yes".to_owned()
        }

        fn can_power_off(&self) -> String {
            "yes".to_owned()
        }
    }

    struct Session;

    #[zbus::interface(name = "org.freedesktop.login1.Session")]
    impl Session {
        #[zbus(property)]
        fn id(&self) -> String {
            SESSION_ID.to_owned()
        }

        #[zbus(property, name = "Type")]
        fn session_type(&self) -> String {
            "wayland".to_owned()
        }

        #[zbus(property)]
        fn active(&self) -> bool {
            true
        }

        #[zbus(property, name = "VTNr")]
        fn vt_nr(&self) -> u32 {
            2
        }

        #[zbus(property)]
        fn seat(&self) -> (String, OwnedObjectPath) {
            (
                "seat0".to_owned(),
                OwnedObjectPath::try_from("/org/freedesktop/login1/seat/seat0").unwrap(),
            )
        }
    }

    /// Serve the stand-in on the bus at `address` for as long as the returned connection lives.
    pub fn serve(address: &str) -> zbus::blocking::Connection {
        zbus::blocking::connection::Builder::address(address)
            .unwrap()
            .name("org.freedesktop.login1")
            .unwrap()
            .serve_at(
                "/org/freedesktop/login1",
                Manager {
                    held: Mutex::new(Vec::new()),
                },
            )
            .unwrap()
            .serve_at(SESSION_PATH, Session)
            .unwrap()
            .build()
            .unwrap()
    }
}
