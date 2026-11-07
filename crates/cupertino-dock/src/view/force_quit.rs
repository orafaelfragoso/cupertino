//! The Force Quit Applications dialog: a window centred on the output with the keyboard. Its steps, selection and
//! keys are the model's ([`crate::force_quit::Dialog`]); the parts are where [`crate::force_quit::Layout`] puts them.
//! The rows are list items and the buttons buttons for assistive technology, which can click them.

use cupertino_kit::core::{
    AccessibleStates, Action, Change, CommandRegistry, Element, Event, EventCx, Key, MeasureCx,
    PaintCx, Propagation, Role, SemanticsCx, Widget, Window, WindowBackground, WindowKey,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::layout::{Constraints, LayoutSpec, Measured};
use cupertino_kit::platform::{DecorationMode, Keysym, WindowSpec};
use cupertino_kit::theme::Color;

use super::paint::{self, Root};
use super::{kit_rect, wm_point, IconSet, Input, FORCE_QUIT_APP_ID};
use crate::force_quit::{self as fq, Button, Layout};

/// Sizes and weights of the dialog's text.
const BODY_PT: f64 = 13.0;
const FOOTER_PT: f64 = 10.0;
const DETAIL_PT: f64 = 11.0;
const REGULAR: u16 = 400;
const MEDIUM: u16 = 500;
const BOLD: u16 = 600;
/// The list's row icon, its inset from the row's leading edge, and where the name starts.
const ROW_ICON: f64 = 16.0;
const ROW_ICON_INSET: f64 = 6.0;
const ROW_TEXT_INSET: f64 = 28.0;
/// Corner radii of the list, a row's highlight and a button.
const LIST_RADIUS: f64 = 6.0;
const ROW_RADIUS: f64 = 5.0;
const BUTTON_RADIUS: f64 = 7.0;
/// A button's frame is its hit area; the visible button is this much narrower on each side.
const BUTTON_INSET: f64 = 3.0;
/// The accent of the default button and of a selected row.
const ACCENT: paint::Rgba = [0.04, 0.52, 1.0, 1.0];

/// The keys the dialog itself takes; any other key, Tab among them, goes on to the toolkit, which moves the focus
/// between the buttons.
const DIALOG_KEYS: [Keysym; 7] = [
    Keysym(0xff1b), // Escape
    Keysym(0xff0d), // Return
    Keysym(0xff8d), // keypad Enter
    Keysym(0xff52), // Up
    Keysym(0xff54), // Down
    Keysym(0xff50), // Home
    Keysym(0xff57), // End
];
/// The keys that press a focused button.
const PRESS_KEYS: [Keysym; 3] = [Keysym(0xff0d), Keysym(0xff8d), Keysym(0x20)];

/// The dialog's colours.
struct Palette {
    fill: paint::Rgba,
    edge: paint::Rgba,
    ink: paint::Rgba,
    faint: paint::Rgba,
    list: paint::Rgba,
    not_responding: paint::Rgba,
    suffix: paint::Rgba,
    /// A button that is not the default.
    plain_button: paint::Rgba,
}

impl Palette {
    /// The light or dark palette; Increase Contrast draws the edge and the secondary text solid.
    fn new(dark: bool, contrast: bool) -> Self {
        let mut p = if dark {
            Palette {
                fill: [0.17, 0.17, 0.19, 1.0],
                edge: [1.0, 1.0, 1.0, 0.14],
                ink: [1.0, 1.0, 1.0, 1.0],
                faint: [1.0, 1.0, 1.0, 0.6],
                list: [1.0, 1.0, 1.0, 0.06],
                not_responding: [1.0, 0.35, 0.3, 1.0],
                suffix: [0.6, 0.6, 0.6, 1.0],
                plain_button: [1.0, 1.0, 1.0, 0.2],
            }
        } else {
            Palette {
                fill: [0.97, 0.97, 0.98, 1.0],
                edge: [0.0, 0.0, 0.0, 0.14],
                ink: [0.0, 0.0, 0.0, 0.9],
                faint: [0.0, 0.0, 0.0, 0.55],
                list: [0.0, 0.0, 0.0, 0.05],
                not_responding: [0.609, 0.0, 0.0, 1.0],
                suffix: [0.6, 0.6, 0.6, 1.0],
                plain_button: [0.0, 0.0, 0.0, 0.1],
            }
        };
        if contrast {
            p.edge[3] = 1.0;
            p.ink[3] = 1.0;
            p.faint[3] = 1.0;
        }
        p
    }
}

/// One row of the list step.
#[derive(Clone, Debug, PartialEq)]
pub struct RowView {
    pub index: usize,
    pub rect: Rect,
    pub name: String,
    pub unresponsive: bool,
    pub selected: bool,
    pub hovered: bool,
    pub icon: IconSet,
}

pub struct ForceQuitView {
    pub output: u32,
    pub layout: Layout,
    pub rows: Vec<RowView>,
    /// No applications run.
    pub empty: bool,
    pub can_force_quit: bool,
    /// The second step's question, and the icon of the first application it asks about.
    pub question: String,
    pub question_icon: IconSet,
    pub hover_button: Option<Button>,
}

pub fn key() -> WindowKey {
    WindowKey(Key::from("force-quit"))
}

pub fn window<M: Clone + 'static>(view: ForceQuitView, input: fn(Input) -> M) -> Window<M> {
    let (width, height) = view.layout.size();
    let mut children: Vec<Element<M>> = Vec::new();
    match &view.layout {
        Layout::List(l) => {
            for row in &view.rows {
                children.push(
                    Element::new(RowNode {
                        row: row.clone(),
                        input,
                    })
                    .key(Key::from(format!("row-{}", row.index))),
                );
            }
            children.push(button(
                kit_rect(&l.force_quit),
                fq::FORCE_QUIT,
                Button::ForceQuit,
                true,
                view.can_force_quit,
                view.hover_button,
                input,
            ));
        }
        Layout::Question(q) => {
            // Cancel is the default button
            children.push(button(
                kit_rect(&q.cancel),
                fq::CANCEL,
                Button::Cancel,
                true,
                true,
                view.hover_button,
                input,
            ));
            children.push(button(
                kit_rect(&q.confirm),
                fq::FORCE_QUIT,
                Button::ForceQuit,
                false,
                true,
                view.hover_button,
                input,
            ));
        }
    }
    let title = match &view.layout {
        Layout::List(_) => fq::TITLE.to_owned(),
        Layout::Question(_) => view.question.clone(),
    };
    let ForceQuitView {
        output,
        layout,
        empty,
        question,
        question_icon,
        ..
    } = view;
    Window {
        key: key(),
        spec: WindowSpec {
            title: title.clone(),
            app_id: format!("{FORCE_QUIT_APP_ID}@{output}"),
            size: Size::new(width, height),
            min_size: None,
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: None,
            input_region: None,
        },
        shortcuts: CommandRegistry::new(),
        on_close: Some(input(Input::Closed(key()))),
        background: WindowBackground::None,
        content: Element::new(Root {
            role: match layout {
                Layout::List(_) => Role::Dialog,
                Layout::Question(_) => Role::AlertDialog,
            },
            name: title,
            pointer: Some(Box::new(move |p| input(Input::ForceQuit(p)))),
            key: Some(Box::new(move |k| {
                DIALOG_KEYS
                    .contains(&k.keysym)
                    .then(|| input(Input::ForceQuitKey(k)))
            })),
            roving: false,
            on_blur: None,
            alpha: 1.0,
            revision: 0,
            under: Some(Box::new(move |cx| {
                paint_body(cx, &layout, empty, &question, &question_icon)
            })),
            children,
        }),
        popup: None,
        menus: None,
    }
}

