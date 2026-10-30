//! What the lock screen and the greeter share: the clock text, the frame bookkeeping, the key translation and the
//! drawing of the screen on a surface.

use cupertino_login::model::{Key, LockScreen};
use cupertino_login::picker::{Grid, NameEntry, Picker, Tile};
use cupertino_login::power::Menu;
use cupertino_login::render::{Field, PickerView, PowerView, Renderer, View};
use cupertino_login::user::{format_time, User};
use cupertino_shell_client::LayerSurface;
use cupertino_ui_wayland::{Ready, ShmPresenter};
use cupertino_wayland_client::{key, Connection, Modifiers, OutputId, SurfaceId, Waker};
use std::error::Error;
use std::sync::mpsc::Sender;

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// `strftime` patterns for the date (`EEEEMMMMd`) and the time (`Jmm`). The hour cycle of `J` follows
/// the locale; the 24-hour cycle is fixed here (approximate), as in the menu bar clock.
const DATE_FORMAT: &std::ffi::CStr = c"%A, %B %-d";
const TIME_FORMAT: &std::ffi::CStr = c"%H:%M";

/// Queues a message for the loop and wakes it; cloned into every thread.
pub struct Notifier<M> {
    sender: Sender<M>,
    waker: Waker,
}

impl<M> Clone for Notifier<M> {
    fn clone(&self) -> Self {
        Notifier {
            sender: self.sender.clone(),
            waker: self.waker.clone(),
        }
    }
}

impl<M> Notifier<M> {
    pub fn new(sender: Sender<M>, waker: Waker) -> Self {
        Notifier { sender, waker }
    }

    pub fn send(&self, message: M) {
        self.sender
            .send(message)
            .expect("the receiver lives as long as the process");
        self.waker.wake();
    }
}

/// The time the screens show: only the display clock, never what times authentication, back-off or the lock
/// ([`cupertino_shell_client::test_clock`]).
pub fn display_unix_seconds() -> Result<i64> {
    Ok(cupertino_shell_client::test_clock::display_unix_seconds()?)
}

/// What a frame shows that changes without the state machine's say: when this differs from the last frame, redraw.
#[derive(PartialEq)]
pub struct FrameKey {
    time: String,
    field: FieldKey,
    notice: Option<String>,
    caps_lock: bool,
    spinner: Option<u32>,
    help: bool,
    /// The row selected in the open power menu.
    power_open: Option<usize>,
    power_prompts: bool,
    opacity: f64,
    shake: f64,
}

/// What the field of a frame holds.
#[derive(PartialEq)]
enum FieldKey {
    Password(usize),
    Name(String),
}

/// What a frame of the password screen is made of besides the clock.
#[derive(Clone, Copy)]
pub struct Scene<'a> {
    pub screen: &'a LockScreen,
    /// Caps Lock is on and the password field has the focus.
    pub caps_lock: bool,
    /// Failed attempts have brought up the password help.
    pub help: bool,
    /// The greeter's power item, if there is one.
    pub power: Option<&'a Menu>,
}

impl FrameKey {
    /// A frame of the password screen.
    pub fn new(scene: &Scene<'_>, now: f64) -> Result<FrameKey> {
        let screen = scene.screen;
        Ok(FrameKey {
            time: format_time(display_unix_seconds()?, TIME_FORMAT)?,
            field: FieldKey::Password(screen.password_len()),
            notice: screen.notice().map(str::to_owned),
            caps_lock: scene.caps_lock,
            spinner: screen.spinner(now),
            help: scene.help,
            power_open: scene.power.and_then(Menu::open),
            power_prompts: scene.power.is_some_and(Menu::will_prompt),
            opacity: screen.opacity(now),
            shake: screen.shake(now),
        })
    }

    /// A frame of the screen where a name is typed for the "Other…" tile, or for an administrator; `notice` says which.
    pub fn name(entry: &NameEntry, notice: Option<&str>, power: Option<&Menu>) -> Result<FrameKey> {
        Ok(FrameKey {
            time: format_time(display_unix_seconds()?, TIME_FORMAT)?,
            field: FieldKey::Name(entry.text().to_owned()),
            notice: notice.map(str::to_owned),
            caps_lock: false,
            spinner: None,
            help: false,
            power_open: power.and_then(Menu::open),
            power_prompts: power.is_some_and(Menu::will_prompt),
            opacity: 1.0,
            shake: 0.0,
        })
    }
}

/// What a frame of the user picker shows that can change: the clock, the selection, the page size, the power menu and which
/// tiles carry the logged-in mark.
#[derive(PartialEq)]
pub struct PickerKey {
    time: String,
    selected: usize,
    max_per_page: usize,
    power_open: Option<usize>,
    power_prompts: bool,
    logged_in: Vec<bool>,
}

impl PickerKey {
    pub fn new(picker: &Picker, max_per_page: usize, power: Option<&Menu>) -> Result<PickerKey> {
        let logged_in = picker
            .tiles()
            .iter()
            .map(|tile| matches!(tile, Tile::Account(user) if user.logged_in))
            .collect();
        Ok(PickerKey {
            time: format_time(display_unix_seconds()?, TIME_FORMAT)?,
            selected: picker.selected_index(),
            max_per_page,
            power_open: power.and_then(Menu::open),
            power_prompts: power.is_some_and(Menu::will_prompt),
            logged_in,
        })
    }
}

