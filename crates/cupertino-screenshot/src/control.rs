//! `org.cupertino.Screenshot1`: what the portal asks of the screenshot tool, served by `cupertino-screenshot --serve` on
//! its own session bus connection, which owns the name. `PickColor` lets the person pick the colour of a pixel on any
//! output; `SelectSources` lets them choose what a screen share shows; `Close` takes either down unanswered.
//!
//! Only the portal may call: the program the server started as `portal` (D20, `Session1.ProgramOf` through
//! `cupertino-peer`'s [`resolve_component`]). Any other caller, another program of the server's or an application, gets
//! `org.freedesktop.DBus.Error.AccessDenied` and nothing is shown.
//!
//! The answers carry a response as the portal's requests do: [`SUCCESS`], [`CANCELLED`] or [`FAILED`].

use cupertino_peer::component::{resolve_component, ComponentCaller};
use zbus::fdo::{self, RequestNameFlags, RequestNameReply};
use zbus::Connection;

use crate::color_picker::Picked;
use crate::share_picker::{Ask, Source};

/// Name and path of the interface; the portal calls it by these strings.
pub const BUS_NAME: &str = "org.cupertino.Screenshot1";
pub const OBJECT_PATH: &str = "/org/cupertino/Screenshot1";
/// The server's name for the one program that may call (`Session1.ProgramOf`).
pub const PORTAL: &str = "portal";
/// The responses of an answer.
pub const SUCCESS: u32 = 0;
pub const CANCELLED: u32 = 1;
pub const FAILED: u32 = 2;
/// Bytes of a handle, an app name or icon, a parent window and each text of a source.
pub const MAX_TEXT: usize = 1024;
/// Sources of one `SelectSources` call.
pub const MAX_SOURCES: usize = 256;

/// How a request ended.
#[derive(Clone, Debug, PartialEq)]
pub enum Answer<T> {
    Done(T),
    Cancelled,
    /// It could not be shown or went wrong, with why.
    Failed(String),
}

/// A call the interface accepted, for the program's loop. Dropping its reply unanswered fails the call.
#[derive(Clone, Debug)]
pub enum Request {
    PickColor {
        handle: String,
        reply: async_channel::Sender<Answer<[f64; 3]>>,
    },
    SelectSources {
        handle: String,
        ask: Ask,
        reply: async_channel::Sender<Answer<(usize, bool)>>,
    },
    /// The portal's request at `handle` was closed: take what shows it down without answering.
    Close { handle: String },
}

impl From<Picked> for Answer<[f64; 3]> {
    fn from(picked: Picked) -> Self {
        match picked {
            Picked::Colour(colour) => Answer::Done(colour),
            Picked::Cancelled => Answer::Cancelled,
        }
    }
}

struct Screenshot1 {
    requests: async_channel::Sender<Request>,
}

fn text(what: &str, value: String) -> fdo::Result<String> {
    if value.len() > MAX_TEXT {
        return Err(fdo::Error::InvalidArgs(format!(
            "{what} is longer than {MAX_TEXT} bytes"
        )));
    }
    Ok(value)
}

/// The response of an answer, or of a request whose reply was dropped.
fn response<T>(answer: Option<Answer<T>>) -> (u32, Option<T>) {
    match answer {
        Some(Answer::Done(value)) => (SUCCESS, Some(value)),
        Some(Answer::Cancelled) => (CANCELLED, None),
        Some(Answer::Failed(why)) => {
            eprintln!("cupertino-screenshot: {why}");
            (FAILED, None)
        }
        None => (FAILED, None),
    }
}

impl Screenshot1 {
    /// Refuses every caller but the portal.
    async fn check(
        &self,
        header: &zbus::message::Header<'_>,
        connection: &Connection,
    ) -> fdo::Result<()> {
        let portal = match header.sender() {
            Some(sender) => {
                resolve_component(connection, sender).await
                    == ComponentCaller::Program(PORTAL.into())
            }
            None => false,
        };
        if portal {
            Ok(())
        } else {
            Err(fdo::Error::AccessDenied(
                "only the portal may ask the screenshot tool".into(),
            ))
        }
    }

