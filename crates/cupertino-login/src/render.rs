//! CPU drawing of the lock screen: the wallpaper with its treatment, then the interface (clock, user, password field)
//! on a layer that fades as a whole; and of the confirmation and interrupt panels.

use std::collections::HashMap;
use std::f64::consts::PI;

use cupertino_assets::{AssetError, Assets, FontRole};
use cupertino_decorations::palette::Rgba;
use cupertino_decorations::shape::{rounded_path, to_skia_path};
use cupertino_decorations::text::TextRenderer;
use cupertino_geometry::{CornerStyle, PathSeg, Point as GeoPoint, Radii, Rect, RoundedRect};
use cupertino_theme::Typography;
use tiny_skia::{
    BlendMode, Color, FillRule, FilterQuality, GradientStop, LineCap, LineJoin, LinearGradient,
    Paint, PathBuilder, Pattern, Pixmap, PixmapPaint, Point, RadialGradient, SpreadMode, Stroke,
    Transform,
};

use crate::interrupt;
use crate::layout::{
    Layout, AVATAR_DIAMETER, HELP_SIZE, NAME_BASELINE_GAP, NAME_MAX_WIDTH, NAME_SIZE, PILL_HEIGHT,
    PILL_WIDTH, USER_VIEW_BORDER,
};
use crate::panel::{wrap_width, Decision, Frames, Kind, COUNTDOWN_SECONDS, INFO_LINE, TITLE_LINE};
use crate::picker::{Grid, Tile};
use crate::power::Action;
use crate::spinner;
use crate::user::User;

/// Black laid over the wallpaper (alpha).
const DARKEN: f32 = 0.25;
/// Black laid over the darkened wallpaper (alpha).
const LIGHTEN: f32 = 0.3;
/// Black in the middle of the vignette, fading to nothing at the corners.
const VIGNETTE: f32 = 0.2;

/// Corner colours and blobs of the procedural wallpaper, the dark set of the server's desktop background
/// (approximate: the design shows the user's picture, which this process has no way to ask for yet).
const WALLPAPER_BASE: [[f32; 3]; 2] = [[0.07, 0.09, 0.20], [0.20, 0.09, 0.27]];
/// Centre in the unit square, falloff `k` of `exp(-k r^2)` with `r` in heights, and colour of each blob.
const WALLPAPER_BLOBS: [([f32; 2], f32, [f32; 3]); 2] = [
    ([0.22, 0.30], 9.0, [0.10, 0.30, 0.58]),
    ([0.80, 0.68], 7.0, [0.50, 0.16, 0.42]),
];
/// Strength of a blob at its centre.
const BLOB_STRENGTH: f32 = 0.85;

/// Avatar halo (shadow radius 15, opacity 0.25).
const AVATAR_SHADOW_RADIUS: f64 = 15.0;
const AVATAR_SHADOW_OPACITY: f32 = 0.25;
/// Avatar ring (1 pt, white 0.1).
const RING_WIDTH: f64 = 1.0;
/// The outline of a password field after a wrong password: the dark appearance's red (approximate).
const FAILURE_OUTLINE: Rgba = [1.0, 69.0 / 255.0, 58.0 / 255.0, 1.0];
const FAILURE_OUTLINE_WIDTH: f64 = 2.0;
const RING_ALPHA: f32 = 0.1;
/// Fill behind the initials (approximate).
const AVATAR_FILL: Rgba = [0.60, 0.62, 0.66, 1.0];
/// The mark on the avatar of a user who has a session: a disc of this fill with a white check, at the lower right of the avatar
/// (approximate, ours: the design's picker marks such users, the shape and place are not read).
const LOGGED_IN_FILL: Rgba = [0.20, 0.72, 0.35, 1.0];
const LOGGED_IN_RADIUS: f64 = 9.0;
/// Initials size as a fraction of the avatar diameter (approximate).
const INITIALS_SIZE: f64 = 0.4;

/// Password pill fill, a stand-in for the server-rendered material 27 that lock surfaces do not get
/// (approximate).
const PILL_FILL: Rgba = [1.0, 1.0, 1.0, 0.2];
/// Password text: system 13 Medium, placeholder white 0.5.
const FIELD_SIZE: f64 = 13.0;
const PLACEHOLDER_ALPHA: f32 = 0.5;
const PLACEHOLDER: &str = "Enter Password";
/// The name field of the "Other…" tile (approximate text).
const NAME_PLACEHOLDER: &str = "Name";
/// The "Other…" tile: its label (`OTHER_USER`) and the mark in its disc (approximate; the design
/// draws the generic artwork "2", 144 pt, which is not part of the payload we use).
const OTHER_LABEL: &str = "Other…";
const OTHER_GLYPH: &str = "…";
/// The selected tile's rounded square behind the avatar: 86 pt (selection size); radius and fill
/// are approximate (the design draws a halo image).
const SELECTION_SIZE: f64 = 86.0;
const SELECTION_RADIUS: f64 = 20.0;
const SELECTION_FILL: Rgba = [1.0, 1.0, 1.0, 0.2];
/// The password help line (approximate: the design opens its own help view, whose text is not in the register; this
/// system has no hint to show, so the line points to whoever can reset the password).
const PASSWORD_HELP: &str = "Forgot your password? Ask an administrator to reset it.";
/// Dots standing for typed characters (approximate).
const DOT_DIAMETER: f64 = 6.0;
const DOT_PITCH: f64 = 11.0;
/// Padding inside the pill's rounded ends for the dots (approximate).
const PILL_PADDING: f64 = 14.0;
/// Widest a notice draws (message width 544).
const NOTICE_MAX_WIDTH: f64 = 544.0;
const NOTICE_ALPHA: f32 = 0.8;

