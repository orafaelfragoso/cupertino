//! The standard-protocol example and the CupertinoKit examples on an isolated headless compositor. Missing Mutter skips;
//! an installed compositor that cannot start or a client that cannot present fails.

#[allow(dead_code)]
mod fixtures {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../ci/test-fixtures.rs"
    ));
}

use std::io::Read;
use std::os::unix::fs::DirBuilderExt;
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

/// The example artifact prepared before the test runner starts.
fn example_binary(package: &str, example: &str) -> PathBuf {
    fixtures::artifact(package, "example", example)
}

/// Whether `mutter` is installed; one that is installed but cannot report its version fails.
fn mutter_installed() -> bool {
    match Command::new("mutter").arg("--version").output() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("SKIPPED: mutter is not installed");
            false
        }
        Err(error) => panic!("run mutter: {error}"),
        Ok(output) => {
            assert!(
                output.status.success(),
                "mutter --version failed: {output:?}"
            );
            true
        }
    }
}

/// A private runtime directory for one compositor run.
fn runtime(name: &str) -> Runtime {
    let runtime = Runtime(
        std::env::temp_dir().join(format!("cupertino-compat-{name}-{}", std::process::id())),
    );
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&runtime.0)
        .unwrap();
    runtime
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
    if !mutter_installed() {
        return;
    }
    let runtime = runtime("simple-window");
    let binary = example_binary("cupertino-wayland-client", "simple-window");
    let _compositor = start(&runtime.0);
    let process = Command::new(binary)
        .args(["--frames", &FRAMES.to_string()])
        .env_clear()
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
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

/// One message of a `WAYLAND_DEBUG=client` trace as libwayland (`[t] {queue}  -> wl_surface#7.frame(new id
/// wl_callback#9)`, events without an arrow) or wayland-rs (`[t][rs] -> wl_surface@7.frame(wl_callback@9)`,
/// `[t][rs] <- wl_callback@9.done, (1)`, strings as `Some("…")`) writes it. Discarded requests are not messages.
#[derive(Debug, PartialEq)]
struct Message<'a> {
    request: bool,
    object: &'a str,
    name: &'a str,
    args: Vec<&'a str>,
}

fn message(line: &str) -> Option<Message<'_>> {
    let mut rest = line.strip_prefix('[')?.split_once(']')?.1;
    rest = rest.strip_prefix("[rs]").unwrap_or(rest).trim_start();
    if let Some(queued) = rest.strip_prefix('{') {
        rest = queued.split_once('}')?.1.trim_start();
    }
    if rest.starts_with("[discarded]") || rest.starts_with("discarded") {
        return None;
    }
    let (request, rest) = match (rest.strip_prefix("-> "), rest.strip_prefix("<- ")) {
        (Some(sent), _) => (true, sent),
        (None, Some(received)) => (false, received),
        (None, None) => (false, rest),
    };
    let (call, args) = rest.split_once('(')?;
    let (object, name) = call.trim_end_matches(", ").rsplit_once('.')?;
    let args = args
        .strip_suffix(')')?
        .split(", ")
        .filter(|arg| !arg.is_empty())
        .map(|arg| {
            let arg = arg.strip_prefix("new id ").unwrap_or(arg);
            arg.strip_prefix("Some(")
                .and_then(|arg| arg.strip_suffix(')'))
                .unwrap_or(arg)
        })
        .collect();
    Some(Message {
        request,
        object,
        name,
        args,
    })
}

