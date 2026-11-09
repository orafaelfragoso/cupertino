//! CPU drawing of the prompt into `tiny-skia` pixmaps at a surface's device scale: the scrim that covers each output,
//! and the panel. Every colour comes from the design tokens of the session's appearance.

use std::f64::consts::PI;
use std::fmt;

use cupertino_assets::{AssetError, Assets, FontRole};
use cupertino_decorations::field::{centred_baseline, draw_field, FieldContent, FieldStyle};
use cupertino_decorations::palette::Rgba;
use cupertino_decorations::shape::rounded_path;
use cupertino_decorations::text::TextRenderer;
use cupertino_geometry::{CornerStyle, Insets, Point, Radii, Rect};
use cupertino_icons::SystemIcons;
use cupertino_theme::{AccentColor, Appearance, DesignTokens, SemanticColor};
use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, PixmapPaint, Stroke, Transform,
};

use crate::model::Phase;
use crate::panel::{
    field_label, Control, Status, View, AUTHENTICATE, BUTTON_INSET, CANCEL, DETAILS, LABEL_COLUMN,
    MAX_MESSAGE_LINES, RADIUS, TITLE, USER,
};

/// The symbol at the top of the panel (approximate).
pub const SYMBOL: &str = "symbol.lock";
const TEXT_SIZE: f64 = 13.0;
const CAPTION_SIZE: f64 = 11.0;
const TITLE_WEIGHT: u16 = 700;
const WEIGHT: u16 = 400;
const CONTROL_RADIUS: f64 = 5.0;
const FIELD_INSET: f64 = 6.0;
const FOCUS_RING: f64 = 3.0;
const FAILURE_OUTLINE: f64 = 2.0;
/// The spinner that stands in for Authenticate's title while the answer is checked (approximate proportions).
const SPINNER: f64 = 12.0;
const SPOKES: usize = 12;
const SPINNER_PERIOD: f64 = 1.0;
/// A typed character of a secret.
const BULLET: char = '•';
/// The disclosure's and the pop-up's chevrons (approximate).
const CHEVRON: f64 = 3.5;
const CHEVRON_STROKE: f64 = 1.5;
const POPUP_CHEVRONS: f64 = 16.0;

/// A colour token missing from the design tokens of an appearance.
#[derive(Debug)]
pub struct MissingColor(pub &'static str);

impl fmt::Display for MissingColor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the design tokens have no colour {}", self.0)
    }
}

impl std::error::Error for MissingColor {}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub scrim: Rgba,
    pub background: Rgba,
    pub separator: Rgba,
    pub text: Rgba,
    pub secondary: Rgba,
    pub placeholder: Rgba,
    pub red: Rgba,
    pub field: Rgba,
    pub border: Rgba,
    pub focus: Rgba,
    pub accent: Rgba,
    pub control: Rgba,
    pub control_text: Rgba,
    pub disabled_text: Rgba,
}

impl Palette {
    pub fn from_tokens(
        tokens: &DesignTokens,
        appearance: Appearance,
    ) -> Result<Self, MissingColor> {
        let named = |name: &'static str| {
            tokens
                .color(appearance, name)
                .map(|c| c.to_array())
                .ok_or(MissingColor(name))
        };
        let get = |c: SemanticColor| named(c.name());
        Ok(Palette {
            scrim: get(SemanticColor::AuthenticationScrim)?,
            background: get(SemanticColor::WindowBackground)?,
            separator: get(SemanticColor::Grid)?,
            text: get(SemanticColor::Label)?,
            secondary: get(SemanticColor::SecondaryLabel)?,
            placeholder: get(SemanticColor::PlaceholderText)?,
            red: get(SemanticColor::SystemRed)?,
            field: get(SemanticColor::TextBackground)?,
            border: get(SemanticColor::ContainerBorder)?,
            focus: get(SemanticColor::KeyboardFocusIndicator)?,
            accent: named(AccentColor::Blue.name())?,
            control: named("control")?,
            control_text: get(SemanticColor::ControlText)?,
            disabled_text: get(SemanticColor::DisabledControlText)?,
        })
    }
}