/// Caps Lock badge: an 18 pt slot at the field's trailing edge, a 16 x 16 unit rounded square of
/// corner radius 4, stretched vertically by `min(field height / 17, 1.2).
const CAPS_SLOT_WIDTH: f64 = 18.0;
const CAPS_BADGE_UNITS: f64 = 16.0;
const CAPS_BADGE_RADIUS: f64 = 4.0;
const CAPS_STRETCH_REFERENCE: f64 = 17.0;
const CAPS_STRETCH_MAX: f64 = 1.2;
/// The up arrow over a bar that is cut out of the badge, in badge units (approximate: gives the glyph's path
/// constants but not their order, so the shape is ours).
const CAPS_GLYPH: [&[(f64, f64)]; 2] = [
    &[
        (8.0, 3.0),
        (12.5, 8.5),
        (10.0, 8.5),
        (10.0, 11.0),
        (6.0, 11.0),
        (6.0, 8.5),
        (3.5, 8.5),
    ],
    &[(5.5, 12.5), (10.5, 12.5), (10.5, 14.0), (5.5, 14.0)],
];
/// Edge of the spinner square in the field (approximate: the small control's intrinsic size is a toolkit metric not in
/// the payload we read, the 18 pt slot minus a point on each side).
const SPINNER_SIZE: f64 = 16.0;

/// The power glyph and the open menu (approximate: the design draws a template glyph, names it only as
/// the white-tinted power glyph, and a system menu): the ring's radius, stroke and the angle of its gap at the top, how many
/// segments the ring is cut into, the menu's corner radius and the inset of its titles.
const POWER_RADIUS: f64 = 5.5;
const POWER_STROKE: f64 = 1.6;
const POWER_GAP_DEGREES: f64 = 60.0;
const POWER_RING_STEPS: usize = 30;
const MENU_RADIUS: f64 = 8.0;
const MENU_TEXT_INSET: f64 = 12.0;

/// Panel body (approximate: the design draws a window of its own material), corner radius of a regular window
/// (settled value, 12 pt) and an edge line like the avatar ring.
const PANEL_FILL: Rgba = [0.16, 0.16, 0.18, 0.96];
const PANEL_RADIUS: f64 = 12.0;
const PANEL_EDGE_ALPHA: f32 = 0.1;
/// Buttons (approximate): the default button filled in an accent blue, the other as a pill of the field's fill.
const BUTTON_RADIUS: f64 = 6.0;
const DEFAULT_BUTTON_FILL: Rgba = [0.04, 0.52, 1.0, 1.0];
const BUTTON_SIZE: f64 = 13.0;
/// Panel text sizes and the distance of a first baseline below the top of its line box (approximate: the design's font
/// metrics are not available; the line boxes, columns and wrap widths are, in `panel`).
const PANEL_TITLE_SIZE: f64 = 13.0;
const PANEL_TITLE_BASELINE: f64 = 13.0;
const PANEL_MESSAGE_SIZE: f64 = 11.0;
const PANEL_MESSAGE_BASELINE: f64 = 11.0;
const PANEL_MESSAGE_ALPHA: f32 = 0.7;

const WHITE: Rgba = [1.0, 1.0, 1.0, 1.0];
const WEIGHT_MEDIUM: u16 = 500;
const WEIGHT_SEMIBOLD: u16 = 600;

/// What one frame shows.
pub struct View<'a> {
    pub time: &'a str,
    pub date: &'a str,
    /// Whose password is asked for; `None` while a name is typed for the "Other…" tile.
    pub user: Option<&'a User>,
    pub field: Field<'a>,
    pub notice: Option<&'a str>,
    /// The password field has the focus and Caps Lock is on: the badge shows.
    pub caps_lock: bool,
    /// The frame of the spinner in the field's trailing slot while an attempt is slow; it takes the badge's place.
    pub spinner: Option<u32>,
    /// Failed attempts have brought up the password help.
    pub help: bool,
    pub power: Option<PowerView<'a>>,
    /// Opacity of everything but the wallpaper.
    pub opacity: f64,
    /// Horizontal offset of the password field in points.
    pub shake: f64,
    /// The last attempt was a wrong password: the field is outlined until the next edit.
    pub failure_outline: bool,
}

/// What the field holds.
#[derive(Clone, Copy, Debug)]
pub enum Field<'a> {
    /// A password of this many characters, shown as dots.
    Password(usize),
    /// A name typed for the picker's "Other…" tile, shown as text.
    Name(&'a str),
}

/// The power item of the greeter's status bar.
#[derive(Clone, Copy)]
pub struct PowerView<'a> {
    /// The menu's items, top to bottom.
    pub items: &'a [Action],
    /// The selected row while the menu is open.
    pub open: Option<usize>,
    /// Restart and Shut Down will ask for an administrator: their titles end in an ellipsis.
    pub will_prompt: bool,
}

/// What one frame of the user picker shows.
pub struct PickerView<'a> {
    pub power: Option<PowerView<'a>>,
    pub time: &'a str,
    pub date: &'a str,
    /// The tiles of the page on screen, in the cells of `grid` row by row.
    pub tiles: &'a [Tile],
    pub grid: Grid,
    /// The selected tile, as an index into `tiles`.
    pub selected: usize,
}

/// What one frame of the confirmation panel shows.
pub struct PanelView {
    pub kind: Kind,
    /// Whole seconds left on the countdown; `None` on a panel without one.
    pub seconds_left: Option<u32>,
    /// The layout from [`Renderer::panel_frames`]; its size is the pixmap's.
    pub frames: Frames,
}

/// Draws the lock screen and the confirmation panel.
pub struct Renderer {
    text: TextRenderer,
    descent_per_em: f64,
    cap_height_per_em: f64,
    /// The wallpaper with its treatment by pixel size, so a frame only copies it.
    wallpapers: HashMap<(u32, u32), Pixmap>,
}

impl Renderer {
    pub fn new(typography: &Typography, assets: &Assets) -> Result<Self, AssetError> {
        let metrics = &typography.metrics;
        Ok(Renderer {
            text: TextRenderer::new(assets, FontRole::Ui)?,
            descent_per_em: metrics.descender.abs() / metrics.units_per_em,
            cap_height_per_em: metrics.cap_height / metrics.units_per_em,
            wallpapers: HashMap::new(),
        })
    }

    /// The layout of an output of `width` x `height` points in this renderer's font.
    pub fn layout(&self, width: f64, height: f64) -> Layout {
        Layout::new(width, height, self.descent_per_em)
    }

