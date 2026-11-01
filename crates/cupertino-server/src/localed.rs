//! The system keyboard layout that `systemd-localed` keeps (`org.freedesktop.locale1`), read once at login. The
//! layouts are the input sources when `keyboard.conf` sets none, and the xkb options always apply.

use crate::kbd::SourceSpec;
use crate::state::Cupertino;

const LOCALED: &str = "org.freedesktop.locale1";
const LOCALED_PATH: &str = "/org/freedesktop/locale1";

/// The X11 keyboard settings of the system, as localed holds them: comma-separated layouts and variants, and the
/// comma-separated xkb options.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SystemLayout {
    pub layout: String,
    pub variant: String,
    pub options: String,
}

impl SystemLayout {
    /// The settings from localed on the bus at `address`, the system bus when `None`.
    pub fn read(address: Option<&str>) -> zbus::Result<Self> {
        let connection = match address {
            Some(address) => zbus::blocking::connection::Builder::address(address)?.build()?,
            None => zbus::blocking::Connection::system()?,
        };
        let localed = zbus::blocking::Proxy::new(&connection, LOCALED, LOCALED_PATH, LOCALED)?;
        Ok(Self {
            layout: localed.get_property("X11Layout")?,
            variant: localed.get_property("X11Variant")?,
            options: localed.get_property("X11Options")?,
        })
    }

    /// The layouts as input sources, in localed's order, each with the variant at its position.
    pub fn sources(&self) -> Vec<SourceSpec> {
        let mut variants = self.variant.split(',').map(str::trim);
        self.layout
            .split(',')
            .map(str::trim)
            .filter_map(|layout| {
                let variant = variants.next().unwrap_or("");
                if layout.is_empty() {
                    return None;
                }
                let id = if variant.is_empty() {
                    layout.to_string()
                } else {
                    format!("{layout}({variant})")
                };
                SourceSpec::parse(&id)
            })
            .collect()
    }
}

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