/// What a client's own Wayland traffic (`WAYLAND_DEBUG=client`) shows of its window, followed by ids: the toplevel that
/// named itself `app_id` and `title`, its xdg_surface and that surface's configure, and on its own wl_surface an attach
/// of a buffer of `size`, a frame request, a commit after both, and that frame callback answered. Another surface's
/// frame (a cursor) or a buffer of the right size that is never attached does not count.
fn presented(trace: &str, app_id: &str, title: &str, size: (u32, u32)) -> Result<(), String> {
    let messages: Vec<Message> = trace.lines().filter_map(message).collect();
    let request = |object: &str, name: &str| {
        let object = object.to_owned();
        let name = name.to_owned();
        move |m: &&Message| m.request && m.object == object && m.name == name
    };
    let named = |name: &str, value: &str| {
        messages
            .iter()
            .find(|m| m.request && m.name == name && m.args == [format!("\"{value}\"").as_str()])
            .map(|m| m.object)
    };
    let toplevel =
        named("set_app_id", app_id).ok_or_else(|| format!("no set_app_id(\"{app_id}\")"))?;
    if named("set_title", title) != Some(toplevel) {
        return Err(format!("no set_title(\"{title}\") on {toplevel}"));
    }
    let created_by = |name: &str, id: &str| {
        messages
            .iter()
            .find(|m| m.request && m.name == name && m.args.first() == Some(&id))
    };
    let xdg_surface = created_by("get_toplevel", toplevel)
        .map(|m| m.object)
        .ok_or_else(|| format!("no get_toplevel for {toplevel}"))?;
    let surface = created_by("get_xdg_surface", xdg_surface)
        .and_then(|m| m.args.get(1).copied())
        .ok_or_else(|| format!("no get_xdg_surface for {xdg_surface}"))?;
    if !messages
        .iter()
        .any(|m| !m.request && m.object == xdg_surface && m.name == "configure")
    {
        return Err(format!("no configure of {xdg_surface}"));
    }
    let (width, height) = (size.0.to_string(), size.1.to_string());
    let buffers: Vec<&str> = messages
        .iter()
        .filter(|m| {
            m.request
                && m.name == "create_buffer"
                && m.args.get(2) == Some(&width.as_str())
                && m.args.get(3) == Some(&height.as_str())
        })
        .filter_map(|m| m.args.first().copied())
        .collect();
    if buffers.is_empty() {
        return Err(format!("no buffer of {}x{}", size.0, size.1));
    }
    let position = |found: &dyn Fn(&&Message) -> bool| messages.iter().position(|m| found(&m));
    let attach = request(surface, "attach");
    let attached =
        position(&|m: &&Message| attach(m) && m.args.first().is_some_and(|b| buffers.contains(b)))
            .ok_or_else(|| format!("no {}x{} buffer attached to {surface}", size.0, size.1))?;
    let frame = request(surface, "frame");
    let commit = request(surface, "commit");
    let answered = messages.iter().enumerate().any(|(i, m)| {
        frame(&m)
            && m.args.first().is_some_and(|callback| {
                messages
                    .iter()
                    .enumerate()
                    .skip(i.max(attached))
                    .find(|(_, m)| commit(m))
                    .is_some_and(|(c, _)| {
                        messages[c..]
                            .iter()
                            .any(|m| !m.request && m.object == *callback && m.name == "done")
                    })
            })
    });
    if !answered {
        return Err(format!(
            "no frame callback of {surface} answered after a commit"
        ));
    }
    Ok(())
}

