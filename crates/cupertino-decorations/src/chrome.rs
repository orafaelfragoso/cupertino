//! Window chrome as a layout system: the outer frame, the drawable content and the unobscured layout rectangle.
//!
//! A window style is a set of independent choices ([`ChromeStyle`]): the frame kind (titled, utility panel,
//! borderless), the toolbar style, whether the title shows, whether the content runs under the title bar and toolbar
//! (full-size content) and whether the title bar is transparent. [`ChromeLayout::new`] turns a style and a content size
//! into three rectangles:
//!
//! * `frame`: the outer window, in frame-local points (y down);
//! * `content`: what the client draws; with full-size content it is the whole frame;
//! * `unobscured`: the part of `content` no chrome covers, where layout starts. Content may scroll behind the chrome
//!   above it, but controls that must stay reachable lie inside this rectangle.
//!
//! The chrome allocation (`frame.height − unobscured.height`) depends on the toolbar style only, never on whether the
//! title shows or the title bar is transparent: those change what is drawn, not what is reserved.
//!
//! The same model places the window control buttons ([`ButtonPlacement`]), follows the layout direction (a
//! right-to-left window mirrors the group to the right edge, close outermost), decides which buttons exist and which are
//! enabled from the window's capabilities ([`button_presence`]), and lays out titlebar accessories, which may keep a
//! minimum height visible in full screen.

use crate::metrics::{ButtonKind, TitlebarStyle};
use cupertino_geometry::{Rect, Size};

/// Height of a titled window's title bar, and of the title row above an expanded or settings-style toolbar.
pub const TITLE_ROW_HEIGHT: f64 = 32.0;
/// Height of a utility panel's title bar.
pub const UTILITY_TITLE_HEIGHT: f64 = 24.0;
/// Height of the unified toolbar row: the title bar and the toolbar in one row (also the automatic toolbar style).
pub const UNIFIED_ROW_HEIGHT: f64 = 52.0;
/// Height of the compact unified toolbar row.
pub const COMPACT_ROW_HEIGHT: f64 = 40.0;
/// Height of an expanded toolbar's own row, below the title row.
pub const EXPANDED_TOOLBAR_ROW: f64 = 36.0;
/// Height of a settings-style toolbar's own row (large symbols over labels), below the title row.
pub const SETTINGS_TOOLBAR_ROW: f64 = 56.0;

/// Side of a window control button of a regular window.
pub const BUTTON_SIZE: f64 = 14.0;
/// Distance between the left edges of neighbouring buttons of a regular window.
pub const BUTTON_PITCH: f64 = 23.0;
/// Side of a utility panel's window control button.
pub const PANEL_BUTTON_SIZE: f64 = 10.0;
/// Distance between the left edges of neighbouring buttons of a utility panel.
pub const PANEL_BUTTON_PITCH: f64 = 17.0;

/// The first button's top-left corner, from the window's leading and top edges, per row kind.
const TITLED_BUTTON_ORIGIN: (f64, f64) = (9.0, 9.0);
const PANEL_BUTTON_ORIGIN: (f64, f64) = (8.0, 8.0);
const UNIFIED_BUTTON_ORIGIN: (f64, f64) = (19.0, 19.0);
const COMPACT_BUTTON_ORIGIN: (f64, f64) = (12.0, 13.0);

/// Room between the button group and what follows it in its row (the title or the toolbar items).
pub const GROUP_GAP: f64 = 8.0;
/// Room kept free at the trailing edge of a toolbar row.
pub const ROW_TRAILING: f64 = 8.0;

/// What kind of frame the window has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FrameKind {
    /// A titled window: document windows, dialogs, settings.
    Titled,
    /// A utility panel: the shorter title bar with small buttons.
    UtilityPanel,
    /// No title bar and no buttons.
    Borderless,
}

/// The toolbar of a titled window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolbarStyle {
    /// No toolbar: the title bar alone.
    None,
    /// The title bar and the toolbar share one row, the title after the buttons (also the automatic style).
    Unified,
    /// [`ToolbarStyle::Unified`] in the compact row.
    UnifiedCompact,
    /// The title in its own row above the toolbar.
    Expanded,
    /// The title above a toolbar of large symbols over labels, as a settings window has.
    Settings,
}

/// Where the title goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitleArrangement {
    /// Hidden: the toolbar items take its room.
    Hidden,
    /// Centred in the title bar (no toolbar) or in the title row above the toolbar.
    Above,
    /// After the button group, in the toolbar row it shares with the items.
    SharesRow,
}

/// Which way the window's titlebar lays out: a right-to-left window has its buttons and leading items at the right.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Direction {
    #[default]
    LeftToRight,
    RightToLeft,
}

