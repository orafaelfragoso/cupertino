//! `org.freedesktop.impl.portal.Screenshot`: `Screenshot` and `PickColor`. A screenshot becomes an [`Ask`] for the
//! dialog, which answers through its [`Reply`]; nothing is captured unless the person agrees. The capture side then
//! copies every display ([`Shoot`]), and the displays are joined into one PNG under the picture directory.
//!
//! A colour pick copies every display at once and hands the copies to the picker ([`PickEvent`]), which freezes them
//! under an overlay until the person clicks a pixel or presses Escape ([`crate::pick`]); the app is answered the
//! colour of that one pixel and nothing else. The pick is the consent, so no window asks first.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cupertino_defaults::user_dirs;
use cupertino_diagnostic_events::{self as diag, Severity, Value as Field};
use tiny_skia::{BlendMode, FilterQuality, Pixmap, PixmapPaint, Transform};
use zbus::message::Header;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{interface, Connection};

use crate::consent::{identify, Identity};
use crate::file_chooser::file_uri;
use crate::pick::Picked;
use crate::request::{answered, from_frontend, Exported, PortalError, Response};

/// The interface version served.
pub const VERSION: u32 = 3;
/// The one target served, the whole screen, as `AvailableTargets` and the `target` option name it.
pub const TARGET_SCREEN: u32 = 1;
/// At most this many screenshots are asked for or taken at once; another call is refused.
pub const MAX_PENDING: usize = 16;
/// At most this many of them for one app id.
pub const MAX_PENDING_PER_APP: usize = 4;
/// Bytes of an app id and a parent window.
pub const MAX_TEXT: usize = 1024;
/// How long the consent window has to leave the screen before the displays are copied.
pub const SETTLE: Duration = Duration::from_millis(300);
/// A copy that fails for a reason that can pass is made at most this many times in all.
pub const MAX_ATTEMPTS: u32 = 3;
/// How long the displays have to settle again before a failed copy is made anew.
pub const RETRY_AFTER: Duration = Duration::from_millis(100);
/// The start of every file name, as the screenshot tool names its files.
const FILE_PREFIX: &str = "Screenshot";
/// Names tried for one second: `<name>.png`, then `<name> (2).png` up to this number.
const MAX_NUMBERED: u32 = 1000;

/// What the consent window shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    pub app_id: String,
    /// The name of the app's desktop entry, else its id; `None` when the id is empty.
    pub app_name: Option<String>,
    /// The icon name of the app's desktop entry, if it has one.
    pub app_icon: Option<String>,
    /// `wayland:<handle>`, `x11:<xid>` or empty.
    pub parent_window: String,
}

/// How the consent window ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Granted,
    Cancelled,
}

/// The consent window's answer to one [`Ask`].
pub struct Reply(pub(crate) async_channel::Sender<Answer>);

impl Reply {
    pub fn send(self, answer: Answer) {
        let _ = self.0.try_send(answer);
    }
}

/// What the dialog side is told.
pub enum Event {
    /// Ask for consent for the call at `handle`.
    Ask {
        handle: OwnedObjectPath,
        ask: Box<Ask>,
        reply: Reply,
    },
    /// The frontend closed the call at `handle`: take its window down without answering.
    Close { handle: OwnedObjectPath },
}

/// One display as the capture side copied it.
pub struct Output {
    /// Where the display sits in the compositor's space.
    pub position: (i32, i32),
    /// Its size in the compositor's space; the image may have more pixels at a scale above 1.
    pub size: (i32, i32),
    pub image: Pixmap,
}

/// Why the capture side could not copy the displays.
#[derive(Debug)]
pub enum Failure {
    /// The displays changed under the copy: one was reconfigured or went away, or its buffer constraints changed. A
    /// new copy may succeed.
    Passing(String),
    /// Copying again would fail the same way.
    Lasting(String),
}

/// What the capture side answers to a [`Shoot`].
pub struct Shot(pub(crate) async_channel::Sender<Result<Vec<Output>, Failure>>);

