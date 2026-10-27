//! Servers listening at the same time each get a socket of their own, also when the 32 `wayland-N` names are taken
//! (by other sessions, or other servers running their tests).

use std::collections::HashSet;
use std::os::unix::net::UnixStream;

use cupertino_server::config::Config;
use cupertino_server::Cupertino;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;

fn server() -> (Cupertino, EventLoop<'static, Cupertino>) {
    let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().unwrap();
    let display: Display<Cupertino> = Display::new().unwrap();
    let state = Cupertino::new(
        display,
        event_loop.handle(),
        event_loop.get_signal(),
        Config::default(),
        true,
        &cupertino_assets::Assets::builtin(),
    )
    .expect("a server finds a socket however many others listen");
    (state, event_loop)
}

#[test]
fn forty_servers_listen_at_once_each_on_a_socket_of_its_own() {
    // a runtime directory of its own: 40 servers hold every free wayland-N name while they run, which another
    // compositor starting on the machine at that moment must not feel
    let dir = tempfile::tempdir().expect("a runtime directory");
    std::env::set_var("XDG_RUNTIME_DIR", dir.path());
    let servers: Vec<_> = (0..40).map(|_| server()).collect();
    let names: HashSet<String> = servers
        .iter()
        .map(|(state, _)| state.socket_name.clone().expect("a listening server"))
        .collect();
    assert_eq!(names.len(), 40, "every server has its own socket name");
    for name in &names {
        let socket = dir.path().join(name);
        UnixStream::connect(&socket).unwrap_or_else(|e| panic!("{}: {e}", socket.display()));
    }
    drop(servers);
    for name in &names {
        for leftover in [name.clone(), format!("{name}.lock")] {
            let path = dir.path().join(&leftover);
            assert!(!path.exists(), "{} is left behind", path.display());
        }
    }
}
