//! Drawing of the bar content: tile icons, running indicators and separators, the switcher's
//! panel, the application grid, the overview's chrome and the context menus. The material below
//! each surface is the server's backdrop, so nothing here fills a panel itself.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use cupertino_assets::{AssetError, Assets, FontRole};
use cupertino_decorations::shape::rounded_path;
use cupertino_decorations::text::TextRenderer;
use cupertino_geometry::{CornerStyle, Radii, Rect as GRect};
use cupertino_icons::{SystemIcons, ThemeIcons};
use cupertino_shell_client::{DockWindowIcon, WindowIconImage};
use cupertino_theme::Typography;
use cupertino_wm::geometry::{Point, Rect};
use tiny_skia::{Color, FillRule, FilterQuality, Paint, Pixmap, PixmapPaint, Stroke, Transform};

use crate::app_grid::{AppGrid, GridLayout, Look, PER_PAGE};
use crate::config::Orientation;
use crate::desktop::DesktopIndex;
use crate::dock::{DockLayout, DraggedTile, PlacedTile};
use crate::menu::{self, MenuItem, MenuLayout};
use crate::overview_ui::OverviewUi;
use crate::session::Session;
use crate::switcher::{Switcher, SwitcherLayout};
use crate::tiles::TileKind;

/// Diameter of the running indicator in points.
const INDICATOR: f64 = 4.0;

/// Semantic names of the generic application icon and of the trash's two states.
const GENERIC_APPLICATION_ICON: &str = "application.generic";
const TRASH_EMPTY_ICON: &str = "trash.empty";
const TRASH_FULL_ICON: &str = "trash.full";

/// How long the trash's state is trusted before the folder is looked at again.
const TRASH_CHECK_EVERY: Duration = Duration::from_secs(1);

/// Whether the home trash holds anything: `$XDG_DATA_HOME/Trash/files` (`~/.local/share` by default) has an entry.
fn trash_is_full() -> bool {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));
    data_home.is_some_and(|d| {
        std::fs::read_dir(d.join("Trash/files")).is_ok_and(|mut entries| entries.next().is_some())
    })
}

/// Icon pixmaps by name and pixel size, loaded once: an application's own icon (an absolute path or the `hicolor`
/// theme) and the system's icons from the asset directory; and the icons windows set for themselves.
pub struct Icons {
    apps: ThemeIcons,
    system: SystemIcons,
    cache: HashMap<(String, u32), Option<Pixmap>>,
    /// Windows' own icons by window id.
    windows: BTreeMap<u32, WindowIcon>,
    trash: Option<(Instant, bool)>,
}

/// The icon a window set for itself: a theme name, an image, or both.
struct WindowIcon {
    /// The window's application id and the dock's key for it (its tile's).
    apps: [String; 2],
    name: Option<String>,
    image: Option<Pixmap>,
    /// The image resampled to each smaller size it is drawn at.
    scaled: BTreeMap<u32, Pixmap>,
}

impl WindowIcon {
    /// The image at `px`: resampled once when it is larger, as it is when not.
    fn at(&mut self, px: u32) -> &Pixmap {
        let image = self.image.as_ref().expect("has an image");
        if image.width() <= px {
            return image;
        }
        self.scaled
            .entry(px)
            .or_insert_with(|| resampled(image, px))
    }
}

/// Where an application's icon comes from.
enum IconSource {
    Theme(String),
    Window(u32),
    Generic,
}

/// A window's ARGB8888 image (premultiplied, little-endian) as a pixmap; a colour channel over its alpha is cut to it.
fn window_pixmap(image: &WindowIconImage) -> Option<Pixmap> {
    let rgba = image
        .argb
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[b, g, r, a]| [r.min(a), g.min(a), b.min(a), a])
        .collect();
    Pixmap::from_vec(rgba, tiny_skia::IntSize::from_wh(image.size, image.size)?)
}

/// A square pixmap at half its side, each pixel the mean of the (up to) four it covers.
fn halved(image: &Pixmap) -> Pixmap {
    let side = image.width();
    let half = side.div_ceil(2);
    let src = image.data();
    let mut out = Pixmap::new(half, half).expect("a non-empty side");
    let at = |x: u32, y: u32| (y.min(side - 1) * side + x.min(side - 1)) as usize * 4;
    for (i, px) in out.data_mut().as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (x, y) = (2 * (i as u32 % half), 2 * (i as u32 / half));
        let corners = [at(x, y), at(x + 1, y), at(x, y + 1), at(x + 1, y + 1)];
        for (c, v) in px.iter_mut().enumerate() {
            let sum: u32 = corners.iter().map(|&o| u32::from(src[o + c])).sum();
            *v = ((sum + 2) / 4) as u8;
        }
    }
    out
}

/// A square image larger than `px` at `px` a side: halved while it is at least twice as large, then bilinear.
fn resampled(image: &Pixmap, px: u32) -> Pixmap {
    let px = px.max(1);
    let mut image = image.clone();
    while image.width() >= 2 * px {
        image = halved(&image);
    }
    if image.width() == px {
        return image;
    }
    let mut out = Pixmap::new(px, px).expect("a non-empty side");
    let fit = px as f32 / image.width() as f32;
    out.draw_pixmap(
        0,
        0,
        image.as_ref(),
        &PixmapPaint {
            quality: FilterQuality::Bilinear,
            ..PixmapPaint::default()
        },
        Transform::from_scale(fit, fit),
        None,
    );
    out
}

impl Icons {
    /// The generic application icon and the trash's are reported at startup when no asset pack provides them; they
    /// are then drawn as placeholders.
    pub fn new(assets: &Assets) -> Self {
        let system = SystemIcons::required(
            assets,
            &[GENERIC_APPLICATION_ICON, TRASH_EMPTY_ICON, TRASH_FULL_ICON],
            &[],
        );
        Self {
            apps: ThemeIcons::new(assets),
            system,
            cache: HashMap::new(),
            windows: BTreeMap::new(),
            trash: None,
        }
    }

    /// A window set its own icon, or removed it; `apps` are its application id and the dock's key for it.
    pub fn window_icon(&mut self, apps: [String; 2], icon: DockWindowIcon) {
        let image = icon.image.as_ref().and_then(window_pixmap);
        if icon.name.is_none() && image.is_none() {
            self.windows.remove(&icon.id);
        } else {
            self.windows.insert(
                icon.id,
                WindowIcon {
                    apps,
                    name: icon.name,
                    image,
                    scaled: BTreeMap::new(),
                },
            );
        }
    }

    pub fn window_closed(&mut self, id: u32) {
        self.windows.remove(&id);
    }

    /// Whether the theme has icon `name` at `px`, loading it once.
    fn has_themed(&mut self, name: &str, px: u32) -> bool {
        let apps = &mut self.apps;
        self.cache
            .entry((name.to_owned(), px))
            .or_insert_with(|| apps.load(name, px))
            .is_some()
    }

    /// The theme's icon `name`, else the asset `application.generic`.
    fn named(&mut self, name: &str, px: u32) -> &Pixmap {
        if self.has_themed(name, px) {
            self.cache[&(name.to_owned(), px)].as_ref().expect("loaded")
        } else {
            self.system.raster(GENERIC_APPLICATION_ICON, px)
        }
    }

    fn source(&mut self, desktop: &DesktopIndex, app_id: &str, px: u32) -> IconSource {
        if let Some(name) = desktop.entry_icon(app_id) {
            if self.has_themed(name, px) {
                return IconSource::Theme(name.to_owned());
            }
        }
        let window = self
            .windows
            .iter()
            .find(|(_, w)| w.apps.iter().any(|a| a == app_id))
            .map(|(id, w)| (*id, w.name.clone(), w.image.is_some()));
        if let Some((id, name, image)) = window {
            if let Some(name) = name.filter(|n| self.has_themed(n, px)) {
                return IconSource::Theme(name);
            }
            if image {
                return IconSource::Window(id);
            }
        }
        if self.has_themed(app_id, px) {
            return IconSource::Theme(app_id.to_owned());
        }
        IconSource::Generic
    }

    /// The icon of an application: its desktop entry's, else the one its window set for itself (a theme name, then
    /// its image), else the theme's icon named as the application, else the asset `application.generic`.
    fn app(&mut self, desktop: &DesktopIndex, app_id: &str, px: u32) -> &Pixmap {
        match self.source(desktop, app_id, px) {
            IconSource::Theme(name) => self.cache[&(name, px)].as_ref().expect("loaded"),
            IconSource::Window(id) => self.windows.get_mut(&id).expect("listed").at(px),
            IconSource::Generic => self.system.raster(GENERIC_APPLICATION_ICON, px),
        }
    }

    /// The trash's icon, empty or full as the trash folder is.
    fn trash(&mut self, px: u32) -> &Pixmap {
        let now = cupertino_shell_client::test_clock::display_now();
        let full = match self.trash {
            Some((at, full)) if now.duration_since(at) < TRASH_CHECK_EVERY => full,
            _ => {
                let full = trash_is_full();
                self.trash = Some((now, full));
                full
            }
        };
        self.system.raster(
            if full {
                TRASH_FULL_ICON
            } else {
                TRASH_EMPTY_ICON
            },
            px,
        )
    }
}

fn paint_of(color: [f32; 4]) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color(
        Color::from_rgba(color[0], color[1], color[2], color[3])
            .expect("colour components are within 0 to 1"),
    );
    paint.anti_alias = true;
    paint
}

fn gr(r: &Rect) -> GRect {
    GRect::new(r.x, r.y, r.w, r.h)
}

