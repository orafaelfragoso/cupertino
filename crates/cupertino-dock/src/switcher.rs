//! The application switcher's panel: what the server's list and selection (`switcher_changed`)
//! turn into on screen. The server keeps the keys and the choice; this module holds the panel's
//! geometry, the show delay and fade, the
//! selection and label colours and the pointer rules.

use cupertino_animation::CubicBezier;
use cupertino_wm::geometry::{Point, Rect};

/// Delay between the switcher opening and its panel appearing; a commit before then switches
/// without a panel (150 ms). The panel then appears without animation.
pub const SHOW_DELAY: f64 = 0.15;

/// Duration of the fade-out after a commit: no duration is set, so the layer framework's default
/// applies (0.25 s); the curve is ease-in-ease-out.
pub const HIDE_DURATION: f64 = 0.25;

/// The pointer selects only after leaving a box this far (in each axis) around where it was when
/// the panel appeared.
pub const HOVER_BOX: f64 = 3.0;

/// Material of the panel: the compositor's switcher glass.
pub const MATERIAL: &str = "SwitcherGlass";

/// One switch as the dock shows it: the server's list (application ids, most recently used first),
/// the selected index, when the panel was asked for and, once a commit started the fade-out, when.
#[derive(Clone, Debug, PartialEq)]
pub struct Switcher {
    apps: Vec<String>,
    selected: usize,
    opened_at: f64,
    closed_at: Option<f64>,
}

impl Switcher {
    /// A switch that opened at `now`; an error when the server broke the protocol (an empty list
    /// or a selection outside it).
    pub fn new(apps: Vec<String>, selected: usize, now: f64) -> Result<Self, &'static str> {
        check(&apps, selected)?;
        Ok(Self {
            apps,
            selected,
            opened_at: now,
            closed_at: None,
        })
    }

    pub fn apps(&self) -> &[String] {
        &self.apps
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Whether `apps` is the list of this switch while it is still open: the server sends the
    /// whole list with every step, and a different one (or one after the fade began) is a new switch.
    pub fn continues_with(&self, apps: &[String]) -> bool {
        self.closed_at.is_none() && self.apps == apps
    }

    /// Move the selection; an error for an index outside the list.
    pub fn select(&mut self, selected: usize) -> Result<(), &'static str> {
        check(&self.apps, selected)?;
        self.selected = selected;
        Ok(())
    }

    /// The switch was committed: the panel fades out from `now`.
    pub fn close(&mut self, now: f64) {
        self.closed_at = Some(now);
    }

    /// Whether the delay passed and the panel is on screen (until it is gone).
    pub fn is_shown(&self, now: f64) -> bool {
        now - self.opened_at >= SHOW_DELAY
    }

    /// Seconds until the panel appears; zero once it did.
    pub fn until_shown(&self, now: f64) -> f64 {
        (self.opened_at + SHOW_DELAY - now).max(0.0)
    }

    /// Panel opacity: 1 until the fade starts, then `1 - ease(t)` over [`HIDE_DURATION`].
    pub fn alpha(&self, now: f64) -> f64 {
        self.closed_at.map_or(1.0, |at| {
            1.0 - CubicBezier::EASE_IN_EASE_OUT.eval(((now - at) / HIDE_DURATION).clamp(0.0, 1.0))
        })
    }

    /// Whether the fade-out has finished and the panel can go.
    pub fn is_gone(&self, now: f64) -> bool {
        self.closed_at.is_some_and(|at| now - at >= HIDE_DURATION)
    }

    /// Whether the panel is fading out and needs a frame per refresh.
    pub fn is_fading(&self) -> bool {
        self.closed_at.is_some()
    }
}

fn check(apps: &[String], selected: usize) -> Result<(), &'static str> {
    if apps.is_empty() {
        return Err("the switcher list is empty");
    }
    if selected >= apps.len() {
        return Err("the switcher selection is outside its list");
    }
    Ok(())
}

/// How a layer's gray combines with what is behind it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compositing {
    /// `D + S − 1`: darkens the backdrop by `1 − S`.
    PlusDarker,
    /// `D + S`: lightens the backdrop by `S`.
    PlusLighter,
}

impl Compositing {
    /// Gray a layer of gray `source` shows over a backdrop of gray `backdrop`.
    pub fn over(self, backdrop: f32, source: f32) -> f32 {
        match self {
            Compositing::PlusDarker => (backdrop + source - 1.0).max(0.0),
            Compositing::PlusLighter => (backdrop + source).min(1.0),
        }
    }
}

/// Switcher panel geometry: tiles of at most 128 pt with 5 pt gaps, 23 pt
/// side insets, 29 pt above and below, a 20 pt display margin before tiles shrink (one row, never
/// wrapping), centred on the output; panel radius 24 pt; selection outset 4 pt with radius 9 pt.
/// Rectangles are in output points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SwitcherLayout {
    pub panel: Rect,
    pub tile: f64,
}

