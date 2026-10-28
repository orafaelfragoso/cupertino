//! CPU drawing of the settings window and the search results popup (tiny-skia, the decorations crate's text
//! renderer, the asset directory's template symbols). Every colour is a semantic token of the design tokens for the
//! active appearance (§6.2); a missing token is an error, not a stand-in colour.

use cupertino_assets::{AssetError, Assets, FontRole};
use cupertino_decorations::field::{
    caret_offsets, centred_baseline, draw_field, FieldContent, FieldStyle,
};
use cupertino_decorations::palette::Rgba;
use cupertino_decorations::shape::rounded_path;
use cupertino_decorations::text::TextRenderer;
use cupertino_geometry::{CornerStyle, Radii, Rect as GeoRect};
use cupertino_icons::SystemIcons;
use cupertino_text_field::TextField;
use cupertino_theme::{Appearance, SemanticColor, Theme};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tiny_skia::{
    Color, FillRule, FilterQuality, Paint, Pattern, Pixmap, PixmapPaint, Rect as SkRect,
    SpreadMode, Stroke, Transform,
};

use crate::controls::{self, metrics as cm, Control};
use crate::form::{metrics as fm, SectionLayout};
use crate::geom::Rect;
use crate::history::History;
use crate::pane::{self, Enclosure, PaneId};
use crate::panes::Section;
use crate::search::{popup, Hit, SearchField};
use crate::sheet::{Kind, Sheet, SheetLayout};
use crate::sidebar::{row, RowSize};
use crate::window::{metrics as m, WindowLayout};

/// The window's colours for one appearance.
#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    pub dark: bool,
    /// The opaque content column.
    pub window_background: Rgba,
    pub text: Rgba,
    pub secondary_text: Rgba,
    pub placeholder: Rgba,
    pub separator: Rgba,
    pub accent: Rgba,
    /// A selected row in an inactive window (role: grey; the tertiary
    /// label token stands for it, approximate).
    pub inactive_selection: Rgba,
    /// A hovered result row.
    pub quaternary: Rgba,
    /// The background of selected text in the search field.
    pub text_selection: Rgba,
    /// The sidebar search field's fill: at [`SEARCH_FILL_SHARE`] of its opacity.
    pub search_fill: Rgba,
    /// A control's own background and an off switch's track, a slider's track.
    pub control_background: Rgba,
    /// A form section's box and the lines between its rows: the box 3 levels from the window background toward the
    /// control background, the line 13 levels from the box toward the text, with no outline (approximate, O4: measured
    /// on a screenshot; the design's fill is unknown).
    pub box_fill: Rgba,
    pub box_separator: Rgba,
    /// The selected sidebar row of an active window: the Selection material's accent state over the sidebar.
    pub sidebar_selection: Rgba,
    pub fill: Rgba,
    /// The text of a refused change under its row.
    pub red: Rgba,
    pub blue: Rgba,
    pub pink: Rgba,
    pub indigo: Rgba,
    pub gray: Rgba,
    pub green: Rgba,
}

/// The box fill's and the row line's distances (8-bit levels; approximate).
const BOX_FILL_LEVELS: f32 = 3.0;
const BOX_SEPARATOR_LEVELS: f32 = 13.0;

/// `from` moved `levels` 8-bit levels toward `to` on each colour channel (never past it); opaque.
fn toward(from: Rgba, to: Rgba, levels: f32) -> Rgba {
    let step = |a: f32, b: f32| {
        if b >= a {
            (a + levels / 255.0).min(b)
        } else {
            (a - levels / 255.0).max(b)
        }
    };
    [
        step(from[0], to[0]),
        step(from[1], to[1]),
        step(from[2], to[2]),
        1.0,
    ]
}

/// The Selection material's accent state (recipe, which state the emphasized selection uses is
/// ): the backdrop blurred and made monochrome white at luminance `l`, a soft-light tint of the stored blue
/// accent, then a fill of the same colour at 0.75: colour = 0.75 A + 0.25 softLight(l, A). The recipe is the light
/// appearance's; the dark one keeps the accent token.
fn selection_over(l: f32) -> Rgba {
    // the soft-light blend of the compositing specification (backdrop b, source s)
    let soft_light = |b: f32, s: f32| {
        if s <= 0.5 {
            b - (1.0 - 2.0 * s) * b * (1.0 - b)
        } else {
            let d = if b <= 0.25 {
                ((16.0 * b - 12.0) * b + 4.0) * b
            } else {
                b.sqrt()
            };
            b + (2.0 * s - 1.0) * (d - b)
        }
    };
    let c = |a: f32| SELECTION_FILL * a + (1.0 - SELECTION_FILL) * soft_light(l, a);
    [
        c(SELECTION_ACCENT[0]),
        c(SELECTION_ACCENT[1]),
        c(SELECTION_ACCENT[2]),
        1.0,
    ]
}

/// The tint and fill colour of the Selection material's accent state, the catalog's stored blue accent, and the fill's opacity.
const SELECTION_ACCENT: [f32; 3] = [0.0392, 0.5098, 1.0];
const SELECTION_FILL: f32 = 0.75;
/// The luminance of the sidebar behind a selected row once made monochrome (approximate: 0.95, the value
/// works through; the real one follows the backdrop, which the client does not see).
const SIDEBAR_LUMINANCE: f32 = 0.95;

/// The search field's share of the quaternary label opacity: the full token darkens the sidebar by 28 levels, the
/// reference field sits 12 levels under its sidebar (approximate: measured on a screenshot, a JPEG, by
/// light appearance only, the dark one takes the same share).
const SEARCH_FILL_SHARE: f32 = 12.0 / 28.0;

/// A token the palette needs is missing from the design tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingToken(pub &'static str);

impl std::fmt::Display for MissingToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the design tokens have no colour {}", self.0)
    }
}

impl std::error::Error for MissingToken {}

impl Palette {
    pub fn from_theme(theme: &Theme, appearance: Appearance) -> Result<Self, MissingToken> {
        let sem = |c: SemanticColor| {
            theme
                .tokens
                .semantic(appearance, c)
                .map(|c| c.to_array())
                .ok_or(MissingToken(c.name()))
        };
        // the accent is, the palette's only accent (no accent setting exists, G3)
        let accent = theme
            .tokens
            .color(appearance, ACCENT)
            .map(|c| c.to_array())
            .ok_or(MissingToken(ACCENT))?;
        let window_background = sem(SemanticColor::WindowBackground)?;
        let control_background = sem(SemanticColor::ControlBackground)?;
        let text = sem(SemanticColor::Label)?;
        let box_fill = toward(window_background, control_background, BOX_FILL_LEVELS);
        Ok(Palette {
            dark: appearance.is_dark(),
            window_background,
            text,
            secondary_text: sem(SemanticColor::SecondaryLabel)?,
            placeholder: sem(SemanticColor::PlaceholderText)?,
            separator: sem(SemanticColor::Grid)?,
            accent,
            inactive_selection: sem(SemanticColor::TertiaryLabel)?,
            quaternary: sem(SemanticColor::QuaternaryLabel)?,
            text_selection: sem(SemanticColor::SelectedTextBackground)?,
            search_fill: {
                let q = sem(SemanticColor::QuaternaryLabel)?;
                [q[0], q[1], q[2], q[3] * SEARCH_FILL_SHARE]
            },
            control_background,
            box_fill,
            box_separator: toward(box_fill, text, BOX_SEPARATOR_LEVELS),
            sidebar_selection: if appearance.is_dark() {
                accent
            } else {
                selection_over(SIDEBAR_LUMINANCE)
            },
            fill: sem(SemanticColor::SystemFill)?,
            red: sem(SemanticColor::SystemRed)?,
            blue: sem(SemanticColor::SystemBlue)?,
            pink: sem(SemanticColor::SystemPink)?,
            indigo: sem(SemanticColor::SystemIndigo)?,
            gray: sem(SemanticColor::SystemGray)?,
            green: sem(SemanticColor::SystemGreen)?,
        })
    }