impl Direction {
    /// The direction of a locale name (`ar_EG.UTF-8`, `he`, `fa-IR`), or of the first of a colon-separated list of them
    /// (`ar:en`): right to left for the languages written that way.
    pub fn of_locale(locale: &str) -> Self {
        const RTL: [&str; 9] = ["ar", "he", "iw", "fa", "ur", "ps", "sd", "yi", "dv"];
        let language = locale
            .split([':', '_', '-', '.', '@'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if RTL.contains(&language.as_str()) {
            Direction::RightToLeft
        } else {
            Direction::LeftToRight
        }
    }
}

/// A window style: independent choices about the chrome.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ChromeStyle {
    pub frame: FrameKind,
    pub toolbar: ToolbarStyle,
    pub title_visible: bool,
    /// The content runs under the title bar and the toolbar: it fills the frame, and the chrome covers its top.
    pub full_size_content: bool,
    /// The title bar draws no background; this changes nothing in the layout.
    pub transparent_titlebar: bool,
}

impl ChromeStyle {
    /// A titled window without a toolbar.
    pub const TITLED: ChromeStyle = ChromeStyle {
        frame: FrameKind::Titled,
        toolbar: ToolbarStyle::None,
        title_visible: true,
        full_size_content: false,
        transparent_titlebar: false,
    };

    /// A titled window with `toolbar`.
    pub const fn with_toolbar(toolbar: ToolbarStyle) -> Self {
        ChromeStyle {
            toolbar,
            ..Self::TITLED
        }
    }

    /// A utility panel.
    pub const UTILITY_PANEL: ChromeStyle = ChromeStyle {
        frame: FrameKind::UtilityPanel,
        ..Self::TITLED
    };

    /// No chrome at all.
    pub const BORDERLESS: ChromeStyle = ChromeStyle {
        frame: FrameKind::Borderless,
        ..Self::TITLED
    };

    /// The chrome of a window the server decorates in `style`: titled draws the title bar above the client's surface;
    /// the unified styles are full-size content under a toolbar row the client draws, with the title; the sidebar style
    /// is the same with the title hidden (the client's sidebar has the row's leading end, its toolbar the rest).
    pub fn of_titlebar(style: TitlebarStyle) -> Self {
        match style {
            TitlebarStyle::Standard => Self::TITLED,
            TitlebarStyle::Unified => ChromeStyle {
                full_size_content: true,
                transparent_titlebar: true,
                ..Self::with_toolbar(ToolbarStyle::Unified)
            },
            TitlebarStyle::UnifiedCompact => ChromeStyle {
                full_size_content: true,
                transparent_titlebar: true,
                ..Self::with_toolbar(ToolbarStyle::UnifiedCompact)
            },
            TitlebarStyle::Sidebar => ChromeStyle {
                full_size_content: true,
                transparent_titlebar: true,
                title_visible: false,
                ..Self::with_toolbar(ToolbarStyle::Unified)
            },
        }
    }

    /// The height the chrome takes above the unobscured content: the title bar and the toolbar. A hidden title and a
    /// transparent title bar take the same room as shown ones.
    pub fn allocation(&self) -> f64 {
        match (self.frame, self.toolbar) {
            (FrameKind::Borderless, _) => 0.0,
            (FrameKind::UtilityPanel, _) => UTILITY_TITLE_HEIGHT,
            (FrameKind::Titled, ToolbarStyle::None) => TITLE_ROW_HEIGHT,
            (FrameKind::Titled, ToolbarStyle::Unified) => UNIFIED_ROW_HEIGHT,
            (FrameKind::Titled, ToolbarStyle::UnifiedCompact) => COMPACT_ROW_HEIGHT,
            (FrameKind::Titled, ToolbarStyle::Expanded) => TITLE_ROW_HEIGHT + EXPANDED_TOOLBAR_ROW,
            (FrameKind::Titled, ToolbarStyle::Settings) => TITLE_ROW_HEIGHT + SETTINGS_TOOLBAR_ROW,
        }
    }

    /// The row the buttons lie in, from the top: the title bar, the unified row, or the title row above the toolbar.
    pub fn button_row(&self) -> f64 {
        match (self.frame, self.toolbar) {
            (FrameKind::Borderless, _) => 0.0,
            (FrameKind::UtilityPanel, _) => UTILITY_TITLE_HEIGHT,
            (FrameKind::Titled, ToolbarStyle::Unified) => UNIFIED_ROW_HEIGHT,
            (FrameKind::Titled, ToolbarStyle::UnifiedCompact) => COMPACT_ROW_HEIGHT,
            (FrameKind::Titled, _) => TITLE_ROW_HEIGHT,
        }
    }

    /// Where the title goes.
    pub fn title(&self) -> TitleArrangement {
        if !self.title_visible || self.frame == FrameKind::Borderless {
            return TitleArrangement::Hidden;
        }
        match self.toolbar {
            ToolbarStyle::Unified | ToolbarStyle::UnifiedCompact
                if self.frame == FrameKind::Titled =>
            {
                TitleArrangement::SharesRow
            }
            _ => TitleArrangement::Above,
        }
    }