impl Shot {
    pub fn send(self, outputs: Result<Vec<Output>, Failure>) {
        let _ = self.0.try_send(outputs);
    }
}

/// What the capture side is asked: a copy of every display, made once `due` has passed.
pub struct Shoot {
    pub due: Instant,
    pub shot: Shot,
}

/// What the colour picker is told.
pub enum PickEvent {
    /// Let the person pick a pixel of `displays` (copied for the call at `handle`) and answer through `reply`.
    /// Dropping `reply` unanswered fails the call.
    Pick {
        handle: OwnedObjectPath,
        displays: Vec<Output>,
        reply: PickReply,
    },
    /// The frontend closed the call at `handle`: take its overlay down without answering.
    Close { handle: OwnedObjectPath },
}

/// The picker's answer to one [`PickEvent::Pick`].
pub struct PickReply(pub(crate) async_channel::Sender<Picked>);

impl PickReply {
    pub fn send(self, picked: Picked) {
        let _ = self.0.try_send(picked);
    }
}

/// The app id of each open call.
type Calls = Arc<Mutex<HashMap<OwnedObjectPath, String>>>;

fn lock(calls: &Calls) -> std::sync::MutexGuard<'_, HashMap<OwnedObjectPath, String>> {
    calls.lock().unwrap_or_else(|e| e.into_inner())
}

/// Keeps one call in [`Calls`] for as long as it lives.
struct Pending {
    calls: Calls,
    handle: OwnedObjectPath,
}

impl Pending {
    fn take(calls: &Calls, handle: &OwnedObjectPath, app_id: &str) -> Result<Pending, PortalError> {
        let mut calls_now = lock(calls);
        if calls_now.contains_key(handle) {
            return Err(invalid(format!("{handle} is already in use")));
        }
        let mine = calls_now.values().filter(|a| *a == app_id).count();
        if calls_now.len() >= MAX_PENDING || mine >= MAX_PENDING_PER_APP {
            return Err(PortalError::LimitsExceeded(
                "too many screenshots are pending".to_owned(),
            ));
        }
        calls_now.insert(handle.clone(), app_id.to_owned());
        Ok(Pending {
            calls: Arc::clone(calls),
            handle: handle.clone(),
        })
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        lock(&self.calls).remove(&self.handle);
    }
}

/// The one colour pick that runs, from the copy of the displays until it is answered.
struct OnePick<'a>(&'a AtomicBool);

impl<'a> OnePick<'a> {
    /// The pick, unless another one runs.
    fn take(running: &'a AtomicBool) -> Option<Self> {
        running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| OnePick(running))
    }
}

impl Drop for OnePick<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// The object on the bus.
pub struct Screenshot {
    applications: Vec<PathBuf>,
    pictures: Box<dyn Fn() -> PathBuf + Send + Sync>,
    events: Box<dyn Fn(Event) + Send + Sync>,
    shoots: Box<dyn Fn(Shoot) + Send + Sync>,
    picks: Option<Box<dyn Fn(PickEvent) + Send + Sync>>,
    /// A colour pick runs ([`OnePick`]).
    picking: AtomicBool,
    calls: Calls,
}

impl Screenshot {
    /// `applications` are the directories desktop entries are read from, in precedence order; `pictures` names the
    /// directory a screenshot is written to, each time one is; `events` reaches the dialog side and `shoots` the
    /// capture side.
    pub fn new(
        applications: Vec<PathBuf>,
        pictures: impl Fn() -> PathBuf + Send + Sync + 'static,
        events: impl Fn(Event) + Send + Sync + 'static,
        shoots: impl Fn(Shoot) + Send + Sync + 'static,
    ) -> Self {
        Self {
            applications,
            pictures: Box::new(pictures),
            events: Box::new(events),
            shoots: Box::new(shoots),
            picks: None,
            picking: AtomicBool::new(false),
            calls: Arc::default(),
        }
    }

