//! The keyring prompt's panel drawn with the authentication prompt's painter: its body, symbol, text styles, fields,
//! focus rings and capsule buttons, so both prompts look alike.

use cupertino_decorations::field::{
    caret_offsets, centred_baseline, draw_field, FieldContent, FieldStyle,
};
use cupertino_geometry::{Insets, Rect};
use tiny_skia::{PathBuilder, Pixmap, PixmapPaint, Transform};

use crate::keyring::model::{Field, Kind, Phase, Prompt};
use crate::keyring::panel::{
    Content, Control, Frames, Panel, Status, View, CHECK_BOX, CHECK_GAP, CONFIRM, MAX_CHOICE_LINES,
    MAX_DESCRIPTION_LINES, MAX_HEADING_LINES, PASSWORD,
};
use crate::panel::{BODY_LINE, RADIUS, TEXT_INSET, TEXT_WIDTH};
use crate::render::{
    line, paint, Painter, BULLET, BUTTON_RADIUS, CAPTION_SIZE, CONTROL_RADIUS, FAILURE_OUTLINE,
    FIELD_INSET, FOCUS_RING, SYMBOL, TEXT_SIZE, TITLE_WEIGHT, WEIGHT,
};

/// What the panel shows for a prompt, wrapped and measured: its frames and its text.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub frames: Frames,
    pub heading: Vec<String>,
    pub description: Vec<String>,
    pub status: Vec<Status>,
    pub choice: Vec<String>,
    pub cancel_title: String,
    pub continue_title: String,
}

impl Layout {
    /// Lay `prompt` out with `painter`'s fonts; `caps_lock` adds its line under a password field.
    pub fn new(painter: &mut Painter, prompt: &Prompt, caps_lock: bool) -> Layout {
        let props = &prompt.properties;
        let heading =
            painter.wrap_lines(props.heading(), TEXT_WIDTH, TITLE_WEIGHT, MAX_HEADING_LINES);
        let description = painter.wrap_lines(
            &props.description,
            TEXT_WIDTH,
            WEIGHT,
            MAX_DESCRIPTION_LINES,
        );
        let status = Status::of(prompt, caps_lock);
        let choice = if prompt.has_choice() {
            let width = TEXT_WIDTH + 2.0 * TEXT_INSET - CHECK_BOX - CHECK_GAP;
            painter.wrap_lines(&props.choice_title(), width, WEIGHT, MAX_CHOICE_LINES)
        } else {
            Vec::new()
        };
        let cancel_title = props.cancel_title();
        let continue_title = props.continue_title();
        let frames = Frames::new(&Content {
            heading_lines: heading.len(),
            description_lines: description.len(),
            password: prompt.kind == Kind::Password,
            confirm: prompt.confirms(),
            choice: prompt.has_choice(),
            choice_lines: choice.len(),
            status_lines: status.len(),
            cancel_title: painter.measure(&cancel_title, TEXT_SIZE, WEIGHT),
            continue_title: painter.measure(&continue_title, TEXT_SIZE, WEIGHT),
        });
        Layout {
            frames,
            heading,
            description,
            status,
            choice,
            cancel_title,
            continue_title,
        }
    }

    /// The view of this layout for `prompt` with `panel`'s focus.
    pub fn view<'a>(&'a self, panel: &'a Panel, prompt: &'a Prompt) -> View<'a> {
        View {
            frames: &self.frames,
            panel,
            prompt,
            heading: &self.heading,
            description: &self.description,
            status: &self.status,
            choice: &self.choice,
            cancel_title: &self.cancel_title,
            continue_title: &self.continue_title,
        }
    }
}

/// The check box's corners and its mark's stroke (approximate).
const CHECK_RADIUS: f64 = 3.5;
const CHECK_STROKE: f64 = 1.75;