fn fill_rounded(pixmap: &mut Pixmap, rect: &Rect, radius: f64, color: [f32; 4], scale: f32) {
    if let Some(path) = rounded_path(
        gr(rect),
        Radii::uniform(radius),
        CornerStyle::continuous(),
        f64::from(scale),
    ) {
        pixmap.fill_path(
            &path,
            &paint_of(color),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

fn stroke_rounded(pixmap: &mut Pixmap, rect: &Rect, radius: f64, color: [f32; 4], scale: f32) {
    if let Some(path) = rounded_path(
        gr(rect),
        Radii::uniform(radius),
        CornerStyle::continuous(),
        f64::from(scale),
    ) {
        pixmap.stroke_path(
            &path,
            &paint_of(color),
            &Stroke {
                width: scale,
                ..Stroke::default()
            },
            Transform::identity(),
            None,
        );
    }
}

/// An icon centred in `rect`, as large as fits.
fn draw_icon(pixmap: &mut Pixmap, icon: &Pixmap, rect: &Rect, opacity: f32, scale: f32) {
    let (w, h) = (f64::from(scale) * rect.w, f64::from(scale) * rect.h);
    let fit = (w / f64::from(icon.width())).min(h / f64::from(icon.height())) as f32;
    let x =
        (f64::from(scale) * rect.x + (w - f64::from(icon.width()) * f64::from(fit)) / 2.0) as f32;
    let y =
        (f64::from(scale) * rect.y + (h - f64::from(icon.height()) * f64::from(fit)) / 2.0) as f32;
    let paint = PixmapPaint {
        opacity,
        quality: FilterQuality::Bilinear,
        ..PixmapPaint::default()
    };
    pixmap.draw_pixmap(
        0,
        0,
        icon.as_ref(),
        &paint,
        Transform::from_row(fit, 0.0, 0.0, fit, x, y),
        None,
    );
}

/// The one-point line that separates two sections, across the bar.
fn draw_separator(
    pixmap: &mut Pixmap,
    tile: &PlacedTile,
    orientation: Orientation,
    dark: bool,
    scale: f32,
) {
    let color = if dark {
        [1.0, 1.0, 1.0, 0.25]
    } else {
        [0.0, 0.0, 0.0, 0.2]
    };
    let r = &tile.rect;
    let line = if orientation.is_horizontal() {
        Rect::new(r.cx() - 0.5, r.y + r.h * 0.15, 1.0, r.h * 0.7)
    } else {
        Rect::new(r.x + r.w * 0.15, r.cy() - 0.5, r.w * 0.7, 1.0)
    };
    fill_rounded(pixmap, &line, 0.5, color, scale);
}

/// The running indicator on the screen-edge side of the tile's resting position.
fn draw_indicator(
    pixmap: &mut Pixmap,
    tile: &PlacedTile,
    orientation: Orientation,
    dark: bool,
    scale: f32,
    alpha: f32,
) {
    let base = if dark {
        [1.0, 1.0, 1.0, 0.85]
    } else {
        [0.1, 0.1, 0.12, 0.8]
    };
    let color = [base[0], base[1], base[2], base[3] * alpha];
    let r = &tile.rect;
    let dot = match orientation {
        Orientation::Bottom => Rect::new(
            r.cx() - INDICATOR / 2.0,
            r.bottom() + 1.0,
            INDICATOR,
            INDICATOR,
        ),
        Orientation::Left => Rect::new(
            r.x - 1.0 - INDICATOR,
            r.cy() - INDICATOR / 2.0,
            INDICATOR,
            INDICATOR,
        ),
        Orientation::Right => Rect::new(
            r.right() + 1.0,
            r.cy() - INDICATOR / 2.0,
            INDICATOR,
            INDICATOR,
        ),
    };
    fill_rounded(pixmap, &dot, INDICATOR / 2.0, color, scale);
}

/// A tile's icon in `rect`: the application's own, else the generic application icon; the trash's.
fn draw_icon_of(
    pixmap: &mut Pixmap,
    scale: f32,
    session: &Session,
    icons: &mut Icons,
    (kind, key): (TileKind, &str),
    rect: &Rect,
    opacity: f32,
) {
    let cfg = &session.dock.cfg;
    let device = |size: f64| (size * f64::from(scale)).ceil().max(1.0) as u32;
    let drawn = device(rect.w.max(rect.h));
    // At the resting and the full magnified size the icon is taken at the size it is drawn at, so it is drawn 1:1
    // and straight edges stay on whole pixels; the sizes a magnification passes through share a few rasters.
    let mut icon_px = device(cfg.large_size.max(cfg.tile_size));
    if drawn == device(cfg.tile_size) || drawn == icon_px {
        icon_px = drawn;
    }
    while icon_px > 2 * drawn {
        icon_px = icon_px.div_ceil(2);
    }
    let icon = if kind == TileKind::Trash {
        icons.trash(icon_px)
    } else {
        icons.app(session.desktop(), key, icon_px)
    };
    draw_icon(pixmap, icon, rect, opacity, scale);
}

fn draw_tile(
    pixmap: &mut Pixmap,
    scale: f32,
    session: &Session,
    icons: &mut Icons,
    t: &PlacedTile,
    rect: &Rect,
    opacity: f32,
) {
    let Some(tile) = session.dock.tile(t.id) else {
        return;
    };
    draw_icon_of(
        pixmap,
        scale,
        session,
        icons,
        (t.kind, &tile.key),
        rect,
        opacity,
    );
}

/// Draw the tiles of `layout` (surface-local points) on a cleared pixmap at `scale` device pixels
/// per point. A tile whose application has no icon shows the generic application icon of the asset
/// directory. The tile held by the pointer is [`paint_dragged`]'s.
pub fn paint(
    pixmap: &mut Pixmap,
    scale: f32,
    session: &Session,
    layout: &DockLayout,
    icons: &mut Icons,
    dark: bool,
) {
    let cfg = &session.dock.cfg;
    let orientation = cfg.orientation;
    for t in layout.tiles.iter().filter(|t| t.presence > 0.01) {
        if t.kind == TileKind::Separator {
            draw_separator(pixmap, t, orientation, dark, scale);
            continue;
        }
        draw_tile(
            pixmap,
            scale,
            session,
            icons,
            t,
            &t.visible_rect(orientation),
            t.presence as f32,
        );
        if t.indicator {
            draw_indicator(pixmap, t, orientation, dark, scale, 1.0);
        }
    }
}

/// The label that shows above a dragged icon once dropping it would remove it: "Remove" at the
/// reference's 14 pt in a pill; the pill's look is approximate (the design
/// draws it with a backdrop material that is not known), as are its padding and its gap to the icon.
const REMOVE_LABEL: &str = "Remove";
const REMOVE_LABEL_PT: f64 = 14.0;
const REMOVE_LABEL_PAD_X: f64 = 10.0;
const REMOVE_LABEL_HEIGHT: f64 = 24.0;
const REMOVE_LABEL_GAP: f64 = 4.0;

fn remove_label_fill(dark: bool) -> [f32; 4] {
    if dark {
        [0.16, 0.16, 0.18, 0.88]
    } else {
        [0.96, 0.96, 0.97, 0.9]
    }
}

/// Draw the tile that is held, settling or fading (surface-local points shifted by `origin`, so the
/// pixmap may be the whole output) on a cleared pixmap at `scale` device pixels per point: its
/// icon at its opacity, the "Remove" label above it and the running indicator under it.
#[allow(clippy::too_many_arguments)]
pub fn paint_dragged(
    pixmap: &mut Pixmap,
    scale: f32,
    session: &Session,
    icons: &mut Icons,
    text: &mut LabelText,
    dragged: &DraggedTile,
    origin: Point,
    dark: bool,
) {
    let rect = dragged.tile.rect.translate(origin.x, origin.y);
    draw_icon_of(
        pixmap,
        scale,
        session,
        icons,
        (dragged.tile.kind, &dragged.key),
        &rect,
        dragged.opacity as f32,
    );
    if dragged.indicator > 0.0 {
        let mut dot = dragged.tile.clone();
        dot.rect = rect;
        draw_indicator(
            pixmap,
            &dot,
            session.dock.cfg.orientation,
            dark,
            scale,
            dragged.indicator as f32,
        );
    }
    if dragged.label > 0.0 {
        text.text.set_scale(scale);
        let width = text.width(REMOVE_LABEL, REMOVE_LABEL_PT) + 2.0 * REMOVE_LABEL_PAD_X;
        let pill = Rect::new(
            rect.cx() - width / 2.0,
            rect.y - REMOVE_LABEL_GAP - REMOVE_LABEL_HEIGHT,
            width,
            REMOVE_LABEL_HEIGHT,
        );
        let mut fill = remove_label_fill(dark);
        fill[3] *= dragged.label as f32;
        fill_rounded(pixmap, &pill, REMOVE_LABEL_HEIGHT / 2.0, fill, scale);
        let mut color = menu_text(dark);
        color[3] *= dragged.label as f32;
        draw_line(
            pixmap,
            text,
            REMOVE_LABEL,
            pill.x + REMOVE_LABEL_PAD_X,
            pill.cy(),
            REMOVE_LABEL_PT,
            400,
            color,
            scale,
        );
    }
}

/// Number of puffs in the removal effect, and the share of the tile size they spread to and start
/// and end at (all approximate, like the effect's timing).
const POOF_PUFFS: usize = 6;
const POOF_SPREAD: f64 = 0.6;
const POOF_RADIUS_FROM: f64 = 0.12;
const POOF_RADIUS_TO: f64 = 0.3;

/// The removal puff: a ring of soft circles that spread out and fade over `progress` 0 to 1, around
/// `centre`, for a tile of `size` points.
pub fn paint_poof(
    pixmap: &mut Pixmap,
    scale: f32,
    centre: Point,
    size: f64,
    progress: f64,
    dark: bool,
) {
    let tint = if dark {
        [0.85, 0.85, 0.88]
    } else {
        [0.62, 0.62, 0.66]
    };
    let alpha = (0.6 * (1.0 - progress)) as f32;
    let reach = size * POOF_SPREAD * progress;
    let radius = size * (POOF_RADIUS_FROM + (POOF_RADIUS_TO - POOF_RADIUS_FROM) * progress);
    for i in 0..POOF_PUFFS {
        let angle = std::f64::consts::TAU * i as f64 / POOF_PUFFS as f64;
        let (cx, cy) = (
            centre.x + reach * angle.cos(),
            centre.y + reach * angle.sin(),
        );
        fill_rounded(
            pixmap,
            &Rect::new(cx - radius, cy - radius, 2.0 * radius, 2.0 * radius),
            radius,
            [tint[0], tint[1], tint[2], alpha],
            scale,
        );
    }
}

/// Weight of the switcher's label (regular).
const LABEL_WEIGHT: u16 = 400;

/// Text drawing for the switcher's label: the `ui` font of the asset directory.
pub struct LabelText {
    text: TextRenderer,
    /// Cap height as a fraction of the font size, to centre a line on its rectangle.
    cap_height: f32,
}

impl LabelText {
    pub fn new(typography: &Typography, assets: &Assets) -> Result<Self, AssetError> {
        Ok(Self {
            text: TextRenderer::new(assets, FontRole::Ui)?,
            cap_height: (typography.metrics.cap_height / typography.metrics.units_per_em) as f32,
        })
    }

    /// Width of `line` at `pt` points (regular weight), in points. The optical size is that of the point size, as
    /// when the painters draw at any scale, so a layout measured here fits what they draw.
    pub fn width(&mut self, line: &str, pt: f64) -> f64 {
        self.text.set_scale(1.0);
        f64::from(self.text.measure(line, pt as f32, 400, 0.0))
    }
}

/// What the switcher's panel shows this frame.
pub struct Panel<'a> {
    pub switcher: &'a Switcher,
    /// The panel's layout on the output; the pixmap holds just the panel.
    pub layout: &'a SwitcherLayout,
    pub desktop: &'a DesktopIndex,
    pub dark: bool,
    /// Opacity of the whole panel (the fade-out).
    pub alpha: f64,
}

/// Multiply every pixel by `alpha` (0 to 1): the panel's fade, applied to the premultiplied pixels.
fn fade(pixmap: &mut Pixmap, alpha: f64) {
    if alpha >= 1.0 {
        return;
    }
    let table = fade_table(alpha);
    for byte in pixmap.data_mut() {
        *byte = table[usize::from(*byte)];
    }
}

/// [`fade`] of `src` written to `dst`, a pixmap of its size.
fn fade_into(dst: &mut Pixmap, src: &Pixmap, alpha: f64) {
    let table = fade_table(alpha);
    for (d, s) in dst.data_mut().iter_mut().zip(src.data()) {
        *d = table[usize::from(*s)];
    }
}

fn fade_table(alpha: f64) -> [u8; 256] {
    std::array::from_fn(|b| (b as f64 * alpha).round() as u8)
}

/// Draw the switcher's panel content on a cleared pixmap of the panel's size at `scale` device
/// pixels per point: the selection highlight, one icon per application (the generic application icon for one
/// without) and the selected application's name. The material
/// below is the server's backdrop.
pub fn paint_switcher(
    pixmap: &mut Pixmap,
    scale: f32,
    panel: &Panel<'_>,
    icons: &mut Icons,
    text: &mut LabelText,
) {
    text.text.set_scale(scale);
    let layout = panel.layout;
    let to_panel = |r: Rect| r.translate(-layout.panel.x, -layout.panel.y);
    let icon_px = (layout.tile * f64::from(scale)).ceil() as u32;
    let selected = panel.switcher.selected();
    fill_rounded(
        pixmap,
        &to_panel(layout.selection(selected)),
        SwitcherLayout::SELECTION_RADIUS,
        SwitcherLayout::selection_fill(panel.dark),
        scale,
    );
    for (i, app) in panel.switcher.apps().iter().enumerate() {
        let rect = to_panel(layout.item(i));
        draw_icon(
            pixmap,
            icons.app(panel.desktop, app, icon_px),
            &rect,
            1.0,
            scale,
        );
    }
    let app = &panel.switcher.apps()[selected];
    let name = panel
        .desktop
        .find(app)
        .map_or(app.as_str(), |e| e.name.as_str());
    let px = (SwitcherLayout::LABEL_FONT_PT * f64::from(scale)) as f32;
    let max_px =
        ((layout.panel.w - 2.0 * SwitcherLayout::INSET_X - SwitcherLayout::LABEL_EXTRA_WIDTH)
            * f64::from(scale)) as f32;
    let (fitted, width_px) = text.text.fit(name, px, LABEL_WEIGHT, 0.0, max_px);
    let label = to_panel(layout.label(selected, f64::from(width_px) / f64::from(scale)));
    let x = (label.x * f64::from(scale)) as f32;
    let baseline = (label.cy() * f64::from(scale)) as f32 + text.cap_height * px / 2.0;
    text.text.draw(
        pixmap,
        &fitted,
        x,
        baseline,
        px,
        LABEL_WEIGHT,
        0.0,
        SwitcherLayout::label_color(panel.dark),
    );
    fade(pixmap, panel.alpha);
}

/// What the application grid shows this frame.
pub struct GridView<'a> {
    pub grid: &'a AppGrid,
    /// The grid's layout on its layer; the pixmap is the whole layer.
    pub layout: &'a GridLayout,
    pub now: f64,
}

/// Grid colours.: labels white with a black shadow at alpha 0.2 one point
/// down (its 2 pt blur is not drawn), the current dot white, the others white at 0.2. approximate:
/// the keyboard selection (white at 0.2, radius 8; the design's colour is not known), the
/// search field's chrome (fill white 0.12, stroke white 0.3, radius 6, text inset 8) and its
/// placeholder at 0.5 ("fades while inactive"), "No Results" as white at the design's gray 0.204
/// (its plus-lighter compositing adds that much to a dark backdrop), and the stand-in square of an
/// application without an icon, in its key colour with the folder radius `trunc(icon × 0.15625).
const GRID_LABEL: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const GRID_LABEL_SHADOW: [f32; 4] = [0.0, 0.0, 0.0, 0.2];
const GRID_DOT_CURRENT: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const GRID_DOT_OTHER: [f32; 4] = [1.0, 1.0, 1.0, 0.2];
const GRID_SELECTION: [f32; 4] = [1.0, 1.0, 1.0, 0.2];
const GRID_SELECTION_RADIUS: f64 = 8.0;
const GRID_SEARCH_FILL: [f32; 4] = [1.0, 1.0, 1.0, 0.12];
const GRID_SEARCH_STROKE: [f32; 4] = [1.0, 1.0, 1.0, 0.3];
const GRID_SEARCH_RADIUS: f64 = 6.0;
const GRID_SEARCH_INSET: f64 = 8.0;
const GRID_PLACEHOLDER: [f32; 4] = [1.0, 1.0, 1.0, 0.5];
const GRID_PLACEHOLDER_TEXT: &str = "Search";
const GRID_NO_RESULTS: [f32; 4] = [1.0, 1.0, 1.0, 0.20392];
const GRID_NO_RESULTS_TEXT: &str = "No Results";

/// Draw the application grid on a cleared pixmap of its layer at `scale` device pixels per point:
/// the pages at their flip position with icons, labels and the keyboard selection, the page dots
/// and the search field, faded as the entry or exit stands. The background is the server's backdrop;
/// the entry's and exit's scale is the compositor's ([`AppGrid::visual`] through the presenter's zoom).
/// While the grid is faded the content is painted into `cache` and reused for as long as its look
/// holds, so a frame of the entry or exit costs one pass over the pixels.
pub fn paint_app_grid(
    pixmap: &mut Pixmap,
    scale: f32,
    view: &GridView<'_>,
    icons: &mut Icons,
    text: &mut LabelText,
    cache: &mut GridCache,
) {
    text.text.set_scale(scale);
    let alpha = view.grid.visual(view.now).alpha;
    if alpha >= 1.0 {
        *cache = GridCache::default();
        paint_grid_content(pixmap, scale, view, icons, text);
        return;
    }
    let key = view
        .grid
        .look()
        .map(|look| (look, scale.to_bits(), pixmap.width(), pixmap.height()));
    let content = match cache.content.take() {
        Some(content) if key.is_some() && cache.key == key => content,
        kept => {
            let mut content = kept
                .filter(|c| c.width() == pixmap.width() && c.height() == pixmap.height())
                .unwrap_or_else(|| {
                    Pixmap::new(pixmap.width(), pixmap.height())
                        .expect("the layer's pixmap has a size")
                });
            content.fill(Color::TRANSPARENT);
            paint_grid_content(&mut content, scale, view, icons, text);
            content
        }
    };
    fade_into(pixmap, &content, alpha);
    *cache = GridCache {
        key,
        content: Some(content),
    };
}

/// The grid's content behind its last faded frame, with the look and buffer it was painted for.
#[derive(Default)]
pub struct GridCache {
    key: Option<(Look, u32, u32, u32)>,
    content: Option<Pixmap>,
}

/// One line of text with its left edge at `x` points, vertically centred on `cy` points by its cap height.
#[allow(clippy::too_many_arguments)]
fn draw_line(
    pixmap: &mut Pixmap,
    text: &mut LabelText,
    line: &str,
    x: f64,
    cy: f64,
    pt: f64,
    weight: u16,
    color: [f32; 4],
    scale: f32,
) {
    let px = (pt * f64::from(scale)) as f32;
    let baseline = (cy * f64::from(scale)) as f32 + text.cap_height * px / 2.0;
    text.text.draw(
        pixmap,
        line,
        (x * f64::from(scale)) as f32,
        baseline,
        px,
        weight,
        0.0,
        color,
    );
}

fn paint_grid_content(
    pixmap: &mut Pixmap,
    scale: f32,
    view: &GridView<'_>,
    icons: &mut Icons,
    text: &mut LabelText,
) {
    let (grid, layout) = (view.grid, view.layout);
    let position = grid.page_position(view.now);
    let icon_px = (layout.icon * f64::from(scale)).ceil() as u32;
    let label_px = (GridLayout::LABEL_FONT_PT * f64::from(scale)) as f32;
    let label_max_px = ((layout.cell.0 - GridLayout::LABEL_CELL_INSET - GridLayout::LABEL_PAD)
        * f64::from(scale)) as f32;
    for page in 0..grid.page_count() {
        let frame = layout.page_frame(page, position);
        if frame.right() <= 0.0 || frame.x >= layout.width {
            continue;
        }
        let first = page * PER_PAGE;
        for slot in 0..PER_PAGE {
            let Some(app) = grid.result(first + slot) else {
                break;
            };
            let cell = layout.cell_rect(&frame, slot);
            if grid.selected() == Some(first + slot) {
                fill_rounded(
                    pixmap,
                    &layout.item_rect(&cell),
                    GRID_SELECTION_RADIUS,
                    GRID_SELECTION,
                    scale,
                );
            }
            let image = layout.image_rect(&cell);
            draw_icon(pixmap, icons.named(&app.icon, icon_px), &image, 1.0, scale);
            let (fitted, width_px) = text.text.fit(
                &app.name,
                label_px,
                GridLayout::LABEL_WEIGHT,
                0.0,
                label_max_px,
            );
            let label = layout.label_rect(&cell, f64::from(width_px) / f64::from(scale));
            let x = label.x + GridLayout::LABEL_PAD / 2.0;
            draw_line(
                pixmap,
                text,
                &fitted,
                x,
                label.cy() + 1.0,
                GridLayout::LABEL_FONT_PT,
                GridLayout::LABEL_WEIGHT,
                GRID_LABEL_SHADOW,
                scale,
            );
            draw_line(
                pixmap,
                text,
                &fitted,
                x,
                label.cy(),
                GridLayout::LABEL_FONT_PT,
                GridLayout::LABEL_WEIGHT,
                GRID_LABEL,
                scale,
            );
        }
    }
    if grid.result_count() == 0 && !grid.query().is_empty() {
        let px = (GridLayout::NO_RESULTS_FONT_PT * f64::from(scale)) as f32;
        let width =
            f64::from(
                text.text
                    .measure(GRID_NO_RESULTS_TEXT, px, GridLayout::SEARCH_WEIGHT, 0.0),
            ) / f64::from(scale);
        draw_line(
            pixmap,
            text,
            GRID_NO_RESULTS_TEXT,
            layout.page.cx() - width / 2.0,
            layout.page.cy(),
            GridLayout::NO_RESULTS_FONT_PT,
            GridLayout::SEARCH_WEIGHT,
            GRID_NO_RESULTS,
            scale,
        );
    }
    let count = grid.page_count();
    for i in 0..count {
        let color = if i == grid.page() {
            GRID_DOT_CURRENT
        } else {
            GRID_DOT_OTHER
        };
        fill_rounded(
            pixmap,
            &layout.dot_shape(i, count),
            GridLayout::DOT_SIZE / 2.0,
            color,
            scale,
        );
    }
    fill_rounded(
        pixmap,
        &layout.search,
        GRID_SEARCH_RADIUS,
        GRID_SEARCH_FILL,
        scale,
    );
    stroke_rounded(
        pixmap,
        &layout.search,
        GRID_SEARCH_RADIUS,
        GRID_SEARCH_STROKE,
        scale,
    );
    let (line, color) = if grid.query().is_empty() {
        (GRID_PLACEHOLDER_TEXT, GRID_PLACEHOLDER)
    } else {
        (grid.query(), GRID_LABEL)
    };
    let search_px = (GridLayout::SEARCH_FONT_PT * f64::from(scale)) as f32;
    let (fitted, _) = text.text.fit(
        line,
        search_px,
        GridLayout::SEARCH_WEIGHT,
        0.0,
        ((layout.search.w - 2.0 * GRID_SEARCH_INSET) * f64::from(scale)) as f32,
    );
    draw_line(
        pixmap,
        text,
        &fitted,
        layout.search.x + GRID_SEARCH_INSET,
        layout.search.cy(),
        GridLayout::SEARCH_FONT_PT,
        GridLayout::SEARCH_WEIGHT,
        color,
        scale,
    );
}

/// What the overview's surface shows this frame.
pub struct OverviewView<'a> {
    pub ui: &'a OverviewUi,
    pub now: f64,
    pub dark: bool,
}

