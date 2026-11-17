//! Between the window-system interface (`cupertino-ui-platform`, `cupertino-ui-text`) and the transport's types.

use cupertino_geometry::{Point, Rect, Size};
use cupertino_keyboard::modifiers::{Profile, RawMods};
use cupertino_ui_platform as platform;
use cupertino_ui_text as text;
use cupertino_wayland_client as wl;
use std::time::Duration;

/// The oldest a frame stamp may be before it is taken for another clock; and how far behind the window system's clock
/// an input timestamp may fall, after a pause as long, before the mapping is anchored afresh.
const MAX_LAG: Duration = Duration::from_secs(1);
/// The oldest an input stamp that reads on the window system's own clock may be and still be taken at its own time:
/// longer than a client stalls and is still waited for.
const MAX_INPUT_AGE: Duration = Duration::from_secs(300);

/// Maps the compositor's input milliseconds onto the window system's clock (nanoseconds). Each event lands the
/// compositor's interval after the one before it, so intervals (a double click, scroll velocity) stay exact however
/// late the client reads them, and the milliseconds' wrap is harmless. The first timestamp anchors the mapping, and so
/// does one that would land in the future, or more than [`MAX_LAG`] in the past after an interval (forwards or back) as
/// long: those read on another clock, or one that drifted. Lateness alone does not: a client that stalls reads its
/// events late on the same clock. An anchor that reads on the window system's own monotonic clock, as a compositor's
/// stamps do, at most [`MAX_INPUT_AGE`] old, is taken at its own time, so a burst read late after a pause keeps its
/// times; any other is taken at the moment it was read.
#[derive(Debug, Default)]
pub(crate) struct InputTimeline {
    /// The last event: its milliseconds and where they landed.
    last: Option<(u32, u64)>,
}

impl InputTimeline {
    pub(crate) fn map(&mut self, millis: u32, now: u64) -> u64 {
        let max_lag = MAX_LAG.as_nanos() as u64;
        let mapped = self.last.and_then(|(last_millis, at)| {
            let gap = i64::from(millis.wrapping_sub(last_millis) as i32) * 1_000_000;
            at.checked_add_signed(gap)
                .filter(|t| *t <= now && (now - t <= max_lag || gap.unsigned_abs() < max_lag))
        });
        let t = mapped
            .or_else(|| on_own_clock(millis, now, MAX_INPUT_AGE))
            .unwrap_or(now);
        self.last = Some((millis, t));
        t
    }
}

/// `millis` read as the window system's own clock's milliseconds (wrapped at 2^32), which reads `now` (nanoseconds):
/// the start of that millisecond, when it is at most `max_age` before `now`; `None` for a stamp from the future or one
/// older, which reads on another clock.
fn on_own_clock(millis: u32, now: u64, max_age: Duration) -> Option<u64> {
    let age = i64::from((now / 1_000_000) as u32).wrapping_sub(i64::from(millis)) as i32;
    let age_nanos = u64::try_from(i64::from(age) * 1_000_000).ok()?;
    if u128::from(age_nanos) > max_age.as_nanos() {
        return None;
    }
    (now - now % 1_000_000).checked_sub(age_nanos)
}

/// A frame callback's time (`millis`, the compositor's milliseconds) on the window system's clock that reads `now`
/// (nanoseconds). The compositor stamps the callback with the frame it shows on the same monotonic clock, so the time
/// is that many milliseconds before `now`; a stamp from the future or over [`MAX_LAG`] old reads on another clock, and
/// the frame is taken as handled when it came: at `now`.
pub(crate) fn frame_time(millis: u32, now: u64) -> u64 {
    on_own_clock(millis, now, MAX_LAG).unwrap_or(now)
}

/// `time` on a clock that reads `clock_now`, on the window system's clock that reads `now`; never later than `now`.
pub(crate) fn on_window_clock(time: u64, clock_now: u64, now: u64) -> u64 {
    now.saturating_sub(clock_now.saturating_sub(time))
}