fn skia(c: Rgba) -> Color {
    Color::from_rgba(
        c[0].clamp(0.0, 1.0),
        c[1].clamp(0.0, 1.0),
        c[2].clamp(0.0, 1.0),
        c[3].clamp(0.0, 1.0),
    )
    .expect("clamped channels are colours")
}

fn paint(c: Rgba) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(skia(c));
    p.anti_alias = true;
    p
}

fn with_alpha(c: Rgba, alpha: f64) -> Rgba {
    [c[0], c[1], c[2], c[3] * alpha as f32]
}

fn line(width: f64, s: f64) -> Stroke {
    Stroke {
        width: (width * s) as f32,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    }
}

/// Fill the whole of `pixmap` with the scrim at `opacity` (0 to 1, as it fades).
pub fn draw_scrim(pixmap: &mut Pixmap, palette: &Palette, opacity: f64) {
    pixmap.fill(skia(scrim_color(palette, opacity)));
}

/// The colour the scrim shows all over at `opacity` (0 to 1) of its own.
pub fn scrim_color(palette: &Palette, opacity: f64) -> Rgba {
    with_alpha(palette.scrim, opacity)
}

/// Draws the panel; it keeps the fonts and the symbol between frames.
pub struct Painter {
    pub palette: Palette,
    text: TextRenderer,
    icons: SystemIcons,
}

impl Painter {
    pub fn new(palette: Palette, assets: &Assets) -> Result<Self, AssetError> {
        Ok(Painter {
            palette,
            text: TextRenderer::new(assets, FontRole::Ui)?,
            icons: SystemIcons::required(assets, &[], &[SYMBOL]),
        })
    }

    /// The width of `text` in points.
    pub fn measure(&mut self, text: &str, size: f64, weight: u16) -> f64 {
        self.text.set_scale(1.0);
        f64::from(self.text.measure(text, size as f32, weight, 0.0))
    }

    /// The widths of Cancel's and Authenticate's titles.
    pub fn button_titles(&mut self) -> (f64, f64) {
        (
            self.measure(CANCEL, TEXT_SIZE, WEIGHT),
            self.measure(AUTHENTICATE, TEXT_SIZE, WEIGHT),
        )
    }

    /// `message` broken at spaces into lines of at most `width` points, at most [`MAX_MESSAGE_LINES`] of them; the
    /// last one ends in an ellipsis when the message goes on. A word longer than a line is cut with an ellipsis.
    pub fn wrap(&mut self, message: &str, width: f64) -> Vec<String> {
        let words: Vec<&str> = message.split_whitespace().collect();
        let mut lines: Vec<String> = Vec::new();
        let mut next = 0;
        while next < words.len() {
            if lines.len() == MAX_MESSAGE_LINES {
                let last = lines.pop().expect("full");
                let rest = words[next..].join(" ");
                lines.push(self.fit(&format!("{last} {rest}"), width));
                break;
            }
            let mut line = words[next].to_owned();
            next += 1;
            while let Some(word) = words.get(next) {
                let joined = format!("{line} {word}");
                if self.measure(&joined, TEXT_SIZE, WEIGHT) > width {
                    break;
                }
                line = joined;
                next += 1;
            }
            lines.push(self.fit(&line, width));
        }
        lines
    }

    fn fit(&mut self, text: &str, width: f64) -> String {
        self.text.set_scale(1.0);
        self.text
            .fit(text, TEXT_SIZE as f32, WEIGHT, 0.0, width as f32)
            .0
    }