    /// The buttons' side, the distance between their left edges, and the first one's top-left corner from the leading
    /// and top edges. `None` for a borderless window.
    pub fn button_metrics(&self) -> Option<(f64, f64, (f64, f64))> {
        match (self.frame, self.toolbar) {
            (FrameKind::Borderless, _) => None,
            (FrameKind::UtilityPanel, _) => {
                Some((PANEL_BUTTON_SIZE, PANEL_BUTTON_PITCH, PANEL_BUTTON_ORIGIN))
            }
            (FrameKind::Titled, ToolbarStyle::Unified) => {
                Some((BUTTON_SIZE, BUTTON_PITCH, UNIFIED_BUTTON_ORIGIN))
            }
            (FrameKind::Titled, ToolbarStyle::UnifiedCompact) => {
                Some((BUTTON_SIZE, BUTTON_PITCH, COMPACT_BUTTON_ORIGIN))
            }
            (FrameKind::Titled, _) => Some((BUTTON_SIZE, BUTTON_PITCH, TITLED_BUTTON_ORIGIN)),
        }
    }
}

/// What a window can do, which decides its buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Capabilities {
    pub closable: bool,
    pub minimizable: bool,
    pub resizable: bool,
}

impl Capabilities {
    /// A standard resizable window.
    pub const ALL: Capabilities = Capabilities {
        closable: true,
        minimizable: true,
        resizable: true,
    };
    /// A utility panel: it closes, and does neither of the others.
    pub const PANEL: Capabilities = Capabilities {
        closable: true,
        minimizable: false,
        resizable: false,
    };
}

/// Whether a button exists and takes clicks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Presence {
    Absent,
    /// Drawn dimmed in its place, and a click on it does nothing.
    Disabled,
    Enabled,
}

impl Presence {
    pub fn enabled(self) -> bool {
        self == Presence::Enabled
    }
    pub fn present(self) -> bool {
        self != Presence::Absent
    }
}

/// The buttons of a window of `frame` with `caps`, close, minimize and zoom in order. Presence and capability are
/// separate decisions: a borderless window, and a titled one that can do none of the three, has no buttons; any other
/// keeps all three in place and disables those it cannot do (zoom follows resizability alone; whether the window may go
/// full screen does not disable it).
pub fn button_presence(frame: FrameKind, caps: Capabilities) -> [Presence; 3] {
    let none = !caps.closable && !caps.minimizable && !caps.resizable;
    if frame == FrameKind::Borderless || none {
        return [Presence::Absent; 3];
    }
    let p = |can: bool| {
        if can {
            Presence::Enabled
        } else {
            Presence::Disabled
        }
    };
    [p(caps.closable), p(caps.minimizable), p(caps.resizable)]
}

/// One window control button: its bounds in frame-local points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ButtonPlacement {
    pub kind: ButtonKind,
    pub bounds: Rect,
    pub presence: Presence,
}

/// Where an accessory sits in the chrome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessoryEdge {
    /// Below the toolbar, across the window: it adds to the chrome's height.
    Bottom,
    /// In the toolbar row at its leading end, after the buttons.
    Leading,
    /// In the toolbar row at its trailing end.
    Trailing,
}

/// A titlebar accessory: a view the window adds to its chrome.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Accessory {
    pub edge: AccessoryEdge,
    /// Height of a bottom accessory, width of a leading or trailing one.
    pub extent: f64,
    /// How much of a bottom accessory stays visible in full screen while the toolbar hides; 0 hides it with the toolbar.
    pub full_screen_min_height: f64,
}

/// The chrome of one window at one size.
#[derive(Clone, Debug, PartialEq)]
pub struct ChromeLayout {
    pub style: ChromeStyle,
    pub direction: Direction,
    pub frame: Rect,
    pub content: Rect,
    pub unobscured: Rect,
    /// The row the buttons lie in, across the window.
    pub button_row: Rect,
    /// The toolbar row, across the window (`None` without a toolbar).
    pub toolbar: Option<Rect>,
    /// Where the title goes (`None` when hidden).
    pub title: Option<Rect>,
    /// The room the toolbar items may take: in the shared row after the group (and the title), else the toolbar row.
    pub items: Option<Rect>,
    /// Close, minimize and zoom; empty when the window has none.
    pub buttons: Vec<ButtonPlacement>,
    /// The accessories, in the order given, each with its rectangle.
    pub accessories: Vec<(Accessory, Rect)>,
}

/// Width a shared-row title takes before the items; the rest of the row goes to the items.
pub const SHARED_TITLE_WIDTH: f64 = 160.0;