    /// Serve `PickColor` through the picker `picks` reaches; without one it answers 2 and shows nothing.
    pub fn with_picker(mut self, picks: impl Fn(PickEvent) + Send + Sync + 'static) -> Self {
        self.picks = Some(Box::new(picks));
        self
    }

    /// The app as the consent window names it and draws it ([`identify`]).
    fn identity(&self, app_id: &str) -> Identity {
        identify(&self.applications, app_id)
    }

    /// Copy every display, once `due` has passed. A copy that fails for a reason that can pass is made again, up to
    /// [`MAX_ATTEMPTS`] in all. The frontend closing the request before the copy ends it.
    async fn copy(&self, request: &Exported, mut due: Instant) -> Result<Vec<Output>, String> {
        let mut attempt = 1;
        loop {
            let (shot, outputs) = async_channel::bounded(1);
            (self.shoots)(Shoot {
                due,
                shot: Shot(shot),
            });
            let outputs = futures_lite::future::or(async { Some(outputs.recv().await) }, async {
                request.closed().await;
                None
            })
            .await
            .ok_or("the request was closed")?
            .map_err(|_| "the capture side went away".to_owned())?;
            match outputs {
                Ok(outputs) => return Ok(outputs),
                Err(Failure::Passing(reason)) if attempt < MAX_ATTEMPTS => {
                    eprintln!(
                        "cupertino-portal: copying the displays failed, trying again: {reason}"
                    );
                    attempt += 1;
                    due = Instant::now() + RETRY_AFTER;
                }
                Err(Failure::Passing(reason) | Failure::Lasting(reason)) => return Err(reason),
            }
        }
    }

    /// Copy every display once the consent window has left the screen and write them as one PNG.
    async fn take(&self, request: &Exported) -> Result<PathBuf, String> {
        let outputs = self.copy(request, Instant::now() + SETTLE).await?;
        let dir = (self.pictures)();
        let (saved, path) = async_channel::bounded(1);
        // joining and encoding a large screen takes a while, and the bus is served meanwhile
        std::thread::spawn(move || {
            let _ = saved.send_blocking(write(&dir, &outputs));
        });
        path.recv()
            .await
            .map_err(|_| "the PNG was not written".to_owned())?
    }
}

type Options = HashMap<String, OwnedValue>;

/// Records `portal.screenshot.failed`: `method` (`Screenshot` or `PickColor`) of `app_id` (when known) was granted but
/// the displays could not be copied, written or picked from.
pub fn screenshot_failed(method: &'static str, app_id: Option<&str>, error: &str) {
    let mut event = diag::event(
        Severity::Error,
        "portal.screenshot.failed",
        "a screenshot or colour pick failed",
    )
    .field("method", Field::Public(method));
    if let Some(app_id) = app_id {
        event = event.field("app_id", Field::Public(app_id));
    }
    event.field("error", Field::Public(error)).emit();
}