/// Overview chrome (all approximate; the server draws the dim and the windows): workspace slots
/// as translucent rounded rectangles with a 2 pt border on the current one, "Desktop n" labels,
/// an add slot with a plus, a remove button on the hovered slot, a 3 pt outline round the
/// selected window and a 13 pt title under every window.
const OVERVIEW_SLOT_RADIUS: f64 = 8.0;
const OVERVIEW_SLOT_LIGHT: [f32; 4] = [0.0, 0.0, 0.0, 0.18];
const OVERVIEW_SLOT_DARK: [f32; 4] = [1.0, 1.0, 1.0, 0.16];
const OVERVIEW_CURRENT_BORDER: [f32; 4] = [1.0, 1.0, 1.0, 0.85];
const OVERVIEW_HIGHLIGHT: [f32; 4] = [1.0, 1.0, 1.0, 0.9];
const OVERVIEW_HIGHLIGHT_RADIUS: f64 = 14.0;
const OVERVIEW_REMOVE_FILL: [f32; 4] = [0.2, 0.2, 0.22, 0.95];
const OVERVIEW_GLYPH: [f32; 4] = [1.0, 1.0, 1.0, 0.95];
const OVERVIEW_LABEL_PT: f64 = 12.0;
const OVERVIEW_TITLE_PT: f64 = 13.0;
const OVERVIEW_TITLE_GAP: f64 = 6.0;
const OVERVIEW_LABEL_WEIGHT: u16 = 500;