    /// The wallpaper over the whole of `pixmap`, then what `draw` puts on a layer above it, which is blended with
    /// `opacity`.
    fn compose(
        &mut self,
        pixmap: &mut Pixmap,
        scale: f32,
        opacity: f64,
        draw: impl FnOnce(&mut Self, &mut Canvas, &Layout),
    ) {
        let (width, height) = (pixmap.width(), pixmap.height());
        let wallpaper = self
            .wallpapers
            .entry((width, height))
            .or_insert_with(|| wallpaper(width, height));
        pixmap.draw_pixmap(
            0,
            0,
            wallpaper.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
        let mut layer = Pixmap::new(width, height).expect("the surface has a size");
        let layout = self.layout(
            f64::from(width) / f64::from(scale),
            f64::from(height) / f64::from(scale),
        );
        draw(
            self,
            &mut Canvas {
                pixmap: &mut layer,
                scale: f64::from(scale),
            },
            &layout,
        );
        let paint = PixmapPaint {
            opacity: opacity as f32,
            ..PixmapPaint::default()
        };
        pixmap.draw_pixmap(0, 0, layer.as_ref(), &paint, Transform::identity(), None);
    }

    /// Draw `view` over the whole of `pixmap` at `scale` device pixels per point.
    pub fn render(&mut self, view: &View<'_>, pixmap: &mut Pixmap, scale: f32) {
        self.compose(pixmap, scale, view.opacity, |this, canvas, layout| {
            this.draw_clock(canvas, view.date, view.time, layout);
            match view.user {
                Some(user) => {
                    this.draw_user(canvas, user, layout.avatar.center(), layout.name_baseline)
                }
                None => this.draw_other(canvas, layout.avatar.center(), layout.name_baseline),
            }
            this.draw_field(canvas, view, layout);
            if view.help {
                let max_width =
                    NOTICE_MAX_WIDTH.min(f64::from(canvas.pixmap.width()) / canvas.scale - 40.0);
                this.centered_text(
                    canvas,
                    PASSWORD_HELP,
                    layout.center_x,
                    layout.help_baseline,
                    HELP_SIZE,
                    WEIGHT_MEDIUM,
                    max_width,
                    with_alpha(WHITE, NOTICE_ALPHA),
                );
            }
            if let Some(power) = view.power {
                this.draw_power(canvas, power, layout);
            }
        });
    }

    /// Draw the user picker `view` over the whole of `pixmap` at `scale` device pixels per point.
    pub fn render_picker(&mut self, view: &PickerView<'_>, pixmap: &mut Pixmap, scale: f32) {
        self.compose(pixmap, scale, 1.0, |this, canvas, layout| {
            this.draw_clock(canvas, view.date, view.time, layout);
            for (i, (tile, frame)) in view
                .tiles
                .iter()
                .zip(layout.tiles(view.grid, view.tiles.len()))
                .enumerate()
            {
                this.draw_tile(canvas, tile, frame, i == view.selected);
            }
            if let Some(power) = view.power {
                this.draw_power(canvas, power, layout);
            }
        });
    }

    /// The layout of the `kind` panel for this font: the title as it wraps and the info line at the most lines it takes while
    /// it counts down, so the size does not change with the number. Measured in points, whatever the output's scale.
    pub fn panel_frames(&mut self, kind: Kind) -> Frames {
        let text = kind.text();
        let wrap = wrap_width(kind);
        let title_lines = self
            .wrap(text.title, PANEL_TITLE_SIZE, WEIGHT_SEMIBOLD, wrap)
            .len();
        let info_lines = (0..=COUNTDOWN_SECONDS as u32)
            .filter_map(|seconds| kind.countdown_message(seconds))
            .map(|message| {
                self.wrap(&message, PANEL_MESSAGE_SIZE, WEIGHT_MEDIUM, wrap)
                    .len()
            })
            .max()
            .unwrap_or(0);
        kind.frames(title_lines as u32, info_lines as u32)
    }

    /// Draw `view` over the whole of `pixmap`, a panel of the size `view.frames` at `scale` device pixels per point.
    pub fn render_panel(&mut self, view: &PanelView, pixmap: &mut Pixmap, scale: f32) {
        let frames = &view.frames;
        let text = view.kind.text();
        let mut canvas = Canvas {
            pixmap,
            scale: f64::from(scale),
        };
        let body = Rect::new(0.0, 0.0, frames.width, frames.height);
        canvas.rounded(body, PANEL_RADIUS, PANEL_FILL);
        canvas.rounded_edge(body, PANEL_RADIUS, [1.0, 1.0, 1.0, PANEL_EDGE_ALPHA]);
        let icon = frames.icon.center();
        canvas.circle(icon.x, icon.y, frames.icon.width() / 2.0, AVATAR_FILL);
        self.wrapped_text(
            &mut canvas,
            text.title,
            frames.title,
            PANEL_TITLE_BASELINE,
            TITLE_LINE,
            PANEL_TITLE_SIZE,
            WEIGHT_SEMIBOLD,
            WHITE,
        );
        if let (Some(info), Some(message)) = (
            frames.info,
            view.seconds_left
                .and_then(|seconds| view.kind.countdown_message(seconds)),
        ) {
            self.wrapped_text(
                &mut canvas,
                &message,
                info,
                PANEL_MESSAGE_BASELINE,
                INFO_LINE,
                PANEL_MESSAGE_SIZE,
                WEIGHT_MEDIUM,
                with_alpha(WHITE, PANEL_MESSAGE_ALPHA),
            );
        }
        for button in &frames.buttons {
            let fill = if button.decision == Decision::Do(view.kind.default_action()) {
                DEFAULT_BUTTON_FILL
            } else {
                PILL_FILL
            };
            canvas.rounded(button.frame, BUTTON_RADIUS, fill);
            let center = button.frame.center();
            let baseline = center.y + self.cap_height_per_em * BUTTON_SIZE / 2.0;
            self.centered_text(
                &mut canvas,
                button.label,
                center.x,
                baseline,
                BUTTON_SIZE,
                WEIGHT_MEDIUM,
                button.frame.width(),
                WHITE,
            );
        }
    }

    /// Draw the interrupt panel for the application `app` over the whole of `pixmap`, a surface of
    /// [`interrupt::WIDTH`] x [`interrupt::HEIGHT`] points at `scale` device pixels per point. No button is the default.
    pub fn render_interrupt(
        &mut self,
        app: &str,
        frames: &interrupt::Frames,
        pixmap: &mut Pixmap,
        scale: f32,
    ) {
        let mut canvas = Canvas {
            pixmap,
            scale: f64::from(scale),
        };
        let body = Rect::new(0.0, 0.0, interrupt::WIDTH, interrupt::HEIGHT);
        canvas.rounded(body, PANEL_RADIUS, PANEL_FILL);
        canvas.rounded_edge(body, PANEL_RADIUS, [1.0, 1.0, 1.0, PANEL_EDGE_ALPHA]);
        let icon = frames.icon.center();
        canvas.circle(icon.x, icon.y, frames.icon.width() / 2.0, AVATAR_FILL);
        self.wrapped_text(
            &mut canvas,
            &interrupt::title(app),
            frames.title,
            PANEL_TITLE_BASELINE,
            TITLE_LINE,
            interrupt::TITLE_SIZE,
            WEIGHT_SEMIBOLD,
            WHITE,
        );
        self.wrapped_text(
            &mut canvas,
            &interrupt::info(app),
            frames.info,
            interrupt::INFO_SIZE,
            INFO_LINE,
            interrupt::INFO_SIZE,
            WEIGHT_MEDIUM,
            with_alpha(WHITE, PANEL_MESSAGE_ALPHA),
        );
        for button in &frames.buttons {
            canvas.rounded(button.frame, BUTTON_RADIUS, PILL_FILL);
            let center = button.frame.center();
            let baseline = center.y + self.cap_height_per_em * BUTTON_SIZE / 2.0;
            self.centered_text(
                &mut canvas,
                button.label,
                center.x,
                baseline,
                BUTTON_SIZE,
                WEIGHT_MEDIUM,
                button.frame.width(),
                WHITE,
            );
        }
    }

    fn draw_clock(&mut self, canvas: &mut Canvas, date: &str, time: &str, layout: &Layout) {
        self.centered_text(
            canvas,
            date,
            layout.center_x,
            layout.date_baseline,
            layout.date_size,
            WEIGHT_MEDIUM,
            f64::INFINITY,
            WHITE,
        );
        self.centered_text(
            canvas,
            time,
            layout.center_x,
            layout.time_baseline,
            layout.time_size,
            WEIGHT_SEMIBOLD,
            f64::INFINITY,
            WHITE,
        );
    }

    /// The avatar centred on `center` with the user's name on `name_baseline` below it.
    fn draw_user(
        &mut self,
        canvas: &mut Canvas,
        user: &User,
        center: GeoPoint,
        name_baseline: f64,
    ) {
        self.draw_avatar(canvas, center, |this, canvas, center, radius| {
            match &user.picture {
                Some(picture) => canvas.picture(picture, center.x, center.y, radius),
                None => {
                    canvas.circle(center.x, center.y, radius, AVATAR_FILL);
                    this.draw_initials(
                        canvas,
                        &crate::user::initials(&user.display),
                        center,
                        radius,
                    );
                }
            }
        });
        self.centered_text(
            canvas,
            &user.display,
            center.x,
            name_baseline,
            NAME_SIZE,
            WEIGHT_MEDIUM,
            NAME_MAX_WIDTH,
            WHITE,
        );
    }

    /// The halo and ring of an avatar with `fill` drawn between them.
    fn draw_avatar(
        &mut self,
        canvas: &mut Canvas,
        center: GeoPoint,
        fill: impl FnOnce(&mut Self, &mut Canvas, GeoPoint, f64),
    ) {
        let radius = AVATAR_DIAMETER / 2.0;
        canvas.halo(
            center.x,
            center.y,
            radius,
            AVATAR_SHADOW_RADIUS,
            AVATAR_SHADOW_OPACITY,
        );
        fill(self, canvas, center, radius);
        canvas.ring(
            center.x,
            center.y,
            radius - RING_WIDTH / 2.0,
            RING_WIDTH,
            [1.0, 1.0, 1.0, RING_ALPHA],
        );
    }

    fn draw_initials(
        &mut self,
        canvas: &mut Canvas,
        initials: &str,
        center: GeoPoint,
        radius: f64,
    ) {
        let size = AVATAR_DIAMETER * INITIALS_SIZE;
        let baseline = center.y + self.cap_height_per_em * size / 2.0;
        self.centered_text(
            canvas,
            initials,
            center.x,
            baseline,
            size,
            WEIGHT_MEDIUM,
            radius * 2.0,
            WHITE,
        );
    }

    /// One tile of the picker in its `frame`: the avatar and name as on the password screen, and behind the avatar
    /// a rounded square when the tile is selected.
    fn draw_tile(&mut self, canvas: &mut Canvas, tile: &Tile, frame: Rect, selected: bool) {
        let center = GeoPoint::new(
            frame.center().x,
            frame.y() + USER_VIEW_BORDER + AVATAR_DIAMETER / 2.0,
        );
        let name_baseline =
            frame.y() + 2.0 * USER_VIEW_BORDER + AVATAR_DIAMETER + NAME_BASELINE_GAP;
        if selected {
            let side = SELECTION_SIZE;
            canvas.rounded(
                Rect::new(center.x - side / 2.0, center.y - side / 2.0, side, side),
                SELECTION_RADIUS,
                SELECTION_FILL,
            );
        }
        match tile {
            Tile::Account(user) => {
                self.draw_user(canvas, user, center, name_baseline);
                if user.logged_in {
                    let offset = AVATAR_DIAMETER / 2.0 * std::f64::consts::FRAC_1_SQRT_2;
                    canvas.logged_in_mark(center.x + offset, center.y + offset);
                }
            }
            Tile::Other => self.draw_other(canvas, center, name_baseline),
        }
    }

    /// The power glyph in the status bar and, while the menu is open, the menu under it.
    fn draw_power(&mut self, canvas: &mut Canvas, power: PowerView<'_>, layout: &Layout) {
        let center = layout.power_item.center();
        canvas.power_glyph(center.x, center.y);
        let Some(selected) = power.open else { return };
        let (body, rows) = layout.power_menu(power.items.len());
        canvas.rounded(body, MENU_RADIUS, PANEL_FILL);
        canvas.rounded_edge(body, MENU_RADIUS, [1.0, 1.0, 1.0, PANEL_EDGE_ALPHA]);
        for (i, (action, row)) in power.items.iter().zip(rows).enumerate() {
            if i == selected {
                canvas.rounded(row, BUTTON_RADIUS, DEFAULT_BUTTON_FILL);
            }
            let baseline = row.center().y + self.cap_height_per_em * BUTTON_SIZE / 2.0;
            self.left_text(
                canvas,
                action.title(power.will_prompt),
                row.x() + MENU_TEXT_INSET,
                baseline,
                BUTTON_SIZE,
                WEIGHT_MEDIUM,
                row.width() - 2.0 * MENU_TEXT_INSET,
                WHITE,
            );
        }
    }

    /// The avatar of the "Other…" tile and its label.
    fn draw_other(&mut self, canvas: &mut Canvas, center: GeoPoint, name_baseline: f64) {
        self.draw_avatar(canvas, center, |this, canvas, center, radius| {
            canvas.circle(center.x, center.y, radius, AVATAR_FILL);
            this.draw_initials(canvas, OTHER_GLYPH, center, radius);
        });
        self.centered_text(
            canvas,
            OTHER_LABEL,
            center.x,
            name_baseline,
            NAME_SIZE,
            WEIGHT_MEDIUM,
            NAME_MAX_WIDTH,
            WHITE,
        );
    }

    fn draw_field(&mut self, canvas: &mut Canvas, view: &View<'_>, layout: &Layout) {
        let pill = Rect::new(
            layout.pill.x() + view.shake,
            layout.pill.y(),
            PILL_WIDTH,
            PILL_HEIGHT,
        );
        canvas.pill(pill, PILL_FILL);
        if view.failure_outline {
            canvas.outline(
                pill,
                PILL_HEIGHT / 2.0,
                FAILURE_OUTLINE_WIDTH,
                FAILURE_OUTLINE,
            );
        }
        let center = pill.center();
        let baseline = center.y + self.cap_height_per_em * FIELD_SIZE / 2.0;
        match view.field {
            Field::Password(0) => self.centered_text(
                canvas,
                PLACEHOLDER,
                center.x,
                baseline,
                FIELD_SIZE,
                WEIGHT_MEDIUM,
                PILL_WIDTH - 2.0 * PILL_PADDING,
                with_alpha(WHITE, PLACEHOLDER_ALPHA),
            ),
            Field::Password(len) => {
                // the same room on both sides, so the dots stay centred and clear of the badge's slot
                let capacity = ((PILL_WIDTH - 2.0 * PILL_PADDING.max(CAPS_SLOT_WIDTH)) / DOT_PITCH)
                    .floor() as usize;
                let shown = len.min(capacity);
                let left = center.x - (shown as f64 * DOT_PITCH - (DOT_PITCH - DOT_DIAMETER)) / 2.0
                    + DOT_DIAMETER / 2.0;
                for i in 0..shown {
                    canvas.circle(
                        left + i as f64 * DOT_PITCH,
                        center.y,
                        DOT_DIAMETER / 2.0,
                        WHITE,
                    );
                }
            }
            Field::Name("") => self.centered_text(
                canvas,
                NAME_PLACEHOLDER,
                center.x,
                baseline,
                FIELD_SIZE,
                WEIGHT_MEDIUM,
                PILL_WIDTH - 2.0 * PILL_PADDING,
                with_alpha(WHITE, PLACEHOLDER_ALPHA),
            ),
            Field::Name(text) => self.centered_text(
                canvas,
                text,
                center.x,
                baseline,
                FIELD_SIZE,
                WEIGHT_MEDIUM,
                PILL_WIDTH - 2.0 * PILL_PADDING,
                WHITE,
            ),
        }
        // one accessory at a time in the slot, as the design stacks them from the trailing edge
        if let Some(frame) = view.spinner {
            canvas.spinner(
                pill.x() + PILL_WIDTH - CAPS_SLOT_WIDTH / 2.0,
                center.y,
                SPINNER_SIZE,
                frame,
            );
        } else if view.caps_lock {
            canvas.caps_badge(pill.x() + PILL_WIDTH, center.y, caps_stretch(PILL_HEIGHT));
        }
        if let Some(notice) = view.notice {
            let max_width =
                NOTICE_MAX_WIDTH.min(f64::from(canvas.pixmap.width()) / canvas.scale - 40.0);
            self.centered_text(
                canvas,
                notice,
                layout.center_x,
                layout.notice_baseline,
                FIELD_SIZE,
                WEIGHT_MEDIUM,
                max_width,
                with_alpha(WHITE, NOTICE_ALPHA),
            );
        }
    }

    /// `text` starting at `left` (points), shortened with an ellipsis to fit `max_width`.
    #[allow(clippy::too_many_arguments)]
    fn left_text(
        &mut self,
        canvas: &mut Canvas,
        text: &str,
        left: f64,
        baseline: f64,
        size: f64,
        weight: u16,
        max_width: f64,
        color: Rgba,
    ) {
        let scale = canvas.scale as f32;
        let weight = self.text.effective_weight(weight);
        let size_px = size as f32 * scale;
        self.text.set_scale(scale);
        let (fitted, _) = self.text.fit(
            text,
            size_px,
            weight,
            0.0,
            (max_width * canvas.scale) as f32,
        );
        self.text.draw(
            canvas.pixmap,
            &fitted,
            (left * canvas.scale) as f32,
            (baseline * canvas.scale) as f32,
            size_px,
            weight,
            0.0,
            color,
        );
    }

    /// `text` broken at spaces into lines of at most `max_width` points (a single longer word stays on a line of its own),
    /// measured at one device pixel per point so that the line count does not depend on the output's scale.
    fn wrap(&mut self, text: &str, size: f64, weight: u16, max_width: f64) -> Vec<String> {
        self.text.set_scale(1.0);
        let measured = self.text.effective_weight(weight);
        let mut lines: Vec<String> = Vec::new();
        for word in text.split(' ') {
            match lines.last_mut() {
                Some(line)
                    if self
                        .text
                        .measure(&format!("{line} {word}"), size as f32, measured, 0.0)
                        <= max_width as f32 =>
                {
                    line.push(' ');
                    line.push_str(word);
                }
                _ => lines.push(word.to_owned()),
            }
        }
        lines
    }

    /// `text` wrapped to the width of `frame`, the first line's baseline `first_baseline` below the frame's top and each next
    /// `line_height` below (a line that still does not fit is shortened with an ellipsis).
    #[allow(clippy::too_many_arguments)]
    fn wrapped_text(
        &mut self,
        canvas: &mut Canvas,
        text: &str,
        frame: Rect,
        first_baseline: f64,
        line_height: f64,
        size: f64,
        weight: u16,
        color: Rgba,
    ) {
        for (i, line) in self
            .wrap(text, size, weight, frame.width())
            .iter()
            .enumerate()
        {
            self.left_text(
                canvas,
                line,
                frame.x(),
                frame.y() + first_baseline + i as f64 * line_height,
                size,
                weight,
                frame.width(),
                color,
            );
        }
    }

    /// `text` centred on `center_x` (points), shortened with an ellipsis to fit `max_width`.
    #[allow(clippy::too_many_arguments)]
    fn centered_text(
        &mut self,
        canvas: &mut Canvas,
        text: &str,
        center_x: f64,
        baseline: f64,
        size: f64,
        weight: u16,
        max_width: f64,
        color: Rgba,
    ) {
        let scale = canvas.scale as f32;
        let weight = self.text.effective_weight(weight);
        let size_px = size as f32 * scale;
        self.text.set_scale(scale);
        let (fitted, width_px) = self.text.fit(
            text,
            size_px,
            weight,
            0.0,
            (max_width * canvas.scale) as f32,
        );
        let x = (center_x * canvas.scale) as f32 - width_px / 2.0;
        self.text.draw(
            canvas.pixmap,
            &fitted,
            x,
            (baseline * canvas.scale) as f32,
            size_px,
            weight,
            0.0,
            color,
        );
    }
}

/// Vertical scale of the Caps Lock badge in a field `field_height` points high: 1.2 in the 28 pt pill.
fn caps_stretch(field_height: f64) -> f64 {
    (field_height / CAPS_STRETCH_REFERENCE).min(CAPS_STRETCH_MAX)
}

fn with_alpha(color: Rgba, alpha: f32) -> Rgba {
    [color[0], color[1], color[2], color[3] * alpha]
}

fn color(rgba: Rgba) -> Color {
    Color::from_rgba(rgba[0], rgba[1], rgba[2], rgba[3]).expect("components are within 0 to 1")
}

fn paint(shader: tiny_skia::Shader<'_>, blend_mode: BlendMode) -> Paint<'_> {
    Paint {
        shader,
        blend_mode,
        anti_alias: true,
        ..Paint::default()
    }
}

fn solid(rgba: Rgba) -> Paint<'static> {
    paint(
        tiny_skia::Shader::SolidColor(color(rgba)),
        BlendMode::SourceOver,
    )
}

/// The procedural wallpaper with the lock screen's treatment on top: a darkening, a lightening and a
/// vignette that darkens toward the corners, each drawn over the one before.
fn wallpaper(width: u32, height: u32) -> Pixmap {
    let mut pixmap = Pixmap::new(width, height).expect("the surface has a size");
    let (w, h) = (width as f32, height as f32);
    let whole = tiny_skia::Rect::from_xywh(0.0, 0.0, w, h).expect("the surface has a size");
    let rgb = |c: [f32; 3], alpha: f32| {
        Color::from_rgba(c[0], c[1], c[2], alpha).expect("components are within 0 to 1")
    };

    // diagonal gradient: 0.45 u + 0.55 v in the unit square
    let (a, b) = (0.45f32, 0.55f32);
    let end = Point::from_xy(a / (a * a + b * b), b / (a * a + b * b));
    let stops = vec![
        GradientStop::new(0.0, rgb(WALLPAPER_BASE[0], 1.0)),
        GradientStop::new(1.0, rgb(WALLPAPER_BASE[1], 1.0)),
    ];
    let base = LinearGradient::new(
        Point::from_xy(0.0, 0.0),
        end,
        stops,
        SpreadMode::Pad,
        Transform::from_scale(w, h),
    )
    .expect("the gradient has distinct ends");
    pixmap.fill_rect(
        whole,
        &paint(base, BlendMode::Source),
        Transform::identity(),
        None,
    );

    for (center, k, blob_color) in WALLPAPER_BLOBS {
        // sample exp(-k r^2) until it is nearly gone; r is in heights
        let reach = (50.0f32.ln() / k).sqrt();
        let stops = (0..=6)
            .map(|i| {
                let t = i as f32 / 6.0;
                GradientStop::new(
                    t,
                    rgb(blob_color, BLOB_STRENGTH * (-k * (t * reach).powi(2)).exp()),
                )
            })
            .collect();
        let middle = Point::from_xy(center[0] * w, center[1] * h);
        let shader = RadialGradient::new(
            middle,
            middle,
            reach * h,
            stops,
            SpreadMode::Pad,
            Transform::identity(),
        )
        .expect("the blob has a radius");
        pixmap.fill_rect(
            whole,
            &paint(shader, BlendMode::SourceOver),
            Transform::identity(),
            None,
        );
    }

    for alpha in [DARKEN, LIGHTEN] {
        pixmap.fill_rect(
            whole,
            &solid([0.0, 0.0, 0.0, alpha]),
            Transform::identity(),
            None,
        );
    }
    let middle = Point::from_xy(w / 2.0, h / 2.0);
    let stops = vec![
        GradientStop::new(0.0, rgb([0.0; 3], VIGNETTE)),
        GradientStop::new(1.0, rgb([0.0; 3], 0.0)),
    ];
    let vignette = RadialGradient::new(
        middle,
        middle,
        w.hypot(h) / 2.0,
        stops,
        SpreadMode::Pad,
        Transform::identity(),
    )
    .expect("the vignette has a radius");
    pixmap.fill_rect(
        whole,
        &paint(vignette, BlendMode::Darken),
        Transform::identity(),
        None,
    );
    pixmap
}

/// Shape drawing in points over a pixmap at `scale` device pixels per point.
struct Canvas<'a> {
    pixmap: &'a mut Pixmap,
    scale: f64,
}