fn button<M: Clone + 'static>(
    rect: Rect,
    label: &str,
    which: Button,
    filled: bool,
    enabled: bool,
    hover: Option<Button>,
    input: fn(Input) -> M,
) -> Element<M> {
    Element::new(ButtonNode {
        rect,
        label: label.to_owned(),
        which,
        filled,
        enabled,
        hovered: hover == Some(which),
        input,
    })
    .key(Key::from(format!("button-{label}")))
}

fn paint_body(cx: &mut PaintCx<'_>, layout: &Layout, empty: bool, question: &str, icon: &IconSet) {
    let env = cx.env().clone();
    let p = Palette::new(paint::is_dark(&env), paint::is_high_contrast(&env));
    let (width, height) = layout.size();
    let body = Rect::new(0.0, 0.0, width, height);
    paint::fill(cx, body, fq::RADIUS, paint::color(p.fill));
    let edge = Rect::new(0.5, 0.5, width - 1.0, height - 1.0);
    paint::stroke(cx, edge, fq::RADIUS - 0.5, 1.0, paint::color(p.edge));
    match layout {
        Layout::Question(q) => {
            let (question_rect, detail) = (kit_rect(&q.question), kit_rect(&q.detail));
            paint::image(cx, icon, kit_rect(&q.icon));
            paint::text(
                cx,
                question,
                BODY_PT,
                BOLD,
                question_rect.x(),
                question_rect.y() + question_rect.height() / 2.0,
                Some(question_rect.width()),
                paint::color(p.ink),
            );
            paint::text(
                cx,
                fq::UNSAVED_CHANGES,
                DETAIL_PT,
                REGULAR,
                detail.x(),
                detail.y() + detail.height() / 2.0,
                Some(detail.width()),
                paint::color(p.faint),
            );
        }
        Layout::List(l) => {
            let (bar, heading, list, footer) = (
                kit_rect(&l.title_bar),
                kit_rect(&l.heading),
                kit_rect(&l.list),
                kit_rect(&l.footer),
            );
            let title_w = paint::width(&env, fq::TITLE, BODY_PT, BOLD);
            paint::text(
                cx,
                fq::TITLE,
                BODY_PT,
                BOLD,
                bar.x() + (bar.width() - title_w) / 2.0,
                bar.y() + bar.height() / 2.0,
                Some(bar.width()),
                paint::color(p.ink),
            );
            let rule = Rect::new(0.0, bar.y() + bar.height() - 1.0, bar.width(), 1.0);
            paint::fill(cx, rule, 0.0, paint::color(p.edge));
            if let Some(text) =
                paint::paragraph(&env, fq::HEADING, BODY_PT, REGULAR, heading.width())
            {
                paint::glyphs(cx, &text, heading.origin, paint::color(p.ink));
            }
            paint::fill(cx, list, LIST_RADIUS, paint::color(p.list));
            if empty {
                let w = paint::width(&env, fq::NO_APPLICATIONS, BODY_PT, REGULAR);
                paint::text(
                    cx,
                    fq::NO_APPLICATIONS,
                    BODY_PT,
                    REGULAR,
                    list.x() + (list.width() - w) / 2.0,
                    list.y() + list.height() / 2.0,
                    Some(list.width()),
                    paint::color(p.faint),
                );
            }
            if let Some(text) =
                paint::paragraph(&env, fq::FOOTER, FOOTER_PT, REGULAR, footer.width())
            {
                paint::glyphs(cx, &text, footer.origin, paint::color(p.faint));
            }
        }
    }
}