/// A `width` pt wide line from `a` to `b`, in points.
fn draw_stroke(
    pixmap: &mut Pixmap,
    a: (f64, f64),
    b: (f64, f64),
    width: f64,
    color: [f32; 4],
    scale: f32,
) {
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(
        (a.0 * f64::from(scale)) as f32,
        (a.1 * f64::from(scale)) as f32,
    );
    pb.line_to(
        (b.0 * f64::from(scale)) as f32,
        (b.1 * f64::from(scale)) as f32,
    );
    if let Some(path) = pb.finish() {
        pixmap.stroke_path(
            &path,
            &paint_of(color),
            &Stroke {
                width: (width * f64::from(scale)) as f32,
                ..Stroke::default()
            },
            Transform::identity(),
            None,
        );
    }
}

/// A line of text centred on (`cx`, `cy`) points, truncated to `max_w` points.
#[allow(clippy::too_many_arguments)]
fn draw_centred(
    pixmap: &mut Pixmap,
    text: &mut LabelText,
    line: &str,
    cx: f64,
    cy: f64,
    max_w: f64,
    pt: f64,
    weight: u16,
    color: [f32; 4],
    scale: f32,
) {
    let px = (pt * f64::from(scale)) as f32;
    let (fitted, width_px) =
        text.text
            .fit(line, px, weight, 0.0, (max_w * f64::from(scale)) as f32);
    let w = f64::from(width_px) / f64::from(scale);
    draw_line(
        pixmap,
        text,
        &fitted,
        cx - w / 2.0,
        cy,
        pt,
        weight,
        color,
        scale,
    );
}

/// Draw the overview's chrome on a cleared pixmap covering the output at `scale` device pixels per
/// point: the workspaces bar, the highlight and the titles, faded as the close stands.
pub fn paint_overview(
    pixmap: &mut Pixmap,
    scale: f32,
    view: &OverviewView<'_>,
    text: &mut LabelText,
) {
    text.text.set_scale(scale);
    let (ui, now) = (view.ui, view.now);
    if let Some(bar) = ui.bar(now) {
        let fill = if view.dark {
            OVERVIEW_SLOT_DARK
        } else {
            OVERVIEW_SLOT_LIGHT
        };
        let drag = ui.drag();
        for (i, slot) in bar.slots.iter().enumerate() {
            let id = slot.id.0 as u32;
            let mut rect = slot.rect;
            if let Some(offset) = drag.filter(|d| d.0 == id).map(|d| d.1) {
                rect = rect.translate(offset.x, offset.y);
            }
            fill_rounded(pixmap, &rect, OVERVIEW_SLOT_RADIUS, fill, scale);
            if slot.current {
                stroke_rounded(
                    pixmap,
                    &rect.inset(1.0),
                    OVERVIEW_SLOT_RADIUS - 1.0,
                    OVERVIEW_CURRENT_BORDER,
                    scale,
                );
                stroke_rounded(
                    pixmap,
                    &rect.inset(2.0),
                    OVERVIEW_SLOT_RADIUS - 2.0,
                    OVERVIEW_CURRENT_BORDER,
                    scale,
                );
            }
            draw_centred(
                pixmap,
                text,
                &format!("Desktop {}", i + 1),
                rect.cx(),
                rect.cy(),
                rect.w - 8.0,
                OVERVIEW_LABEL_PT,
                OVERVIEW_LABEL_WEIGHT,
                OVERVIEW_GLYPH,
                scale,
            );
            if ui.hovered_slot() == Some(id) && drag.is_none() && bar.slots.len() > 1 {
                let button = OverviewUi::remove_button(&rect);
                fill_rounded(pixmap, &button, button.w / 2.0, OVERVIEW_REMOVE_FILL, scale);
                let (cx, cy, d) = (button.cx(), button.cy(), button.w * 0.22);
                draw_stroke(
                    pixmap,
                    (cx - d, cy - d),
                    (cx + d, cy + d),
                    1.5,
                    OVERVIEW_GLYPH,
                    scale,
                );
                draw_stroke(
                    pixmap,
                    (cx - d, cy + d),
                    (cx + d, cy - d),
                    1.5,
                    OVERVIEW_GLYPH,
                    scale,
                );
            }
        }
        if let Some(add) = bar.add {
            fill_rounded(pixmap, &add, OVERVIEW_SLOT_RADIUS, fill, scale);
            let (cx, cy, d) = (add.cx(), add.cy(), add.w.min(add.h) * 0.18);
            draw_stroke(
                pixmap,
                (cx - d, cy),
                (cx + d, cy),
                2.0,
                OVERVIEW_GLYPH,
                scale,
            );
            draw_stroke(
                pixmap,
                (cx, cy - d),
                (cx, cy + d),
                2.0,
                OVERVIEW_GLYPH,
                scale,
            );
        }
    }
    if let Some(h) = ui.highlight(now) {
        let color = [
            OVERVIEW_HIGHLIGHT[0],
            OVERVIEW_HIGHLIGHT[1],
            OVERVIEW_HIGHLIGHT[2],
            OVERVIEW_HIGHLIGHT[3] * h.opacity as f32,
        ];
        for inset in [0.0, 1.0, 2.0] {
            stroke_rounded(
                pixmap,
                &h.rect.inset(inset),
                OVERVIEW_HIGHLIGHT_RADIUS - inset,
                color,
                scale,
            );
        }
    }
    for w in ui.placed(now) {
        let shadow = [0.0, 0.0, 0.0, 0.2 * w.title_opacity as f32];
        let color = [1.0, 1.0, 1.0, w.title_opacity as f32];
        let title = ui_title(ui, w.id.0 as u32);
        let cy = w.frame.bottom() + OVERVIEW_TITLE_GAP + OVERVIEW_TITLE_PT / 2.0;
        draw_centred(
            pixmap,
            text,
            title,
            w.frame.cx(),
            cy + 1.0,
            w.frame.w,
            OVERVIEW_TITLE_PT,
            OVERVIEW_LABEL_WEIGHT,
            shadow,
            scale,
        );
        draw_centred(
            pixmap,
            text,
            title,
            w.frame.cx(),
            cy,
            w.frame.w,
            OVERVIEW_TITLE_PT,
            OVERVIEW_LABEL_WEIGHT,
            color,
            scale,
        );
    }
    fade(pixmap, ui.alpha(now));
}

fn ui_title(ui: &OverviewUi, id: u32) -> &str {
    ui.title(id).unwrap_or("")
}

/// Colours of a context menu, as the menu bar's popups (approximate).
fn menu_text(dark: bool) -> [f32; 4] {
    if dark {
        [0.97, 0.97, 0.98, 0.95]
    } else {
        [0.05, 0.05, 0.06, 0.92]
    }
}
const MENU_ROW_HIGHLIGHT: [f32; 4] = [0.04, 0.45, 1.0, 1.0];
const MENU_ROW_HIGHLIGHT_TEXT: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const MENU_DISABLED_ALPHA: f32 = 0.4;
fn menu_separator(dark: bool) -> [f32; 4] {
    if dark {
        [1.0, 1.0, 1.0, 0.16]
    } else {
        [0.0, 0.0, 0.0, 0.13]
    }
}

/// Draw a context menu on a cleared pixmap of the popup's size (the material is the server's
/// backdrop): the rows with the hovered one highlighted, separators, radio marks and labels.
pub fn paint_menu(
    pixmap: &mut Pixmap,
    scale: f32,
    items: &[MenuItem],
    layout: &MenuLayout,
    hovered: Option<usize>,
    dark: bool,
    text: &mut LabelText,
) {
    text.text.set_scale(scale);
    let origin = (layout.rect.x, layout.rect.y);
    for (i, (item, row)) in items.iter().zip(&layout.rows).enumerate() {
        let r = row.translate(-origin.0, -origin.1);
        if item.separator {
            let line = Rect::new(
                r.x + menu::TEXT_INSET - 4.0,
                r.cy() - 0.5,
                r.w - 2.0 * (menu::TEXT_INSET - 4.0),
                1.0,
            );
            fill_rounded(pixmap, &line, 0.0, menu_separator(dark), scale);
            continue;
        }
        let hot = hovered == Some(i) && item.enabled;
        if hot {
            fill_rounded(
                pixmap,
                &Rect::new(r.x + 4.0, r.y, r.w - 8.0, r.h),
                4.0,
                MENU_ROW_HIGHLIGHT,
                scale,
            );
        }
        let base = if hot {
            MENU_ROW_HIGHLIGHT_TEXT
        } else {
            menu_text(dark)
        };
        let alpha = if item.enabled {
            1.0
        } else {
            MENU_DISABLED_ALPHA
        };
        let color = [base[0], base[1], base[2], base[3] * alpha];
        if item.checked {
            let (cx, cy) = (r.x + menu::TEXT_INSET / 2.0, r.cy());
            fill_rounded(
                pixmap,
                &Rect::new(cx - 2.5, cy - 2.5, 5.0, 5.0),
                2.5,
                color,
                scale,
            );
        }
        draw_line(
            pixmap,
            text,
            &item.label,
            r.x + menu::TEXT_INSET,
            r.cy(),
            menu::FONT_PT,
            400,
            color,
            scale,
        );
    }
}