    /// The fill of an icon enclosure (colour names; black is the colour itself).
    pub fn enclosure(&self, e: Enclosure) -> Rgba {
        match e {
            Enclosure::Blue => self.blue,
            Enclosure::Black => [0.0, 0.0, 0.0, 1.0],
            Enclosure::Gray => self.gray,
            Enclosure::Pink => self.pink,
            Enclosure::Indigo => self.indigo,
            Enclosure::Green => self.green,
        }
    }
}

/// The template symbols of the window's chrome; with the panes' ([`pane::symbols`]) the process requires them.
pub const CHROME_SYMBOLS: [&str; 6] = [
    "symbol.chevron.left",
    "symbol.chevron.right",
    "symbol.search",
    "symbol.xmark.circle",
    "symbol.chevron.up.down",
    "symbol.checkmark",
];

/// Every template symbol the window draws.
pub fn required_symbols() -> Vec<&'static str> {
    let mut all = pane::symbols();
    all.extend(CHROME_SYMBOLS);
    all
}

/// The accent colour's token.
const ACCENT: &str = "control-accent-blue";
const WHITE: Rgba = [1.0, 1.0, 1.0, 1.0];
/// The chrome glyphs: the back / forward chevrons and the search field's glyphs. approximate.
const TOOLBAR_GLYPH: f64 = 16.0;
const FIELD_GLYPH: f64 = 12.0;
/// The search field's corner radius, its text size and the glyph's leading inset. approximate.
const FIELD_RADIUS: f64 = 6.0;
const FIELD_TEXT: f64 = 13.0;
const FIELD_GLYPH_INSET: f64 = 6.0;
/// A result row's selection fill radius (approximate, §5).
const RESULT_RADIUS: f64 = 6.0;

fn paint(c: Rgba) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(Color::from_rgba(c[0], c[1], c[2], c[3]).expect("palette colours are in range"));
    p.anti_alias = true;
    p
}

fn fill_rect(pm: &mut Pixmap, r: &Rect, s: f32, c: Rgba) {
    if let Some(rr) = SkRect::from_xywh(
        r.x as f32 * s,
        r.y as f32 * s,
        r.w as f32 * s,
        r.h as f32 * s,
    ) {
        pm.fill_rect(rr, &paint(c), Transform::identity(), None);
    }
}

