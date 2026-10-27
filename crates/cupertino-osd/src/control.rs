//! `org.cupertino.Osd1`: the control interface of the `cupertino-osd` process. The server calls the step methods for
//! the volume, brightness and keyboard-backlight keys; the control panel calls `Show` after it changed a level itself.
//!
//! The step methods change hardware levels, so they check their caller (`cupertino-peer`): only the server, which started
//! this process, and this process itself may call them; any other caller, a sandboxed or anonymous one included, gets an
//! error of `org.cupertino.Error.*` and nothing changes. `Show` only draws the overlay and stays open to every caller.

use cupertino_peer::{check_call, CallerError, Relation};
use zbus::fdo;
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::{connection, Connection};

use crate::controller::Command;
use crate::model::{Kind, Overlay};

/// Name, path and interface of the control interface (the server calls it by these strings).
pub const BUS_NAME: &str = "org.cupertino.Osd1";
pub const OBJECT_PATH: &str = "/org/cupertino/Osd1";

/// Who may change a level.
const SERVER: &[Relation] = &[Relation::Itself, Relation::Parent];

struct Control {
    on_command: Box<dyn Fn(Command) + Send + Sync>,
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

#[zbus::interface(name = "org.cupertino.Osd1")]
impl Control {
    /// Raise (`direction` > 0) or lower the volume one press; `fine` is a Shift+Alt press.
    async fn volume_step(
        &self,
        direction: i32,
        fine: bool,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), CallerError> {
        check_call(
            connection,
            &header,
            "org.cupertino.Osd1",
            "VolumeStep",
            SERVER,
        )
        .await?;
        (self.on_command)(Command::VolumeStep {
            direction: check_direction(direction)?,
            fine,
        });
        Ok(())
    }

    /// Mute, or unmute when muted.
    async fn toggle_mute(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), CallerError> {
        check_call(
            connection,
            &header,
            "org.cupertino.Osd1",
            "ToggleMute",
            SERVER,
        )
        .await?;
        (self.on_command)(Command::ToggleMute);
        Ok(())
    }

    /// Raise or lower the display brightness one press.
    async fn brightness_step(
        &self,
        direction: i32,
        fine: bool,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), CallerError> {
        check_call(
            connection,
            &header,
            "org.cupertino.Osd1",
            "BrightnessStep",
            SERVER,
        )
        .await?;
        (self.on_command)(Command::BrightnessStep {
            direction: check_direction(direction)?,
            fine,
        });
        Ok(())
    }

    /// Raise or lower the keyboard backlight one press.
    async fn keyboard_brightness_step(
        &self,
        direction: i32,
        fine: bool,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), CallerError> {
        check_call(
            connection,
            &header,
            "org.cupertino.Osd1",
            "KeyboardBrightnessStep",
            SERVER,
        )
        .await?;
        (self.on_command)(Command::KeyboardBrightnessStep {
            direction: check_direction(direction)?,
            fine,
        });
        Ok(())
    }

    /// Show `kind` (`volume`, `mute`, `brightness` or `keyboard-brightness`) at `level` (0 to 1) without changing
    /// anything; for a sender that changed the level itself.
    fn show(&self, kind: &str, level: f64) -> fdo::Result<()> {
        let kind =
            Kind::from_name(kind).ok_or_else(|| invalid(format!("unknown overlay {kind:?}")))?;
        let overlay = Overlay::new(kind, level).map_err(|e| invalid(e.to_string()))?;
        (self.on_command)(Command::Show(overlay));
        Ok(())
    }
}

/// Export the interface at [`OBJECT_PATH`] and claim [`BUS_NAME`] on the bus `builder` connects to. `on_command` runs
/// on the bus thread for every accepted call. The interface is served while the returned connection lives. Fails with
/// `zbus::Error::NameTaken` when another overlay owns the name.
pub fn serve(
    builder: connection::Builder<'static>,
    on_command: impl Fn(Command) + Send + Sync + 'static,
) -> zbus::Result<Connection> {
    zbus::block_on(async {
        let connection = builder
            .serve_at(
                OBJECT_PATH,
                Control {
                    on_command: Box::new(on_command),
                },
            )?
            .build()
            .await?;
        // after the object exists, so no call finds the name without it; never queue behind the owner
        match connection
            .request_name_with_flags(BUS_NAME, RequestNameFlags::DoNotQueue.into())
            .await?
        {
            RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => Ok(connection),
            RequestNameReply::Exists | RequestNameReply::InQueue => Err(zbus::Error::NameTaken),
        }
    })
}