/// A size in logical points as whole points, rounded up and at least 1.
pub(crate) fn whole(size: Size) -> (u32, u32) {
    let side = |v: f64| v.ceil().clamp(1.0, f64::from(u32::MAX)) as u32;
    (side(size.width), side(size.height))
}

pub(crate) fn mods(m: wl::Modifiers, profile: &Profile) -> platform::Mods {
    let raw = [
        (m.shift, RawMods::SHIFT),
        (m.ctrl, RawMods::CTRL),
        (m.alt, RawMods::ALT),
        (m.logo, RawMods::SUPER),
    ]
    .into_iter()
    .filter(|(held, _)| *held)
    .fold(RawMods::default(), |all, (_, bit)| all | bit);
    profile.translate(raw)
}

/// Linux input codes (`linux/input-event-codes.h`) of the side buttons.
const BTN_SIDE: u32 = 0x113;
const BTN_EXTRA: u32 = 0x114;
const BTN_FORWARD: u32 = 0x115;
const BTN_BACK: u32 = 0x116;

pub(crate) fn button(b: wl::Button) -> platform::MouseButton {
    match b {
        wl::Button::Left => platform::MouseButton::Left,
        wl::Button::Right => platform::MouseButton::Right,
        wl::Button::Middle => platform::MouseButton::Middle,
        wl::Button::Other(BTN_SIDE | BTN_BACK) => platform::MouseButton::Back,
        wl::Button::Other(BTN_EXTRA | BTN_FORWARD) => platform::MouseButton::Forward,
        wl::Button::Other(code) => platform::MouseButton::Other(code),
    }
}

/// A scroll without a source predates `wl_pointer` version 5, which only wheels sent.
pub(crate) fn scroll_source(source: Option<wl::ScrollSource>) -> platform::ScrollSource {
    match source {
        Some(wl::ScrollSource::Finger) => platform::ScrollSource::Finger,
        Some(wl::ScrollSource::Continuous) => platform::ScrollSource::Continuous,
        Some(wl::ScrollSource::WheelTilt) => platform::ScrollSource::WheelTilt,
        _ => platform::ScrollSource::Wheel,
    }
}

/// `serial` is that of the latest input event, which a button press is.
pub(crate) fn pointer(
    event: wl::PointerEvent,
    serial: u32,
    mods: platform::Mods,
) -> platform::PointerEvent {
    match event {
        wl::PointerEvent::Enter(position) => platform::PointerEvent::Enter { position },
        wl::PointerEvent::Motion(position) => platform::PointerEvent::Motion { position },
        wl::PointerEvent::Leave => platform::PointerEvent::Leave,
        wl::PointerEvent::Button {
            button: b,
            pressed,
            position,
        } => platform::PointerEvent::Button {
            position,
            button: button(b),
            pressed,
            serial,
            mods,
        },
        wl::PointerEvent::Scroll {
            dx,
            dy,
            position,
            source,
            value120,
            stop,
            inverted,
        } => platform::PointerEvent::Scroll {
            position,
            delta: Point::new(dx, dy),
            value120,
            source: scroll_source(source),
            stop,
            inverted,
        },
    }
}

pub(crate) fn touch(event: wl::TouchEvent) -> platform::TouchEvent {
    match event {
        wl::TouchEvent::Down { id, position } => platform::TouchEvent::Down { id, position },
        wl::TouchEvent::Motion { id, position } => platform::TouchEvent::Motion { id, position },
        wl::TouchEvent::Up { id } => platform::TouchEvent::Up { id },
        wl::TouchEvent::Cancel => platform::TouchEvent::Cancel,
    }
}

pub(crate) fn states(s: wl::WindowStates) -> platform::WindowStates {
    platform::WindowStates {
        activated: s.activated,
        maximized: s.maximized,
        fullscreen: s.fullscreen,
        tiled: platform::TiledEdges {
            top: s.tiled_top,
            bottom: s.tiled_bottom,
            left: s.tiled_left,
            right: s.tiled_right,
        },
        resizing: s.resizing,
        suspended: s.suspended,
    }
}