    /// `text` cut in its middle with an ellipsis to fit `width` points at caption size: a path keeps its start and
    /// its file name.
    fn fit_middle(&mut self, text: &str, width: f64) -> String {
        if self.measure(text, CAPTION_SIZE, WEIGHT) <= width {
            return text.to_owned();
        }
        let chars: Vec<char> = text.chars().collect();
        let cut = |keep: usize| {
            let head = keep.div_ceil(2);
            let tail = keep / 2;
            let mut s: String = chars[..head].iter().collect();
            s.push('…');
            s.extend(&chars[chars.len() - tail..]);
            s
        };
        let (mut lo, mut hi) = (0, chars.len());
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if self.measure(&cut(mid), CAPTION_SIZE, WEIGHT) <= width {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        cut(lo)
    }

    /// Draw `view` over the whole of `pixmap` at `scale` device pixels per point; `elapsed` is the seconds since the
    /// answer went to the helper, for the spinner.
    pub fn draw(&mut self, view: &View<'_>, pixmap: &mut Pixmap, scale: f64, elapsed: f64) {
        let p = self.palette;
        let f = view.frames;
        let s = scale;
        self.text.set_scale(s as f32);
        let body = Rect::new(0.0, 0.0, f.width, f.height);
        self.rounded(pixmap, body, RADIUS, s, p.background);
        self.rounded_stroke(pixmap, body.outset(-0.5), RADIUS - 0.5, 1.0, s, p.separator);
        let px = (f.symbol.width() * s).round() as u32;
        let symbol = self.icons.symbol(SYMBOL, px, p.secondary);
        pixmap.draw_pixmap(
            (f.symbol.x() * s).round() as i32,
            (f.symbol.y() * s).round() as i32,
            symbol.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
        self.centred(pixmap, TITLE, f.title, TEXT_SIZE, TITLE_WEIGHT, s, p.text);
        for (text, frame) in view.message.iter().zip(&f.message) {
            self.centred(pixmap, text, *frame, TEXT_SIZE, WEIGHT, s, p.secondary);
        }
        let prompt = view.prompt;
        let checking = prompt.phase == Phase::Checking;
        let focus = view.panel.focus;
        if let (Some(user), Some(frame)) = (view.user, f.user) {
            let label = Rect::new(frame.x(), frame.y(), LABEL_COLUMN, frame.height());
            self.leading(pixmap, USER, label, TEXT_SIZE, s, p.text);
            let button = Rect::new(
                frame.x() + LABEL_COLUMN,
                frame.y(),
                frame.width() - LABEL_COLUMN,
                frame.height(),
            );
            self.bordered(pixmap, button, s, p.control);
            let title = Rect::new(
                button.x() + BUTTON_INSET,
                button.y(),
                button.width() - BUTTON_INSET - POPUP_CHEVRONS,
                button.height(),
            );
            let fitted = self.fit(user, title.width());
            self.leading(pixmap, &fitted, title, TEXT_SIZE, s, p.control_text);
            let c = Point::new(button.max_x() - POPUP_CHEVRONS / 2.0, button.center().y);
            self.chevron(
                pixmap,
                Point::new(c.x, c.y - 2.5),
                -PI / 2.0,
                s,
                p.control_text,
            );
            self.chevron(
                pixmap,
                Point::new(c.x, c.y + 2.5),
                PI / 2.0,
                s,
                p.control_text,
            );
            if focus == Control::User {
                self.ring(pixmap, button, s);
            }
        }
        self.field(view, pixmap, s);
        for (status, frame) in view.status.iter().zip(&f.status) {
            let color = match status {
                Status::Error(_) => p.red,
                Status::Info(_) | Status::CapsLock => p.secondary,
            };
            self.leading(pixmap, status.text(), *frame, CAPTION_SIZE, s, color);
        }
        let chevron = Point::new(f.details.x() + CHEVRON, f.details.center().y);
        let turn = if view.panel.details { PI / 2.0 } else { 0.0 };
        self.chevron(pixmap, chevron, turn, s, p.secondary);
        let title = Rect::new(
            f.details.x() + 4.0 * CHEVRON,
            f.details.y(),
            f.details.width() - 4.0 * CHEVRON,
            f.details.height(),
        );
        self.leading(pixmap, DETAILS, title, CAPTION_SIZE, s, p.secondary);
        if focus == Control::Details {
            let width = self.measure(DETAILS, CAPTION_SIZE, WEIGHT) + 4.0 * CHEVRON;
            let ring = Rect::new(f.details.x(), f.details.y(), width, f.details.height());
            self.ring(pixmap, ring, s);
        }
        if view.panel.details {
            for ((label, value), row) in view.details.iter().zip(&f.detail_rows) {
                let left = Rect::new(row.x(), row.y(), LABEL_COLUMN, row.height());
                self.leading(pixmap, label, left, CAPTION_SIZE, s, p.secondary);
                let right = Rect::new(
                    row.x() + LABEL_COLUMN,
                    row.y(),
                    row.width() - LABEL_COLUMN,
                    row.height(),
                );
                let fitted = self.fit_middle(value, right.width());
                self.leading(pixmap, &fitted, right, CAPTION_SIZE, s, p.text);
            }
        }
        self.bordered(pixmap, f.cancel, s, p.control);
        self.centred(
            pixmap,
            CANCEL,
            f.cancel,
            TEXT_SIZE,
            WEIGHT,
            s,
            p.control_text,
        );
        if focus == Control::Cancel {
            self.ring(pixmap, f.cancel, s);
        }
        let enabled = prompt.can_answer();
        if enabled || checking {
            self.rounded(pixmap, f.authenticate, CONTROL_RADIUS, s, p.accent);
        } else {
            self.bordered(pixmap, f.authenticate, s, p.control);
        }
        if checking {
            self.spinner(pixmap, f.authenticate.center(), s, elapsed);
        } else {
            let color = if enabled {
                [1.0, 1.0, 1.0, 1.0]
            } else {
                p.disabled_text
            };
            self.centred(
                pixmap,
                AUTHENTICATE,
                f.authenticate,
                TEXT_SIZE,
                WEIGHT,
                s,
                color,
            );
        }
        if focus == Control::Authenticate {
            self.ring(pixmap, f.authenticate, s);
        }
    }

    fn field(&mut self, view: &View<'_>, pixmap: &mut Pixmap, s: f64) {
        let p = self.palette;
        let prompt = view.prompt;
        let frame = view.frames.field;
        self.rounded(pixmap, frame, CONTROL_RADIUS, s, p.field);
        if prompt.error.is_some() {
            self.rounded_stroke(
                pixmap,
                frame.outset(FAILURE_OUTLINE / 2.0),
                CONTROL_RADIUS + FAILURE_OUTLINE / 2.0,
                FAILURE_OUTLINE,
                s,
                p.red,
            );
        } else {
            self.rounded_stroke(
                pixmap,
                frame.outset(-0.5),
                CONTROL_RADIUS - 0.5,
                1.0,
                s,
                p.border,
            );
            if view.panel.focus == Control::Field {
                self.ring(pixmap, frame, s);
            }
        }
        let area = frame.inset(Insets::new(0.0, FIELD_INSET, 0.0, FIELD_INSET));
        let shown = match prompt.shown_field() {
            Some(text) => text.to_owned(),
            None => std::iter::repeat_n(BULLET, prompt.field_len()).collect(),
        };
        let checking = prompt.phase == Phase::Checking;
        if shown.is_empty() {
            let label = field_label(&prompt.question);
            self.leading(pixmap, &label, area, TEXT_SIZE, s, p.placeholder);
        }
        let style = FieldStyle {
            size: TEXT_SIZE,
            weight: WEIGHT,
            letter_spacing: 0.0,
            text: if checking { p.disabled_text } else { p.text },
            selection: p.accent,
            caret: p.text,
            caret_width: 1.0,
        };
        let boundaries = [0, shown.len()];
        let offsets = cupertino_decorations::field::caret_offsets(
            &mut self.text,
            &shown,
            &boundaries,
            &style,
            s as f32,
        );
        let caret = (view.panel.focus == Control::Field && prompt.phase == Phase::Asking)
            .then_some(shown.len());
        draw_field(
            &mut self.text,
            pixmap,
            &area,
            centred_baseline(&area, TEXT_SIZE),
            s as f32,
            &FieldContent {
                text: &shown,
                boundaries: &boundaries,
                offsets: &offsets,
                selection: 0..0,
                composition: None,
                caret,
                scroll: (offsets[1] + 1.0 - area.width()).max(0.0),
            },
            &style,
        );
        self.text.set_scale(s as f32);
    }

    #[allow(clippy::too_many_arguments)]
    fn centred(
        &mut self,
        pixmap: &mut Pixmap,
        text: &str,
        frame: Rect,
        size: f64,
        weight: u16,
        s: f64,
        color: Rgba,
    ) {
        let width = self.measure(text, size, weight);
        self.text.set_scale(s as f32);
        let x = frame.center().x - width / 2.0;
        self.text.draw(
            pixmap,
            text,
            (x * s) as f32,
            (centred_baseline(&frame, size) * s) as f32,
            (size * s) as f32,
            weight,
            0.0,
            color,
        );
    }

    fn leading(
        &mut self,
        pixmap: &mut Pixmap,
        text: &str,
        frame: Rect,
        size: f64,
        s: f64,
        color: Rgba,
    ) {
        self.text.set_scale(s as f32);
        self.text.draw(
            pixmap,
            text,
            (frame.x() * s) as f32,
            (centred_baseline(&frame, size) * s) as f32,
            (size * s) as f32,
            WEIGHT,
            0.0,
            color,
        );
    }

    fn rounded(&self, pixmap: &mut Pixmap, r: Rect, radius: f64, s: f64, c: Rgba) {
        if let Some(path) = rounded_path(r, Radii::uniform(radius), CornerStyle::Circular, s) {
            pixmap.fill_path(
                &path,
                &paint(c),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }

    fn rounded_stroke(
        &self,
        pixmap: &mut Pixmap,
        r: Rect,
        radius: f64,
        width: f64,
        s: f64,
        c: Rgba,
    ) {
        if let Some(path) = rounded_path(r, Radii::uniform(radius), CornerStyle::Circular, s) {
            pixmap.stroke_path(
                &path,
                &paint(c),
                &line(width, s),
                Transform::identity(),
                None,
            );
        }
    }

    /// A push button's or a pop-up's body: the control fill with its border.
    fn bordered(&self, pixmap: &mut Pixmap, r: Rect, s: f64, fill: Rgba) {
        self.rounded(pixmap, r, CONTROL_RADIUS, s, fill);
        self.rounded_stroke(
            pixmap,
            r.outset(-0.5),
            CONTROL_RADIUS - 0.5,
            1.0,
            s,
            self.palette.border,
        );
    }

    /// The keyboard focus ring around `r`.
    fn ring(&self, pixmap: &mut Pixmap, r: Rect, s: f64) {
        self.rounded_stroke(
            pixmap,
            r.outset(FOCUS_RING / 2.0),
            CONTROL_RADIUS + FOCUS_RING / 2.0,
            FOCUS_RING,
            s,
            self.palette.focus,
        );
    }

    /// A chevron pointing right, turned by `turn` radians, centred on `c`.
    fn chevron(&self, pixmap: &mut Pixmap, c: Point, turn: f64, s: f64, color: Rgba) {
        let (sin, cos) = turn.sin_cos();
        let at = |x: f64, y: f64| {
            (
                ((c.x + x * cos - y * sin) * s) as f32,
                ((c.y + x * sin + y * cos) * s) as f32,
            )
        };
        let half = CHEVRON / 2.0;
        let mut pb = PathBuilder::new();
        let (x, y) = at(-half / 2.0, -half);
        pb.move_to(x, y);
        let (x, y) = at(half / 2.0, 0.0);
        pb.line_to(x, y);
        let (x, y) = at(-half / 2.0, half);
        pb.line_to(x, y);
        if let Some(path) = pb.finish() {
            pixmap.stroke_path(
                &path,
                &paint(color),
                &line(CHEVRON_STROKE, s),
                Transform::identity(),
                None,
            );
        }
    }

    /// Twelve spokes around `c`, the brightest turning once per second.
    fn spinner(&self, pixmap: &mut Pixmap, c: Point, s: f64, elapsed: f64) {
        let head = ((elapsed / SPINNER_PERIOD).fract() * SPOKES as f64) as usize;
        for spoke in 0..SPOKES {
            let angle = 2.0 * PI * spoke as f64 / SPOKES as f64 - PI / 2.0;
            let (sin, cos) = angle.sin_cos();
            let (inner, outer) = (SPINNER * 0.25, SPINNER * 0.5);
            let behind = (head + SPOKES - spoke) % SPOKES;
            let alpha = 1.0 - 0.75 * behind as f64 / SPOKES as f64;
            let mut pb = PathBuilder::new();
            pb.move_to(
                ((c.x + inner * cos) * s) as f32,
                ((c.y + inner * sin) * s) as f32,
            );
            pb.line_to(
                ((c.x + outer * cos) * s) as f32,
                ((c.y + outer * sin) * s) as f32,
            );
            if let Some(path) = pb.finish() {
                pixmap.stroke_path(
                    &path,
                    &paint([1.0, 1.0, 1.0, alpha as f32]),
                    &line(1.5, s),
                    Transform::identity(),
                    None,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests;
