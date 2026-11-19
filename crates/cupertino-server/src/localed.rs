//! Compositor wiring for [`cupertino_server_input::localed`].

use crate::state::Cupertino;

pub use cupertino_server_input::localed::SystemLayout;

/// Read localed on a worker thread and apply its layout when it answers, so a slow or missing localed does not hold up
/// the login; until then the seat keeps the keymap of `keyboard.conf`.
pub fn start(state: &mut Cupertino) {
    use smithay::reexports::calloop::channel::{channel, Event};
    let (send, receive) = channel::<SystemLayout>();
    let inserted = state
        .loop_handle
        .insert_source(receive, |event, _, state: &mut Cupertino| {
            if let Event::Msg(system) = event {
                state.apply_system_layout(system);
            }
        });
    if let Err(e) = inserted {
        tracing::warn!(error = %e, "cannot wait for the system keyboard layout");
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("localed".into())
        .spawn(move || match SystemLayout::read(None) {
            Ok(system) => {
                let _ = send.send(system);
            }
            Err(e) => {
                tracing::info!(error = %e, "no system keyboard layout from localed: keyboard.conf and the xkb defaults apply")
            }
        });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "cannot read the system keyboard layout");
    }
}