#[interface(name = "org.freedesktop.impl.portal.Screenshot")]
impl Screenshot {
    async fn screenshot(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        handle: OwnedObjectPath,
        app_id: String,
        parent_window: String,
        options: Options,
    ) -> Result<(u32, HashMap<String, OwnedValue>), PortalError> {
        let app = app_id.clone();
        let result = async {
            from_frontend(&header, connection).await?;
            let app_id = text("app_id", app_id)?;
            let parent_window = text("parent_window", parent_window)?;
            // the consent window is a window of its own over its parent, and the same window whether interactive or not;
            // it shows even when the frontend checked the permission store, so nothing is captured without consent
            option::<bool>(&options, "modal")?;
            option::<bool>(&options, "interactive")?;
            option::<bool>(&options, "permission_store_checked")?;
            if let Some(target) = option::<u32>(&options, "target")? {
                if target != TARGET_SCREEN {
                    return Err(invalid(format!("target {target} is not served")));
                }
            }

            let _pending = Pending::take(&self.calls, &handle, &app_id)?;
            let request = Exported::at(connection, handle.as_ref()).await?;
            let (reply, answer) = async_channel::bounded(1);
            let identity = self.identity(&app_id);
            (self.events)(Event::Ask {
                handle: handle.clone(),
                ask: Box::new(Ask {
                    app_name: identity.name,
                    app_icon: identity.icon,
                    app_id,
                    parent_window,
                }),
                reply: Reply(reply),
            });
            let answer =
                futures_lite::future::or(async { Some(answer.recv().await.ok()) }, async {
                    request.closed().await;
                    None
                })
                .await;
            let answered = match answer {
                Some(Some(Answer::Granted)) => match self.take(&request).await {
                    Ok(path) => match file_uri(&path) {
                        Some(uri) => (Response::Success as u32, results(uri)?),
                        None => (Response::Other as u32, HashMap::new()),
                    },
                    Err(reason) => {
                        eprintln!("cupertino-portal: the screenshot for {handle} failed: {reason}");
                        screenshot_failed("Screenshot", Some(&app), &reason);
                        (Response::Other as u32, HashMap::new())
                    }
                },
                Some(Some(Answer::Cancelled)) => (Response::Cancelled as u32, HashMap::new()),
                Some(None) => (Response::Other as u32, HashMap::new()),
                None => {
                    (self.events)(Event::Close { handle });
                    (Response::Other as u32, HashMap::new())
                }
            };
            request.remove().await;
            Ok(answered)
        }
        .await;
        answered(
            "org.freedesktop.impl.portal.Screenshot",
            "Screenshot",
            Some(&app),
            result,
        )
    }

    /// The colour of a pixel the person clicks on any display ([`crate::pick`]); 1 when they press Escape, 2 without
    /// a picker, while another pick runs (refused before anything is copied) or when the displays cannot be copied.
    async fn pick_color(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        handle: OwnedObjectPath,
        app_id: String,
        parent_window: String,
        // the pick has no option: it is always made by the person, over every display
        _options: Options,
    ) -> Result<(u32, HashMap<String, OwnedValue>), PortalError> {
        let app = app_id.clone();
        let result = async {
            from_frontend(&header, connection).await?;
            let app_id = text("app_id", app_id)?;
            text("parent_window", parent_window)?;
            let Some(picks) = &self.picks else {
                return Ok((Response::Other as u32, HashMap::new()));
            };
            let _pending = Pending::take(&self.calls, &handle, &app_id)?;
            // one pick at a time, refused before a single display is copied for it
            let Some(_one) = OnePick::take(&self.picking) else {
                eprintln!("cupertino-portal: {handle} asked for a colour during another pick");
                return Ok((Response::Other as u32, HashMap::new()));
            };
            let request = Exported::at(connection, handle.as_ref()).await?;
            // nothing of the portal is on screen yet, so the displays are copied at once
            let answered = match self.copy(&request, Instant::now()).await {
                Ok(displays) => {
                    let (reply, picked) = async_channel::bounded(1);
                    picks(PickEvent::Pick {
                        handle: handle.clone(),
                        displays,
                        reply: PickReply(reply),
                    });
                    let picked =
                        futures_lite::future::or(async { Some(picked.recv().await.ok()) }, async {
                            request.closed().await;
                            None
                        })
                        .await;
                    match picked {
                        Some(Some(Picked::Colour(colour))) => {
                            (Response::Success as u32, colour_results(colour)?)
                        }
                        Some(Some(Picked::Cancelled)) => {
                            (Response::Cancelled as u32, HashMap::new())
                        }
                        Some(None) => (Response::Other as u32, HashMap::new()),
                        None => {
                            picks(PickEvent::Close { handle });
                            (Response::Other as u32, HashMap::new())
                        }
                    }
                }
                Err(reason) => {
                    eprintln!("cupertino-portal: the colour pick for {handle} failed: {reason}");
                    screenshot_failed("PickColor", Some(&app), &reason);
                    (Response::Other as u32, HashMap::new())
                }
            };
            request.remove().await;
            Ok(answered)
        }
        .await;
        answered(
            "org.freedesktop.impl.portal.Screenshot",
            "PickColor",
            Some(&app),
            result,
        )
    }