impl ChromeLayout {
    /// The chrome around `content` (the size the client draws, or the size of the unobscured content when the content
    /// is not full-size). Accessories at the bottom add to the obscured height.
    pub fn new(
        content: Size,
        style: ChromeStyle,
        caps: Capabilities,
        direction: Direction,
        accessories: &[Accessory],
    ) -> Self {
        let bottom: f64 = accessories
            .iter()
            .filter(|a| a.edge == AccessoryEdge::Bottom)
            .map(|a| a.extent)
            .sum();
        let allocation = style.allocation() + bottom;
        Self::build(content, style, caps, direction, accessories, allocation)
    }

    /// The chrome of a full-screen window: the title bar and toolbar hide, and only the bottom accessories' minimum
    /// full-screen heights stay reserved at the top.
    pub fn full_screen(
        size: Size,
        style: ChromeStyle,
        direction: Direction,
        accessories: &[Accessory],
    ) -> Self {
        let kept: f64 = accessories
            .iter()
            .filter(|a| a.edge == AccessoryEdge::Bottom)
            .map(|a| a.full_screen_min_height.min(a.extent))
            .sum();
        let frame = Rect::new(0.0, 0.0, size.width, size.height);
        let mut y = 0.0;
        let accessories = accessories
            .iter()
            .filter(|a| a.edge == AccessoryEdge::Bottom && a.full_screen_min_height > 0.0)
            .map(|a| {
                let h = a.full_screen_min_height.min(a.extent);
                let r = Rect::new(0.0, y, size.width, h);
                y += h;
                (*a, r)
            })
            .collect();
        ChromeLayout {
            style,
            direction,
            frame,
            content: frame,
            unobscured: Rect::new(0.0, kept, size.width, (size.height - kept).max(0.0)),
            button_row: Rect::new(0.0, 0.0, size.width, 0.0),
            toolbar: None,
            title: None,
            items: None,
            buttons: Vec::new(),
            accessories,
        }
    }

    fn build(
        content: Size,
        style: ChromeStyle,
        caps: Capabilities,
        direction: Direction,
        accessories: &[Accessory],
        allocation: f64,
    ) -> Self {
        let w = content.width;
        let (frame, content_rect, unobscured) = if style.full_size_content {
            let frame = Rect::new(0.0, 0.0, w, content.height);
            let top = allocation.min(content.height);
            (frame, frame, Rect::new(0.0, top, w, content.height - top))
        } else {
            let frame = Rect::new(0.0, 0.0, w, allocation + content.height);
            let c = Rect::new(0.0, allocation, w, content.height);
            (frame, c, c)
        };
        // mirror a rectangle laid out from the leading edge
        let place = |x: f64, y: f64, width: f64, height: f64| match direction {
            Direction::LeftToRight => Rect::new(x, y, width, height),
            Direction::RightToLeft => Rect::new(w - x - width, y, width, height),
        };
        let row = style.button_row();
        let button_row = Rect::new(0.0, 0.0, w, row);
        let presence = button_presence(style.frame, caps);
        let buttons: Vec<ButtonPlacement> = match style.button_metrics() {
            Some((side, pitch, (left, top))) if presence[0].present() => ButtonKind::ALL
                .iter()
                .map(|&kind| ButtonPlacement {
                    kind,
                    bounds: place(left + kind.index() as f64 * pitch, top, side, side),
                    presence: presence[kind.index()],
                })
                .collect(),
            _ => Vec::new(),
        };
        // the leading end of the button row the group takes, gap included
        let group_end = style
            .button_metrics()
            .filter(|_| !buttons.is_empty())
            .map_or(0.0, |(side, pitch, (left, _))| {
                left + 2.0 * pitch + side + GROUP_GAP
            });
        let toolbar = match style.toolbar {
            _ if style.frame != FrameKind::Titled => None,
            ToolbarStyle::None => None,
            ToolbarStyle::Unified | ToolbarStyle::UnifiedCompact => Some(button_row),
            ToolbarStyle::Expanded | ToolbarStyle::Settings => Some(Rect::new(
                0.0,
                TITLE_ROW_HEIGHT,
                w,
                style.allocation() - TITLE_ROW_HEIGHT,
            )),
        };
        // leading and trailing accessories sit in the toolbar row, or the title bar without one
        let accessory_row = toolbar.unwrap_or(button_row);
        let mut lead = match style.toolbar {
            ToolbarStyle::Unified | ToolbarStyle::UnifiedCompact => group_end,
            _ if toolbar.is_some() => ROW_TRAILING,
            _ => group_end,
        };
        let mut trail = ROW_TRAILING;
        let mut bottom_y = style.allocation();
        let placed = accessories
            .iter()
            .map(|a| {
                let r = match a.edge {
                    AccessoryEdge::Leading => {
                        let r = place(
                            lead,
                            accessory_row.min_y(),
                            a.extent,
                            accessory_row.height(),
                        );
                        lead += a.extent + GROUP_GAP;
                        r
                    }
                    AccessoryEdge::Trailing => {
                        trail += a.extent;
                        let r = place(
                            w - trail,
                            accessory_row.min_y(),
                            a.extent,
                            accessory_row.height(),
                        );
                        trail += GROUP_GAP;
                        r
                    }
                    AccessoryEdge::Bottom => {
                        let r = Rect::new(0.0, bottom_y, w, a.extent);
                        bottom_y += a.extent;
                        r
                    }
                };
                (*a, r)
            })
            .collect();
        let (title, items) = match style.title() {
            TitleArrangement::Hidden => (None, toolbar.map(|t| span(t, lead, w - trail, place))),
            TitleArrangement::Above => {
                // centred in the title bar or the title row, clear of the group on both sides
                let reserve = group_end.max(GROUP_GAP);
                let title = (w > 2.0 * reserve)
                    .then(|| Rect::new(reserve, 0.0, w - 2.0 * reserve, row.min(TITLE_ROW_HEIGHT)));
                (title, toolbar.map(|t| span(t, lead, w - trail, place)))
            }
            TitleArrangement::SharesRow => {
                let t = toolbar.unwrap_or(button_row);
                let title_w = SHARED_TITLE_WIDTH.min((w - trail - lead).max(0.0));
                let title = place(lead, t.min_y(), title_w, t.height());
                (
                    Some(title),
                    Some(span(t, lead + title_w + GROUP_GAP, w - trail, place)),
                )
            }
        };
        ChromeLayout {
            style,
            direction,
            frame,
            content: content_rect,
            unobscured,
            button_row,
            toolbar,
            title,
            items,
            buttons,
            accessories: placed,
        }
    }

