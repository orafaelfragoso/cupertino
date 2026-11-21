//! The keyboard's panels as CupertinoKit draws them: the switcher's rows on regular glass, faded as one layer with the
//! glass, and the picker's numbered candidates on the menu material, each a button that picks its candidate when the
//! pointer is released on it. Neither window ever takes the keyboard.

use std::sync::Arc;

use cupertino_keyboard::switcher;
use cupertino_kit::core::{
    glass_outline, Change, CommandRegistry, Element, Event, EventCx, Glass, Key, MeasureCx,
    PaintCx, Propagation, Role, SemanticsCx, Widget, Window, WindowBackground, WindowKey,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::layout::{Constraints, ContainerKind, LayoutSpec, Measured, SizeSpec};
use cupertino_kit::platform::{DecorationMode, MouseButton, PointerEvent, WindowSpec};
use cupertino_kit::render::{Paint, PaintOp};
use cupertino_kit::text::{AttributedText, FontWeight, ParagraphStyle, TextAlign, TextStyle};
use cupertino_kit::theme::{Color, Surface};

use crate::panels::{
    source_code, AccentsPanel, PickerLayout, SourcesPanel, CANDIDATE_MIDDLE, CANDIDATE_SIZE,
    CELL_RADIUS, CODE_SIZE, CODE_WEIGHT, ICON_RADIUS, ICON_SIZE, LABEL_SIZE, NUMBER_ALPHA,
    NUMBER_FROM_BOTTOM, NUMBER_SIZE, PICKER_RADIUS, ROW_INSET,
};
use crate::view::Pacer;

/// The `app_id` of the panels' windows.
pub const APP_ID: &str = "cupertino-osd";
/// The weight of the switcher's labels and of the candidates.
const REGULAR: u16 = 400;
/// The fill of the selected row and of the focused candidate (approximate).
const HIGHLIGHT: Color = Color::new(0.04, 0.45, 1.0, 1.0);
/// The text on the highlight.
const HIGHLIGHT_TEXT: Color = Color::new(1.0, 1.0, 1.0, 1.0);
/// The fill of the selected row's source icon (approximate).
const HIGHLIGHT_ICON: Color = Color::new(1.0, 1.0, 1.0, 0.28);
/// The opacity of an icon's fill in the text's colour on a row that is not selected (approximate).
const ICON_ALPHA: f32 = 0.14;
/// The text in the dark and the light appearance (approximate).
const DARK_TEXT: Color = Color::new(0.97, 0.97, 0.98, 0.95);
const LIGHT_TEXT: Color = Color::new(0.05, 0.05, 0.06, 0.92);

/// The text's colour in the session's appearance.
fn text_color(cx: &PaintCx<'_>) -> Color {
    if cx.env().theme.preferences.appearance.is_dark() {
        DARK_TEXT
    } else {
        LIGHT_TEXT
    }
}

/// The width one line of `s` takes at `size` points and `weight`, in points, with the text system of the calling
/// thread's component.
pub fn text_width(s: &str, size: f64, weight: u16) -> f64 {
    let style = TextStyle {
        weight: FontWeight(weight),
        ..TextStyle::new(size as f32)
    };
    let env = cupertino_kit_shell::environment();
    env.text
        .paragraph(&AttributedText::plain(s, style), &ParagraphStyle::default())
        .map_or(0.0, |p| p.layout(None).size().width)
}

/// One line of `s` in `left..right` (cut with an ellipsis when it does not fit), its cap height centred on `middle`.
#[allow(clippy::too_many_arguments)]
fn line(
    cx: &mut PaintCx<'_>,
    s: &str,
    (left, right): (f64, f64),
    middle: f64,
    size: f64,
    weight: u16,
    align: TextAlign,
    color: Color,
) {
    let width = right - left;
    if s.is_empty() || width <= 0.0 {
        return;
    }
    let style = TextStyle {
        weight: FontWeight(weight),
        ..TextStyle::new(size as f32)
    };
    let paragraph = ParagraphStyle {
        align,
        max_lines: Some(1),
        ellipsis: true,
        ..ParagraphStyle::default()
    };
    let system = Arc::clone(&cx.env().text);
    let Ok(text) = system.paragraph(&AttributedText::plain(s, style), &paragraph) else {
        return;
    };
    let layout = text.layout(Some(width as f32));
    let baseline = middle + cupertino_kit_shell::design::menu_bar().cap_height * size / 2.0;
    for line in layout.lines() {
        let top = baseline - line.baseline;
        for run in &line.runs {
            cx.push(PaintOp::Glyphs {
                origin: Point::new(left + run.origin.x, top + run.origin.y),
                run: run.run.clone(),
                color,
            });
        }
    }
}

/// A rectangle with the continuous corners of `radius`, filled.
fn rounded(cx: &mut PaintCx<'_>, rect: Rect, radius: f64, color: Color) {
    cx.push(PaintOp::Fill {
        shape: glass_outline(rect, radius, 0.0),
        paint: Paint::Solid(color),
    });
}

/// A leaf of a fixed size at a place in its panel.
fn placed(rect: Rect) -> LayoutSpec {
    LayoutSpec {
        width: SizeSpec::Fixed(rect.width()),
        height: SizeSpec::Fixed(rect.height()),
        position: rect.origin,
        ..LayoutSpec::new(ContainerKind::Leaf)
    }
}

fn measured(size: Size) -> Measured {
    Measured {
        size,
        baseline: None,
    }
}

/// A panel: its children at their places on the glass or the material, faded by `alpha` as one layer.
struct Body<M> {
    size: Size,
    /// The regular glass of an alert, with these corners, under the switcher's rows; the picker's material is its
    /// window's background.
    glass: Option<f64>,
    alpha: f64,
    label: String,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Body<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.size.width),
            height: SizeSpec::Fixed(self.size.height),
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.size != old.size {
            Change::Layout
        } else if self.alpha != old.alpha || self.glass != old.glass || self.label != old.label {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        measured(self.size)
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let alpha = self.alpha.clamp(0.0, 1.0);
        if let Some(radius) = self.glass {
            cx.glass(Glass::regular().radius(radius).alpha(alpha));
        }
        if alpha < 1.0 {
            cx.set_group(alpha as f32, Vec::new());
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::List);
        cx.name(self.label.clone());
    }
}