/// A compositor that never announced its capabilities supports them all.
pub(crate) fn configure(c: &wl::WindowConfigure) -> platform::Configure {
    let size = |(w, h): (u32, u32)| Size::new(f64::from(w), f64::from(h));
    let caps = c.capabilities.unwrap_or(wl::WmCapabilities {
        window_menu: true,
        maximize: true,
        fullscreen: true,
        minimize: true,
    });
    platform::Configure {
        id: platform::ConfigureId(c.serial),
        size: c.size.map(size),
        states: states(c.states),
        bounds: c.bounds.map(size),
        server_decorations: c.decorations == wl::Decorations::Server,
        wm_capabilities: platform::WmCapabilities {
            window_menu: caps.window_menu,
            maximize: caps.maximize,
            fullscreen: caps.fullscreen,
            minimize: caps.minimize,
        },
    }
}

/// Where the compositor placed a popup, in its parent's coordinates.
pub(crate) fn popup_placement(c: &wl::PopupConfigure) -> Rect {
    Rect::new(
        f64::from(c.x),
        f64::from(c.y),
        f64::from(c.width),
        f64::from(c.height),
    )
}

pub(crate) fn popup_configure(c: &wl::PopupConfigure) -> platform::Configure {
    platform::Configure {
        id: platform::ConfigureId(c.serial),
        size: Some(Size::new(f64::from(c.width), f64::from(c.height))),
        states: platform::WindowStates::default(),
        bounds: None,
        server_decorations: false,
        wm_capabilities: platform::WmCapabilities {
            window_menu: false,
            maximize: false,
            fullscreen: false,
            minimize: false,
        },
    }
}

/// The `n`th configure of a surface with a role from [`crate::SurfaceRoles`], at `size`. The role's protocol has no
/// activated state, so the window system passes whether the window has the keyboard (`activated`): otherwise every
/// configure would turn a window inactive that keys still reach.
pub(crate) fn role_configure(n: u32, size: (u32, u32), activated: bool) -> platform::Configure {
    platform::Configure {
        id: platform::ConfigureId(n),
        size: Some(Size::new(f64::from(size.0), f64::from(size.1))),
        states: platform::WindowStates {
            activated,
            ..platform::WindowStates::default()
        },
        bounds: None,
        server_decorations: false,
        wm_capabilities: platform::WmCapabilities {
            window_menu: false,
            maximize: false,
            fullscreen: false,
            minimize: false,
        },
    }
}

/// The window style the desktop's effects give a window of `mode`, if it is not the default titled one.
pub(crate) fn window_style(
    mode: platform::DecorationMode,
) -> Option<cupertino_protocol_app::WindowStyle> {
    use cupertino_protocol_app::WindowStyle;
    match mode {
        platform::DecorationMode::Unified => Some(WindowStyle::Unified),
        platform::DecorationMode::UnifiedCompact => Some(WindowStyle::UnifiedCompact),
        platform::DecorationMode::Sidebar => Some(WindowStyle::Sidebar),
        _ => None,
    }
}

pub(crate) fn decorations(mode: platform::DecorationMode) -> wl::Decorations {
    match mode {
        platform::DecorationMode::Preferred
        | platform::DecorationMode::Unified
        | platform::DecorationMode::UnifiedCompact
        | platform::DecorationMode::Sidebar => wl::Decorations::Server,
        platform::DecorationMode::Client | platform::DecorationMode::None => {
            wl::Decorations::Client
        }
    }
}

/// The other application's window a window opens as a child of, for the transport.
pub(crate) fn foreign_parent(parent: &platform::ForeignParent) -> wl::ForeignParent {
    wl::ForeignParent {
        handle: parent.handle.clone(),
        modal: parent.modal,
    }
}