/// Fill `r` with corners of `radius`, continuous (the platform's squircle) or circular.
fn fill_rounded(pm: &mut Pixmap, r: &Rect, radius: f64, continuous: bool, s: f32, c: Rgba) {
    let style = if continuous {
        CornerStyle::continuous()
    } else {
        CornerStyle::Circular
    };
    if let Some(path) = rounded_path(
        GeoRect::new(r.x, r.y, r.w, r.h),
        Radii::uniform(radius.min(r.w / 2.0).min(r.h / 2.0)),
        style,
        f64::from(s),
    ) {
        pm.fill_path(
            &path,
            &paint(c),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

/// Fill `r` (rounded) with `image` scaled to cover it, the longer side cropped evenly.
fn fill_cover(pm: &mut Pixmap, r: &Rect, image: &Pixmap, radius: f64, s: f32) {
    let (w, h) = (r.w as f32 * s, r.h as f32 * s);
    let scale = (w / image.width() as f32).max(h / image.height() as f32);
    let transform = Transform::from_translate(
        r.x as f32 * s + (w - image.width() as f32 * scale) / 2.0,
        r.y as f32 * s + (h - image.height() as f32 * scale) / 2.0,
    )
    .pre_scale(scale, scale);
    let fill = Paint {
        anti_alias: true,
        shader: Pattern::new(
            image.as_ref(),
            SpreadMode::Pad,
            FilterQuality::Bilinear,
            1.0,
            transform,
        ),
        ..Paint::default()
    };
    if let Some(path) = rounded_path(
        GeoRect::new(r.x, r.y, r.w, r.h),
        Radii::uniform(radius),
        CornerStyle::Circular,
        f64::from(s),
    ) {
        pm.fill_path(&path, &fill, FillRule::Winding, Transform::identity(), None);
    }
}

fn stroke_rounded(pm: &mut Pixmap, r: &Rect, radius: f64, width: f64, s: f32, c: Rgba) {
    if let Some(path) = rounded_path(
        GeoRect::new(r.x, r.y, r.w, r.h),
        Radii::uniform(radius),
        CornerStyle::Circular,
        f64::from(s),
    ) {
        pm.stroke_path(
            &path,
            &paint(c),
            &Stroke {
                width: width as f32 * s,
                ..Default::default()
            },
            Transform::identity(),
            None,
        );
    }
}

/// A text size and weight.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Font {
    size: f64,
    weight: u16,
}

/// What the window shows besides its layout.
pub struct WindowState<'a> {
    pub selected: PaneId,
    /// Whether the machine has a battery, once known (the Battery row's title).
    pub has_battery: Option<bool>,
    pub active: bool,
    pub history: &'a History<PaneId>,
    pub search: &'a SearchField,
    /// The search field has the keyboard (its outline shows).
    pub search_focused: bool,
    /// The search field's caret shows now (focused, nothing selected, the blink on).
    pub search_caret: bool,
}

/// The text area of the search field `f`: right of the magnifier, left of the clear button when there is text.
pub fn search_text_area(f: &Rect, has_text: bool) -> Rect {
    let x = f.x + FIELD_GLYPH_INSET + FIELD_GLYPH + FIELD_GLYPH_INSET;
    let right = if has_text {
        f.right() - FIELD_GLYPH_INSET - FIELD_GLYPH - FIELD_GLYPH_INSET / 2.0
    } else {
        f.right() - FIELD_GLYPH_INSET
    };
    Rect::new(x, f.y, (right - x).max(0.0), f.h)
}

fn search_style(pal: &Palette) -> FieldStyle {
    FieldStyle {
        size: FIELD_TEXT,
        weight: 400,
        letter_spacing: 0.0,
        text: pal.text,
        selection: pal.text_selection,
        caret: pal.text,
        caret_width: 1.0,
    }
}

/// Text, symbols and the drawing of the window and the results popup.
pub struct Painter {
    pub text: TextRenderer,
    symbols: SystemIcons,
    /// The thumbnails of the pictures of a grid, by the image they show.
    thumbnails: HashMap<PathBuf, Pixmap>,
}

impl Painter {
    /// Paint the approved About layout with the same text, semantic colours and button geometry as Settings.
    pub fn paint_about(
        &mut self,
        pm: &mut Pixmap,
        s: f32,
        about: &crate::about::About,
        pal: &Palette,
    ) {
        use crate::about::metrics::*;
        pm.fill(
            Color::from_rgba(
                pal.window_background[0],
                pal.window_background[1],
                pal.window_background[2],
                pal.window_background[3],
            )
            .expect("semantic colour"),
        );
        let mark = cupertino_icons::system_mark((MARK * f64::from(s)).round() as u32, pal.text);
        pm.draw_pixmap(
            ((f64::from(WIDTH) - MARK) / 2.0 * f64::from(s)).round() as i32,
            (TOP * f64::from(s)).round() as i32,
            mark.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
        for (text, y, height, size, weight, color) in [
            ("Cupertino", NAME_Y, NAME_HEIGHT, 22.0, 600, pal.text),
            (
                about.version.as_str(),
                VERSION_Y,
                VERSION_HEIGHT,
                11.0,
                400,
                pal.secondary_text,
            ),
        ] {
            let available = f64::from(WIDTH) - PADDING * 2.0;
            let width =
                f64::from(self.text.measure(text, size as f32 * s, weight, 0.0) / s).min(available);
            self.label(
                pm,
                &Rect::new((f64::from(WIDTH) - width) / 2.0, y, width, height),
                text,
                Font { size, weight },
                s,
                color,
            );
        }
        let centre = f64::from(WIDTH) / 2.0;
        let column_width = centre - COLUMN_GAP / 2.0 - PADDING;
        for (i, row) in about.rows.iter().enumerate() {
            let y = ROWS_Y + i as f64 * (ROW_HEIGHT + ROW_GAP);
            let label = if i > 0 && about.rows[i - 1].label == row.label {
                ""
            } else {
                row.label
            };
            let width =
                f64::from(self.text.measure(label, 13.0 * s, 400, 0.0) / s).min(column_width);
            self.label(
                pm,
                &Rect::new(centre - COLUMN_GAP / 2.0 - width, y, width, ROW_HEIGHT),
                label,
                Font {
                    size: 13.0,
                    weight: 400,
                },
                s,
                pal.secondary_text,
            );
            self.label(
                pm,
                &Rect::new(centre + COLUMN_GAP / 2.0, y, column_width, ROW_HEIGHT),
                &row.value,
                Font {
                    size: 13.0,
                    weight: 400,
                },
                s,
                pal.text,
            );
        }
        let button = about.button();
        fill_rounded(pm, &button, cm::RADIUS, false, s, pal.accent);
        let title = "More Info…";
        let width = f64::from(self.text.measure(title, 13.0 * s, 400, 0.0) / s);
        self.label(
            pm,
            &Rect::new(
                button.x + (button.w - width) / 2.0,
                button.y,
                width + 1.0,
                button.h,
            ),
            title,
            Font {
                size: 13.0,
                weight: 400,
            },
            s,
            WHITE,
        );
    }

    /// A painter with the `ui` font and the template symbols of the asset packs. Fails only when no usable font is
    /// left ([`TextRenderer::new`]); the symbols of [`required_symbols`] no pack provides are reported and drawn as placeholders.
    pub fn new(assets: &Assets) -> Result<Self, AssetError> {
        let mut text = TextRenderer::new(assets, FontRole::Ui)?;
        // the design application is built with the declarative toolkit, whose renderer floors the dilation
        // size
        text.set_declarative(true);
        Ok(Painter {
            text,
            symbols: SystemIcons::required(assets, &[], &required_symbols()),
            thumbnails: HashMap::new(),
        })
    }

    /// Show the PNG `thumbnail` for the picture `image` from now on; `false` when it cannot be read.
    pub fn set_thumbnail(&mut self, image: &Path, thumbnail: &Path) -> bool {
        match Pixmap::load_png(thumbnail) {
            Ok(pixmap) => {
                self.thumbnails.insert(image.to_path_buf(), pixmap);
                true
            }
            Err(_) => false,
        }
    }

    /// Whether the thumbnail of `image` is in.
    pub fn has_thumbnail(&self, image: &Path) -> bool {
        self.thumbnails.contains_key(image)
    }

    /// Draw the raster icon `name` fitted into `r` (a placeholder when no pack or host theme gives it).
    fn raster_icon(&mut self, pm: &mut Pixmap, r: &Rect, name: &str, s: f32) {
        let px = (r.w.min(r.h) * f64::from(s)).round().max(1.0) as u32;
        let icon = self.symbols.raster(name, px);
        pm.draw_pixmap(
            (r.x as f32 * s).round() as i32,
            (r.y as f32 * s).round() as i32,
            icon.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }

    /// Draw the template symbol `name` in `color`, fitted to `fit` points and centred in `r` (a placeholder when no
    /// asset pack provides it).
    fn symbol(&mut self, pm: &mut Pixmap, r: &Rect, name: &str, fit: f64, s: f32, color: Rgba) {
        let px = (fit * f64::from(s)).round().max(1.0) as u32;
        let glyph = self.symbols.symbol(name, px, color);
        let c = r.center();
        let (x, y) = (
            c.x as f32 * s - glyph.width() as f32 / 2.0,
            c.y as f32 * s - glyph.height() as f32 / 2.0,
        );
        pm.draw_pixmap(
            x.round() as i32,
            y.round() as i32,
            glyph.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }

    /// Draw `text` in `font`, left-aligned in `r` and centred on its middle, cut to `r`'s width.
    fn label(&mut self, pm: &mut Pixmap, r: &Rect, text: &str, font: Font, s: f32, color: Rgba) {
        let px = font.size as f32 * s;
        let (fitted, _) = self.text.fit(text, px, font.weight, 0.0, r.w as f32 * s);
        self.text.draw(
            pm,
            &fitted,
            r.x as f32 * s,
            (r.y + r.h / 2.0) as f32 * s + px * 0.36,
            px,
            font.weight,
            0.0,
            color,
        );
    }

    /// A pane's icon: the rounded enclosure of its colour with the white symbol (§4.3, approximate shape).
    fn enclosure(&mut self, pm: &mut Pixmap, icon: &Rect, pane: PaneId, s: f32, pal: &Palette) {
        let p = pane.pane();
        fill_rounded(
            pm,
            icon,
            icon.w * row::ENCLOSURE_RADIUS,
            true,
            s,
            pal.enclosure(p.enclosure),
        );
        self.symbol(pm, icon, p.symbol, icon.w * row::ENCLOSURE_GLYPH, s, WHITE);
    }

    /// Draw the window of `layout` with `pane` in its content column into `pm` (device pixels, `s` per point). The
    /// sidebar stays clear: the server draws its material below the surface.
    pub fn paint_window(
        &mut self,
        pm: &mut Pixmap,
        s: f32,
        layout: &WindowLayout,
        state: &WindowState<'_>,
        pane: &PaneView<'_>,
        pal: &Palette,
    ) {
        self.text.set_scale(s);
        fill_rect(pm, &layout.content, s, pal.control_background);
        // the pane scrolls under the title region, which takes the content colour (no strip, no separator; content
        // scrolling under the title region is not designed yet)
        self.paint_pane(pm, s, pane, pal);
        fill_rect(pm, &layout.toolbar, s, pal.control_background);
        fill_rect(pm, &layout.divider, s, pal.separator);
        self.paint_toolbar(pm, s, layout, state, pal);
        self.paint_search_field(pm, s, layout, state, pal);
        for (pane, r) in &layout.rows {
            let selected = *pane == state.selected;
            if selected {
                fill_rounded(
                    pm,
                    r,
                    row::SELECTION_RADIUS,
                    false,
                    s,
                    if state.active {
                        pal.sidebar_selection
                    } else {
                        pal.inactive_selection
                    },
                );
            }
            let icon = layout.icon(r);
            self.enclosure(pm, &icon, *pane, s, pal);
            let x = icon.right() + row::ICON_TITLE_GAP;
            let title = Rect::new(x, r.y, (r.right() - x - row::ICON_LEADING).max(0.0), r.h);
            let color = if selected && state.active {
                WHITE
            } else {
                pal.text
            };
            self.label(
                pm,
                &title,
                pane.title(state.has_battery),
                Font {
                    size: layout.row_size.font_size(),
                    weight: 400,
                },
                s,
                color,
            );
        }
    }

    fn paint_toolbar(
        &mut self,
        pm: &mut Pixmap,
        s: f32,
        layout: &WindowLayout,
        state: &WindowState<'_>,
        pal: &Palette,
    ) {
        let dimmed = [pal.text[0], pal.text[1], pal.text[2], pal.text[3] * 0.3];
        if let (Some(b), Some(f)) = (layout.back, layout.forward) {
            let back = if state.history.can_go_back() {
                pal.text
            } else {
                dimmed
            };
            let forward = if state.history.can_go_forward() {
                pal.text
            } else {
                dimmed
            };
            self.symbol(pm, &b, "symbol.chevron.left", TOOLBAR_GLYPH, s, back);
            self.symbol(pm, &f, "symbol.chevron.right", TOOLBAR_GLYPH, s, forward);
        }
        let color = if state.active {
            pal.text
        } else {
            pal.secondary_text
        };
        self.label(
            pm,
            &layout.title,
            state.selected.title(state.has_battery),
            Font {
                size: m::TITLE_SIZE,
                weight: m::TITLE_WEIGHT,
            },
            s,
            color,
        );
    }

    fn paint_search_field(
        &mut self,
        pm: &mut Pixmap,
        s: f32,
        layout: &WindowLayout,
        state: &WindowState<'_>,
        pal: &Palette,
    ) {
        let f = layout.search;
        fill_rounded(pm, &f, FIELD_RADIUS, false, s, pal.search_fill);
        if state.search_focused {
            stroke_rounded(
                pm,
                &f.inset(-1.5, -1.5),
                FIELD_RADIUS + 1.5,
                3.0,
                s,
                [pal.accent[0], pal.accent[1], pal.accent[2], 0.5],
            );
        }
        let glyph = Rect::new(f.x + FIELD_GLYPH_INSET, f.y, FIELD_GLYPH, f.h);
        self.symbol(
            pm,
            &glyph,
            "symbol.search",
            FIELD_GLYPH,
            s,
            pal.secondary_text,
        );
        let typed = state.search.text();
        let text = search_text_area(&f, !typed.is_empty());
        if typed.is_empty() {
            self.label(
                pm,
                &text,
                "Search",
                Font {
                    size: FIELD_TEXT,
                    weight: 400,
                },
                s,
                pal.placeholder,
            );
        } else {
            let clear = Rect::new(
                f.right() - FIELD_GLYPH_INSET - FIELD_GLYPH,
                f.y,
                FIELD_GLYPH,
                f.h,
            );
            self.symbol(
                pm,
                &clear,
                "symbol.xmark.circle",
                FIELD_GLYPH,
                s,
                pal.secondary_text,
            );
        }
        let field = state.search.field();
        let style = search_style(pal);
        let boundaries = field.boundaries();
        let offsets = caret_offsets(&mut self.text, typed, &boundaries, &style, s);
        let content = FieldContent {
            text: typed,
            boundaries: &boundaries,
            offsets: &offsets,
            selection: field.selection(),
            composition: field.composition(),
            caret: state.search_caret.then_some(field.caret()),
            scroll: field.scroll(),
        };
        let area = GeoRect::new(text.x, text.y, text.w, text.h);
        draw_field(
            &mut self.text,
            pm,
            &area,
            centred_baseline(&area, FIELD_TEXT),
            s,
            &content,
            &style,
        );
    }

    /// The x in points of each caret position of the search field's text ([`TextField::boundaries`]) at scale `s`.
    pub fn search_offsets(&mut self, field: &TextField, s: f32, pal: &Palette) -> Vec<f64> {
        self.text.set_scale(s);
        caret_offsets(
            &mut self.text,
            field.text(),
            &field.boundaries(),
            &search_style(pal),
            s,
        )
    }

    /// Draw the results popup of `field`: `rows` are the result rows' rectangles in popup points
    /// ([`popup::rows`]), `hovered` the row under the pointer.
    pub fn paint_popup(
        &mut self,
        pm: &mut Pixmap,
        s: f32,
        field: &SearchField,
        rows: &[Rect],
        hovered: Option<usize>,
        pal: &Palette,
    ) {
        self.text.set_scale(s);
        let size = RowSize::Medium;
        let (_, hpad) = popup::row_padding(size);
        let results = field.results();
        if results.is_empty() {
            let r = rows[0];
            self.label(
                pm,
                &Rect::new(r.x + hpad, r.y, r.w - 2.0 * hpad, r.h),
                "No results",
                Font {
                    size: 13.0,
                    weight: 400,
                },
                s,
                pal.secondary_text,
            );
            return;
        }
        for (i, (hit, r)) in results.iter().zip(rows).enumerate() {
            let selected = field.highlighted() == Some(i);
            if selected {
                fill_rounded(pm, r, RESULT_RADIUS, false, s, pal.accent);
            } else if hovered == Some(i) {
                fill_rounded(pm, r, RESULT_RADIUS, false, s, pal.quaternary);
            }
            let side = size.result_icon();
            let icon = Rect::new(r.x + hpad, r.y + (r.h - side) / 2.0, side, side);
            self.enclosure(pm, &icon, hit.pane, s, pal);
            let x = icon.right() + row::ICON_TITLE_GAP;
            let w = (r.right() - x - hpad).max(0.0);
            let (title, sub) = (
                if selected { WHITE } else { pal.text },
                if selected { WHITE } else { pal.secondary_text },
            );
            match hit.subtitle() {
                Some(section) => {
                    self.label(
                        pm,
                        &Rect::new(x, r.y, w, r.h / 2.0 + 2.0),
                        hit.pane.pane().title,
                        Font {
                            size: 13.0,
                            weight: 400,
                        },
                        s,
                        title,
                    );
                    self.label(
                        pm,
                        &Rect::new(x, r.y + r.h / 2.0, w, r.h / 2.0 - 2.0),
                        section,
                        Font {
                            size: popup::SUBTEXT_FONT,
                            weight: 400,
                        },
                        s,
                        sub,
                    );
                }
                None => self.label(
                    pm,
                    &Rect::new(x, r.y, w, r.h),
                    hit.pane.pane().title,
                    Font {
                        size: 13.0,
                        weight: 400,
                    },
                    s,
                    title,
                ),
            }
        }
    }
}

/// A pane as drawn: its sections, their layout and the content's scroll offset.
pub struct PaneView<'a> {
    pub sections: &'a [Section],
    pub layout: &'a [SectionLayout],
    /// How far the content is scrolled up (points).
    pub scroll: f64,
    /// A slider being dragged: its section, row and the value shown.
    pub dragging: Option<(usize, usize, f64)>,
}

/// The rows of a choice menu (a pop-up button's): 22 high with 5 pt padding (approximate: the menu bar's rows).
pub const MENU_ROW: f64 = 22.0;
pub const MENU_PADDING: f64 = 5.0;
/// The menu's width and its check-mark column. approximate.
pub const MENU_WIDTH: f64 = 200.0;
pub const MENU_CHECK: f64 = 22.0;

/// The rectangles of a choice menu's `count` rows, in menu points, and the menu's size.
pub fn menu_rows(count: usize) -> (Vec<Rect>, (f64, f64)) {
    let rows = (0..count)
        .map(|i| {
            Rect::new(
                MENU_PADDING,
                MENU_PADDING + i as f64 * MENU_ROW,
                MENU_WIDTH - 2.0 * MENU_PADDING,
                MENU_ROW,
            )
        })
        .collect();
    (
        rows,
        (MENU_WIDTH, 2.0 * MENU_PADDING + count as f64 * MENU_ROW),
    )
}

impl Painter {
    /// Draw the pane `view` into the content column (the toolbar is drawn over it afterwards).
    pub fn paint_pane(&mut self, pm: &mut Pixmap, s: f32, view: &PaneView<'_>, pal: &Palette) {
        let dy = -view.scroll;
        for (si, (section, l)) in view.sections.iter().zip(view.layout).enumerate() {
            if let (Some(text), Some(r)) = (&section.header, l.header) {
                self.label(
                    pm,
                    &r.offset(0.0, dy),
                    text,
                    Font {
                        size: fm::HEADER_SIZE,
                        weight: 600,
                    },
                    s,
                    pal.secondary_text,
                );
            }
            let frame = l.frame.offset(0.0, dy);
            let grid = section.rows.first().is_some_and(|r| {
                matches!(r.control, Control::Tiles { .. } | Control::Preview { .. })
            });
            if !grid {
                fill_rounded(pm, &frame, fm::BOX_RADIUS, false, s, pal.box_fill);
            }
            for (ri, (row, rl)) in section.rows.iter().zip(&l.rows).enumerate() {
                if let Control::Tiles { .. } | Control::Preview { .. } = row.control {
                    self.paint_control(pm, s, &row.control, &rl.control.offset(0.0, dy), None, pal);
                    continue;
                }
                if ri > 0 {
                    let r = rl.row.offset(0.0, dy);
                    fill_rect(
                        pm,
                        &Rect::new(
                            r.x + fm::SEPARATOR_INSET,
                            r.y,
                            r.w - fm::SEPARATOR_INSET - fm::SEPARATOR_TRAILING,
                            fm::BOX_HAIRLINE,
                        ),
                        s,
                        pal.box_separator,
                    );
                }
                self.label(
                    pm,
                    &rl.label.offset(0.0, dy),
                    &row.label,
                    Font {
                        size: fm::LABEL_SIZE,
                        weight: 400,
                    },
                    s,
                    pal.text,
                );
                // a refused change in red, else why the control is disabled
                if let Some(r) = rl.error {
                    match (&row.error, &row.disabled) {
                        (Some(error), _) => self.label(
                            pm,
                            &r.offset(0.0, dy),
                            error,
                            Font {
                                size: fm::ERROR_SIZE,
                                weight: 400,
                            },
                            s,
                            pal.red,
                        ),
                        (None, Some(reason)) => self.label(
                            pm,
                            &r.offset(0.0, dy),
                            reason,
                            Font {
                                size: fm::ERROR_SIZE,
                                weight: 400,
                            },
                            s,
                            pal.secondary_text,
                        ),
                        (None, None) => {}
                    }
                }
                let shown = view
                    .dragging
                    .filter(|(a, b, _)| (*a, *b) == (si, ri))
                    .map(|(_, _, v)| v);
                let frame = rl.control.offset(0.0, dy);
                self.paint_control(pm, s, &row.control, &frame, shown, pal);
                if row.disabled.is_some() {
                    // disabled: veiled by the box's colour (alpha 0.5 light, 0.7 dark, after 's disabled sliders)
                    let veil = [
                        pal.box_fill[0],
                        pal.box_fill[1],
                        pal.box_fill[2],
                        if pal.dark { 0.7 } else { 0.5 },
                    ];
                    fill_rect(pm, &frame.inset(-2.0, -2.0), s, veil);
                }
            }
            if let (Some(text), Some(r)) = (&section.footer, l.footer) {
                self.label(
                    pm,
                    &r.offset(0.0, dy),
                    text,
                    Font {
                        size: fm::FOOTER_SIZE,
                        weight: 400,
                    },
                    s,
                    pal.secondary_text,
                );
            }
        }
    }

    /// Draw `control` in `frame`; `shown` is a dragged slider's value.
    fn paint_control(
        &mut self,
        pm: &mut Pixmap,
        s: f32,
        control: &Control,
        frame: &Rect,
        shown: Option<f64>,
        pal: &Palette,
    ) {
        match control {
            Control::Switch { on } => {
                fill_rounded(
                    pm,
                    frame,
                    frame.h / 2.0,
                    false,
                    s,
                    if *on { pal.accent } else { pal.fill },
                );
                let d = frame.h - 4.0;
                let x = if *on {
                    frame.right() - 2.0 - d
                } else {
                    frame.x + 2.0
                };
                fill_rounded(
                    pm,
                    &Rect::new(x, frame.y + 2.0, d, d),
                    d / 2.0,
                    false,
                    s,
                    WHITE,
                );
            }
            Control::Segmented { segments, selected } => {
                fill_rounded(pm, frame, cm::RADIUS, false, s, pal.fill);
                for (i, segment) in segments.iter().enumerate() {
                    let r = Rect::new(
                        frame.x + i as f64 * cm::SEGMENT_WIDTH,
                        frame.y,
                        cm::SEGMENT_WIDTH,
                        frame.h,
                    );
                    if i == *selected {
                        fill_rounded(
                            pm,
                            &r.inset(1.0, 1.0),
                            cm::RADIUS - 1.0,
                            false,
                            s,
                            pal.control_background,
                        );
                    }
                    let w = self.text.measure(segment, 12.0 * s, 400, 0.0) / s;
                    self.label(
                        pm,
                        &Rect::new(
                            r.x + (r.w - f64::from(w)) / 2.0,
                            r.y,
                            f64::from(w) + 1.0,
                            r.h,
                        ),
                        segment,
                        Font {
                            size: 12.0,
                            weight: 400,
                        },
                        s,
                        pal.text,
                    );
                }
            }
            Control::Popup { choices, selected } => {
                fill_rounded(pm, frame, cm::RADIUS, false, s, pal.control_background);
                stroke_rounded(
                    pm,
                    &frame.inset(0.5, 0.5),
                    cm::RADIUS,
                    1.0,
                    s,
                    pal.separator,
                );
                let chevron = Rect::new(frame.right() - 18.0, frame.y, 14.0, frame.h);
                self.symbol(
                    pm,
                    &chevron,
                    "symbol.chevron.up.down",
                    10.0,
                    s,
                    pal.secondary_text,
                );
                self.label(
                    pm,
                    &Rect::new(frame.x + 8.0, frame.y, chevron.x - frame.x - 10.0, frame.h),
                    &choices[*selected],
                    Font {
                        size: 13.0,
                        weight: 400,
                    },
                    s,
                    pal.text,
                );
            }
            Control::Slider {
                value,
                stops,
                marks,
                labels,
            } => {
                let value = shown.unwrap_or(*value);
                let (a, b) = controls::slider_span(frame);
                let cy = frame.y + cm::SLIDER_KNOB / 2.0;
                fill_rounded(
                    pm,
                    &Rect::new(a, cy - cm::SLIDER_TRACK / 2.0, b - a, cm::SLIDER_TRACK),
                    cm::SLIDER_TRACK / 2.0,
                    false,
                    s,
                    pal.fill,
                );
                let ticks: Vec<f64> = match stops {
                    Some(n) => (0..*n).map(|i| Control::stop_value(i, *n)).collect(),
                    None => marks.to_vec(),
                };
                for t in ticks {
                    let x = a + (b - a) * t;
                    fill_rounded(
                        pm,
                        &Rect::new(
                            x - cm::TICK / 2.0,
                            cy + cm::SLIDER_TRACK / 2.0 + 2.0,
                            cm::TICK,
                            cm::TICK,
                        ),
                        cm::TICK / 2.0,
                        false,
                        s,
                        pal.secondary_text,
                    );
                }
                fill_rounded(
                    pm,
                    &Rect::new(
                        a,
                        cy - cm::SLIDER_TRACK / 2.0,
                        (b - a) * value,
                        cm::SLIDER_TRACK,
                    ),
                    cm::SLIDER_TRACK / 2.0,
                    false,
                    s,
                    pal.accent,
                );
                let knob = controls::slider_knob(frame, value);
                fill_rounded(pm, &knob, knob.w / 2.0, false, s, WHITE);
                stroke_rounded(
                    pm,
                    &knob.inset(0.5, 0.5),
                    knob.w / 2.0,
                    1.0,
                    s,
                    pal.separator,
                );
                let label_row = Rect::new(
                    frame.x,
                    frame.y + cm::SLIDER_KNOB,
                    frame.w,
                    cm::SLIDER_LABEL_HEIGHT,
                );
                self.label(
                    pm,
                    &label_row,
                    labels.0,
                    Font {
                        size: cm::SLIDER_LABEL_SIZE,
                        weight: 400,
                    },
                    s,
                    pal.secondary_text,
                );
                let w = f64::from(
                    self.text
                        .measure(labels.1, cm::SLIDER_LABEL_SIZE as f32 * s, 400, 0.0)
                        / s,
                );
                self.label(
                    pm,
                    &Rect::new(
                        label_row.right() - w - 1.0,
                        label_row.y,
                        w + 1.0,
                        label_row.h,
                    ),
                    labels.1,
                    Font {
                        size: cm::SLIDER_LABEL_SIZE,
                        weight: 400,
                    },
                    s,
                    pal.secondary_text,
                );
            }
            Control::Level { value } => {
                let value = shown.unwrap_or(*value).clamp(0.0, 1.0);
                fill_rounded(pm, frame, frame.h / 2.0, false, s, pal.fill);
                // the knob's centre runs from minX + 11 to maxX − 11; the fill reaches its right edge
                let knob_x = frame.x + (frame.w - cm::LEVEL_TRACK) * value;
                fill_rounded(
                    pm,
                    &Rect::new(
                        frame.x,
                        frame.y,
                        knob_x + cm::LEVEL_TRACK - frame.x,
                        frame.h,
                    ),
                    frame.h / 2.0,
                    false,
                    s,
                    pal.accent,
                );
                let knob = Rect::new(knob_x, frame.y, cm::LEVEL_TRACK, cm::LEVEL_TRACK);
                fill_rounded(pm, &knob, knob.w / 2.0, false, s, WHITE);
                stroke_rounded(
                    pm,
                    &knob.inset(0.5, 0.5),
                    knob.w / 2.0,
                    1.0,
                    s,
                    pal.separator,
                );
            }
            Control::Button { title } => {
                fill_rounded(pm, frame, cm::RADIUS, false, s, pal.control_background);
                stroke_rounded(
                    pm,
                    &frame.inset(0.5, 0.5),
                    cm::RADIUS,
                    1.0,
                    s,
                    pal.separator,
                );
                let w = f64::from(self.text.measure(title, 13.0 * s, 400, 0.0) / s);
                self.label(
                    pm,
                    &Rect::new(frame.x + (frame.w - w) / 2.0, frame.y, w + 1.0, frame.h),
                    title,
                    Font {
                        size: 13.0,
                        weight: 400,
                    },
                    s,
                    pal.text,
                );
            }
            Control::Buttons { titles } => {
                for (i, title) in titles.iter().enumerate() {
                    let r = Rect::new(
                        frame.x + i as f64 * (cm::BUTTON.0 + cm::BUTTON_GAP),
                        frame.y,
                        cm::BUTTON.0,
                        frame.h,
                    );
                    self.paint_control(
                        pm,
                        s,
                        &Control::Button {
                            title: title.clone(),
                        },
                        &r,
                        None,
                        pal,
                    );
                }
            }
            Control::Preview { image, title, note } => {
                let (pw, ph) = cm::PREVIEW;
                let picture = Rect::new(frame.x, frame.y, pw.min(frame.w), ph);
                match image.as_ref().and_then(|i| self.thumbnails.get(i)) {
                    Some(thumbnail) => fill_cover(pm, &picture, thumbnail, cm::TILE_RADIUS, s),
                    None => fill_rounded(pm, &picture, cm::TILE_RADIUS, false, s, pal.box_fill),
                }
                stroke_rounded(pm, &picture, cm::TILE_RADIUS, 1.0, s, pal.separator);
                let text_x = picture.right() + cm::PREVIEW_GAP;
                let text_w = (frame.right() - text_x).max(0.0);
                let mut y = picture.y + 8.0;
                self.label(
                    pm,
                    &Rect::new(text_x, y, text_w, 18.0),
                    title,
                    Font {
                        size: 13.0,
                        weight: 600,
                    },
                    s,
                    pal.text,
                );
                y += 20.0;
                if let Some(note) = note {
                    self.label(
                        pm,
                        &Rect::new(text_x, y, text_w, 16.0),
                        note,
                        Font {
                            size: 11.0,
                            weight: 400,
                        },
                        s,
                        pal.secondary_text,
                    );
                }
            }
            Control::Tiles { tiles, selected } => {
                let caption = controls::tile_caption_height(tiles);
                for (i, (tile, r)) in tiles
                    .iter()
                    .zip(controls::tile_rects(frame, tiles.len(), caption))
                    .enumerate()
                {
                    match self.thumbnails.get(&tile.image) {
                        Some(thumbnail) => fill_cover(pm, &r, thumbnail, cm::TILE_RADIUS, s),
                        None => fill_rounded(pm, &r, cm::TILE_RADIUS, false, s, pal.box_fill),
                    }
                    if tile.pair {
                        // a light and a dark half: the picture changes with the appearance
                        let pill = Rect::new(r.right() - 30.0, r.bottom() - 20.0, 24.0, 14.0);
                        let dark = [0.12, 0.12, 0.14, 1.0];
                        fill_rounded(pm, &pill, 7.0, false, s, [1.0, 1.0, 1.0, 0.95]);
                        fill_rect(
                            pm,
                            &Rect::new(pill.x + 12.0, pill.y + 2.0, 6.0, 10.0),
                            s,
                            dark,
                        );
                        fill_rounded(
                            pm,
                            &Rect::new(pill.x + 12.0, pill.y, 12.0, 14.0),
                            7.0,
                            false,
                            s,
                            dark,
                        );
                    }
                    if *selected == Some(i) {
                        stroke_rounded(
                            pm,
                            &r.inset(-2.0, -2.0),
                            cm::TILE_RADIUS + 2.0,
                            3.0,
                            s,
                            pal.accent,
                        );
                    } else {
                        stroke_rounded(pm, &r, cm::TILE_RADIUS, 1.0, s, pal.separator);
                    }
                    self.label(
                        pm,
                        &Rect::new(r.x, r.bottom() + 2.0, r.w, cm::TILE_TITLE_HEIGHT - 2.0),
                        &tile.title,
                        Font {
                            size: 12.0,
                            weight: if *selected == Some(i) { 600 } else { 400 },
                        },
                        s,
                        pal.text,
                    );
                    if let Some(author) = &tile.credit {
                        self.label(
                            pm,
                            &Rect::new(r.x, r.bottom() + 17.0, r.w, 14.0),
                            author,
                            Font {
                                size: 11.0,
                                weight: 400,
                            },
                            s,
                            pal.secondary_text,
                        );
                    }
                }
            }
            Control::Text { value } => {
                let w = f64::from(self.text.measure(value, 13.0 * s, 400, 0.0) / s).min(frame.w);
                self.label(
                    pm,
                    &Rect::new(frame.right() - w - 1.0, frame.y, w + 1.0, frame.h),
                    value,
                    Font {
                        size: 13.0,
                        weight: 400,
                    },
                    s,
                    pal.secondary_text,
                );
            }
        }
    }

    /// Draw `sheet` laid out as `l` over the content column `content`, which it dims.
    pub fn paint_sheet(
        &mut self,
        pm: &mut Pixmap,
        s: f32,
        sheet: &Sheet,
        l: &SheetLayout,
        content: &Rect,
        pal: &Palette,
    ) {
        fill_rect(
            pm,
            content,
            s,
            [0.0, 0.0, 0.0, if pal.dark { 0.4 } else { 0.2 }],
        );
        fill_rounded(
            pm,
            &l.frame,
            crate::sheet::metrics::RADIUS,
            false,
            s,
            pal.window_background,
        );
        stroke_rounded(
            pm,
            &l.frame.inset(0.5, 0.5),
            crate::sheet::metrics::RADIUS,
            1.0,
            s,
            pal.separator,
        );
        self.label(
            pm,
            &l.title,
            &sheet.title(),
            Font {
                size: 13.0,
                weight: 600,
            },
            s,
            pal.text,
        );
        let failed = matches!(
            &sheet.kind,
            Kind::Passphrase {
                failure: Some(_),
                ..
            }
        );
        self.label(
            pm,
            &l.message,
            &sheet.message(),
            Font {
                size: 11.0,
                weight: 400,
            },
            s,
            if failed { pal.red } else { pal.secondary_text },
        );
        if let (Some(field), Some(masked)) = (l.field, sheet.field_text()) {
            fill_rounded(pm, &field, 5.0, false, s, pal.control_background);
            stroke_rounded(
                pm,
                &field.inset(-1.5, -1.5),
                6.5,
                3.0,
                s,
                [pal.accent[0], pal.accent[1], pal.accent[2], 0.5],
            );
            self.label(
                pm,
                &field.inset(6.0, 0.0),
                &masked,
                Font {
                    size: 13.0,
                    weight: 400,
                },
                s,
                pal.text,
            );
        }
        self.paint_control(
            pm,
            s,
            &Control::Button {
                title: "Cancel".into(),
            },
            &l.cancel,
            None,
            pal,
        );
        match &sheet.kind {
            // the default button uses the accent (no pulse in this build)
            Kind::Passphrase { .. }
            | Kind::Resolution { .. }
            | Kind::Picker { .. }
            | Kind::Pairing { .. } => {
                fill_rounded(pm, &l.action, cm::RADIUS, false, s, pal.accent);
                let w = f64::from(self.text.measure(sheet.action_title(), 13.0 * s, 400, 0.0) / s);
                self.label(
                    pm,
                    &Rect::new(
                        l.action.x + (l.action.w - w) / 2.0,
                        l.action.y,
                        w + 1.0,
                        l.action.h,
                    ),
                    sheet.action_title(),
                    Font {
                        size: 13.0,
                        weight: 400,
                    },
                    s,
                    WHITE,
                );
            }
            Kind::Forget { .. } => {
                fill_rounded(pm, &l.action, cm::RADIUS, false, s, pal.control_background);
                stroke_rounded(
                    pm,
                    &l.action.inset(0.5, 0.5),
                    cm::RADIUS,
                    1.0,
                    s,
                    pal.separator,
                );
                let w = f64::from(self.text.measure(sheet.action_title(), 13.0 * s, 400, 0.0) / s);
                self.label(
                    pm,
                    &Rect::new(
                        l.action.x + (l.action.w - w) / 2.0,
                        l.action.y,
                        w + 1.0,
                        l.action.h,
                    ),
                    sheet.action_title(),
                    Font {
                        size: 13.0,
                        weight: 400,
                    },
                    s,
                    pal.red,
                );
            }
        }
        if let (
            Kind::Picker {
                chooser,
                top,
                hovered,
            },
            Some(list),
            Some(up),
        ) = (&sheet.kind, l.list, l.up)
        {
            self.paint_control(
                pm,
                s,
                &Control::Button { title: "Up".into() },
                &up,
                None,
                pal,
            );
            fill_rounded(pm, &list, cm::RADIUS, false, s, pal.control_background);
            stroke_rounded(pm, &list.inset(0.5, 0.5), cm::RADIUS, 1.0, s, pal.separator);
            let entries = chooser.0.entries();
            if entries.is_empty() {
                self.label(
                    pm,
                    &list,
                    if chooser.0.loading() {
                        "Loading\u{2026}"
                    } else {
                        "Nothing to show here"
                    },
                    Font {
                        size: 12.0,
                        weight: 400,
                    },
                    s,
                    pal.secondary_text,
                );
            }
            for (k, (i, entry)) in entries
                .iter()
                .enumerate()
                .skip(*top)
                .take(crate::sheet::metrics::PICKER_ROWS)
                .enumerate()
            {
                let row = Rect::new(
                    list.x + 4.0,
                    list.y + k as f64 * crate::sheet::metrics::PICKER_ROW,
                    list.w - 8.0,
                    crate::sheet::metrics::PICKER_ROW,
                );
                let on = chooser.0.highlighted() == Some(i);
                if on {
                    fill_rounded(pm, &row, 5.0, false, s, pal.accent);
                } else if *hovered == Some(i) {
                    fill_rounded(pm, &row, 5.0, false, s, pal.box_fill);
                }
                self.raster_icon(
                    pm,
                    &Rect::new(row.x + 6.0, row.y + 3.0, 16.0, 16.0),
                    if entry.is_dir {
                        "folder"
                    } else {
                        "document.image"
                    },
                    s,
                );
                let color = if on { WHITE } else { pal.text };
                self.label(
                    pm,
                    &Rect::new(row.x + 28.0, row.y, row.w - 36.0, row.h),
                    &entry.name,
                    Font {
                        size: 12.0,
                        weight: 400,
                    },
                    s,
                    color,
                );
            }
        }
    }

    /// Draw a choice menu of `choices` with `selected` checked and `hovered` highlighted.
    pub fn paint_menu(
        &mut self,
        pm: &mut Pixmap,
        s: f32,
        choices: &[String],
        selected: usize,
        hovered: Option<usize>,
        pal: &Palette,
    ) {
        self.text.set_scale(s);
        let (rows, _) = menu_rows(choices.len());
        for (i, (choice, r)) in choices.iter().zip(&rows).enumerate() {
            let hot = hovered == Some(i);
            if hot {
                fill_rounded(pm, r, 4.0, false, s, pal.accent);
            }
            let color = if hot { WHITE } else { pal.text };
            if i == selected {
                self.symbol(
                    pm,
                    &Rect::new(r.x, r.y, MENU_CHECK, r.h),
                    "symbol.checkmark",
                    11.0,
                    s,
                    color,
                );
            }
            self.label(
                pm,
                &Rect::new(r.x + MENU_CHECK, r.y, r.w - MENU_CHECK - 4.0, r.h),
                choice,
                Font {
                    size: 13.0,
                    weight: 400,
                },
                s,
                color,
            );
        }
    }
}

/// The result hit by a click in the popup at `y` (popup points) over `rows`.
pub fn popup_hit(rows: &[Rect], results: &[Hit], x: f64, y: f64) -> Option<usize> {
    rows.iter()
        .take(results.len())
        .position(|r| r.contains(crate::Point { x, y }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_palette_comes_from_the_tokens_of_both_appearances() {
        let theme = Theme::load().expect("the design tokens of the repository");
        let light = Palette::from_theme(&theme, Appearance::Light).unwrap();
        let dark = Palette::from_theme(&theme, Appearance::Dark).unwrap();
        assert!(!light.dark && dark.dark);
        assert_ne!(light.window_background, dark.window_background);
        assert_eq!(light.enclosure(Enclosure::Black), [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(light.enclosure(Enclosure::Blue), light.blue);
        // works the recipe through to (65, 158, 253) at luminance 0.95
        let bytes = light.sidebar_selection.map(|c| (c * 255.0).round() as u8);
        assert_eq!(bytes, [65, 158, 253, 255]);
        assert_eq!(dark.sidebar_selection, dark.accent);
    }

    /// The search field of `search` painted at scale 1, its caret showing when `caret`.
    fn paint_field(
        painter: &mut Painter,
        layout: &WindowLayout,
        search: &SearchField,
        caret: bool,
        pal: &Palette,
    ) -> Pixmap {
        let history = History::new(PaneId::Appearance);
        let mut pm = Pixmap::new(
            layout.sidebar.w as u32,
            (layout.search.bottom() + 4.0) as u32,
        )
        .unwrap();
        let state = WindowState {
            selected: PaneId::Appearance,
            has_battery: None,
            active: true,
            history: &history,
            search,
            search_focused: true,
            search_caret: caret,
        };
        painter.paint_search_field(&mut pm, 1.0, layout, &state, pal);
        pm
    }

    #[test]
    fn selected_search_text_lies_on_the_selected_text_token_and_the_caret_follows_the_text() {
        let theme = Theme::load().expect("the design tokens of the repository");
        let mut painter = Painter::new(&Assets::builtin()).unwrap();
        let layout = WindowLayout::new(
            crate::window::default_size(1.0),
            1.0,
            RowSize::Medium,
            &crate::sidebar::groups(),
            false,
        );
        let f = layout.search;
        // the row above the glyphs, inside the selection band and the caret
        let y = (f.y + f.h / 2.0 - FIELD_TEXT / 2.0) as u32;
        for appearance in [Appearance::Light, Appearance::Dark] {
            let pal = Palette::from_theme(&theme, appearance).unwrap();
            assert_eq!(
                pal.text_selection.map(|c| (c * 255.0).round() as u8),
                [0, 122, 255, 89],
                "{appearance:?}"
            );
            let mut search = SearchField::default();
            search.edit(|t| t.insert("Mouse"));
            let row = |pm: &Pixmap| -> Vec<[u8; 4]> {
                (f.x as u32 + 1..f.right() as u32 - 1)
                    .map(|x| {
                        let p = pm.pixel(x, y).unwrap().demultiply();
                        [p.red(), p.green(), p.blue(), p.alpha()]
                    })
                    .collect()
            };
            let bluish = |row: &[[u8; 4]]| {
                row.iter()
                    .filter(|p| p[3] > 64 && p[2] > p[0].saturating_add(64))
                    .count()
            };
            let plain = row(&paint_field(&mut painter, &layout, &search, false, &pal));
            search.edit(TextField::select_all);
            let selected = row(&paint_field(&mut painter, &layout, &search, false, &pal));
            assert_eq!(
                bluish(&plain),
                0,
                "{appearance:?}: no band without a selection"
            );
            assert!(bluish(&selected) > 20, "{appearance:?}: the band shows");
            search.edit(|t| t.select(5..5));
            let with_caret = row(&paint_field(&mut painter, &layout, &search, true, &pal));
            let changed: Vec<usize> = (0..plain.len())
                .filter(|i| plain[*i] != with_caret[*i])
                .collect();
            let text = search_text_area(&f, true);
            let end = painter.search_offsets(search.field(), 1.0, &pal)[5];
            assert_eq!(
                changed,
                vec![(text.x.round() + end.round()) as usize - f.x as usize - 1],
                "{appearance:?}: one caret column after the text"
            );
        }
    }

    #[test]
    fn a_ten_thousand_character_paste_is_cut_to_the_field_and_redrawn_quickly() {
        let theme = Theme::load().expect("the design tokens of the repository");
        let pal = Palette::from_theme(&theme, Appearance::Light).unwrap();
        let mut painter = Painter::new(&Assets::builtin()).unwrap();
        let layout = WindowLayout::new(
            crate::window::default_size(1.0),
            1.0,
            RowSize::Medium,
            &crate::sidebar::groups(),
            false,
        );
        let mut search = SearchField::default();
        let start = std::time::Instant::now();
        search.edit(|f| f.paste(&"word ".repeat(2_000)));
        assert_eq!(search.text().len(), crate::search::MAX_LEN);
        paint_field(&mut painter, &layout, &search, true, &pal);
        let took = start.elapsed();
        assert!(took < std::time::Duration::from_secs(2), "{took:?}");
    }

    #[test]
    fn the_window_requires_every_pane_symbol_and_its_chrome() {
        let all = required_symbols();
        for p in &pane::PANES {
            assert!(all.contains(&p.symbol), "{}", p.symbol);
        }
        assert!(all.contains(&"symbol.search") && all.contains(&"symbol.xmark.circle"));
    }

    #[test]
    fn the_content_and_the_toolbar_are_filled_with_the_control_background_in_both_appearances() {
        let theme = Theme::load().expect("the design tokens of the repository");
        let mut painter = Painter::new(&Assets::builtin()).expect("the painter");
        let size = crate::window::default_size(1.0);
        let layout = WindowLayout::new(
            size,
            1.0,
            RowSize::default(),
            &crate::sidebar::groups(),
            false,
        );
        let history = History::new(PaneId::Wifi);
        let search = SearchField::default();
        let state = WindowState {
            selected: PaneId::Wifi,
            has_battery: None,
            active: true,
            history: &history,
            search: &search,
            search_focused: false,
            search_caret: false,
        };
        let view = PaneView {
            sections: &[],
            layout: &[],
            scroll: 0.0,
            dragging: None,
        };
        for appearance in [Appearance::Light, Appearance::Dark] {
            let pal = Palette::from_theme(&theme, appearance).unwrap();
            let mut pm = Pixmap::new(size.w as u32, size.h as u32).unwrap();
            painter.paint_window(&mut pm, 1.0, &layout, &state, &view, &pal);
            let expected = pal.control_background.map(|c| (c * 255.0).round() as u8);
            for (name, r) in [("content", layout.content), ("toolbar", layout.toolbar)] {
                let px = pm
                    .pixel((r.x + r.w / 2.0) as u32, (r.y + r.h / 2.0) as u32)
                    .unwrap();
                assert_eq!(
                    [px.red(), px.green(), px.blue(), px.alpha()],
                    expected,
                    "{name} in {appearance:?}"
                );
            }
        }
    }
}