struct RowNode<M> {
    row: RowView,
    input: fn(Input) -> M,
}

impl<M: Clone + 'static> Widget<M> for RowNode<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        paint::placed(self.row.rect)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.row.rect != old.row.rect {
            Change::Layout
        } else if self.row != old.row {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(self.row.rect.width(), self.row.rect.height()),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Action(request) if request.action == Action::Click => {
                cx.emit((self.input)(Input::ForceQuitRow(self.row.index)));
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let env = cx.env().clone();
        let p = Palette::new(paint::is_dark(&env), paint::is_high_contrast(&env));
        let size = cx.size();
        let shown = Rect::new(0.0, 1.0, size.width, size.height - 2.0);
        if self.row.selected {
            paint::fill(cx, shown, ROW_RADIUS, paint::color(ACCENT));
        } else if self.row.hovered {
            paint::fill(cx, shown, ROW_RADIUS, paint::color(p.list));
        }
        let cy = size.height / 2.0;
        paint::image(
            cx,
            &self.row.icon,
            Rect::new(ROW_ICON_INSET, cy - ROW_ICON / 2.0, ROW_ICON, ROW_ICON),
        );
        let name_color = match (self.row.selected, self.row.unresponsive) {
            (true, _) => [1.0, 1.0, 1.0, 1.0],
            (false, true) => p.not_responding,
            (false, false) => p.ink,
        };
        let suffix_w = if self.row.unresponsive {
            paint::width(&env, fq::NOT_RESPONDING_SUFFIX, BODY_PT, REGULAR)
        } else {
            0.0
        };
        let name_w = paint::text(
            cx,
            &self.row.name,
            BODY_PT,
            REGULAR,
            ROW_TEXT_INSET,
            cy,
            Some(size.width - ROW_TEXT_INSET - suffix_w - ROW_ICON_INSET),
            paint::color(name_color),
        );
        if self.row.unresponsive {
            paint::text(
                cx,
                fq::NOT_RESPONDING_SUFFIX,
                BODY_PT,
                REGULAR,
                ROW_TEXT_INSET + name_w,
                cy,
                None,
                paint::color(p.suffix),
            );
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::ListItem);
        cx.name(self.row.name.clone());
        if self.row.unresponsive {
            cx.description(
                fq::NOT_RESPONDING_SUFFIX
                    .trim()
                    .trim_matches(['(', ')'])
                    .to_owned(),
            );
        }
        cx.states(AccessibleStates {
            selected: self.row.selected,
            ..AccessibleStates::default()
        });
        cx.action(Action::Click);
    }
}

