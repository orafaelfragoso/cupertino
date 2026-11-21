//! The X display's socket watch is switched only when the display starts or stops waiting for a client: calloop
//! registers the descriptors again on every enable, so enabling a watch that is on made epoll fail with EEXIST.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cupertino_server::xwayland::{XDisplay, XWayland};
use smithay::reexports::calloop::EventLoop;

/// Collects what the `tracing` subscriber writes.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl Write for Log {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Log {
    type Writer = Log;
    fn make_writer(&'a self) -> Log {
        self.clone()
    }
}

/// A private directory under the target directory, removed when dropped.
struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("xwayland-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(path.join(".X11-unix")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::set_permissions(
            path.join(".X11-unix"),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        Self(path)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn watch_is_switched_only_on_a_change_of_phase() {
    let log = Log::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(log.clone())
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        let dir = Dir::new();
        let display = XDisplay::open(&dir.0, &dir.0).unwrap();
        let socket = dir
            .0
            .join(".X11-unix")
            .join(format!("X{}", &display.name()[1..]));
        let mut event_loop: EventLoop<'static, Option<XWayland>> = EventLoop::try_new().unwrap();
        let handle = event_loop.handle();
        let x = XWayland::start(
            display,
            dir.0.join("no-such-satellite"),
            "wayland-test".into(),
            &handle,
            |state| state.as_mut(),
            |_| {},
        )
        .unwrap();
        let mut state = Some(x);

        // listening with the watch on: polling does not switch it again
        state.as_mut().unwrap().poll(&handle, Instant::now());
        state.as_mut().unwrap().poll(&handle, Instant::now());

        // a client waits, the satellite cannot start: the watch goes off, the display waits for the restart
        let _client = UnixStream::connect(&socket).unwrap();
        event_loop
            .dispatch(Some(Duration::from_millis(500)), &mut state)
            .unwrap();
        let at = state.as_ref().unwrap().next_deadline().expect("waiting");
        state.as_mut().unwrap().poll(&handle, Instant::now());
        assert!(state.as_ref().unwrap().next_deadline().is_some());

        // the restart time passes: the watch goes on once, and polling again leaves it
        state
            .as_mut()
            .unwrap()
            .poll(&handle, at + Duration::from_millis(1));
        assert!(state.as_ref().unwrap().next_deadline().is_none());
        state
            .as_mut()
            .unwrap()
            .poll(&handle, at + Duration::from_secs(1));
    });
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    assert!(
        !text.contains("switching the X socket watch failed"),
        "{text}"
    );
}
