//! The standard-protocol example on an isolated headless compositor. Missing Mutter skips;
//! an installed compositor that cannot start or a client that cannot present fails.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(20);
const SOCKET: &str = "wayland-compat-test";
const FRAMES: u32 = 30;

const TERM_GRACE: Duration = Duration::from_secs(5);

// Starts the compositor with only stdio open: a descriptor inherited from the test runner, such as a gate lock,
// would otherwise stay held for as long as the compositor lives.
const CLOSE_FDS_THEN_EXEC: &str = r#"for fd in /proc/self/fd/*; do n=${fd##*/}; [ "$n" -gt 2 ] 2>/dev/null && eval "exec $n>&-"; done; exec "$@""#;

// Removes the runtime directory and kills the group when the test process is gone, which no Drop can do after a
// SIGKILL from the runner's timeout. The directory goes first: the kill ends this shell too.
const CLEAN_UP_WHEN_ORPHANED: &str =
    r#"while kill -0 "$1" 2>/dev/null; do sleep 1; done; rm -rf -- "$3"; kill -KILL -- "-$2""#;

struct Compositor {
    wrapper: Child,
    watchdog: Option<Child>,
}

impl Compositor {
    fn group_alive(&self) -> bool {
        Command::new("/usr/bin/kill")
            .args(["-0", "--", &format!("-{}", self.wrapper.id())])
            .stderr(Stdio::null())
            .status()
            .expect("inspect compositor group")
            .success()
    }

    fn signal_group(&self, signal: &str) {
        let result = Command::new("/usr/bin/kill")
            .args([signal, "--", &format!("-{}", self.wrapper.id())])
            .status();
        match result {
            Ok(status) if status.success() => {}
            other => eprintln!("signalling test compositor group with {signal}: {other:?}"),
        }
    }

    // A member that exited is a zombie until reaped and still counts as alive, so the two children we own are
    // reaped on the way.
    fn wait_for_group(&mut self, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        loop {
            if let Some(watchdog) = &mut self.watchdog {
                let _ = watchdog.try_wait();
            }
            let _ = self.wrapper.try_wait();
            if !self.group_alive() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Compositor {
    fn drop(&mut self) {
        // The private bus and compositor share only this test's process group.
        self.signal_group("-TERM");
        // The bus wrapper can exit before its compositor finishes writing fixture data.
        if !self.wait_for_group(TERM_GRACE) {
            eprintln!("test compositor group ignored SIGTERM");
            self.signal_group("-KILL");
        }
        assert!(
            self.wait_for_group(TIMEOUT),
            "compositor group did not exit"
        );
    }
}

struct Client(Child);

impl Drop for Client {
    fn drop(&mut self) {
        match self.0.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => self.0.kill().expect("terminate test client"),
            Err(error) => panic!("inspect test client: {error}"),
        }
        self.0.wait().expect("reap test client");
    }
}

struct Runtime(PathBuf);

impl Drop for Runtime {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            eprintln!("remove test runtime {}: {error}", self.0.display());
        }
    }
}

fn simple_window_binary() -> PathBuf {
    let exe = std::env::current_exe().expect("test executable");
    let profile = exe.parent().unwrap().parent().unwrap();
    let mut build = Command::new(std::env::var_os("CARGO").expect("cargo path"));
    build.args([
        "build",
        "-p",
        "cupertino-wayland-client",
        "--example",
        "simple-window",
    ]);
    build.arg("--target-dir").arg(profile.parent().unwrap());
    if profile.file_name().unwrap() == "release" {
        build.arg("--release");
    }
    let status = build.status().expect("build example");
    assert!(status.success(), "building simple-window: {status}");
    profile.join("examples/simple-window")
}

fn start(runtime: &Path) -> Compositor {
    let log = std::fs::File::create(runtime.join("compositor.log")).unwrap();
    let wrapper = Command::new("bash")
        .args([
            "-c",
            CLOSE_FDS_THEN_EXEC,
            "compositor",
            "dbus-run-session",
            "--",
            "mutter",
            "--headless",
            "--wayland",
            "--no-x11",
            "--virtual-monitor",
            "1280x800",
            "--wayland-display",
            SOCKET,
        ])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").expect("PATH"))
        .env("HOME", runtime)
        .env("XDG_RUNTIME_DIR", runtime)
        .env("XDG_CONFIG_HOME", runtime.join("config"))
        .env("XDG_DATA_HOME", runtime.join("data"))
        .env("GSETTINGS_BACKEND", "memory")
        .env("GIO_USE_VFS", "local")
        .env("LIBGL_ALWAYS_SOFTWARE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log)
        .process_group(0)
        .spawn()
        .expect("start private bus and compositor");
    let group = wrapper.id().to_string();
    let mut running = Compositor {
        wrapper,
        watchdog: None,
    };
    let watchdog = Command::new("bash")
        .args([
            "-c",
            CLOSE_FDS_THEN_EXEC,
            "watchdog",
            "bash",
            "-c",
            CLEAN_UP_WHEN_ORPHANED,
            "watchdog",
            &std::process::id().to_string(),
            &group,
            &runtime.to_string_lossy(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(running.wrapper.id() as i32)
        .spawn();
    running.watchdog = Some(watchdog.expect("start compositor watchdog"));
    let deadline = Instant::now() + TIMEOUT;
    while !runtime.join(SOCKET).exists() {
        let status = running.wrapper.try_wait().unwrap();
        assert!(
            status.is_none() && Instant::now() < deadline,
            "compositor did not listen: {status:?}\n{}",
            std::fs::read_to_string(runtime.join("compositor.log")).unwrap()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    running
}

#[test]
fn simple_window_presents_frames_with_client_decorations_on_mutter() {
    match Command::new("mutter").arg("--version").output() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("SKIPPED: mutter is not installed");
            return;
        }
        Err(error) => panic!("run mutter: {error}"),
        Ok(output) => assert!(
            output.status.success(),
            "mutter --version failed: {output:?}"
        ),
    }
    let runtime =
        Runtime(std::env::temp_dir().join(format!("cupertino-compat-{}", std::process::id())));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&runtime.0)
        .unwrap();
    let binary = simple_window_binary();
    let _compositor = start(&runtime.0);
    let process = Command::new(binary)
        .args(["--frames", &FRAMES.to_string()])
        .env_clear()
        .env("XDG_RUNTIME_DIR", &runtime.0)
        .env("WAYLAND_DISPLAY", SOCKET)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .expect("start simple-window");
    let mut client = Client(process);
    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        if let Some(status) = client.0.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "simple-window did not finish");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "simple-window failed: {status}");
    let mut output = String::new();
    client
        .0
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut output)
        .unwrap();
    assert_eq!(
        output.lines().last(),
        Some("frames 30 size 320x240 scale 1 decorations client")
    );
}

use std::os::unix::fs::DirBuilderExt;