impl SwitcherLayout {
    pub const MAX_TILE: u32 = 128;
    pub const GAP: f64 = 5.0;
    pub const INSET_X: f64 = 23.0;
    pub const INSET_Y: f64 = 29.0;
    pub const DISPLAY_MARGIN: f64 = 20.0;
    pub const RADIUS: f64 = 24.0;
    pub const SELECTION_OUTSET: f64 = 4.0;
    pub const SELECTION_RADIUS: f64 = 9.0;
    /// Label rectangle height: one line of the 15 pt system font, (1980 + 432) / 2048 × 15, not
    /// rounded.
    pub const LABEL_HEIGHT: f64 = 17.666;
    /// Label font size.
    pub const LABEL_FONT_PT: f64 = 15.0;
    /// Gap between the label rectangle and the panel bottom.
    pub const LABEL_BOTTOM: f64 = 3.0;
    /// The label layer's preferred width is the text width plus 1 pt.
    pub const LABEL_EXTRA_WIDTH: f64 = 1.0;
    /// Gray of the panel's fill layer, light and dark: the backdrop the
    /// selection and label compositing is evaluated over.
    const PANEL_FILL_GRAY: [f32; 2] = [0.9646, 0.1569];
    /// Dark selection gray, plus-darker.
    const DARK_SELECTION_GRAY: f32 = 0.9098;
    /// Label gray: light 0.5 plus-darker, dark 0.4863 plus-lighter.
    const LABEL_GRAY: [f32; 2] = [0.5, 0.4863];

    /// Layout for `n` items on an output of `w × h` points; integer arithmetic as in the design.
    pub fn new(n: usize, w: f64, h: f64) -> Self {
        let n32 = n.max(1) as u32;
        let avail = (w - 2.0 * Self::INSET_X - Self::DISPLAY_MARGIN).max(0.0) as u32;
        let gaps = 5 * (n32 - 1);
        let tile = if Self::MAX_TILE * n32 + gaps <= avail {
            Self::MAX_TILE
        } else {
            avail.saturating_sub(gaps) / n32
        };
        let tile = f64::from(tile);
        let pw = f64::from(n32) * tile + f64::from(gaps) + 2.0 * Self::INSET_X;
        let ph = tile + 2.0 * Self::INSET_Y;
        Self {
            panel: Rect::new(
                (w / 2.0 - pw / 2.0).trunc(),
                (h / 2.0 - ph / 2.0).trunc(),
                pw,
                ph,
            ),
            tile,
        }
    }

    pub fn item(&self, i: usize) -> Rect {
        let x = self.panel.x + Self::INSET_X + i as f64 * (self.tile + Self::GAP);
        Rect::new(x, self.panel.y + Self::INSET_Y, self.tile, self.tile)
    }

    pub fn selection(&self, i: usize) -> Rect {
        let (it, o) = (self.item(i), Self::SELECTION_OUTSET);
        Rect::new(it.x - o, it.y - o, it.w + 2.0 * o, it.h + 2.0 * o)
    }

    /// The entry whose tile `p` (output points) is over.
    pub fn hit(&self, p: Point, n: usize) -> Option<usize> {
        (0..n).find(|&i| self.item(i).contains(p))
    }

    /// Label rectangle of item `i` for a text `text_width` pt wide: the text width plus 1 pt (at most
    /// the panel width minus both insets), centred under the selection, kept between the insets,
    /// 3 pt above the panel bottom.
    pub fn label(&self, i: usize, text_width: f64) -> Rect {
        let w = (text_width + Self::LABEL_EXTRA_WIDTH).min(self.panel.w - 2.0 * Self::INSET_X);
        let x = (self.selection(i).cx() - w / 2.0).trunc().clamp(
            self.panel.x + Self::INSET_X,
            self.panel.right() - Self::INSET_X - w,
        );
        Rect::new(
            x,
            self.panel.bottom() - Self::LABEL_BOTTOM - Self::LABEL_HEIGHT,
            w,
            Self::LABEL_HEIGHT,
        )
    }

    /// Selection highlight fill. Light: gray 0.75 at layer opacity 0.65 (the linear-burn
    /// blend is drawn as normal alpha, approximate). Dark: plus-darker with gray 0.9098, drawn as
    /// black whose alpha darkens the panel's fill gray by the same amount (approximate: the dock
    /// draws without blend modes, so this is exact only over a backdrop of that gray).
    pub fn selection_fill(dark: bool) -> [f32; 4] {
        if !dark {
            return [0.75, 0.75, 0.75, 0.65];
        }
        let backdrop = Self::PANEL_FILL_GRAY[1];
        let darkened = Compositing::PlusDarker.over(backdrop, Self::DARK_SELECTION_GRAY);
        [0.0, 0.0, 0.0, 1.0 - darkened / backdrop]
    }

    /// Label colour: the gray the label's compositing leaves over the panel's fill gray
    /// (approximate the same way as [`Self::selection_fill`]).
    pub fn label_color(dark: bool) -> [f32; 4] {
        let (mode, i) = if dark {
            (Compositing::PlusLighter, 1)
        } else {
            (Compositing::PlusDarker, 0)
        };
        let gray = mode.over(Self::PANEL_FILL_GRAY[i], Self::LABEL_GRAY[i]);
        [gray, gray, gray, 1.0]
    }
}

/// Decides when the pointer may select by hovering: not until it left a
/// [`HOVER_BOX`] around the first position it was seen at on the panel.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HoverGate {
    origin: Option<Point>,
    armed: bool,
}

impl HoverGate {
    /// Feed a pointer position; whether hovering may select now. Once armed, it stays armed.
    pub fn moved(&mut self, p: Point) -> bool {
        let origin = *self.origin.get_or_insert(p);
        self.armed |= (p.x - origin.x).abs() > HOVER_BOX || (p.y - origin.y).abs() > HOVER_BOX;
        self.armed
    }
}