fn anchor(edge: platform::PopupEdge) -> wl::Anchor {
    match edge {
        platform::PopupEdge::None => wl::Anchor::None,
        platform::PopupEdge::Top => wl::Anchor::Top,
        platform::PopupEdge::Bottom => wl::Anchor::Bottom,
        platform::PopupEdge::Left => wl::Anchor::Left,
        platform::PopupEdge::Right => wl::Anchor::Right,
        platform::PopupEdge::TopLeft => wl::Anchor::TopLeft,
        platform::PopupEdge::TopRight => wl::Anchor::TopRight,
        platform::PopupEdge::BottomLeft => wl::Anchor::BottomLeft,
        platform::PopupEdge::BottomRight => wl::Anchor::BottomRight,
    }
}

fn gravity(edge: platform::PopupEdge) -> wl::Gravity {
    match edge {
        platform::PopupEdge::None => wl::Gravity::None,
        platform::PopupEdge::Top => wl::Gravity::Top,
        platform::PopupEdge::Bottom => wl::Gravity::Bottom,
        platform::PopupEdge::Left => wl::Gravity::Left,
        platform::PopupEdge::Right => wl::Gravity::Right,
        platform::PopupEdge::TopLeft => wl::Gravity::TopLeft,
        platform::PopupEdge::TopRight => wl::Gravity::TopRight,
        platform::PopupEdge::BottomLeft => wl::Gravity::BottomLeft,
        platform::PopupEdge::BottomRight => wl::Gravity::BottomRight,
    }
}

