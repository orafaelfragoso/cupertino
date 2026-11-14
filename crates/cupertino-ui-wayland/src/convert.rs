//! Between the window-system interface (`cupertino-ui-platform`, `cupertino-ui-text`) and the transport's types.

use cupertino_geometry::{Point, Rect, Size};
use cupertino_modifiers::{Profile, RawMods};
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

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_geometry::Rect;

    const MS: u64 = 1_000_000;

    #[test]
    fn input_times_keep_the_compositors_spacing_from_the_first_anchor() {
        let mut t = InputTimeline::default();
        assert_eq!(t.map(5_000, 100 * MS), 100 * MS);
        // read late in the same batch: the click interval stays the compositor's 250 ms
        assert_eq!(t.map(5_250, 400 * MS), 350 * MS);
        // the compositor's clock wraps
        let mut t = InputTimeline::default();
        t.map(u32::MAX - 5, 50 * MS);
        assert_eq!(t.map(4, 60 * MS), 60 * MS);
    }

    #[test]
    fn a_frame_takes_the_time_the_compositor_stamped_on_it() {
        // stamped 7 ms before it was handled
        assert_eq!(frame_time(12_000, 12_007 * MS + 300_000), 12_000 * MS);
        // handled in the millisecond it was stamped
        assert_eq!(frame_time(12_007, 12_007 * MS + 300_000), 12_007 * MS);
        // the compositor's milliseconds wrap at 2^32
        let now = (1u64 << 32) * MS + 5 * MS;
        assert_eq!(frame_time(u32::MAX - 4, now), now - 10 * MS);
    }

    #[test]
    fn a_frame_stamped_on_another_clock_is_taken_at_its_handling() {
        let now = 50_000 * MS + 123;
        assert_eq!(frame_time(50_010, now), now, "from the future");
        assert_eq!(frame_time(40_000, now), now, "ten seconds old");
        assert_eq!(frame_time(7, now), now, "an unrelated base");
    }

    #[test]
    fn a_presentation_time_keeps_its_distance_from_now_and_is_never_later() {
        assert_eq!(on_window_clock(9_960 * MS, 10_000 * MS, 500 * MS), 460 * MS);
        assert_eq!(
            on_window_clock(10_005 * MS, 10_000 * MS, 500 * MS),
            500 * MS
        );
        assert_eq!(on_window_clock(0, 10_000 * MS, 500 * MS), 0);
    }

    /// Milliseconds on a clock other than the window system's: days ahead of it.
    const ELSEWHERE: u32 = 1_000_000_000;

    #[test]
    fn a_time_in_the_future_or_long_past_anchors_again() {
        let mut t = InputTimeline::default();
        t.map(ELSEWHERE + 1_000, 10_000 * MS);
        assert_eq!(
            t.map(ELSEWHERE + 3_000, 11_000 * MS),
            11_000 * MS,
            "ahead of now"
        );
        assert_eq!(
            t.map(ELSEWHERE + 3_100, 11_100 * MS),
            11_100 * MS,
            "spaced from the new anchor"
        );
        assert_eq!(
            t.map(ELSEWHERE + 3_150, 13_000 * MS),
            11_150 * MS,
            "read late, close after the last: the compositor's spacing"
        );
        assert_eq!(
            t.map(ELSEWHERE + 5_200, 14_500 * MS),
            14_500 * MS,
            "more than a second behind and after a pause as long"
        );
        assert_eq!(
            t.map(ELSEWHERE + 1_000, 14_600 * MS),
            14_600 * MS,
            "going back past a second: another clock"
        );
    }

    #[test]
    fn a_burst_after_a_pause_read_late_on_the_window_systems_clock_keeps_its_times() {
        let mut t = InputTimeline::default();
        assert_eq!(t.map(100_000, 100_000 * MS + 300), 100_000 * MS);
        // two seconds later a burst comes, and the client reads it a second and a half after it came
        let burst: Vec<u64> = [102_000, 102_016, 102_032, 102_048]
            .into_iter()
            .map(|millis| t.map(millis, 103_550 * MS))
            .collect();
        assert_eq!(
            burst,
            [102_000 * MS, 102_016 * MS, 102_032 * MS, 102_048 * MS]
        );
        // the first event the client sees can be late too
        let mut t = InputTimeline::default();
        assert_eq!(t.map(50_000, 53_000 * MS), 50_000 * MS);
        assert_eq!(t.map(50_200, 53_000 * MS), 50_200 * MS);
    }

    #[test]
    fn an_old_stamp_on_the_window_systems_clock_is_taken_for_another_clock() {
        let mut t = InputTimeline::default();
        let now = 1_000_000 * MS;
        let old = 1_000_000 - MAX_INPUT_AGE.as_millis() as u32 - 1;
        assert_eq!(t.map(old, now), now);
    }

    #[test]
    fn a_double_click_read_late_after_a_stall_keeps_its_interval() {
        let mut t = InputTimeline::default();
        assert_eq!(t.map(5_000, 100 * MS), 100 * MS, "the first press, on time");
        // the client stalls; the second press, sent 200 ms later, is read 1.3 s after the first
        assert_eq!(t.map(5_200, 1_400 * MS), 300 * MS);
    }

    #[test]
    fn a_burst_read_long_after_it_came_keeps_its_spacing() {
        let mut t = InputTimeline::default();
        t.map(5_000, 100 * MS);
        let burst: Vec<u64> = [5_016, 5_032, 5_048, 5_064]
            .into_iter()
            .map(|millis| t.map(millis, 5_100 * MS))
            .collect();
        assert_eq!(burst, [116 * MS, 132 * MS, 148 * MS, 164 * MS]);
    }

    #[test]
    fn deltas_from_event_to_event_survive_the_compositors_wrap() {
        let mut t = InputTimeline::default();
        let mut millis = u32::MAX - 1_000;
        let mut now = 50 * MS;
        t.map(millis, now);
        // ten minutes of events half a second apart cross the wrap
        for _ in 0..1_200 {
            millis = millis.wrapping_add(500);
            now += 500 * MS;
            assert_eq!(t.map(millis, now), now);
        }
    }

    #[test]
    fn modifiers_follow_the_profile() {
        let held = wl::Modifiers {
            logo: true,
            shift: true,
            ..wl::Modifiers::default()
        };
        assert_eq!(
            mods(held, &Profile::cupertino()),
            platform::Mods::PRIMARY | platform::Mods::SHIFT
        );
        assert_eq!(
            mods(held, &Profile::classic_pc()),
            platform::Mods::CTRL | platform::Mods::SHIFT
        );
    }

    #[test]
    fn side_buttons_are_back_and_forward() {
        assert_eq!(
            button(wl::Button::Other(BTN_SIDE)),
            platform::MouseButton::Back
        );
        assert_eq!(
            button(wl::Button::Other(BTN_EXTRA)),
            platform::MouseButton::Forward
        );
        assert_eq!(
            button(wl::Button::Other(0x117)),
            platform::MouseButton::Other(0x117)
        );
    }

    #[test]
    fn a_scroll_keeps_its_source_detents_stop_and_physical_direction() {
        let scroll = |source| {
            pointer(
                wl::PointerEvent::Scroll {
                    dx: 0.25,
                    dy: -7.5,
                    position: Point::new(3.0, 4.0),
                    source,
                    value120: Some((0, -30)),
                    stop: false,
                    inverted: (false, true),
                },
                0,
                platform::Mods::NONE,
            )
        };
        assert_eq!(
            scroll(Some(wl::ScrollSource::WheelTilt)),
            platform::PointerEvent::Scroll {
                position: Point::new(3.0, 4.0),
                delta: Point::new(0.25, -7.5),
                value120: Some((0, -30)),
                source: platform::ScrollSource::WheelTilt,
                stop: false,
                inverted: (false, true),
            }
        );
        for (wl_source, source) in [
            (None, platform::ScrollSource::Wheel),
            (
                Some(wl::ScrollSource::Finger),
                platform::ScrollSource::Finger,
            ),
            (
                Some(wl::ScrollSource::Continuous),
                platform::ScrollSource::Continuous,
            ),
        ] {
            assert!(
                matches!(scroll(wl_source), platform::PointerEvent::Scroll { source: s, .. } if s == source)
            );
        }
    }

    #[test]
    fn a_configure_without_announced_capabilities_allows_everything() {
        let c = configure(&wl::WindowConfigure {
            serial: 7,
            size: Some((640, 480)),
            bounds: None,
            states: wl::WindowStates {
                tiled_left: true,
                activated: true,
                ..wl::WindowStates::default()
            },
            decorations: wl::Decorations::Server,
            capabilities: None,
        });
        assert_eq!(c.id, platform::ConfigureId(7));
        assert_eq!(c.size, Some(Size::new(640.0, 480.0)));
        assert!(c.server_decorations && c.states.activated && c.states.tiled.left);
        assert!(c.wm_capabilities.minimize && c.wm_capabilities.window_menu);
    }

    #[test]
    fn a_popup_configure_tells_where_the_compositor_placed_the_popup_in_its_parent() {
        let c = wl::PopupConfigure {
            serial: 3,
            x: -180,
            y: 26,
            width: 160,
            height: 120,
        };
        // flipped to the left of the parent, past its edge
        assert_eq!(popup_placement(&c), Rect::new(-180.0, 26.0, 160.0, 120.0));
        assert_eq!(popup_configure(&c).size, Some(Size::new(160.0, 120.0)));
    }

    #[test]
    fn a_popup_offset_is_sent_in_whole_points_and_constraints_cover_both_axes() {
        let p = placement(&platform::PopupSpec {
            anchor_rect: Rect::new(10.0, 20.0, 30.0, 4.0),
            anchor: platform::PopupEdge::BottomLeft,
            gravity: platform::PopupEdge::BottomRight,
            offset: Point::new(-2.4, 3.5),
            size: Size::new(100.4, 50.0),
            constraint: platform::ConstraintAdjust {
                slide: true,
                flip: true,
                resize: false,
            },
            reactive: true,
            grab: true,
        });
        assert_eq!(p.anchor_rect, Rect::new(10.0, 20.0, 30.0, 4.0));
        assert_eq!(p.offset, (-2, 4));
        assert!(p.reactive);
        assert_eq!(p.size, (101, 50));
        assert_eq!(
            (p.anchor, p.gravity),
            (wl::Anchor::BottomLeft, wl::Gravity::BottomRight)
        );
        assert_eq!(
            p.constraint_adjustment,
            wl::ConstraintAdjustment::SlideX
                | wl::ConstraintAdjustment::SlideY
                | wl::ConstraintAdjustment::FlipX
                | wl::ConstraintAdjustment::FlipY
        );
        assert!(p.grab);
    }

    #[test]
    fn the_text_input_state_maps_hints_and_drops_misplaced_surrounding_text() {
        let state = text::TextInputState {
            surrounding: Some(text::Surrounding {
                text: "héllo".into(),
                cursor: 3,
                anchor: 1,
            }),
            purpose: text::ContentPurpose::Email,
            hints: text::ContentHints {
                spellcheck: true,
                hidden_text: true,
                ..text::ContentHints::default()
            },
            cause: text::ChangeCause::InputMethod,
            cursor_rect: Some(Rect::new(1.0, 2.0, 1.0, 14.0)),
            reset: false,
        };
        let sent = text_input(&state);
        assert_eq!(sent.surrounding, wl::SurroundingText::new("héllo", 3, 1));
        assert_eq!(sent.purpose, wl::ContentPurpose::Email);
        assert_eq!(
            sent.hint,
            wl::ContentHint::Spellcheck | wl::ContentHint::HiddenText
        );
        assert_eq!(sent.cause, wl::ChangeCause::InputMethod);
        let inside_a_character = text::TextInputState {
            surrounding: Some(text::Surrounding {
                text: "héllo".into(),
                cursor: 2,
                anchor: 2,
            }),
            ..state
        };
        assert_eq!(text_input(&inside_a_character).surrounding, None);
    }

    #[test]
    fn an_input_method_batch_keeps_its_order_and_staleness() {
        let event = ime(wl::TextInputEvent::Done(wl::TextInputDone {
            preedit: Some(wl::Preedit {
                text: "añ".into(),
                cursor: Some((1, 3)),
            }),
            commit: Some("x".into()),
            delete: (1, 2),
            current: false,
        }));
        let text::ImeEvent::Done(batch) = event else {
            panic!("a batch");
        };
        assert_eq!(
            batch.preedit.as_ref().and_then(|p| p.cursor.clone()),
            Some(1..3)
        );
        assert_eq!(
            (batch.commit.as_deref(), batch.delete, batch.current),
            (Some("x"), (1, 2), false)
        );
    }
}