/// What the renderer needs of the power item.
fn power_view(power: Option<&Menu>) -> Option<PowerView<'_>> {
    power.map(|menu| PowerView {
        items: menu.items(),
        open: menu.open(),
        will_prompt: menu.will_prompt(),
    })
}

/// The key of the screen a pressed key stands for, or `None` for a key that is only activity (a shortcut is not part of
/// a password).
pub fn typed_key(code: u32, mods: Modifiers, character: Option<char>) -> Option<Key> {
    match code {
        key::ENTER | key::KEYPAD_ENTER => Some(Key::Return),
        key::BACKSPACE => Some(Key::Backspace),
        key::ESCAPE => Some(Key::Escape),
        _ if mods.ctrl || mods.alt || mods.logo => None,
        _ => character.map(Key::Char),
    }
}

/// The full-output surface of one output: a lock surface, or a layer surface (`layer`) for the greeter.
pub struct OutputSurface {
    pub output: OutputId,
    pub layer: Option<LayerSurface>,
    pub presenter: ShmPresenter,
}

impl OutputSurface {
    pub fn surface(&self) -> SurfaceId {
        self.presenter.surface()
    }

    /// A configure of the surface with the size as sent.
    pub fn configured(&mut self, size: (u32, u32)) -> Result<()> {
        let size = match &self.layer {
            Some(layer) => layer.configured(size)?,
            None => size,
        };
        self.presenter.configured(size);
        Ok(())
    }
}

/// The surfaces of `surfaces` that can take a frame now; poll again after drawing them.
pub fn ready(conn: &Connection, surfaces: &mut [OutputSurface]) -> Result<Vec<SurfaceId>> {
    let mut ready = Vec::new();
    for s in surfaces {
        loop {
            match s.presenter.poll(conn)? {
                // a draw follows every scale change
                Ready::ScaleChanged(_) => {}
                Ready::Draw => break ready.push(s.surface()),
                Ready::Idle => break,
            }
        }
    }
    Ok(ready)
}

/// Draw the frame `key` on `presenter`, the password screen of `user` (`None`: the name screen of "Other…").
pub fn draw(
    conn: &mut Connection,
    presenter: &mut ShmPresenter,
    renderer: &mut Renderer,
    key: &FrameKey,
    user: Option<&User>,
    power: Option<&Menu>,
) -> Result<()> {
    let date = format_time(display_unix_seconds()?, DATE_FORMAT)?;
    let field = match &key.field {
        FieldKey::Password(len) => Field::Password(*len),
        FieldKey::Name(text) => Field::Name(text),
    };
    let view = View {
        time: &key.time,
        date: &date,
        user,
        field,
        notice: key.notice.as_deref(),
        caps_lock: key.caps_lock,
        spinner: key.spinner,
        help: key.help,
        power: power_view(power),
        opacity: key.opacity,
        shake: key.shake,
    };
    presenter.draw(conn, None, |pixmap, scale| {
        renderer.render(&view, pixmap, scale.factor() as f32)
    })?;
    Ok(())
}

/// Draw the frame `key` of the user picker on `presenter`.
pub fn draw_picker(
    conn: &mut Connection,
    presenter: &mut ShmPresenter,
    renderer: &mut Renderer,
    key: &PickerKey,
    picker: &Picker,
    power: Option<&Menu>,
) -> Result<()> {
    let date = format_time(display_unix_seconds()?, DATE_FORMAT)?;
    let page = picker.page(key.max_per_page);
    let view = PickerView {
        power: power_view(power),
        time: &key.time,
        date: &date,
        tiles: &picker.tiles()[page.clone()],
        grid: Grid::for_count(key.max_per_page.min(picker.tiles().len())),
        selected: key.selected - page.start,
    };
    presenter.draw(conn, None, |pixmap, scale| {
        renderer.render_picker(&view, pixmap, scale.factor() as f32)
    })?;
    Ok(())
}

/// Keep exactly one surface per output the server has described, made by `create`.
pub fn arrange_surfaces(
    conn: &mut Connection,
    surfaces: &mut Vec<OutputSurface>,
    create: impl Fn(&mut Connection, OutputId) -> Result<(SurfaceId, Option<LayerSurface>)>,
) -> Result<()> {
    let outputs: Vec<OutputId> = conn
        .outputs()
        .into_iter()
        .filter(|o| o.geometry.is_some())
        .map(|o| o.id)
        .collect();
    let mut gone = Vec::new();
    surfaces.retain(|s| {
        let keep = outputs.contains(&s.output);
        if !keep {
            gone.push(s.surface());
        }
        keep
    });
    for surface in gone {
        conn.destroy(surface)?;
    }
    for output in outputs {
        if !surfaces.iter().any(|s| s.output == output) {
            let (surface, layer) = create(conn, output)?;
            surfaces.push(OutputSurface {
                output,
                layer,
                presenter: ShmPresenter::new(surface),
            });
        }
    }
    Ok(())
}
