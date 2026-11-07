//! The icons the dock shows, loaded for the toolkit's images, and the two surfaces the process still paints itself:
//! the switcher's panel and the overview's chrome, which move to views of their own with the window-manager surfaces.
//! Everything else the dock shows is drawn by CupertinoKit ([`crate::view`]). The material below the switcher's
//! panel is the server's backdrop, so nothing here fills it.

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
use cupertino_wm::geometry::Rect;
use tiny_skia::{Color, FillRule, FilterQuality, Paint, Pixmap, PixmapPaint, Stroke, Transform};

use crate::desktop::DesktopIndex;
use crate::overview_ui::OverviewUi;
use crate::switcher::{Switcher, SwitcherLayout};

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
    pub fn named(&mut self, name: &str, px: u32) -> &Pixmap {
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
    pub fn app(&mut self, desktop: &DesktopIndex, app_id: &str, px: u32) -> &Pixmap {
        match self.source(desktop, app_id, px) {
            IconSource::Theme(name) => self.cache[&(name, px)].as_ref().expect("loaded"),
            IconSource::Window(id) => self.windows.get_mut(&id).expect("listed").at(px),
            IconSource::Generic => self.system.raster(GENERIC_APPLICATION_ICON, px),
        }
    }

    /// Whether the trash folder holds anything; looked at again once [`TRASH_CHECK_EVERY`] passed.
    pub fn trash_full(&mut self) -> bool {
        let now = cupertino_shell_client::test_clock::display_now();
        match self.trash {
            Some((at, full)) if now.duration_since(at) < TRASH_CHECK_EVERY => full,
            _ => {
                let full = trash_is_full();
                self.trash = Some((now, full));
                full
            }
        }
    }

    /// The trash's icon, empty or full.
    pub fn trash(&mut self, full: bool, px: u32) -> &Pixmap {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Uses the built-in assets only, so the result does not depend on the packs installed on the machine.
    fn label_text() -> LabelText {
        let assets = Assets::builtin();
        LabelText::new(&Typography::default(), &assets).unwrap_or_else(|e| panic!("{e}"))
    }

    /// The overview's layout keeps room under every window for its title: the gap and a line of the title with its
    /// shadow's point fit in it, so a title is never drawn over the window below.
    #[test]
    fn an_overview_title_fits_the_space_the_layout_keeps_under_a_window() {
        let space = cupertino_overview::OverviewConfig::default()
            .layout
            .title_space;
        assert!(OVERVIEW_TITLE_GAP + OVERVIEW_TITLE_PT + 1.0 <= space);
    }
    use crate::desktop::DesktopIndex;

    /// Keys no application has an icon for, and an asset directory without icons (`bare_icons`), so the tiles take the stand-in drawing on any host.
    const FIRST: &str = "zz-unthemed-first";
    const SECOND: &str = "zz-unthemed-second";

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

    fn rgba(icon: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let p = icon.pixel(x, y).unwrap();
        [p.red(), p.green(), p.blue(), p.alpha()]
    }

    /// The pixel at the middle of an icon, whatever size it came at.
    fn middle(icon: &Pixmap) -> [u8; 4] {
        rgba(icon, icon.width() / 2, icon.height() / 2)
    }

    #[test]
    fn an_application_without_an_icon_gets_the_generic_application_icon_of_the_assets() {
        let mut icons = bare_icons();
        let desktop = DesktopIndex::default();
        assert_eq!(
            rgba(icons.app(&desktop, FIRST, 16), 8, 8),
            GENERIC_APPLICATION_PIXEL
        );
    }

    #[test]
    fn the_trash_draws_the_empty_or_the_full_trash_icon() {
        let mut icons = bare_icons();
        assert_eq!(icons.trash(false, 16).pixel(8, 8).unwrap().red(), 10);
        assert_eq!(icons.trash(true, 16).pixel(8, 8).unwrap().red(), 200);
    }

    #[test]
    fn an_icon_no_pack_provides_is_drawn_as_the_placeholder() {
        let dir = tempfile::tempdir().unwrap();
        let mut icons = Icons::new(&assets_with(dir.path(), &[("trash.empty", [10, 0, 0])]));
        let desktop = DesktopIndex::default();
        assert!(
            rgba(icons.app(&desktop, FIRST, 16), 8, 8)[3] > 0,
            "no generic application icon: the placeholder"
        );
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
        let mut icons = bare_icons();
        let desktop = DesktopIndex::default();
        icons.window_icon([FIRST.into(), FIRST.into()], red_window_icon());
        assert_eq!(middle(icons.app(&desktop, FIRST, 48)), [255, 0, 0, 255]);

        let (pixmap, switcher) = switcher_pixmap_with(1, 1.0, &mut icons);
        let at = |i: usize| {
            let c = switcher.item(i).center();
            pixel_at(&pixmap, &switcher, c.x, c.y)
        };
        assert_eq!(at(0), [255, 0, 0, 255], "the window's own icon");
        assert_eq!(at(1), GENERIC_APPLICATION_PIXEL, "no window icon");

        icons.window_closed(1);
        assert_eq!(
            middle(icons.app(&desktop, FIRST, 48)),
            GENERIC_APPLICATION_PIXEL
        );
    }

    /// A 512 px checkerboard of 4 px squares: two by two squares averaged into each pixel of the 64 px a tile draws at.
    #[test]
    fn a_large_window_image_is_averaged_down_to_the_tile_not_sampled() {
        let mut icons = bare_icons();
        let desktop = DesktopIndex::default();
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
        let icon = icons.app(&desktop, FIRST, 64);
        for (x, y) in [(28, 28), (32, 32), (35, 30), (37, 37)] {
            let p = rgba(icon, x, y);
            assert!(
                p[..3].iter().all(|v| (120..=136).contains(v)),
                "({x}, {y}) is mid grey, not a sampled black or white: {p:?}"
            );
        }
    }

    /// A 96 px image, black left of column 40 and white from it, loaded for a 48 px tile: taken at the 48 px it is
    /// drawn at, the edge falls on a whole pixel (column 20), with no blended column.
    #[test]
    fn an_icon_loaded_at_its_drawn_size_keeps_a_straight_edge_sharp() {
        let mut icons = bare_icons();
        let desktop = DesktopIndex::default();
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
        let icon = icons.app(&desktop, FIRST, 48);
        assert_eq!(icon.width(), 48);
        let blended: Vec<_> = (0..48)
            .map(|x| rgba(icon, x, 24)[0])
            .filter(|v| (16..=239).contains(v))
            .collect();
        assert_eq!(blended, Vec::<u8>::new());
        assert_eq!(rgba(icon, 19, 24)[0], 0);
        assert_eq!(rgba(icon, 20, 24)[0], 255);
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

    use crate::overview_ui::{Mode, WindowEntry, Workspace};

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
}