impl Canvas<'_> {
    fn px(&self, points: f64) -> f32 {
        (points * self.scale) as f32
    }

    fn circle(&mut self, x: f64, y: f64, radius: f64, rgba: Rgba) {
        let path = PathBuilder::from_circle(self.px(x), self.px(y), self.px(radius))
            .expect("a circle has a radius");
        self.pixmap.fill_path(
            &path,
            &solid(rgba),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }

    fn ring(&mut self, x: f64, y: f64, radius: f64, width: f64, rgba: Rgba) {
        let path = PathBuilder::from_circle(self.px(x), self.px(y), self.px(radius))
            .expect("a circle has a radius");
        let stroke = Stroke {
            width: self.px(width),
            ..Stroke::default()
        };
        self.pixmap
            .stroke_path(&path, &solid(rgba), &stroke, Transform::identity(), None);
    }

    /// Black fading out over `extent` points beyond a circle of `radius`.
    fn halo(&mut self, x: f64, y: f64, radius: f64, extent: f64, opacity: f32) {
        let outer = radius + extent;
        let middle = Point::from_xy(self.px(x), self.px(y));
        let stops = vec![
            GradientStop::new(0.0, color([0.0, 0.0, 0.0, opacity])),
            GradientStop::new((radius / outer) as f32, color([0.0, 0.0, 0.0, opacity])),
            GradientStop::new(1.0, color([0.0, 0.0, 0.0, 0.0])),
        ];
        let shader = RadialGradient::new(
            middle,
            middle,
            self.px(outer),
            stops,
            SpreadMode::Pad,
            Transform::identity(),
        )
        .expect("the halo has a radius");
        let path = PathBuilder::from_circle(middle.x, middle.y, self.px(outer))
            .expect("a circle has a radius");
        self.pixmap.fill_path(
            &path,
            &paint(shader, BlendMode::SourceOver),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }

    /// `picture` cropped to the middle square, scaled into the circle.
    fn picture(&mut self, picture: &Pixmap, x: f64, y: f64, radius: f64) {
        let side = picture.width().min(picture.height()) as f32;
        let k = self.px(radius * 2.0) / side;
        let left = self.px(x - radius) - (picture.width() as f32 - side) / 2.0 * k;
        let top = self.px(y - radius) - (picture.height() as f32 - side) / 2.0 * k;
        let shader = Pattern::new(
            picture.as_ref(),
            SpreadMode::Pad,
            FilterQuality::Bilinear,
            1.0,
            Transform::from_scale(k, k).post_translate(left, top),
        );
        let path = PathBuilder::from_circle(self.px(x), self.px(y), self.px(radius))
            .expect("a circle has a radius");
        self.pixmap.fill_path(
            &path,
            &paint(shader, BlendMode::SourceOver),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }

    /// A rectangle with corners of `radius`.
    fn rounded(&mut self, rect: Rect, radius: f64, rgba: Rgba) {
        let path = rounded_path(
            rect,
            Radii::uniform(radius),
            CornerStyle::Circular,
            self.scale,
        )
        .expect("the rectangle has a size");
        self.pixmap.fill_path(
            &path,
            &solid(rgba),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }

    /// A line of [`RING_WIDTH`] along the edge of a rectangle with corners of `radius`.
    fn rounded_edge(&mut self, rect: Rect, radius: f64, rgba: Rgba) {
        self.outline(rect, radius, RING_WIDTH, rgba);
    }

    /// A line `width` points wide along the edge of a rectangle with corners of `radius`.
    fn outline(&mut self, rect: Rect, radius: f64, width: f64, rgba: Rgba) {
        let path = rounded_path(
            rect,
            Radii::uniform(radius),
            CornerStyle::Circular,
            self.scale,
        )
        .expect("the rectangle has a size");
        let stroke = Stroke {
            width: self.px(width),
            ..Stroke::default()
        };
        self.pixmap
            .stroke_path(&path, &solid(rgba), &stroke, Transform::identity(), None);
    }

    /// The power glyph centred on (`x`, `y`): a ring open at the top with a bar through the gap, white, round caps.
    /// The mark of a user with a session, centred on `(x, y)`: a disc with a check.
    fn logged_in_mark(&mut self, x: f64, y: f64) {
        self.circle(x, y, LOGGED_IN_RADIUS, LOGGED_IN_FILL);
        let mut check = PathBuilder::new();
        check.move_to(
            self.px(x - LOGGED_IN_RADIUS * 0.45),
            self.px(y + LOGGED_IN_RADIUS * 0.02),
        );
        check.line_to(
            self.px(x - LOGGED_IN_RADIUS * 0.1),
            self.px(y + LOGGED_IN_RADIUS * 0.38),
        );
        check.line_to(
            self.px(x + LOGGED_IN_RADIUS * 0.5),
            self.px(y - LOGGED_IN_RADIUS * 0.35),
        );
        let path = check.finish().expect("the check has a length");
        let stroke = Stroke {
            width: self.px(1.8),
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        self.pixmap
            .stroke_path(&path, &solid(WHITE), &stroke, Transform::identity(), None);
    }

    fn power_glyph(&mut self, x: f64, y: f64) {
        let mut ring = PathBuilder::new();
        // from just right of the top clockwise to just left of it, in steps of 10 degrees
        for step in 0..=POWER_RING_STEPS {
            let angle = (POWER_GAP_DEGREES / 2.0
                + step as f64 * (360.0 - POWER_GAP_DEGREES) / POWER_RING_STEPS as f64)
                .to_radians();
            let (px, py) = (
                self.px(x + POWER_RADIUS * angle.sin()),
                self.px(y - POWER_RADIUS * angle.cos()),
            );
            if step == 0 {
                ring.move_to(px, py);
            } else {
                ring.line_to(px, py);
            }
        }
        ring.move_to(self.px(x), self.px(y - POWER_RADIUS - 1.0));
        ring.line_to(self.px(x), self.px(y - 1.0));
        let path = ring.finish().expect("the glyph has a length");
        let stroke = Stroke {
            width: self.px(POWER_STROKE),
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        self.pixmap
            .stroke_path(&path, &solid(WHITE), &stroke, Transform::identity(), None);
    }

    /// The Caps Lock badge in white, centred in the slot that ends at `slot_right`, with the glyph cut out.
    /// `stretch` is the vertical scale; the corners and the glyph are drawn in badge units.
    fn caps_badge(&mut self, slot_right: f64, center_y: f64, stretch: f64) {
        let (width, height) = (CAPS_BADGE_UNITS, CAPS_BADGE_UNITS * stretch);
        let (left, top) = (
            slot_right - CAPS_SLOT_WIDTH / 2.0 - width / 2.0,
            center_y - height / 2.0,
        );
        let mut path = RoundedRect::with_radii(
            Rect::new(left, top, width, height),
            Radii::uniform(CAPS_BADGE_RADIUS),
            CornerStyle::Circular,
        )
        .path();
        for polygon in CAPS_GLYPH {
            for (i, &(x, y)) in polygon.iter().enumerate() {
                let point = GeoPoint::new(left + x, top + y * stretch);
                path.segments.push(if i == 0 {
                    PathSeg::MoveTo(point)
                } else {
                    PathSeg::LineTo(point)
                });
            }
            path.segments.push(PathSeg::Close);
        }
        let path = to_skia_path(&path, self.scale).expect("the badge has a size");
        // even-odd: the glyph polygons lie inside the body, so they are holes
        self.pixmap.fill_path(
            &path,
            &solid(WHITE),
            FillRule::EvenOdd,
            Transform::identity(),
            None,
        );
    }

    /// The spinner's `frame` in a square of `size` points centred on (`x`, `y`): twelve white spokes, each with the
    /// opacity of [`spinner::opacity`]. Spokes have round ends at both sides (approximate: the design has a round
    /// outer cap and a flat inner one; at this size the difference is below a pixel).
    fn spinner(&mut self, x: f64, y: f64, size: f64, frame: u32) {
        let unit = size / spinner::CANVAS;
        let half_width = spinner::SPOKE_WIDTH / 2.0;
        for spoke in 0..spinner::SPOKES {
            // clockwise from 12 o'clock
            let angle = spoke as f64 * 2.0 * PI / spinner::SPOKES as f64;
            let (sin, cos) = angle.sin_cos();
            let at = |radius: f64| {
                (
                    self.px(x + sin * radius * unit),
                    self.px(y - cos * radius * unit),
                )
            };
            let (from, to) = (
                at(spinner::INNER_RADIUS + half_width),
                at(spinner::OUTER_RADIUS - half_width),
            );
            let mut path = PathBuilder::new();
            path.move_to(from.0, from.1);
            path.line_to(to.0, to.1);
            let path = path.finish().expect("a spoke has a length");
            let stroke = Stroke {
                width: self.px(spinner::SPOKE_WIDTH * unit),
                line_cap: LineCap::Round,
                ..Stroke::default()
            };
            self.pixmap.stroke_path(
                &path,
                &solid(with_alpha(WHITE, spinner::opacity(frame, spoke))),
                &stroke,
                Transform::identity(),
                None,
            );
        }
    }

    /// A rectangle with fully rounded short ends.
    fn pill(&mut self, rect: Rect, rgba: Rgba) {
        let path = rounded_path(
            rect,
            Radii::uniform(rect.height() / 2.0),
            CornerStyle::Circular,
            self.scale,
        )
        .expect("the pill has a size");
        self.pixmap.fill_path(
            &path,
            &solid(rgba),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha(pixmap: &Pixmap, x: u32, y: u32) -> u8 {
        pixmap.pixel(x, y).expect("inside the pixmap").alpha()
    }

    #[test]
    fn the_caps_badge_is_a_white_rounded_square_with_the_glyph_cut_out() {
        assert_eq!(caps_stretch(PILL_HEIGHT), 1.2);
        let mut pixmap = Pixmap::new(40, 40).unwrap();
        // the 16 x 19.2 pt badge of a 28 pt field centred in a slot that ends at x 30: x 13 to 29, y 10.4 to 29.6
        Canvas {
            pixmap: &mut pixmap,
            scale: 1.0,
        }
        .caps_badge(30.0, 20.0, caps_stretch(PILL_HEIGHT));
        // the body, the middle of the arrow's head, a point left of the badge, and the rounded-off corner
        assert_eq!(
            (
                alpha(&pixmap, 14, 20),
                alpha(&pixmap, 21, 19),
                alpha(&pixmap, 5, 20),
                alpha(&pixmap, 13, 10)
            ),
            (255, 0, 0, 0)
        );
    }

    #[test]
    fn the_spinner_draws_the_head_spoke_opaque_and_the_tail_fainter() {
        let mut pixmap = Pixmap::new(41, 41).unwrap();
        // 32 pt square (half the 64 unit canvas) centred in the pixel (20, 20); frame 0 has its head on spoke 11
        Canvas {
            pixmap: &mut pixmap,
            scale: 1.0,
        }
        .spinner(20.5, 20.5, 32.0, 0);
        // the head (330 degrees), the spoke at 12 o'clock (eleven steps behind: 0.161) and the one at 6 o'clock (0.600), and the hub
        let near =
            |x: u32, y: u32, expected: i32| (i32::from(alpha(&pixmap, x, y)) - expected).abs() <= 3;
        assert!(near(15, 12, 255), "head {}", alpha(&pixmap, 15, 12));
        assert!(
            near(20, 10, 41),
            "twelve o'clock {}",
            alpha(&pixmap, 20, 10)
        );
        assert!(near(20, 30, 153), "six o'clock {}", alpha(&pixmap, 20, 30));
        assert_eq!(alpha(&pixmap, 20, 20), 0);
    }
}