    /// The chrome's height above the unobscured content.
    pub fn allocation(&self) -> f64 {
        self.frame.height() - self.unobscured.height()
    }

    /// The button of `kind`, if the window has buttons.
    pub fn button(&self, kind: ButtonKind) -> Option<&ButtonPlacement> {
        self.buttons.iter().find(|b| b.kind == kind)
    }
}

/// The part of row `t` from `start` to `end` measured from the leading edge, mirrored for right to left.
fn span(t: Rect, start: f64, end: f64, place: impl Fn(f64, f64, f64, f64) -> Rect) -> Rect {
    place(start, t.min_y(), (end - start).max(0.0), t.height())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONTENT: Size = Size {
        width: 800.0,
        height: 600.0,
    };

    fn layout(style: ChromeStyle) -> ChromeLayout {
        ChromeLayout::new(
            CONTENT,
            style,
            Capabilities::ALL,
            Direction::LeftToRight,
            &[],
        )
    }

    fn full_size(style: ChromeStyle) -> ChromeStyle {
        ChromeStyle {
            full_size_content: true,
            ..style
        }
    }

    /// (outer height, content height, unobscured height, first button left, top, side) for a 600 pt content.
    fn measure(l: &ChromeLayout) -> (f64, f64, f64, Option<(f64, f64, f64)>) {
        (
            l.frame.height(),
            l.content.height(),
            l.unobscured.height(),
            l.buttons
                .first()
                .map(|b| (b.bounds.min_x(), b.bounds.min_y(), b.bounds.width())),
        )
    }

    #[test]
    fn each_style_reserves_its_measured_chrome_and_places_its_buttons() {
        let hidden = ChromeStyle {
            title_visible: false,
            ..ChromeStyle::TITLED
        };
        let transparent = ChromeStyle {
            transparent_titlebar: true,
            ..full_size(ChromeStyle::TITLED)
        };
        let hidden_unified_full = ChromeStyle {
            title_visible: false,
            ..full_size(ChromeStyle::with_toolbar(ToolbarStyle::Unified))
        };
        let cases = [
            (
                "titled",
                ChromeStyle::TITLED,
                (632.0, 600.0, 600.0),
                Some((9.0, 9.0, 14.0)),
            ),
            (
                "borderless",
                ChromeStyle::BORDERLESS,
                (600.0, 600.0, 600.0),
                None,
            ),
            (
                "utility panel",
                ChromeStyle::UTILITY_PANEL,
                (624.0, 600.0, 600.0),
                Some((8.0, 8.0, 10.0)),
            ),
            (
                "full-size content",
                full_size(ChromeStyle::TITLED),
                (600.0, 600.0, 568.0),
                Some((9.0, 9.0, 14.0)),
            ),
            (
                "transparent + full-size",
                transparent,
                (600.0, 600.0, 568.0),
                Some((9.0, 9.0, 14.0)),
            ),
            (
                "hidden title, no toolbar",
                hidden,
                (632.0, 600.0, 600.0),
                Some((9.0, 9.0, 14.0)),
            ),
            (
                "unified",
                ChromeStyle::with_toolbar(ToolbarStyle::Unified),
                (652.0, 600.0, 600.0),
                Some((19.0, 19.0, 14.0)),
            ),
            (
                "expanded",
                ChromeStyle::with_toolbar(ToolbarStyle::Expanded),
                (668.0, 600.0, 600.0),
                Some((9.0, 9.0, 14.0)),
            ),
            (
                "settings",
                ChromeStyle::with_toolbar(ToolbarStyle::Settings),
                (688.0, 600.0, 600.0),
                Some((9.0, 9.0, 14.0)),
            ),
            (
                "compact",
                ChromeStyle::with_toolbar(ToolbarStyle::UnifiedCompact),
                (640.0, 600.0, 600.0),
                Some((12.0, 13.0, 14.0)),
            ),
            (
                "hidden title + unified + full-size",
                hidden_unified_full,
                (600.0, 600.0, 548.0),
                Some((19.0, 19.0, 14.0)),
            ),
        ];
        for (name, style, (outer, content, unobscured), button) in cases {
            let l = layout(style);
            assert_eq!(measure(&l), (outer, content, unobscured, button), "{name}");
        }
    }

    #[test]
    fn the_allocation_per_style_is_outer_minus_unobscured() {
        let expect = [
            (ChromeStyle::TITLED, 32.0),
            (ChromeStyle::UTILITY_PANEL, 24.0),
            (ChromeStyle::with_toolbar(ToolbarStyle::Unified), 52.0),
            (
                ChromeStyle::with_toolbar(ToolbarStyle::UnifiedCompact),
                40.0,
            ),
            (ChromeStyle::with_toolbar(ToolbarStyle::Expanded), 68.0),
            (ChromeStyle::with_toolbar(ToolbarStyle::Settings), 88.0),
            (ChromeStyle::BORDERLESS, 0.0),
        ];
        for (style, a) in expect {
            assert_eq!(style.allocation(), a, "{style:?}");
            assert_eq!(layout(style).allocation(), a, "{style:?}");
            assert_eq!(
                layout(full_size(style)).allocation(),
                a,
                "full-size {style:?}"
            );
        }
    }

    #[test]
    fn hiding_the_title_or_clearing_the_bar_never_changes_the_reserved_room() {
        for toolbar in [
            ToolbarStyle::None,
            ToolbarStyle::Unified,
            ToolbarStyle::UnifiedCompact,
            ToolbarStyle::Expanded,
            ToolbarStyle::Settings,
        ] {
            let base = ChromeStyle::with_toolbar(toolbar);
            for variant in [
                ChromeStyle {
                    title_visible: false,
                    ..base
                },
                ChromeStyle {
                    transparent_titlebar: true,
                    ..base
                },
            ] {
                let (a, b) = (layout(base), layout(variant));
                assert_eq!(
                    (a.frame, a.unobscured),
                    (b.frame, b.unobscured),
                    "{variant:?}"
                );
                assert_eq!(a.buttons, b.buttons, "{variant:?}");
            }
        }
    }

    #[test]
    fn full_size_content_draws_under_the_chrome_and_lays_out_below_it() {
        let l = layout(full_size(ChromeStyle::with_toolbar(ToolbarStyle::Unified)));
        assert_eq!(l.content, l.frame, "the content fills the frame");
        assert_eq!(l.unobscured, Rect::new(0.0, 52.0, 800.0, 548.0));
        let bar = l.toolbar.unwrap();
        assert_eq!(
            l.content.intersection(&bar),
            Some(bar),
            "the toolbar lies over the content"
        );
        assert!(!l.unobscured.intersects(&l.toolbar.unwrap()));
        for b in &l.buttons {
            assert!(
                !l.unobscured.intersects(&b.bounds),
                "{:?} stays out of the layout area",
                b.kind
            );
        }
    }

    #[test]
    fn the_buttons_are_23_pt_apart_and_9_pt_from_each_other() {
        let l = layout(ChromeStyle::TITLED);
        let lefts: Vec<f64> = l.buttons.iter().map(|b| b.bounds.min_x()).collect();
        assert_eq!(lefts, [9.0, 32.0, 55.0]);
        for b in &l.buttons {
            assert_eq!(b.bounds.size, Size::new(14.0, 14.0));
            assert_eq!(b.bounds.min_y(), 9.0);
        }
        assert_eq!(
            l.buttons[1].bounds.min_x() - l.buttons[0].bounds.max_x(),
            9.0
        );
        let p = layout(ChromeStyle::UTILITY_PANEL);
        let lefts: Vec<f64> = p.buttons.iter().map(|b| b.bounds.min_x()).collect();
        assert_eq!(lefts, [8.0, 25.0, 42.0], "panel buttons 17 pt apart");
    }

    #[test]
    fn the_button_table_follows_capabilities_and_keeps_disabled_buttons_in_place() {
        use Presence::*;
        let caps = |closable, minimizable, resizable| Capabilities {
            closable,
            minimizable,
            resizable,
        };
        let titled = FrameKind::Titled;
        let rows = [
            (titled, caps(true, true, true), [Enabled, Enabled, Enabled]),
            (
                titled,
                caps(true, true, false),
                [Enabled, Enabled, Disabled],
            ),
            (
                titled,
                caps(true, false, true),
                [Enabled, Disabled, Enabled],
            ),
            (
                titled,
                caps(false, true, true),
                [Disabled, Enabled, Enabled],
            ),
            (titled, caps(false, false, false), [Absent; 3]),
            (FrameKind::Borderless, Capabilities::ALL, [Absent; 3]),
            (
                FrameKind::UtilityPanel,
                Capabilities::PANEL,
                [Enabled, Disabled, Disabled],
            ),
        ];
        for (frame, c, expect) in rows {
            assert_eq!(button_presence(frame, c), expect, "{frame:?} {c:?}");
        }
        // a disabled button keeps its place: the group does not collapse
        let fixed = ChromeLayout::new(
            CONTENT,
            ChromeStyle::TITLED,
            caps(true, true, false),
            Direction::LeftToRight,
            &[],
        );
        assert_eq!(fixed.button(ButtonKind::Zoom).unwrap().bounds.min_x(), 55.0);
        assert_eq!(fixed.button(ButtonKind::Zoom).unwrap().presence, Disabled);
        let none = ChromeLayout::new(
            CONTENT,
            ChromeStyle::TITLED,
            caps(false, false, false),
            Direction::LeftToRight,
            &[],
        );
        assert!(none.buttons.is_empty());
        assert_eq!(none.allocation(), 32.0, "no buttons, the same title bar");
    }

    #[test]
    fn a_right_to_left_window_mirrors_the_group_and_its_leading_items() {
        for style in [
            ChromeStyle::TITLED,
            ChromeStyle::with_toolbar(ToolbarStyle::Unified),
            ChromeStyle::with_toolbar(ToolbarStyle::UnifiedCompact),
            ChromeStyle::UTILITY_PANEL,
        ] {
            let ltr = layout(style);
            let rtl = ChromeLayout::new(
                CONTENT,
                style,
                Capabilities::ALL,
                Direction::RightToLeft,
                &[],
            );
            for (a, b) in ltr.buttons.iter().zip(&rtl.buttons) {
                assert_eq!(a.kind, b.kind);
                assert_eq!(
                    b.bounds.min_x(),
                    800.0 - a.bounds.max_x(),
                    "{style:?} {:?}",
                    a.kind
                );
                assert_eq!(a.bounds.min_y(), b.bounds.min_y());
            }
            // close is outermost on the right
            assert!(rtl.buttons[0].bounds.min_x() > rtl.buttons[2].bounds.min_x());
            assert_eq!(ltr.unobscured, rtl.unobscured);
            if let (Some(a), Some(b)) = (ltr.items, rtl.items) {
                assert_eq!(b.min_x(), 800.0 - a.max_x(), "{style:?} items mirrored");
            }
        }
        assert_eq!(Direction::of_locale("ar_EG.UTF-8"), Direction::RightToLeft);
        assert_eq!(Direction::of_locale("he"), Direction::RightToLeft);
        assert_eq!(Direction::of_locale("fa-IR"), Direction::RightToLeft);
        assert_eq!(Direction::of_locale("en_US.UTF-8"), Direction::LeftToRight);
        assert_eq!(Direction::of_locale("C"), Direction::LeftToRight);
        assert_eq!(Direction::of_locale(""), Direction::LeftToRight);
        assert_eq!(Direction::of_locale("ar:en_US"), Direction::RightToLeft);
        assert_eq!(Direction::of_locale("en_US:ar"), Direction::LeftToRight);
    }

    #[test]
    fn the_title_sits_above_shares_the_row_or_gives_its_room_to_the_items() {
        let unified = ChromeStyle::with_toolbar(ToolbarStyle::Unified);
        let shared = layout(unified);
        assert_eq!(unified.title(), TitleArrangement::SharesRow);
        let title = shared.title.unwrap();
        let group = shared.buttons[2].bounds.max_x();
        assert!(title.min_x() >= group + GROUP_GAP);
        assert!(shared.items.unwrap().min_x() > title.max_x());
        // hiding the title lets the items start right after the group
        let hidden = layout(ChromeStyle {
            title_visible: false,
            ..unified
        });
        assert!(hidden.title.is_none());
        assert_eq!(hidden.items.unwrap().min_x(), title.min_x());
        assert!(hidden.items.unwrap().width() > shared.items.unwrap().width());
        // title above the toolbar: centred in the title row, the toolbar the row below, full width
        for toolbar in [ToolbarStyle::Expanded, ToolbarStyle::Settings] {
            let l = layout(ChromeStyle::with_toolbar(toolbar));
            let t = l.title.unwrap();
            assert_eq!((t.min_y(), t.height()), (0.0, 32.0), "{toolbar:?}");
            assert_eq!(t.center().x, 400.0, "{toolbar:?} centred");
            let bar = l.toolbar.unwrap();
            assert_eq!(bar.min_y(), 32.0);
            assert_eq!(bar.max_y(), l.style.allocation());
            assert_eq!(l.items.unwrap().min_x(), ROW_TRAILING);
        }
        // compact: the title shares the shorter row
        let compact = layout(ChromeStyle::with_toolbar(ToolbarStyle::UnifiedCompact));
        assert_eq!(compact.title.unwrap().height(), 40.0);
        // no toolbar: the title centred in the bar, no items
        let titled = layout(ChromeStyle::TITLED);
        assert_eq!(titled.title.unwrap().center().x, 400.0);
        assert!(titled.items.is_none() && titled.toolbar.is_none());
    }

    #[test]
    fn accessories_take_room_beside_the_items_or_below_the_toolbar_and_can_stay_in_full_screen() {
        let unified = full_size(ChromeStyle::with_toolbar(ToolbarStyle::Unified));
        let bar = Accessory {
            edge: AccessoryEdge::Bottom,
            extent: 28.0,
            full_screen_min_height: 20.0,
        };
        let lead = Accessory {
            edge: AccessoryEdge::Leading,
            extent: 60.0,
            full_screen_min_height: 0.0,
        };
        let trail = Accessory {
            edge: AccessoryEdge::Trailing,
            extent: 40.0,
            full_screen_min_height: 0.0,
        };
        let l = ChromeLayout::new(
            CONTENT,
            unified,
            Capabilities::ALL,
            Direction::LeftToRight,
            &[bar, lead, trail],
        );
        assert_eq!(l.allocation(), 80.0, "the bottom accessory adds its height");
        assert_eq!(l.accessories[0].1, Rect::new(0.0, 52.0, 800.0, 28.0));
        let (lr, tr) = (l.accessories[1].1, l.accessories[2].1);
        assert!(
            lr.min_x() > l.buttons[2].bounds.max_x(),
            "leading after the group"
        );
        assert_eq!(tr.max_x(), 800.0 - ROW_TRAILING);
        let items = l.items.unwrap();
        assert!(
            !items.intersects(&lr) && !items.intersects(&tr),
            "no collision with the items"
        );
        assert!(!l.title.unwrap().intersects(&lr));
        // mirrored
        let r = ChromeLayout::new(
            CONTENT,
            unified,
            Capabilities::ALL,
            Direction::RightToLeft,
            &[bar, lead, trail],
        );
        assert_eq!(r.accessories[1].1.max_x(), 800.0 - lr.min_x());
        assert_eq!(r.accessories[2].1.min_x(), ROW_TRAILING);
        // full screen: the toolbar hides; the accessory keeps its minimum
        let f = ChromeLayout::full_screen(
            Size::new(1440.0, 900.0),
            unified,
            Direction::LeftToRight,
            &[bar, lead],
        );
        assert!(f.buttons.is_empty() && f.toolbar.is_none());
        assert_eq!(f.unobscured.min_y(), 20.0);
        assert_eq!(
            f.accessories,
            vec![(bar, Rect::new(0.0, 0.0, 1440.0, 20.0))]
        );
        let hidden = ChromeLayout::full_screen(
            Size::new(1440.0, 900.0),
            unified,
            Direction::LeftToRight,
            &[Accessory {
                full_screen_min_height: 0.0,
                ..bar
            }],
        );
        assert_eq!(hidden.unobscured, hidden.frame, "nothing kept");
    }

    #[test]
    fn the_server_styles_map_to_their_chrome() {
        let cases = [
            (TitlebarStyle::Standard, 32.0, false, (9.0, 9.0)),
            (TitlebarStyle::Unified, 52.0, true, (19.0, 19.0)),
            (TitlebarStyle::UnifiedCompact, 40.0, true, (12.0, 13.0)),
            (TitlebarStyle::Sidebar, 52.0, true, (19.0, 19.0)),
        ];
        for (style, allocation, full, origin) in cases {
            let c = ChromeStyle::of_titlebar(style);
            assert_eq!(c.allocation(), allocation, "{style:?}");
            assert_eq!(c.full_size_content, full, "{style:?}");
            assert_eq!(c.button_metrics().unwrap().2, origin, "{style:?}");
        }
        assert!(!ChromeStyle::of_titlebar(TitlebarStyle::Sidebar).title_visible);
        let sidebar = layout(ChromeStyle::of_titlebar(TitlebarStyle::Sidebar));
        assert_eq!(sidebar.unobscured.height(), 548.0);
    }
}