/// The lines of a client trace to show on a failure: the client's own messages and its window traffic, not the rest of
/// `WAYLAND_DEBUG`.
fn window_lines(trace: &str) -> String {
    trace
        .lines()
        .filter(|l| {
            !l.starts_with('[')
                || ["xdg_", "wl_surface", "wl_shm", "wl_callback", "error"]
                    .iter()
                    .any(|part| l.contains(part))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A CupertinoKit example names itself, is configured and presents frames of its window's size on Mutter
/// ([`presented`]). Still running when the test ends it only means it did not crash; that it renders is what
/// [`presented`] shows.
fn kit_example_presents_frames_on_mutter(
    example: &str,
    app_id: &str,
    title: &str,
    size: (u32, u32),
) {
    if !mutter_installed() {
        return;
    }
    let binary = example_binary("cupertino-kit", example);
    let runtime = runtime(example);
    let _compositor = start(&runtime.0);
    let trace = runtime.0.join("client-trace");
    let process = Command::new(binary)
        .env_clear()
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .env("XDG_RUNTIME_DIR", &runtime.0)
        .env("WAYLAND_DISPLAY", SOCKET)
        .env("WAYLAND_DEBUG", "client")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&trace).unwrap())
        .spawn()
        .expect("start the example");
    let mut client = Client(process);
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let text = std::fs::read_to_string(&trace).unwrap_or_default();
        let result = presented(&text, app_id, title, size);
        if result.is_ok() {
            break;
        }
        if let Some(status) = client.0.try_wait().unwrap() {
            panic!(
                "{example} exited ({status}) before it presented: {result:?}\n{}",
                window_lines(&text)
            );
        }
        assert!(
            Instant::now() < deadline,
            "{example} did not present on mutter: {result:?}\n{}",
            window_lines(&text)
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        client.0.try_wait().unwrap(),
        None,
        "{example} ended by itself"
    );
}

#[test]
fn the_counter_example_presents_frames_on_mutter() {
    kit_example_presents_frames_on_mutter(
        "counter",
        "org.cupertino.examples.Counter",
        "Counter",
        (200, 64),
    );
}

#[test]
fn the_forms_example_presents_frames_on_mutter() {
    kit_example_presents_frames_on_mutter(
        "forms",
        "org.cupertino.examples.Forms",
        "Forms",
        (280, 180),
    );
}

#[test]
fn the_virtual_list_example_presents_frames_on_mutter() {
    kit_example_presents_frames_on_mutter(
        "virtual-list",
        "org.cupertino.examples.VirtualList",
        "Virtual List",
        (480, 600),
    );
}

#[test]
fn the_editor_example_presents_frames_on_mutter() {
    kit_example_presents_frames_on_mutter(
        "editor",
        "org.cupertino.examples.Editor",
        "Untitled",
        (480, 320),
    );
}

/// The trace check follows the window's own toplevel, xdg_surface and wl_surface by id and names what is missing, in
/// libwayland's and the Rust backend's format.
#[test]
fn a_trace_is_presented_only_by_the_windows_own_surface() {
    let full = [
        "[1.0] {Default Queue}  -> xdg_wm_base#6.get_xdg_surface(new id xdg_surface#8, wl_surface#7)",
        "[1.0] {Default Queue}  -> xdg_surface#8.get_toplevel(new id xdg_toplevel#9)",
        "[1.0] {Default Queue}  -> xdg_toplevel#9.set_app_id(\"org.example.App\")",
        "[1.1] {Default Queue}  -> xdg_toplevel#9.set_title(\"App\")",
        "[1.2] {Default Queue} xdg_surface#8.configure(3)",
        "[1.3] {Default Queue}  -> wl_shm_pool#11.create_buffer(new id wl_buffer#12, 0, 200, 64, 800, 0)",
        "[1.4] {Default Queue}  -> wl_surface#7.attach(wl_buffer#12, 0, 0)",
        "[1.4] {Default Queue}  -> wl_surface#7.frame(new id wl_callback#13)",
        "[1.5] {Default Queue}  -> wl_surface#7.commit()",
        "[1.6] {Default Queue} wl_callback#13.done(1234)",
    ];
    let check = |lines: &[&str]| presented(&lines.join("\n"), "org.example.App", "App", (200, 64));
    let replaced = |n: usize, line: &'static str| {
        let mut lines = full.to_vec();
        lines[n] = line;
        lines
    };
    let without = |n: usize| [&full[..n], &full[n + 1..]].concat();
    assert_eq!(check(&full), Ok(()));
    assert_eq!(
        check(&without(2)),
        Err("no set_app_id(\"org.example.App\")".to_owned())
    );
    assert_eq!(
        check(&without(4)),
        Err("no configure of xdg_surface#8".to_owned())
    );
    assert_eq!(check(&without(5)), Err("no buffer of 200x64".to_owned()));
    // a buffer of the right size that is never attached, another one attached instead
    assert_eq!(
        check(&replaced(
            6,
            "[1.4] {Default Queue}  -> wl_surface#7.attach(wl_buffer#14, 0, 0)"
        )),
        Err("no 200x64 buffer attached to wl_surface#7".to_owned())
    );
    // the only answered frame callback is a cursor surface's
    let cursor = [
        &full[..7],
        &[
            "[1.4] {Default Queue}  -> wl_surface#5.frame(new id wl_callback#13)",
            "[1.5] {Default Queue}  -> wl_surface#5.commit()",
            "[1.5] {Default Queue}  -> wl_surface#7.commit()",
            "[1.6] {Default Queue} wl_callback#13.done(1234)",
        ][..],
    ]
    .concat();
    assert_eq!(
        check(&cursor),
        Err("no frame callback of wl_surface#7 answered after a commit".to_owned())
    );
    assert_eq!(
        check(&full[..9]),
        Err("no frame callback of wl_surface#7 answered after a commit".to_owned())
    );
    assert_eq!(
        check(&replaced(
            9,
            "[1.6] {Default Queue} wl_callback#14.done(1234)"
        )),
        Err("no frame callback of wl_surface#7 answered after a commit".to_owned())
    );
    let libwayland = [
        "[100.0]  -> xdg_wm_base@6.get_xdg_surface(new id xdg_surface@8, wl_surface@7)",
        "[100.0]  -> xdg_surface@8.get_toplevel(new id xdg_toplevel@9)",
        "[100.0]  -> xdg_toplevel@9.set_app_id(\"org.example.App\")",
        "[100.1]  -> xdg_toplevel@9.set_title(\"App\")",
        "[100.2] xdg_surface@8.configure(3)",
        "[100.3]  -> wl_shm_pool@11.create_buffer(new id wl_buffer@12, 0, 200, 64, 800, 0)",
        "[100.4]  -> wl_surface@7.attach(wl_buffer@12, 0, 0)",
        "[100.4]  -> wl_surface@7.frame(new id wl_callback@13)",
        "[100.5]  -> wl_surface@7.commit()",
        "[100.6] wl_callback@13.done(1234)",
    ];
    assert_eq!(check(&libwayland), Ok(()));
    let wayland_rs = [
        "[1869906.700][rs] -> xdg_wm_base@6.get_xdg_surface(xdg_surface@21, wl_surface@20)",
        "[1869906.701][rs] -> xdg_surface@21.get_toplevel(xdg_toplevel@22)",
        "[1869906.785][rs] -> xdg_toplevel@22.set_app_id(Some(\"org.example.App\"))",
        "[1869906.816][rs] -> xdg_toplevel@22.set_title(Some(\"App\"))",
        "[1869909.907][rs] <- xdg_surface@21.configure, (1)",
        "[1869911.405][rs] -> wl_shm_pool@25.create_buffer(wl_buffer@26, 0, 200, 64, 800, 0)",
        "[1869911.500][rs] -> wl_surface@20.attach(Some(wl_buffer@26), 0, 0)",
        "[1869911.616][rs] -> wl_surface@20.frame(wl_callback@27)",
        "[1869911.644][rs] -> wl_surface@20.commit()",
        "[1869926.975][rs] <- wl_callback@27.done, (49385751)",
    ];
    assert_eq!(check(&wayland_rs), Ok(()));
    let mut wide = wayland_rs.to_vec();
    wide[5] = "[1869911.405][rs] -> wl_shm_pool@25.create_buffer(wl_buffer@26, 0, 64, 200, 256, 0)";
    assert_eq!(check(&wide), Err("no buffer of 200x64".to_owned()));
    let mut discarded = wayland_rs.to_vec();
    discarded[2] =
        "[1869906.785][rs][discarded] -> xdg_toplevel@22.set_app_id(Some(\"org.example.App\"))";
    assert_eq!(
        check(&discarded),
        Err("no set_app_id(\"org.example.App\")".to_owned())
    );
}