    /// Hands `request` to the program and waits for its reply.
    async fn ask<T>(
        &self,
        request: impl FnOnce(async_channel::Sender<Answer<T>>) -> Request,
    ) -> Option<Answer<T>> {
        let (reply, answer) = async_channel::bounded(1);
        // the program holds the receiver for as long as the interface is served
        self.requests.try_send(request(reply)).ok()?;
        answer.recv().await.ok()
    }
}

#[zbus::interface(name = "org.cupertino.Screenshot1")]
impl Screenshot1 {
    /// The colour of a pixel the person clicks on any output, as red, green and blue in sRGB from 0 to 1; the response
    /// says whether there is one. `handle` names the call for `Close`. One pick runs at a time: another fails.
    async fn pick_color(
        &self,
        handle: String,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> fdo::Result<(u32, (f64, f64, f64))> {
        self.check(&header, connection).await?;
        let handle = text("handle", handle)?;
        let answer = self.ask(|reply| Request::PickColor { handle, reply }).await;
        let (response, colour) = response(answer);
        let [r, g, b] = colour.unwrap_or_default();
        Ok((response, (r, g, b)))
    }

    /// The source the person chooses to share among `sources`, by its index, and whether they let the app share it again
    /// without asking (only offered with `offer_remember`). Each source is `(kind, a, b)`: `("display", connector,
    /// description)` or `("window", title, app id)`. The app is named `app_name` and drawn with the icon `app_icon`
    /// (empty for none), the window opens as a child of `parent_window` (`wayland:<handle>` or empty).
    #[allow(clippy::too_many_arguments)]
    async fn select_sources(
        &self,
        handle: String,
        app_name: String,
        app_icon: String,
        parent_window: String,
        offer_remember: bool,
        sources: Vec<(String, String, String)>,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> fdo::Result<(u32, u32, bool)> {
        self.check(&header, connection).await?;
        if sources.len() > MAX_SOURCES {
            return Err(fdo::Error::InvalidArgs(format!(
                "more than {MAX_SOURCES} sources"
            )));
        }
        let sources = sources
            .into_iter()
            .map(|(kind, a, b)| {
                let (a, b) = (text("a source", a)?, text("a source", b)?);
                match kind.as_str() {
                    "display" => Ok(Source::display(&a, &b)),
                    "window" => Ok(Source::window(&a, &b)),
                    other => Err(fdo::Error::InvalidArgs(format!(
                        "{other:?} is no kind of source"
                    ))),
                }
            })
            .collect::<fdo::Result<Vec<Source>>>()?;
        let nonempty = |s: String| Some(s).filter(|s| !s.is_empty());
        let ask = Ask {
            app_name: nonempty(text("app_name", app_name)?),
            app_icon: nonempty(text("app_icon", app_icon)?),
            parent_window: text("parent_window", parent_window)?,
            sources,
            offer_remember,
        };
        let handle = text("handle", handle)?;
        let answer = self
            .ask(|reply| Request::SelectSources { handle, ask, reply })
            .await;
        let (response, chosen) = response(answer);
        let (index, remember) = chosen.unwrap_or_default();
        Ok((response, index as u32, remember))
    }

    /// Take what shows the call `handle` down; it answers nothing more.
    async fn close(
        &self,
        handle: String,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> fdo::Result<()> {
        self.check(&header, connection).await?;
        let _ = self.requests.try_send(Request::Close { handle });
        Ok(())
    }
}

/// Exports the interface at [`OBJECT_PATH`] on `connection` and claims [`BUS_NAME`] on it; every accepted call goes
/// to `requests`. Fails with `zbus::Error::NameTaken` when another program owns the name.
pub async fn serve(
    connection: &Connection,
    requests: async_channel::Sender<Request>,
) -> zbus::Result<()> {
    connection
        .object_server()
        .at(OBJECT_PATH, Screenshot1 { requests })
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