/// What the Force Quit Applications dialog shows this frame.
pub struct ForceQuitView<'a> {
    pub dialog: &'a crate::force_quit::Dialog,
    pub layout: &'a crate::force_quit::Layout,
    pub desktop: &'a DesktopIndex,
    pub dark: bool,
    /// The row under the pointer, and the button under it.
    pub hover_row: Option<usize>,
    pub hover_button: Option<crate::force_quit::Button>,
}

/// The dialog's colours (approximate, except the unresponsive name's red and the suffix's grey).
struct FqPalette {
    fill: [f32; 4],
    edge: [f32; 4],
    ink: [f32; 4],
    faint: [f32; 4],
    list: [f32; 4],
    name_not_responding: [f32; 4],
    suffix: [f32; 4],
}

const FQ_ACCENT: [f32; 4] = [0.04, 0.52, 1.0, 1.0];

impl FqPalette {
    fn new(dark: bool) -> Self {
        if dark {
            FqPalette {
                fill: [0.17, 0.17, 0.19, 1.0],
                edge: [1.0, 1.0, 1.0, 0.14],
                ink: [1.0, 1.0, 1.0, 1.0],
                faint: [1.0, 1.0, 1.0, 0.6],
                list: [1.0, 1.0, 1.0, 0.06],
                name_not_responding: [1.0, 0.35, 0.3, 1.0],
                suffix: [0.6, 0.6, 0.6, 1.0],
            }
        } else {
            FqPalette {
                fill: [0.97, 0.97, 0.98, 1.0],
                edge: [0.0, 0.0, 0.0, 0.14],
                ink: [0.0, 0.0, 0.0, 0.9],
                faint: [0.0, 0.0, 0.0, 0.55],
                list: [0.0, 0.0, 0.0, 0.05],
                name_not_responding: [0.609, 0.0, 0.0, 1.0],
                suffix: [0.6, 0.6, 0.6, 1.0],
            }
        }
    }
}

/// Draw `line` left-aligned at `x`, centred on `cy`, in points at `scale`; `max_w` cuts it with an ellipsis.
fn fq_text(
    pixmap: &mut Pixmap,
    text: &mut LabelText,
    scale: f32,
    line: &str,
    (x, cy, max_w): (f64, f64, f64),
    (pt, weight): (f64, u16),
    color: [f32; 4],
) -> f64 {
    let px = (pt * f64::from(scale)) as f32;
    let (fitted, width_px) =
        text.text
            .fit(line, px, weight, 0.0, (max_w * f64::from(scale)) as f32);
    let baseline = (cy * f64::from(scale)) as f32 + text.cap_height * px / 2.0;
    text.text.draw(
        pixmap,
        &fitted,
        (x * f64::from(scale)) as f32,
        baseline,
        px,
        weight,
        0.0,
        color,
    );
    f64::from(width_px) / f64::from(scale)
}