    #[zbus(property)]
    fn available_targets(&self) -> u32 {
        TARGET_SCREEN
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        VERSION
    }
}

fn invalid(why: impl Into<String>) -> PortalError {
    PortalError::InvalidArgs(why.into())
}

fn text(what: &str, value: String) -> Result<String, PortalError> {
    if value.len() > MAX_TEXT {
        return Err(invalid(format!("{what} is longer than {MAX_TEXT} bytes")));
    }
    Ok(value)
}

fn option<T>(options: &Options, key: &str) -> Result<Option<T>, PortalError>
where
    T: TryFrom<OwnedValue>,
{
    options
        .get(key)
        .map(|value| {
            value
                .try_clone()
                .ok()
                .and_then(|value| T::try_from(value).ok())
                .ok_or_else(|| invalid(format!("option {key} has the wrong type")))
        })
        .transpose()
}

fn results(uri: String) -> Result<HashMap<String, OwnedValue>, PortalError> {
    let uri = OwnedValue::try_from(Value::from(uri)).map_err(|e| PortalError::ZBus(e.into()))?;
    Ok(HashMap::from([("uri".to_owned(), uri)]))
}

/// `color` as `(ddd)`: red, green and blue in `[0, 1]`.
fn colour_results(colour: [f64; 3]) -> Result<HashMap<String, OwnedValue>, PortalError> {
    let colour = OwnedValue::try_from(Value::from((colour[0], colour[1], colour[2])))
        .map_err(|e| PortalError::ZBus(e.into()))?;
    Ok(HashMap::from([("color".to_owned(), colour)]))
}

/// The image in a shm buffer of `width` x `height` pixels with rows `stride` bytes apart, its pixels 32-bit
/// little-endian words (blue, green, red, alpha); without `alpha` the alpha byte is undefined and the image opaque.
/// The server's pixels are premultiplied; a colour channel above its alpha is limited to it.
pub fn from_shm(
    width: u32,
    height: u32,
    stride: usize,
    data: &[u8],
    alpha: bool,
) -> Option<Pixmap> {
    let mut pixmap = Pixmap::new(width, height)?;
    let row = width as usize * 4;
    if stride < row || data.len() < stride * (height as usize - 1) + row {
        return None;
    }
    let out = pixmap.data_mut();
    for y in 0..height as usize {
        let src = &data[y * stride..y * stride + row];
        let dst = &mut out[y * row..(y + 1) * row];
        for (s, d) in src
            .as_chunks::<4>()
            .0
            .iter()
            .zip(dst.as_chunks_mut::<4>().0.iter_mut())
        {
            let a = if alpha { s[3] } else { 255 };
            *d = [s[2].min(a), s[1].min(a), s[0].min(a), a];
        }
    }
    Some(pixmap)
}

/// The displays joined as they sit in the compositor's space, at the largest scale among them; a display at a smaller
/// scale is enlarged, and what no display covers stays transparent. `None` without a display.
pub fn compose(outputs: &[Output]) -> Option<Pixmap> {
    let shown: Vec<&Output> = outputs
        .iter()
        .filter(|o| o.size.0 > 0 && o.size.1 > 0)
        .collect();
    let scale = shown
        .iter()
        .map(|o| f64::from(o.image.width()) / f64::from(o.size.0))
        .fold(0.0, f64::max);
    let left = shown.iter().map(|o| i64::from(o.position.0)).min()?;
    let top = shown.iter().map(|o| i64::from(o.position.1)).min()?;
    let right = shown
        .iter()
        .map(|o| i64::from(o.position.0) + i64::from(o.size.0))
        .max()?;
    let bottom = shown
        .iter()
        .map(|o| i64::from(o.position.1) + i64::from(o.size.1))
        .max()?;
    let px = |v: i64| (v as f64 * scale).round();
    let mut canvas = Pixmap::new(px(right - left) as u32, px(bottom - top) as u32)?;
    let paint = PixmapPaint {
        opacity: 1.0,
        blend_mode: BlendMode::Source,
        quality: FilterQuality::Bilinear,
    };
    for o in shown {
        let (x, y) = (
            i64::from(o.position.0) - left,
            i64::from(o.position.1) - top,
        );
        let (x0, y0) = (px(x), px(y));
        let width = px(x + i64::from(o.size.0)) - x0;
        let height = px(y + i64::from(o.size.1)) - y0;
        let transform = Transform::from_row(
            (width / f64::from(o.image.width())) as f32,
            0.0,
            0.0,
            (height / f64::from(o.image.height())) as f32,
            x0 as f32,
            y0 as f32,
        );
        canvas.draw_pixmap(0, 0, o.image.as_ref(), &paint, transform, None);
    }
    Some(canvas)
}

