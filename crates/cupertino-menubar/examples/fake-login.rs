//! A stand-in for the session agent's `org.cupertino.Login1` on the session bus, for captures and manual tries of the
//! system menu: the properties it reads (`Locked`, `UserName`, `Capabilities`) and the calls it takes, which it
//! only prints. Start it as a client of a capture: `capture.sh out.png --spawn target/release/examples/fake-login`.

use std::collections::HashMap;

struct FakeLogin;

#[zbus::interface(name = "org.cupertino.Login1")]
impl FakeLogin {
    #[zbus(property)]
    fn locked(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn user_name(&self) -> String {
        "Rafael".into()
    }

    #[zbus(property)]
    fn capabilities(&self) -> HashMap<String, bool> {
        HashMap::new()
    }

    fn sleep(&self) {
        eprintln!("fake-login: Sleep");
    }

    fn lock_now(&self) {
        eprintln!("fake-login: LockNow");
    }

    fn request_log_out(&self) {
        eprintln!("fake-login: RequestLogOut");
    }

    fn request_restart(&self) {
        eprintln!("fake-login: RequestRestart");
    }

    fn request_shut_down(&self) {
        eprintln!("fake-login: RequestShutDown");
    }

    fn switch_to_login_window(&self) {
        eprintln!("fake-login: SwitchToLoginWindow");
    }
}

fn main() -> zbus::Result<()> {
    let connection = zbus::block_on(
        zbus::connection::Builder::session()?
            .name("org.cupertino.Login1")?
            .serve_at("/org/cupertino/Login1", FakeLogin)?
            .build(),
    )?;
    // end with the bus, so that whatever waits for this process (a capture) is not held up
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
        let alive = zbus::block_on(connection.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "GetId",
            &(),
        ));
        if alive.is_err() {
            return Ok(());
        }
    }
}