/// Words broken at spaces into lines of at most `max_w` points.
fn fq_wrap(text: &mut LabelText, line: &str, pt: f64, max_w: f64) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in line.split_whitespace() {
        match lines.last_mut() {
            Some(last) if text.width(&format!("{last} {word}"), pt) <= max_w => {
                last.push(' ');
                last.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    lines
}

/// Lines of `line` wrapped to `rect`'s width, `line_height` apart from the top of `rect`.
fn fq_paragraph(
    pixmap: &mut Pixmap,
    text: &mut LabelText,
    scale: f32,
    line: &str,
    rect: &Rect,
    (pt, weight, line_height): (f64, u16, f64),
    color: [f32; 4],
) {
    for (k, part) in fq_wrap(text, line, pt, rect.w).iter().enumerate() {
        let cy = rect.y + line_height / 2.0 + k as f64 * line_height;
        fq_text(
            pixmap,
            text,
            scale,
            part,
            (rect.x, cy, rect.w),
            (pt, weight),
            color,
        );
    }
}

fn fq_button(
    pixmap: &mut Pixmap,
    text: &mut LabelText,
    scale: f32,
    palette: &FqPalette,
    rect: &Rect,
    label: &str,
    (filled, hovered, enabled): (bool, bool, bool),
) {
    let base = if filled {
        FQ_ACCENT
    } else if palette.ink[0] > 0.5 {
        [1.0, 1.0, 1.0, 0.2]
    } else {
        [0.0, 0.0, 0.0, 0.1]
    };
    let alpha = match (enabled, hovered) {
        (false, _) => 0.4,
        (true, true) => 0.85,
        (true, false) => 1.0,
    };
    // the frame is the button's hit area; the visible button is narrower, so two side by side keep a gap
    let shown = Rect::new(rect.x + 3.0, rect.y, rect.w - 6.0, rect.h);
    fill_rounded(
        pixmap,
        &shown,
        7.0,
        [base[0], base[1], base[2], base[3] * alpha],
        scale,
    );
    let ink = palette.ink;
    let color = if filled {
        [1.0, 1.0, 1.0, alpha]
    } else {
        [ink[0], ink[1], ink[2], ink[3] * alpha]
    };
    let width = text.width(label, 13.0);
    fq_text(
        pixmap,
        text,
        scale,
        label,
        (rect.x + (rect.w - width) / 2.0, rect.cy(), rect.w),
        (13.0, 500),
        color,
    );
}

pub fn paint_force_quit(
    pixmap: &mut Pixmap,
    scale: f32,
    view: &ForceQuitView<'_>,
    icons: &mut Icons,
    text: &mut LabelText,
) {
    use crate::force_quit::{self as fq, Button, Layout};
    text.text.set_scale(scale);
    let (dialog, palette) = (view.dialog, FqPalette::new(view.dark));
    let (width, height) = view.layout.size();
    let body = Rect::new(0.0, 0.0, width, height);
    fill_rounded(pixmap, &body, fq::RADIUS, palette.fill, scale);
    stroke_rounded(pixmap, &body.inset(0.5), fq::RADIUS, palette.edge, scale);
    let button = |pixmap: &mut Pixmap,
                  text: &mut LabelText,
                  rect: &Rect,
                  label: &str,
                  state: (bool, bool, bool)| {
        fq_button(pixmap, text, scale, &palette, rect, label, state)
    };
    match view.layout {
        Layout::Question(q) => {
            let asked = dialog.confirming().unwrap_or_default();
            let icon_px = (q.icon.w * f64::from(scale)).ceil() as u32;
            if let Some(first) = asked.first() {
                draw_icon(
                    pixmap,
                    icons.app(view.desktop, &first.app_id, icon_px),
                    &q.icon,
                    1.0,
                    scale,
                );
            }
            fq_text(
                pixmap,
                text,
                scale,
                &fq::question(&asked),
                (q.question.x, q.question.cy(), q.question.w),
                (13.0, 600),
                palette.ink,
            );
            fq_text(
                pixmap,
                text,
                scale,
                fq::UNSAVED_CHANGES,
                (q.detail.x, q.detail.cy(), q.detail.w),
                (11.0, 400),
                palette.faint,
            );
            // Cancel is the default button
            let hovered = |b| view.hover_button == Some(b);
            button(
                pixmap,
                text,
                &q.cancel,
                fq::CANCEL,
                (true, hovered(Button::Cancel), true),
            );
            button(
                pixmap,
                text,
                &q.confirm,
                fq::FORCE_QUIT,
                (false, hovered(Button::ForceQuit), true),
            );
        }
        Layout::List(l) => {
            let title_w = text.width(fq::TITLE, 13.0);
            fq_text(
                pixmap,
                text,
                scale,
                fq::TITLE,
                (
                    l.title_bar.x + (l.title_bar.w - title_w) / 2.0,
                    l.title_bar.cy(),
                    l.title_bar.w,
                ),
                (13.0, 600),
                palette.ink,
            );
            fill_rounded(
                pixmap,
                &Rect::new(0.0, l.title_bar.bottom() - 1.0, l.title_bar.w, 1.0),
                0.0,
                palette.edge,
                scale,
            );
            fq_paragraph(
                pixmap,
                text,
                scale,
                fq::HEADING,
                &l.heading,
                (13.0, 400, 17.0),
                palette.ink,
            );
            fill_rounded(pixmap, &l.list, 6.0, palette.list, scale);
            if dialog.entries().is_empty() {
                let width = text.width(fq::NO_APPLICATIONS, 13.0);
                fq_text(
                    pixmap,
                    text,
                    scale,
                    fq::NO_APPLICATIONS,
                    (l.list.x + (l.list.w - width) / 2.0, l.list.cy(), l.list.w),
                    (13.0, 400),
                    palette.faint,
                );
            }
            let icon_px = (16.0 * f64::from(scale)).ceil() as u32;
            for (i, row) in &l.rows {
                let entry = &dialog.entries()[*i];
                let selected = dialog.is_selected(*i);
                let shown = Rect::new(row.x, row.y + 1.0, row.w, row.h - 2.0);
                if selected {
                    fill_rounded(pixmap, &shown, 5.0, FQ_ACCENT, scale);
                } else if view.hover_row == Some(*i) {
                    fill_rounded(pixmap, &shown, 5.0, palette.list, scale);
                }
                draw_icon(
                    pixmap,
                    icons.app(view.desktop, &entry.app_id, icon_px),
                    &Rect::new(row.x + 6.0, row.cy() - 8.0, 16.0, 16.0),
                    1.0,
                    scale,
                );
                let name_color = match (selected, entry.unresponsive) {
                    (true, _) => [1.0, 1.0, 1.0, 1.0],
                    (false, true) => palette.name_not_responding,
                    (false, false) => palette.ink,
                };
                let suffix_w = if entry.unresponsive {
                    text.width(fq::NOT_RESPONDING_SUFFIX, 13.0)
                } else {
                    0.0
                };
                let name_w = fq_text(
                    pixmap,
                    text,
                    scale,
                    &entry.name,
                    (row.x + 28.0, row.cy(), row.w - 28.0 - suffix_w - 6.0),
                    (13.0, 400),
                    name_color,
                );
                if entry.unresponsive {
                    fq_text(
                        pixmap,
                        text,
                        scale,
                        fq::NOT_RESPONDING_SUFFIX,
                        (row.x + 28.0 + name_w, row.cy(), suffix_w + 2.0),
                        (13.0, 400),
                        palette.suffix,
                    );
                }
            }
            fq_paragraph(
                pixmap,
                text,
                scale,
                fq::FOOTER,
                &l.footer,
                (10.0, 400, 13.0),
                palette.faint,
            );
            button(
                pixmap,
                text,
                &l.force_quit,
                fq::FORCE_QUIT,
                (
                    true,
                    view.hover_button == Some(Button::ForceQuit),
                    dialog.can_force_quit(),
                ),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Uses the built-in assets only, so the result does not depend on the packs installed on the machine.
    fn label_text() -> LabelText {
        let assets = Assets::builtin();
        LabelText::new(&Typography::default(), &assets).unwrap_or_else(|e| panic!("{e}"))
    }
    use crate::app_grid::AppEntry;
    use crate::config::DockConfig;
    use crate::desktop::DesktopIndex;
    use crate::dock::Dock;
    use cupertino_wm::geometry::Point;

    const SIZE: (u32, u32) = (400, 120);
    const SCREEN: Rect = Rect::new(0.0, 0.0, SIZE.0 as f64, SIZE.1 as f64);

    /// Keys no application has an icon for, and an asset directory without icons (`bare_icons`), so the tiles take the stand-in drawing on any host.
    const FIRST: &str = "zz-unthemed-first";
    const SECOND: &str = "zz-unthemed-second";

    /// A session of two pinned applications on a surface of `SIZE`, the first of them running.
    fn first_running() -> Session {
        let mut dock = Dock::new(DockConfig {
            tile_size: 48.0,
            ..DockConfig::default()
        });
        dock.set_pinned(&[FIRST, SECOND]);
        let mut session = Session::new(dock, DesktopIndex::default());
        session.window(1, FIRST, "shell", false, 0.0);
        session
    }

    fn render(session: &Session, dark: bool) -> (Pixmap, DockLayout) {
        let layout = session.dock.layout(SCREEN, 10.0);
        let mut pixmap = Pixmap::new(SIZE.0, SIZE.1).unwrap();
        paint(&mut pixmap, 1.0, session, &layout, &mut bare_icons(), dark);
        (pixmap, layout)
    }

    fn alpha_at(pixmap: &Pixmap, x: f64, y: f64) -> u8 {
        pixmap.pixel(x as u32, y as u32).unwrap().alpha()
    }

    /// An asset directory in `root` whose only icons are the solid-colour 16 px rasters `icons`.
    fn assets_with(root: &std::path::Path, icons: &[(&str, [u8; 3])]) -> Assets {
        let entries: Vec<String> = icons
            .iter()
            .map(|(n, _)| format!("\"{n}\": {{\"raster\": [16]}}"))
            .collect();
        let manifest = format!("{{\"version\": 1, \"fonts\": {{\"ui\": \"F\"}}, \"icons\": {{{}}}, \"cursors\": \"c\", \"wallpaper\": \"w.png\"}}", entries.join(", "));
        std::fs::write(root.join("manifest.json"), manifest).unwrap();
        for rel in ["fonts/a.ttf", "cursors/c/index.theme", "w.png"] {
            std::fs::create_dir_all(root.join(rel).parent().unwrap()).unwrap();
            std::fs::write(root.join(rel), b"x").unwrap();
        }
        for (name, [r, g, b]) in icons {
            std::fs::create_dir_all(root.join("icons").join(name)).unwrap();
            let mut icon = tiny_skia::Pixmap::new(16, 16).unwrap();
            icon.fill(Color::from_rgba8(*r, *g, *b, 255));
            icon.save_png(root.join("icons").join(name).join("16.png"))
                .unwrap();
        }
        Assets::open(root).unwrap()
    }

    /// Icons over a temporary asset directory, which lives as long as they do (the icons load lazily).
    struct BareIcons {
        _dir: tempfile::TempDir,
        icons: Icons,
    }

    impl std::ops::Deref for BareIcons {
        type Target = Icons;
        fn deref(&self) -> &Icons {
            &self.icons
        }
    }

    impl std::ops::DerefMut for BareIcons {
        fn deref_mut(&mut self) -> &mut Icons {
            &mut self.icons
        }
    }

    /// Icons from an asset directory that has the three system icons the dock needs and no application icon.
    fn bare_icons() -> BareIcons {
        let dir = tempfile::tempdir().unwrap();
        let icons = Icons::new(&assets_with(dir.path(), &SYSTEM_ICONS));
        BareIcons { _dir: dir, icons }
    }

    /// What a pixel inside the generic application icon of [`SYSTEM_ICONS`] is.
    const GENERIC_APPLICATION_PIXEL: [u8; 4] = [0, 0, 255, 255];

    /// The icons `Icons::new` requires, as distinct solid colours.
    const SYSTEM_ICONS: [(&str, [u8; 3]); 3] = [
        ("application.generic", [0, 0, 255]),
        ("trash.empty", [10, 0, 0]),
        ("trash.full", [200, 0, 0]),
    ];

    fn first_tile_centre(session: &Session, layout: &DockLayout) -> Point {
        let first = session.dock.app_tile(FIRST).unwrap().id;
        layout
            .tiles
            .iter()
            .find(|t| t.id == first)
            .unwrap()
            .rect
            .center()
    }

    #[test]
    fn an_application_without_an_icon_gets_the_generic_application_icon_of_the_assets() {
        let session = first_running();
        let layout = session.dock.layout(SCREEN, 10.0);
        let dir = tempfile::tempdir().unwrap();
        let mut icons = Icons::new(&assets_with(dir.path(), &SYSTEM_ICONS));
        let mut pixmap = Pixmap::new(SIZE.0, SIZE.1).unwrap();
        paint(&mut pixmap, 1.0, &session, &layout, &mut icons, false);
        let c = first_tile_centre(&session, &layout);
        let p = pixmap.pixel(c.x as u32, c.y as u32).unwrap();
        assert_eq!((p.red(), p.green(), p.blue()), (0, 0, 255));
    }

    #[test]
    fn the_trash_tile_draws_the_empty_or_the_full_trash_icon() {
        let dir = tempfile::tempdir().unwrap();
        let mut icons = Icons::new(&assets_with(dir.path(), &SYSTEM_ICONS));
        let red = |icons: &mut Icons| icons.trash(16).pixel(8, 8).unwrap().red();
        icons.trash = Some((Instant::now(), false));
        assert_eq!(red(&mut icons), 10);
        icons.trash = Some((Instant::now(), true));
        assert_eq!(red(&mut icons), 200);
    }

    #[test]
    fn a_tile_whose_icon_no_pack_provides_is_drawn_as_the_placeholder() {
        let session = first_running();
        let layout = session.dock.layout(SCREEN, 10.0);
        let dir = tempfile::tempdir().unwrap();
        let mut icons = Icons::new(&assets_with(dir.path(), &[("trash.empty", [10, 0, 0])]));
        let mut pixmap = Pixmap::new(SIZE.0, SIZE.1).unwrap();
        paint(&mut pixmap, 1.0, &session, &layout, &mut icons, false);
        let c = first_tile_centre(&session, &layout);
        assert!(
            pixmap.pixel(c.x as u32, c.y as u32).unwrap().alpha() > 0,
            "no generic application icon: the placeholder"
        );
    }

    #[test]
    fn only_a_running_application_has_an_indicator_under_its_tile() {
        let session = first_running();
        let (pixmap, layout) = render(&session, false);
        let dot_at = |key: &str| {
            let r = layout
                .tiles
                .iter()
                .find(|t| t.id == session.dock.app_tile(key).unwrap().id)
                .unwrap()
                .rect;
            alpha_at(&pixmap, r.cx(), r.bottom() + 3.0)
        };
        assert!(dot_at(FIRST) > 200, "indicator under the running tile");
        assert_eq!(dot_at(SECOND), 0, "no indicator under an idle tile");
    }

    #[test]
    fn the_dragged_tile_is_drawn_where_it_was_grabbed_and_dimmed_once_a_drop_would_remove_it() {
        let mut session = first_running();
        let id = session.dock.app_tile(SECOND).unwrap().id;
        let rest = session
            .dock
            .layout(SCREEN, 10.0)
            .tiles
            .iter()
            .find(|t| t.id == id)
            .unwrap()
            .rect;
        let press = Point::new(rest.x + 10.0, rest.cy());
        assert!(session.dock.begin_drag(
            id,
            press,
            Point::new(press.x + 20.0, press.y),
            SCREEN,
            10.0
        ));
        let mut text = label_text();
        // the pixmap is the output: the bar's surface sits 150 pt down in it, so a tile above the surface shows
        let origin = Point::new(0.0, 150.0);
        let mut paint_held = |session: &Session, now: f64| {
            let mut pixmap = Pixmap::new(SIZE.0, SIZE.1 + 150).unwrap();
            let held = session
                .dock
                .layout(SCREEN, now)
                .dragging
                .expect("a held tile");
            paint_dragged(
                &mut pixmap,
                1.0,
                session,
                &mut bare_icons(),
                &mut text,
                &held,
                origin,
                false,
            );
            (pixmap, held)
        };
        let alpha_of = |pixmap: &Pixmap, held: &DraggedTile| {
            alpha_at(
                pixmap,
                held.tile.rect.cx() + origin.x,
                held.tile.rect.cy() + origin.y,
            )
        };
        let (pixmap, held) = paint_held(&session, 10.0);
        assert_eq!(
            held.tile.rect,
            Rect::new(rest.x + 20.0, rest.y, rest.w, rest.h),
            "the icon keeps its place relative to the pointer"
        );
        assert_eq!(alpha_of(&pixmap, &held), 255);
        // far above the bar the drop removes the tile, but only once the delay is over
        let above = Point::new(press.x, -20.0);
        session.dock.drag_move(above, SCREEN, 10.1);
        assert!(
            !session.dock.drag_will_remove(10.2),
            "the removal delay is not over"
        );
        assert!(session.dock.drag_will_remove(10.7));
        session.dock.tick(10.7);
        let (pixmap, held) = paint_held(&session, 10.7 + crate::tile_drag::LOOK_S);
        assert!(held.tile.rect.y < 0.0, "the tile is above its surface");
        let alpha = alpha_of(&pixmap, &held);
        assert!(
            (i32::from(alpha) - 89).abs() <= 2,
            "35 % opacity, got {alpha}"
        );
        assert!(held.label > 0.99, "the label is shown");
        let pill_y = held.tile.rect.y - REMOVE_LABEL_GAP - REMOVE_LABEL_HEIGHT / 2.0;
        assert!(
            alpha_at(&pixmap, held.tile.rect.cx() + origin.x, pill_y + origin.y) > 100,
            "the \"Remove\" pill is above the icon"
        );
    }

    #[test]
    fn the_indicator_follows_the_dock_edge() {
        let mut dock = Dock::new(DockConfig {
            tile_size: 48.0,
            orientation: Orientation::Left,
            ..DockConfig::default()
        });
        dock.set_pinned(&[FIRST]);
        let mut session = Session::new(dock, DesktopIndex::default());
        session.window(1, FIRST, "shell", false, 0.0);
        let layout = session.dock.layout(Rect::new(0.0, 0.0, 120.0, 400.0), 10.0);
        let mut pixmap = Pixmap::new(120, 400).unwrap();
        paint(&mut pixmap, 1.0, &session, &layout, &mut bare_icons(), true);
        let r = layout
            .tiles
            .iter()
            .find(|t| t.id == session.dock.app_tile(FIRST).unwrap().id)
            .unwrap()
            .rect;
        assert!(
            alpha_at(&pixmap, r.x - 3.0, r.cy()) > 200,
            "dot on the screen-edge side of a left dock"
        );
    }

    #[test]
    fn separators_are_a_faint_line_across_the_bar() {
        let session = first_running();
        let (pixmap, layout) = render(&session, true);
        let sep = layout
            .tiles
            .iter()
            .find(|t| t.kind == TileKind::Separator)
            .unwrap()
            .rect;
        let (left, right) = (sep.cx().floor() - 1.0, sep.cx().floor());
        let a = u32::from(alpha_at(&pixmap, left, sep.cy()))
            + u32::from(alpha_at(&pixmap, right, sep.cy()));
        assert!(
            (60..=68).contains(&a),
            "a one-point line at a quarter opacity spans two pixels, got {a}"
        );
        assert_eq!(
            alpha_at(&pixmap, sep.x + 1.0, sep.cy()),
            0,
            "nothing beside the line"
        );
    }

    #[test]
    fn a_bouncing_tile_is_drawn_raised() {
        let mut session = first_running();
        session.launching(SECOND, 0.0);
        let layout = session.dock.layout(SCREEN, 0.175);
        let mut pixmap = Pixmap::new(SIZE.0, SIZE.1).unwrap();
        paint(
            &mut pixmap,
            1.0,
            &session,
            &layout,
            &mut bare_icons(),
            false,
        );
        let second = layout
            .tiles
            .iter()
            .find(|t| t.id == session.dock.app_tile(SECOND).unwrap().id)
            .unwrap();
        assert!(second.lift > 5.0);
        let top = second.rect.y - second.lift + 4.0;
        assert!(
            alpha_at(&pixmap, second.rect.cx(), top) > 200,
            "tile pixels above its resting rectangle"
        );
    }

    /// A panel of the two unthemed applications with `selected` chosen, on a 400 × 300 output.
    fn switcher_pixmap(selected: usize, alpha: f64) -> (Pixmap, SwitcherLayout) {
        switcher_pixmap_with(selected, alpha, &mut bare_icons())
    }

    fn switcher_pixmap_with(
        selected: usize,
        alpha: f64,
        icons: &mut Icons,
    ) -> (Pixmap, SwitcherLayout) {
        let apps = vec![FIRST.to_string(), SECOND.to_string()];
        let switcher = Switcher::new(apps, selected, 0.0).unwrap();
        let layout = SwitcherLayout::new(2, 400.0, 300.0);
        let desktop = DesktopIndex::default();
        let mut pixmap = Pixmap::new(layout.panel.w as u32, layout.panel.h as u32).unwrap();
        let panel = Panel {
            switcher: &switcher,
            layout: &layout,
            desktop: &desktop,
            dark: false,
            alpha,
        };
        paint_switcher(&mut pixmap, 1.0, &panel, icons, &mut label_text());
        (pixmap, layout)
    }

    fn pixel_at(pixmap: &Pixmap, layout: &SwitcherLayout, x: f64, y: f64) -> [u8; 4] {
        let p = pixmap
            .pixel((x - layout.panel.x) as u32, (y - layout.panel.y) as u32)
            .unwrap();
        [p.red(), p.green(), p.blue(), p.alpha()]
    }

    #[test]
    fn the_force_quit_dialog_paints_both_steps() {
        use crate::force_quit::{Dialog, Entry, Layout};
        let entries = vec![
            Entry {
                app_id: FIRST.into(),
                name: "First".into(),
                unresponsive: true,
                built_in: false,
            },
            Entry {
                app_id: SECOND.into(),
                name: "Second".into(),
                unresponsive: false,
                built_in: false,
            },
        ];
        let desktop = DesktopIndex::default();
        let paint = |dialog: &Dialog| {
            let layout = Layout::for_dialog(dialog);
            let (width, height) = layout.size();
            let mut pixmap = Pixmap::new(width as u32, height as u32).unwrap();
            let view = ForceQuitView {
                dialog,
                layout: &layout,
                desktop: &desktop,
                dark: false,
                hover_row: None,
                hover_button: None,
            };
            paint_force_quit(
                &mut pixmap,
                1.0,
                &view,
                &mut bare_icons(),
                &mut label_text(),
            );
            (pixmap, layout)
        };
        let (list, layout) = paint(&Dialog::new(entries.clone()));
        let crate::force_quit::Layout::List(l) = &layout else {
            panic!("the first step is the list")
        };
        let c = l.force_quit.center();
        assert!(list.pixel(c.x as u32, c.y as u32).unwrap().alpha() > 200);
        let (confirm, confirm_layout) = paint(&Dialog::asking_about(entries, FIRST));
        assert!(
            confirm_layout.size().1 < layout.size().1,
            "the question has no list"
        );
        assert_ne!(list.data(), confirm.data(), "the second step differs");
    }

    #[test]
    fn the_switcher_highlights_the_selected_tile_and_draws_every_tile() {
        let (pixmap, layout) = switcher_pixmap(1, 1.0);
        let ring = layout.selection(1);
        // light highlight: gray 0.75 at opacity 0.65, premultiplied
        assert_eq!(
            pixel_at(&pixmap, &layout, ring.x + 1.0, ring.cy()),
            [124, 124, 124, 166]
        );
        let unselected = layout.item(0);
        assert_eq!(
            pixel_at(&pixmap, &layout, unselected.x - 2.0, unselected.cy())[3],
            0,
            "no highlight around the other tile"
        );
        for (i, key) in [FIRST, SECOND].into_iter().enumerate() {
            let c = layout.item(i).center();
            let px = pixel_at(&pixmap, &layout, c.x, c.y);
            assert_eq!(
                px, GENERIC_APPLICATION_PIXEL,
                "tile {i} ({key}) shows the generic application icon"
            );
        }
    }

    /// Window 1's own icon: an 8 px image of opaque red, in the server's ARGB8888 bytes.
    fn red_window_icon() -> DockWindowIcon {
        DockWindowIcon {
            id: 1,
            name: None,
            image: Some(WindowIconImage {
                size: 8,
                argb: [0, 0, 255, 255].repeat(64),
            }),
        }
    }

    #[test]
    fn an_application_without_a_desktop_icon_shows_its_windows_own_image_in_the_dock_and_the_switcher(
    ) {
        let session = first_running();
        let layout = session.dock.layout(SCREEN, 10.0);
        let mut icons = bare_icons();
        icons.window_icon([FIRST.into(), FIRST.into()], red_window_icon());
        let tile = |icons: &mut Icons| {
            let mut pixmap = Pixmap::new(SIZE.0, SIZE.1).unwrap();
            paint(&mut pixmap, 1.0, &session, &layout, icons, false);
            let c = first_tile_centre(&session, &layout);
            let p = pixmap.pixel(c.x as u32, c.y as u32).unwrap();
            [p.red(), p.green(), p.blue(), p.alpha()]
        };
        assert_eq!(tile(&mut icons), [255, 0, 0, 255]);

        let (pixmap, switcher) = switcher_pixmap_with(1, 1.0, &mut icons);
        let at = |i: usize| {
            let c = switcher.item(i).center();
            pixel_at(&pixmap, &switcher, c.x, c.y)
        };
        assert_eq!(at(0), [255, 0, 0, 255], "the window's own icon");
        assert_eq!(at(1), GENERIC_APPLICATION_PIXEL, "no window icon");

        icons.window_closed(1);
        assert_eq!(tile(&mut icons), GENERIC_APPLICATION_PIXEL);
    }

    /// A 512 px checkerboard of 4 px squares: still one pixel squares at the 128 px a large tile draws at.
    #[test]
    fn a_large_window_image_is_averaged_down_to_the_tile_not_sampled() {
        let session = first_running();
        let layout = session.dock.layout(SCREEN, 10.0);
        let mut icons = bare_icons();
        let checker = (0..512 * 512)
            .flat_map(|i| {
                [(i % 512 / 4 + i / 512 / 4) % 2 * 255; 3]
                    .into_iter()
                    .chain([255])
            })
            .map(|v| v as u8)
            .collect();
        icons.window_icon(
            [FIRST.into(), FIRST.into()],
            DockWindowIcon {
                id: 1,
                name: None,
                image: Some(WindowIconImage {
                    size: 512,
                    argb: checker,
                }),
            },
        );
        let mut pixmap = Pixmap::new(SIZE.0, SIZE.1).unwrap();
        paint(&mut pixmap, 1.0, &session, &layout, &mut icons, false);
        let c = first_tile_centre(&session, &layout);
        for (dx, dy) in [(-4, -4), (0, 0), (3, -2), (5, 5)] {
            let p = pixmap
                .pixel((c.x as i32 + dx) as u32, (c.y as i32 + dy) as u32)
                .unwrap();
            assert!(
                [p.red(), p.green(), p.blue()]
                    .iter()
                    .all(|v| (120..=136).contains(v)),
                "({dx}, {dy}) is mid grey, not a sampled black or white: {p:?}"
            );
        }
    }

    /// A 96 px image, black left of column 40 and white from it, in a resting 48 px tile: taken at the 48 px it is
    /// drawn at, the edge falls on a whole pixel (column 20), with no blended column.
    #[test]
    fn a_resting_tile_keeps_a_straight_edge_of_its_icon_sharp() {
        let session = first_running();
        let layout = session.dock.layout(SCREEN, 10.0);
        let mut icons = bare_icons();
        let split = (0..96 * 96)
            .flat_map(|i| {
                [if i % 96 < 40 { 0 } else { 255 }; 3]
                    .into_iter()
                    .chain([255])
            })
            .map(|v| v as u8)
            .collect();
        icons.window_icon(
            [FIRST.into(), FIRST.into()],
            DockWindowIcon {
                id: 1,
                name: None,
                image: Some(WindowIconImage {
                    size: 96,
                    argb: split,
                }),
            },
        );
        let mut pixmap = Pixmap::new(SIZE.0, SIZE.1).unwrap();
        paint(&mut pixmap, 1.0, &session, &layout, &mut icons, false);
        let c = first_tile_centre(&session, &layout);
        let blended: Vec<_> = (c.x as i32 - 20..c.x as i32 + 20)
            .map(|x| pixmap.pixel(x as u32, c.y as u32).unwrap().red())
            .filter(|v| (16..=239).contains(v))
            .collect();
        assert_eq!(blended, Vec::<u8>::new());
    }

    /// A grid of `n` unthemed applications on a 1440 × 790 layer (the 1440 × 900 output of the
    /// computed table), opened at 0 and drawn at `now`.
    fn grid_pixmap(
        n: usize,
        now: f64,
        setup: impl FnOnce(&mut AppGrid),
    ) -> (Pixmap, AppGrid, GridLayout) {
        let apps = (0..n)
            .map(|i| AppEntry {
                key: format!("zz-unthemed-{i:02}"),
                name: format!("App {i:02}"),
                icon: format!("zz-unthemed-{i:02}"),
            })
            .collect();
        let mut grid = AppGrid::new(apps, 0.0);
        setup(&mut grid);
        let layout = GridLayout::new(1440.0, 790.0);
        let mut pixmap = Pixmap::new(1440, 790).unwrap();
        let view = GridView {
            grid: &grid,
            layout: &layout,
            now,
        };
        paint_app_grid(
            &mut pixmap,
            1.0,
            &view,
            &mut bare_icons(),
            &mut label_text(),
            &mut GridCache::default(),
        );
        (pixmap, grid, layout)
    }

    fn rgba_at(pixmap: &Pixmap, p: Point) -> [u8; 4] {
        let px = pixmap.pixel(p.x as u32, p.y as u32).unwrap();
        [px.red(), px.green(), px.blue(), px.alpha()]
    }

    #[test]
    fn the_grid_draws_the_tiles_the_dots_and_the_search_field() {
        let (pixmap, _, layout) = grid_pixmap(40, 1.0, |g| {
            g.key(crate::app_grid::Key::Right, 0.0);
        });
        let frame = layout.page_frame(0, 0.0);
        for slot in [0, 34] {
            let image = layout.image_rect(&layout.cell_rect(&frame, slot));
            assert_eq!(
                rgba_at(&pixmap, image.center()),
                GENERIC_APPLICATION_PIXEL,
                "slot {slot} shows the generic application icon"
            );
        }
        let selection = layout.item_rect(&layout.cell_rect(&frame, 0));
        assert_eq!(
            rgba_at(&pixmap, Point::new(selection.cx(), selection.y + 2.0)),
            [51, 51, 51, 51],
            "the selected item's highlight: white at 0.2"
        );
        let other = layout.item_rect(&layout.cell_rect(&frame, 1));
        assert_eq!(
            rgba_at(&pixmap, Point::new(other.cx(), other.y + 2.0))[3],
            0,
            "no highlight on the others"
        );
        // two pages: the current dot is opaque white, the other white at 0.2
        assert_eq!(
            rgba_at(&pixmap, layout.dot_shape(0, 2).center()),
            [255, 255, 255, 255]
        );
        assert_eq!(
            rgba_at(&pixmap, layout.dot_shape(1, 2).center()),
            [51, 51, 51, 51]
        );
        assert!(
            rgba_at(&pixmap, layout.search.center())[3] > 20,
            "the search field has a fill"
        );
        assert_eq!(
            rgba_at(&pixmap, Point::new(frame.x + 2.0, frame.y + 2.0))[3],
            0,
            "a cell corner is empty"
        );
        let first_slot = layout.image_rect(&layout.cell_rect(&frame, 0)).center();
        let second_slot = layout.image_rect(&layout.cell_rect(&frame, 5)).center();
        // flipped to page 1 and settled: result 35 takes slot 0's place and slot 5 is empty (40 results)
        let (flipped, _, _) = grid_pixmap(40, 2.0, |g| g.go_to_page(1, 1.0));
        assert_eq!(
            rgba_at(&flipped, first_slot),
            GENERIC_APPLICATION_PIXEL,
            "page 1's first tile"
        );
        assert_eq!(rgba_at(&flipped, second_slot)[3], 0);
        assert_eq!(
            rgba_at(&flipped, layout.dot_shape(1, 2).center()),
            [255, 255, 255, 255],
            "the current dot moved"
        );
    }

    #[test]
    fn the_grid_enters_faded_and_leaves_its_scale_to_the_compositor() {
        let (pixmap, _, _) = grid_pixmap(3, 0.0, |_| {});
        assert!(
            pixmap.data().iter().all(|&b| b == 0),
            "opacity 0 at the start"
        );
        let (pixmap, grid, layout) = grid_pixmap(3, 0.125, |_| {});
        let image = layout.image_rect(&layout.cell_rect(&layout.page_frame(0, 0.0), 0));
        let alpha = grid.visual(0.125).alpha;
        assert!(alpha > 0.0 && alpha < 1.0);
        let px = rgba_at(&pixmap, image.center());
        assert!(
            (f64::from(px[3]) - alpha * 255.0).abs() <= 1.0,
            "the tile in its place at the fade's opacity: {px:?} for alpha {alpha}"
        );
    }

    #[test]
    fn the_fade_scales_every_pixel() {
        let (pixmap, layout) = switcher_pixmap(0, 0.5);
        let ring = layout.selection(0);
        assert_eq!(
            pixel_at(&pixmap, &layout, ring.x + 1.0, ring.cy()),
            [62, 62, 62, 83]
        );
        let (gone, layout) = switcher_pixmap(0, 0.0);
        assert_eq!(
            pixel_at(&gone, &layout, ring.x + 1.0, ring.cy()),
            [0, 0, 0, 0]
        );
    }

    use crate::menu::{tile_menu, TileFacts};
    use crate::overview_ui::{Mode, WindowEntry, Workspace};
    use crate::tiles::TileKind;

    fn overview_ui() -> OverviewUi {
        let windows = vec![
            WindowEntry {
                id: 1,
                frame: Rect::new(100.0, 100.0, 600.0, 400.0),
                title: "one".into(),
                minimized: false,
            },
            WindowEntry {
                id: 2,
                frame: Rect::new(200.0, 150.0, 600.0, 400.0),
                title: "two".into(),
                minimized: false,
            },
        ];
        OverviewUi::new(
            Mode::All,
            Rect::new(0.0, 24.0, 1000.0, 600.0),
            &windows,
            vec![
                Workspace {
                    id: 10,
                    current: true,
                },
                Workspace {
                    id: 11,
                    current: false,
                },
            ],
            0.0,
        )
    }

    #[test]
    fn the_overview_chrome_draws_the_slots_the_add_slot_and_fades_on_close() {
        let mut ui = overview_ui();
        let mut text = label_text();
        let mut pixmap = Pixmap::new(1000, 624).unwrap();
        paint_overview(
            &mut pixmap,
            1.0,
            &OverviewView {
                ui: &ui,
                now: 1.0,
                dark: true,
            },
            &mut text,
        );
        let bar = ui.bar(1.0).unwrap();
        let slot = bar.slots[0].rect;
        assert!(
            alpha_at(&pixmap, slot.x + 3.0, slot.y + 3.0) > 0,
            "a slot is filled"
        );
        let add = bar.add.unwrap();
        assert!(
            alpha_at(&pixmap, add.cx(), add.cy()) > 200,
            "the plus of the add slot"
        );
        let (_, frame) = ui.frames()[0];
        assert_eq!(
            alpha_at(&pixmap, frame.cx(), frame.cy()),
            0,
            "the windows are the server's: nothing over them"
        );
        ui.close(1.0);
        let mut faded = Pixmap::new(1000, 624).unwrap();
        paint_overview(
            &mut faded,
            1.0,
            &OverviewView {
                ui: &ui,
                now: 1.0 + crate::overview_ui::HIDE_DURATION / 2.0,
                dark: true,
            },
            &mut text,
        );
        let (before, after) = (
            alpha_at(&pixmap, add.cx(), add.cy()),
            alpha_at(&faded, add.cx(), add.cy()),
        );
        assert!(
            after > 0 && after < before,
            "half faded: {after} vs {before}"
        );
    }

    #[test]
    fn the_menu_highlights_the_hovered_row_and_dims_a_disabled_one() {
        let items = tile_menu(&TileFacts {
            kind: TileKind::App,
            key: "k",
            pinned: false,
            built_in: false,
            running_app_id: None,
            unresponsive: false,
            entry_path: None,
        });
        let mut text = label_text();
        let layout = menu::layout(
            &items,
            &Rect::new(400.0, 30.0, 64.0, 64.0),
            &Rect::new(0.0, -600.0, 1000.0, 700.0),
            crate::config::Orientation::Bottom,
            &|s| s.len() as f64 * 7.0,
        );
        let mut pixmap = Pixmap::new(layout.rect.w as u32, layout.rect.h as u32).unwrap();
        paint_menu(&mut pixmap, 1.0, &items, &layout, Some(0), false, &mut text);
        let row = |i: usize| layout.rows[i].translate(-layout.rect.x, -layout.rect.y);
        let hot = pixmap
            .pixel(row(0).cx() as u32, row(0).y as u32 + 2)
            .unwrap();
        assert!(
            hot.blue() > 200 && hot.red() < 60,
            "the hovered row is filled with the highlight: {hot:?}"
        );
        assert_eq!(
            alpha_at(&pixmap, row(3).cx(), row(3).y + 2.0),
            0,
            "an unhovered row has no fill"
        );
        let sep = row(1);
        assert!(
            alpha_at(&pixmap, sep.cx(), sep.cy()) > 0,
            "the separator line"
        );
    }

    #[test]
    fn a_faded_frame_shows_a_selection_made_during_the_entry() {
        let apps = (0..3)
            .map(|i| AppEntry {
                key: format!("zz-unthemed-{i:02}"),
                name: format!("App {i:02}"),
                icon: format!("zz-unthemed-{i:02}"),
            })
            .collect();
        let mut grid = AppGrid::new(apps, 0.0);
        let layout = GridLayout::new(1440.0, 790.0);
        let (mut icons, mut text, mut cache) = (bare_icons(), label_text(), GridCache::default());
        let mut frame = |grid: &AppGrid, now: f64| {
            let mut pixmap = Pixmap::new(1440, 790).unwrap();
            let view = GridView {
                grid,
                layout: &layout,
                now,
            };
            paint_app_grid(&mut pixmap, 1.0, &view, &mut icons, &mut text, &mut cache);
            pixmap
        };
        let selection = layout.item_rect(&layout.cell_rect(&layout.page_frame(0, 0.0), 0));
        let at = Point::new(selection.cx(), selection.y + 2.0);
        assert_eq!(rgba_at(&frame(&grid, 0.2), at)[3], 0, "nothing selected");
        grid.key(crate::app_grid::Key::Right, 0.2);
        assert!(grid.visual(0.22).alpha < 1.0);
        assert!(
            rgba_at(&frame(&grid, 0.22), at)[3] > 0,
            "the selection shows in the next faded frame"
        );
    }
}
