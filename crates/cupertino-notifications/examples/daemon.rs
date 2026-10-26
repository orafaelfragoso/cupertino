//! Minimal standalone notification daemon: serves the session bus and prints what happens.
//! No window is shown; it exists to exercise the bus API (`notify-send "Hello" "World"`).
//!
//! `cargo run -p cupertino-notifications --example notification-daemon`

use cupertino_notifications::dbus::{Server, ServerOptions};
use cupertino_notifications::Wakeup;
use std::time::Duration;

fn main() -> zbus::Result<()> {
    let server = Server::start(ServerOptions::default())?;
    eprintln!("serving org.freedesktop.Notifications");
    let mut last_revision = 0;
    loop {
        let now = server.now();
        {
            let mut c = server.center();
            c.tick(now);
            if c.revision() != last_revision {
                last_revision = c.revision();
                for n in c.history() {
                    eprintln!(
                        "[{}] {}: {} — {}",
                        n.id,
                        n.app_name,
                        n.summary,
                        n.body_text()
                    );
                }
                eprintln!(
                    "{} banner(s) visible, {} queued",
                    c.visible_count(),
                    c.queued_count()
                );
            }
        }
        server.flush_events()?;
        let sleep = match server.center().next_wakeup(now) {
            Some(Wakeup::Frame) => 16,
            Some(Wakeup::At(t)) => (((t - now).max(0.0)) * 1000.0).min(250.0) as u64 + 1,
            None => 100,
        };
        std::thread::sleep(Duration::from_millis(sleep));
    }
}