/// A row of the switcher: the source's icon with its code, and its name; the selected row on the highlight.
#[derive(Clone, Debug, PartialEq)]
struct SourceRow {
    rect: Rect,
    code: String,
    name: String,
    selected: bool,
}

impl<M: 'static> Widget<M> for SourceRow {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        placed(self.rect)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if self != old {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        measured(self.rect.size)
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = self.rect.size;
        let middle = size.height / 2.0;
        let text = text_color(cx);
        if self.selected {
            rounded(
                cx,
                Rect::new(0.0, 0.0, size.width, size.height),
                switcher::HUD_BACKGROUND_PATTERN_RADIUS,
                HIGHLIGHT,
            );
        }
        let color = if self.selected { HIGHLIGHT_TEXT } else { text };
        let icon = Rect::new(ROW_INSET, middle - ICON_SIZE / 2.0, ICON_SIZE, ICON_SIZE);
        let icon_fill = if self.selected {
            HIGHLIGHT_ICON
        } else {
            color.with_alpha(ICON_ALPHA)
        };
        rounded(cx, icon, ICON_RADIUS, icon_fill);
        line(
            cx,
            &self.code,
            (icon.x(), icon.x() + icon.width()),
            middle,
            CODE_SIZE,
            CODE_WEIGHT,
            TextAlign::Center,
            color,
        );
        let left = icon.x() + ICON_SIZE + ROW_INSET;
        line(
            cx,
            &self.name,
            (left, size.width - ROW_INSET),
            middle,
            LABEL_SIZE,
            REGULAR,
            TextAlign::Start,
            color,
        );
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::StaticText);
        cx.name(self.name.clone());
    }
}

/// A candidate of the picker with the digit that picks it; the focused one on the highlight. A release of the pointer
/// on it after a press on it sends `pick`.
struct Candidate<M> {
    rect: Rect,
    text: String,
    number: usize,
    focused: bool,
    pick: M,
}

