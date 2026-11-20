//! `org.cupertino.Osd1`: the control interface of the on-screen displays, served on the component's own session bus
//! connection, which owns the name. The server calls the step methods for the volume, brightness and keyboard-backlight
//! keys; the control panel calls `Show` after it changed a level itself.
//!
//! The step methods change levels, so they check their caller: only the server may call them (D20: the connection that
//! owns `org.cupertino.Session1`, from which the server sends them); any other caller, another connection of the
//! server's process, a sandboxed or anonymous one included, gets `org.freedesktop.DBus.Error.AccessDenied` and nothing
//! changes. `Show` only draws the overlay and
//! stays open to every caller.

use cupertino_kit_shell::bus::{self, Caller};
use zbus::fdo::{self, RequestNameFlags, RequestNameReply};
use zbus::Connection;

use crate::controller::Command;
use crate::model::{Kind, Overlay};

/// Name, path and interface of the control interface (the server calls it by these strings).
pub const BUS_NAME: &str = "org.cupertino.Osd1";
pub const OBJECT_PATH: &str = "/org/cupertino/Osd1";

struct Control {
    commands: async_channel::Sender<Command>,
}

fn invalid(message: String) -> fdo::Error {
    fdo::Error::InvalidArgs(message)
}

/// A press direction must be a sign: negative lowers, positive raises.
fn check_direction(direction: i32) -> fdo::Result<i32> {
    if direction == 0 {
        return Err(invalid("direction must be negative or positive".into()));
    }
    Ok(direction)
}

impl Control {
    /// Hands `command` to the component when the server sent it.
    async fn accept_from_server(
        &self,
        header: &zbus::message::Header<'_>,
        connection: &Connection,
        command: Command,
    ) -> fdo::Result<()> {
        let allowed = match header.sender() {
            Some(sender) => bus::caller(connection, sender).await == Caller::Server,
            None => false,
        };
        if !allowed {
            return Err(fdo::Error::AccessDenied(format!(
                "only the server may ask for a {}",
                command.name()
            )));
        }
        self.send(command);
        Ok(())
    }

    fn send(&self, command: Command) {
        // the component holds the receiver for as long as the interface is served
        let _ = self.commands.try_send(command);
    }
}

#[zbus::interface(name = "org.cupertino.Osd1")]
impl Control {
    /// Raise (`direction` > 0) or lower the volume one press; `fine` is a Shift+Alt press.
    async fn volume_step(
        &self,
        direction: i32,
        fine: bool,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> fdo::Result<()> {
        let direction = check_direction(direction)?;
        self.accept_from_server(&header, connection, Command::VolumeStep { direction, fine })
            .await
    }

    /// Mute, or unmute when muted.
    async fn toggle_mute(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> fdo::Result<()> {
        self.accept_from_server(&header, connection, Command::ToggleMute)
            .await
    }

    /// Mute the microphone, or unmute it when muted.
    async fn toggle_microphone_mute(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> fdo::Result<()> {
        self.accept_from_server(&header, connection, Command::ToggleMicrophoneMute)
            .await
    }

    /// Raise or lower the display brightness one press.
    async fn brightness_step(
        &self,
        direction: i32,
        fine: bool,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> fdo::Result<()> {
        let direction = check_direction(direction)?;
        self.accept_from_server(
            &header,
            connection,
            Command::BrightnessStep { direction, fine },
        )
        .await
    }

    /// Raise or lower the keyboard backlight one press.
    async fn keyboard_brightness_step(
        &self,
        direction: i32,
        fine: bool,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> fdo::Result<()> {
        let direction = check_direction(direction)?;
        self.accept_from_server(
            &header,
            connection,
            Command::KeyboardBrightnessStep { direction, fine },
        )
        .await
    }

    /// Show `kind` (`volume`, `mute`, `brightness`, `keyboard-brightness`, `microphone` or `microphone-mute`) at
    /// `level` (0 to 1) without changing anything; for a sender that changed the level itself.
    fn show(&self, kind: &str, level: f64) -> fdo::Result<()> {
        let kind =
            Kind::from_name(kind).ok_or_else(|| invalid(format!("unknown overlay {kind:?}")))?;
        let overlay = Overlay::new(kind, level).map_err(|e| invalid(e.to_string()))?;
        self.send(Command::Show(overlay));
        Ok(())
    }
}

/// Exports the interface at [`OBJECT_PATH`] on `connection` and claims [`BUS_NAME`] on it; every accepted call goes
/// to `commands`. Fails with `zbus::Error::NameTaken` when another overlay owns the name.
pub async fn serve(
    connection: &Connection,
    commands: async_channel::Sender<Command>,
) -> zbus::Result<()> {
    connection
        .object_server()
        .at(OBJECT_PATH, Control { commands })
        .await?;
    // after the object exists, so no call finds the name without it; never queue behind the owner
    match connection
        .request_name_with_flags(BUS_NAME, RequestNameFlags::DoNotQueue.into())
        .await?
    {
        RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => Ok(()),
        RequestNameReply::Exists | RequestNameReply::InQueue => Err(zbus::Error::NameTaken),
    }
}