/// A local wall-clock time, as a file name shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i32,
    /// 1 to 12.
    pub month: u32,
    pub day: u32,
    /// 0 to 23.
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl LocalTime {
    /// The local wall-clock time now (`kit::time`).
    pub fn now() -> io::Result<LocalTime> {
        let now = cupertino_kit::time::now().ok_or_else(io::Error::last_os_error)?;
        Ok(LocalTime {
            year: now.year,
            month: u32::from(now.month),
            day: u32::from(now.day),
            hour: u32::from(now.hour),
            minute: u32::from(now.minute),
            second: u32::from(now.second),
        })
    }

    /// The file name without its extension, as the screenshot tool names its files, such as
    /// `Screenshot 2026-10-25 at 3-04-05 PM`.
    pub fn base_name(&self) -> String {
        let hour12 = match self.hour % 12 {
            0 => 12,
            h => h,
        };
        let period = if self.hour < 12 { "AM" } else { "PM" };
        format!(
            "{FILE_PREFIX} {:04}-{:02}-{:02} at {hour12}-{:02}-{:02} {period}",
            self.year, self.month, self.day, self.minute, self.second
        )
    }
}

/// The directory screenshots are written to: `XDG_PICTURES_DIR` of the user directories
/// ([`cupertino_defaults::user_dirs::read`]), else `~/Pictures`; a file that cannot be read is one line on standard
/// error.
pub fn pictures() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from("/"));
    let assigned = user_dirs::read(&home).unwrap_or_else(|e| {
        eprintln!("cupertino-portal: cannot read the user directories: {e}");
        Vec::new()
    });
    assigned
        .into_iter()
        .rev()
        .find(|(name, _)| name == "PICTURES")
        .map_or_else(|| home.join("Pictures"), |(_, path)| path)
}

/// Join `outputs` and write them as a PNG named after the local time in `dir`, created when missing.
fn write(dir: &Path, outputs: &[Output]) -> Result<PathBuf, String> {
    let image = compose(outputs).ok_or("there is no display to capture")?;
    let png = image
        .encode_png()
        .map_err(|e| format!("encoding the PNG failed: {e}"))?;
    let now = LocalTime::now().map_err(|e| format!("reading the local time failed: {e}"))?;
    save(dir, &now.base_name(), &png)
        .map_err(|e| format!("writing to {} failed: {e}", dir.display()))
}

/// Create `<base>.png` in `dir`, else `<base> (2).png` and so on, holding `bytes` and readable by its owner only. A
/// file that is already there is never replaced, and a file this call could not fill is removed.
pub fn save(dir: &Path, base: &str, bytes: &[u8]) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    for n in 1..=MAX_NUMBERED {
        let path = match n {
            1 => dir.join(format!("{base}.png")),
            n => dir.join(format!("{base} ({n}).png")),
        };
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        };
        return match file.write_all(bytes) {
            Ok(()) => Ok(path),
            Err(e) => {
                drop(file);
                let _ = std::fs::remove_file(&path);
                Err(e)
            }
        };
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("{base}.png and its numbered names are all taken"),
    ))
}