impl<M: Clone + 'static> Widget<M> for Candidate<M> {
    /// The pointer was pressed on the candidate and not yet released.
    type State = bool;

    fn layout(&self) -> LayoutSpec {
        placed(self.rect)
    }

    fn state(&self) -> bool {
        false
    }

    fn update(&self, old: &Self, _: &mut bool) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if self.text != old.text || self.number != old.number || self.focused != old.focused
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &bool, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        measured(self.rect.size)
    }

    fn event(&self, pressed: &mut bool, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        let Event::Pointer(
            PointerEvent::Button {
                position,
                button: MouseButton::Left,
                pressed: down,
                ..
            },
            _,
        ) = event
        else {
            return Propagation::Continue;
        };
        if *down {
            *pressed = true;
            cx.capture_pointer();
        } else if std::mem::take(pressed) {
            cx.release_pointer();
            if cx.hit(*position) {
                cx.emit(self.pick.clone());
            }
        }
        Propagation::Stop
    }

    fn paint(&self, _: &bool, cx: &mut PaintCx<'_>) {
        let size = self.rect.size;
        let text = text_color(cx);
        if self.focused {
            rounded(
                cx,
                Rect::new(0.0, 0.0, size.width, size.height),
                CELL_RADIUS,
                HIGHLIGHT,
            );
        }
        let color = if self.focused { HIGHLIGHT_TEXT } else { text };
        line(
            cx,
            &self.text,
            (0.0, size.width),
            size.height * CANDIDATE_MIDDLE,
            CANDIDATE_SIZE,
            REGULAR,
            TextAlign::Center,
            color,
        );
        line(
            cx,
            &self.number.to_string(),
            (0.0, size.width),
            size.height - NUMBER_FROM_BOTTOM,
            NUMBER_SIZE,
            REGULAR,
            TextAlign::Center,
            color.with_alpha(color.a * NUMBER_ALPHA),
        );
    }

    fn semantics(&self, _: &bool, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Button);
        cx.name(self.text.clone());
    }
}

/// A panel's window: no decorations, the size of the panel.
fn window<M: Clone + 'static>(
    key: String,
    title: &str,
    size: Size,
    input: bool,
    background: WindowBackground,
    content: Element<M>,
) -> Window<M> {
    Window {
        key: WindowKey(Key::from(key)),
        spec: WindowSpec {
            title: title.to_owned(),
            app_id: APP_ID.to_owned(),
            size,
            min_size: None,
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: None,
            input_region: (!input).then(Vec::new),
        },
        shortcuts: CommandRegistry::new(),
        on_close: None,
        background,
        content,
        popup: None,
        menus: None,
    }
}

/// The key of the switcher's window of `panel`.
pub fn sources_key(panel: &SourcesPanel) -> String {
    format!("osd-sources-{}", panel.window)
}

/// The key of the picker's window of `panel`.
pub fn accents_key(panel: &AccentsPanel) -> String {
    format!("osd-accents-{}", panel.serial)
}

/// The switcher's window: `size` from [`SourcesPanel::size`], at `alpha`; it takes no input. `frame` is sent at every
/// frame of the display while it is given.
pub fn sources_window<M: Clone + 'static>(
    panel: &SourcesPanel,
    size: Size,
    alpha: f64,
    frame: Option<M>,
) -> Window<M> {
    let pad = switcher::HUD_PADDING;
    let mut children: Vec<Element<M>> = panel
        .sources
        .iter()
        .enumerate()
        .map(|(i, source)| {
            Element::new(SourceRow {
                rect: Rect::new(
                    pad,
                    pad + i as f64 * switcher::HUD_ROW_HEIGHT,
                    size.width - 2.0 * pad,
                    switcher::HUD_ROW_HEIGHT,
                ),
                code: source_code(&source.id),
                name: source.name.clone(),
                selected: i == panel.selected,
            })
        })
        .collect();
    children.extend(frame.map(|frame| Element::new(Pacer { frame })));
    let body = Element::new(Body {
        size,
        glass: Some(switcher::HUD_CORNER_RADIUS),
        alpha,
        label: "Input sources".to_owned(),
        children,
    });
    window(
        sources_key(panel),
        "Input sources",
        size,
        false,
        WindowBackground::None,
        body,
    )
}

/// The picker's window, laid out as `layout`; `pick` is the message of a click on candidate `index`.
pub fn accents_window<M: Clone + 'static>(
    panel: &AccentsPanel,
    layout: &PickerLayout,
    pick: impl Fn(usize) -> M,
) -> Window<M> {
    let children = layout
        .cells
        .iter()
        .zip(&panel.candidates)
        .enumerate()
        .map(|(i, (cell, text))| {
            Element::new(Candidate {
                rect: *cell,
                text: text.clone(),
                number: i + 1,
                focused: panel.focused == Some(i),
                pick: pick(i),
            })
        })
        .collect();
    let size = layout.rect.size;
    let body = Element::new(Body {
        size,
        glass: None,
        alpha: 1.0,
        label: "Accents".to_owned(),
        children,
    });
    window(
        accents_key(panel),
        "Accents",
        size,
        true,
        WindowBackground::Surface {
            surface: Surface::Menu,
            radius: PICKER_RADIUS,
            alpha: 1.0,
        },
        body,
    )
}