/// The offset is rounded to whole points, which is all `xdg_positioner.set_offset` takes.
pub(crate) fn placement(spec: &platform::PopupSpec) -> wl::PopupPlacement {
    let c = spec.constraint;
    let adjust = [
        (
            c.slide,
            wl::ConstraintAdjustment::SlideX | wl::ConstraintAdjustment::SlideY,
        ),
        (
            c.flip,
            wl::ConstraintAdjustment::FlipX | wl::ConstraintAdjustment::FlipY,
        ),
        (
            c.resize,
            wl::ConstraintAdjustment::ResizeX | wl::ConstraintAdjustment::ResizeY,
        ),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .fold(wl::ConstraintAdjustment::None, |all, (_, bits)| all | bits);
    let point = |v: f64| v.round().clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
    wl::PopupPlacement {
        anchor_rect: spec.anchor_rect,
        size: whole(spec.size),
        anchor: anchor(spec.anchor),
        gravity: gravity(spec.gravity),
        constraint_adjustment: adjust,
        offset: (point(spec.offset.x), point(spec.offset.y)),
        reactive: spec.reactive,
        parent_size: None,
        grab: spec.grab,
    }
}

pub(crate) fn resize_edge(edge: platform::ResizeEdge) -> wl::ResizeEdge {
    match edge {
        platform::ResizeEdge::Top => wl::ResizeEdge::Top,
        platform::ResizeEdge::Bottom => wl::ResizeEdge::Bottom,
        platform::ResizeEdge::Left => wl::ResizeEdge::Left,
        platform::ResizeEdge::Right => wl::ResizeEdge::Right,
        platform::ResizeEdge::TopLeft => wl::ResizeEdge::TopLeft,
        platform::ResizeEdge::TopRight => wl::ResizeEdge::TopRight,
        platform::ResizeEdge::BottomLeft => wl::ResizeEdge::BottomLeft,
        platform::ResizeEdge::BottomRight => wl::ResizeEdge::BottomRight,
    }
}

pub(crate) fn cursor(shape: platform::CursorShape) -> wl::CursorShape {
    match shape {
        platform::CursorShape::Default => wl::CursorShape::Default,
        platform::CursorShape::Pointer => wl::CursorShape::Pointer,
        platform::CursorShape::Text => wl::CursorShape::Text,
        platform::CursorShape::Grab => wl::CursorShape::Grab,
        platform::CursorShape::Grabbing => wl::CursorShape::Grabbing,
        platform::CursorShape::Move => wl::CursorShape::Move,
        platform::CursorShape::NotAllowed => wl::CursorShape::NotAllowed,
        platform::CursorShape::Wait => wl::CursorShape::Wait,
        platform::CursorShape::Progress => wl::CursorShape::Progress,
        platform::CursorShape::Crosshair => wl::CursorShape::Crosshair,
        platform::CursorShape::ResizeEw => wl::CursorShape::EwResize,
        platform::CursorShape::ResizeNs => wl::CursorShape::NsResize,
        platform::CursorShape::ResizeNwse => wl::CursorShape::NwseResize,
        platform::CursorShape::ResizeNesw => wl::CursorShape::NeswResize,
    }
}

fn purpose(p: text::ContentPurpose) -> wl::ContentPurpose {
    match p {
        text::ContentPurpose::Normal => wl::ContentPurpose::Normal,
        text::ContentPurpose::Alpha => wl::ContentPurpose::Alpha,
        text::ContentPurpose::Digits => wl::ContentPurpose::Digits,
        text::ContentPurpose::Number => wl::ContentPurpose::Number,
        text::ContentPurpose::Phone => wl::ContentPurpose::Phone,
        text::ContentPurpose::Url => wl::ContentPurpose::Url,
        text::ContentPurpose::Email => wl::ContentPurpose::Email,
        text::ContentPurpose::Name => wl::ContentPurpose::Name,
        text::ContentPurpose::Password => wl::ContentPurpose::Password,
        text::ContentPurpose::Pin => wl::ContentPurpose::Pin,
        text::ContentPurpose::Date => wl::ContentPurpose::Date,
        text::ContentPurpose::Time => wl::ContentPurpose::Time,
        text::ContentPurpose::DateTime => wl::ContentPurpose::Datetime,
        text::ContentPurpose::Terminal => wl::ContentPurpose::Terminal,
    }
}

fn hints(h: text::ContentHints) -> wl::ContentHint {
    [
        (h.completion, wl::ContentHint::Completion),
        (h.spellcheck, wl::ContentHint::Spellcheck),
        (h.auto_capitalization, wl::ContentHint::AutoCapitalization),
        (h.lowercase, wl::ContentHint::Lowercase),
        (h.uppercase, wl::ContentHint::Uppercase),
        (h.titlecase, wl::ContentHint::Titlecase),
        (h.hidden_text, wl::ContentHint::HiddenText),
        (h.sensitive_data, wl::ContentHint::SensitiveData),
        (h.latin, wl::ContentHint::Latin),
        (h.multiline, wl::ContentHint::Multiline),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .fold(wl::ContentHint::None, |all, (_, bit)| all | bit)
}

/// Surrounding text whose offsets do not fall on characters of its text is left out, as if the field did not know it.
pub(crate) fn text_input(state: &text::TextInputState) -> wl::TextInputState {
    wl::TextInputState {
        surrounding: state
            .surrounding
            .as_ref()
            .and_then(|s| wl::SurroundingText::new(&s.text, s.cursor, s.anchor)),
        cause: match state.cause {
            text::ChangeCause::InputMethod => wl::ChangeCause::InputMethod,
            text::ChangeCause::Other => wl::ChangeCause::Other,
        },
        hint: hints(state.hints),
        purpose: purpose(state.purpose),
        cursor_rect: state.cursor_rect,
        reset: state.reset,
    }
}

pub(crate) fn ime(event: wl::TextInputEvent) -> text::ImeEvent {
    match event {
        wl::TextInputEvent::Enter => text::ImeEvent::Enter,
        wl::TextInputEvent::Leave => text::ImeEvent::Leave,
        wl::TextInputEvent::Done(done) => text::ImeEvent::Done(text::ImeBatch {
            preedit: done.preedit.map(|p| text::Preedit {
                cursor: p.cursor.map(|(begin, end)| begin..end),
                text: p.text,
            }),
            commit: done.commit,
            delete: done.delete,
            current: done.current,
        }),
    }
}