impl Painter {
    /// Draw the keyring prompt `view` over the whole of `pixmap` at `scale` device pixels per point; `elapsed` is the
    /// seconds since the answer went to the caller, for the spinner.
    pub fn draw_keyring(&mut self, view: &View<'_>, pixmap: &mut Pixmap, scale: f64, elapsed: f64) {
        let p = self.palette;
        let f = view.frames;
        let s = scale;
        self.text.set_scale(s as f32);
        let body = Rect::new(0.0, 0.0, f.width, f.height);
        if !self.glass {
            self.rounded(pixmap, body, RADIUS, s, p.background);
        }
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
        for (text, frame) in view.heading.iter().zip(&f.heading) {
            self.weighted(pixmap, text, *frame, TEXT_SIZE, TITLE_WEIGHT, s, p.text);
        }
        for (text, frame) in view.description.iter().zip(&f.description) {
            self.weighted(pixmap, text, *frame, TEXT_SIZE, WEIGHT, s, p.text);
        }
        let prompt = view.prompt;
        let asking = prompt.phase == Phase::Asking;
        let focus = view.panel.focus;
        if let Some(frame) = f.password {
            let len = prompt.field_len(Field::Password);
            let failed = !prompt.properties.warning.trim().is_empty();
            self.secure_field(
                pixmap,
                frame,
                len,
                PASSWORD,
                focus == Control::Password,
                asking,
                failed,
                s,
            );
        }
        if let Some(frame) = f.confirm {
            let len = prompt.field_len(Field::Confirm);
            self.secure_field(
                pixmap,
                frame,
                len,
                CONFIRM,
                focus == Control::Confirm,
                asking,
                prompt.mismatch,
                s,
            );
        }
        if let Some(row) = f.choice {
            // one line fills the row; of two, the box sits beside the first
            let line_height = if view.choice.len() > 1 {
                BODY_LINE
            } else {
                row.height()
            };
            let check = Rect::new(
                row.x(),
                row.y() + (line_height - CHECK_BOX) / 2.0,
                CHECK_BOX,
                CHECK_BOX,
            );
            let chosen = prompt.properties.choice_chosen;
            if chosen {
                self.rounded(pixmap, check, CHECK_RADIUS, s, p.accent);
                let mut pb = PathBuilder::new();
                let at = |x: f64, y: f64| {
                    (
                        ((check.x() + x * CHECK_BOX) * s) as f32,
                        ((check.y() + y * CHECK_BOX) * s) as f32,
                    )
                };
                let (x, y) = at(0.25, 0.52);
                pb.move_to(x, y);
                let (x, y) = at(0.43, 0.7);
                pb.line_to(x, y);
                let (x, y) = at(0.75, 0.3);
                pb.line_to(x, y);
                if let Some(path) = pb.finish() {
                    pixmap.stroke_path(
                        &path,
                        &paint([1.0, 1.0, 1.0, 1.0]),
                        &line(CHECK_STROKE, s),
                        Transform::identity(),
                        None,
                    );
                }
            } else {
                self.rounded(pixmap, check, CHECK_RADIUS, s, p.field);
                self.rounded_stroke(
                    pixmap,
                    check.outset(-0.5),
                    CHECK_RADIUS - 0.5,
                    1.0,
                    s,
                    p.border,
                );
            }
            if focus == Control::Choice {
                self.rounded_stroke(
                    pixmap,
                    check.outset(FOCUS_RING / 2.0),
                    CHECK_RADIUS + FOCUS_RING / 2.0,
                    FOCUS_RING,
                    s,
                    p.focus,
                );
            }
            let color = if asking { p.text } else { p.disabled_text };
            for (i, text) in view.choice.iter().enumerate() {
                let label = Rect::new(
                    check.max_x() + CHECK_GAP,
                    row.y() + i as f64 * BODY_LINE,
                    row.max_x() - check.max_x() - CHECK_GAP,
                    line_height,
                );
                self.leading(pixmap, text, label, TEXT_SIZE, s, color);
            }
        }
        for (status, frame) in view.status.iter().zip(&f.status) {
            let color = match status {
                Status::Warning(_) | Status::Mismatch => p.red,
                Status::CapsLock => p.secondary,
            };
            let fitted = self.fit(status.text(), frame.width());
            self.leading(pixmap, &fitted, *frame, CAPTION_SIZE, s, color);
        }
        self.capsule(pixmap, f.cancel, s, p.control);
        let color = if asking {
            p.control_text
        } else {
            p.disabled_text
        };
        self.centred(
            pixmap,
            view.cancel_title,
            f.cancel,
            TEXT_SIZE,
            WEIGHT,
            s,
            color,
        );
        if focus == Control::Cancel && asking {
            self.capsule_ring(pixmap, f.cancel, s);
        }
        self.rounded(pixmap, f.continue_, BUTTON_RADIUS, s, p.accent);
        if asking {
            self.centred(
                pixmap,
                view.continue_title,
                f.continue_,
                TEXT_SIZE,
                WEIGHT,
                s,
                [1.0, 1.0, 1.0, 1.0],
            );
            if focus == Control::Continue {
                self.capsule_ring(pixmap, f.continue_, s);
            }
        } else {
            // the caller checks the answer
            self.spinner(pixmap, f.continue_.center(), s, elapsed);
        }
    }

    /// A field whose text is a secret: one bullet per character, the placeholder while empty, the caret while it
    /// has the keyboard, and a red outline after a failure.
    #[allow(clippy::too_many_arguments)]
    fn secure_field(
        &mut self,
        pixmap: &mut Pixmap,
        frame: Rect,
        len: usize,
        placeholder: &str,
        focused: bool,
        enabled: bool,
        failed: bool,
        s: f64,
    ) {
        let p = self.palette;
        self.rounded(pixmap, frame, CONTROL_RADIUS, s, p.field);
        if failed {
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
        }
        // the red outline stands where the focus ring would, as on the authentication prompt
        if focused && enabled && !failed {
            self.ring(pixmap, frame, s);
        }
        let area = frame.inset(Insets::new(0.0, FIELD_INSET, 0.0, FIELD_INSET));
        let shown: String = std::iter::repeat_n(BULLET, len).collect();
        if shown.is_empty() {
            self.leading(pixmap, placeholder, area, TEXT_SIZE, s, p.placeholder);
        }
        let style = FieldStyle {
            size: TEXT_SIZE,
            weight: WEIGHT,
            letter_spacing: 0.0,
            text: if enabled { p.text } else { p.disabled_text },
            selection: p.accent,
            caret: p.text,
            caret_width: 1.0,
        };
        let boundaries = [0, shown.len()];
        let offsets = caret_offsets(&mut self.text, &shown, &boundaries, &style, s as f32);
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
                caret: (focused && enabled).then_some(shown.len()),
                scroll: (offsets[1] + 1.0 - area.width()).max(0.0),
            },
            &style,
        );
        self.text.set_scale(s as f32);
    }
}

#[cfg(test)]
mod tests;