struct ButtonNode<M> {
    rect: Rect,
    label: String,
    which: Button,
    filled: bool,
    enabled: bool,
    hovered: bool,
    input: fn(Input) -> M,
}

impl<M: Clone + 'static> Widget<M> for ButtonNode<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        paint::placed(self.rect)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if self.label != old.label
            || self.filled != old.filled
            || self.enabled != old.enabled
            || self.hovered != old.hovered
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(self.rect.width(), self.rect.height()),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Action(request) if request.action == Action::Click && self.enabled => {
                cx.emit((self.input)(Input::ForceQuitButton(self.which)));
                Propagation::Stop
            }
            Event::Action(request) if request.action == Action::Focus && self.enabled => {
                cx.request_focus();
                Propagation::Stop
            }
            Event::Key(key, _)
                if key.pressed
                    && !key.repeat
                    && self.enabled
                    && PRESS_KEYS.contains(&key.keysym) =>
            {
                cx.emit((self.input)(Input::ForceQuitButton(self.which)));
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let env = cx.env().clone();
        let p = Palette::new(paint::is_dark(&env), paint::is_high_contrast(&env));
        let size = cx.size();
        let base = if self.filled { ACCENT } else { p.plain_button };
        let alpha = match (self.enabled, self.hovered) {
            (false, _) => 0.4,
            (true, true) => 0.85,
            (true, false) => 1.0,
        };
        let shown = Rect::new(
            BUTTON_INSET,
            0.0,
            size.width - 2.0 * BUTTON_INSET,
            size.height,
        );
        paint::fill(
            cx,
            shown,
            BUTTON_RADIUS,
            paint::color(paint::faded(base, alpha)),
        );
        let ink: Color = if self.filled {
            paint::color([1.0, 1.0, 1.0, alpha as f32])
        } else {
            paint::color(paint::faded(p.ink, alpha))
        };
        let w = paint::width(&env, &self.label, BODY_PT, MEDIUM);
        paint::text(
            cx,
            &self.label,
            BODY_PT,
            MEDIUM,
            (size.width - w) / 2.0,
            size.height / 2.0,
            Some(size.width),
            ink,
        );
        if cx.is_focused() && cx.focus_visible() {
            paint::focus_ring(cx, shown, BUTTON_RADIUS);
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Button);
        cx.name(self.label.clone());
        cx.states(AccessibleStates {
            disabled: !self.enabled,
            focusable: self.enabled,
            ..AccessibleStates::default()
        });
        if self.enabled {
            // a Tab stop, so the question's Force Quit, which Return does not press, is reached from the keyboard
            cx.focusable();
            cx.action(Action::Click);
            cx.action(Action::Focus);
        }
    }
}

/// Where `p` is in the dialog: a row or a button.
pub fn hit(layout: &Layout, p: Point) -> (Option<usize>, Option<Button>) {
    let p = wm_point(p);
    (layout.row_at(p), layout.button_at(p))
}
