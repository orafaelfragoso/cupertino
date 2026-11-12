//! The progress window: one row for each running file operation (its folder, what it does, its bar, its Stop button
//! and how far it came), then each operation that failed with its reason. Its Close button only hides it ([`Jobs`]).
//! A click on a browser window's progress row shows it ([`ShowsProgress`]).

use cupertino_kit::controls::{
    Icon, IconDirection, Progress, ScrollView, Scroller, Text, TextRole,
};
use cupertino_kit::core::{
    Change, CommandRegistry, Element, Event, EventCx, MeasureCx, PaintCx, Phase, Propagation,
    SemanticsCx, Widget, WindowBackground,
};
use cupertino_kit::geometry::{Insets, Size};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Flex, Justify, LayoutSpec, Measured, SizeSpec,
};
use cupertino_kit::platform::{DecorationMode, MouseButton, PointerEvent, WindowSpec};
use cupertino_kit::prelude::*;
use cupertino_kit::text::{AttributedText, TextStyle};

use super::jobs::{Failure, Job, JobEvent, Jobs};
use super::Message;

/// The progress window's key among the application's windows.
pub(super) const KEY: &str = "progress";
/// Its title when the operations it lists are of several kinds, or it lists failures alone; one kind names it.
const TITLE: &str = "File Operations";
/// Its width, and the most rows it grows to before its list scrolls.
const WIDTH: f64 = 420.0;
const MAX_ROWS: usize = 4;
/// A row: about its height, which sizes the window, the room around it and between its parts, and its folder icon.
const ROW_HEIGHT: f64 = 76.0;
const ROW_INSETS: Insets = Insets {
    top: 8.0,
    left: 16.0,
    bottom: 8.0,
    right: 16.0,
};
const SPACING: f64 = 12.0;
const LINE_SPACING: f64 = 4.0;
const ICON: f64 = 32.0;
/// The symbol beside an operation, and beside a failure.
const FOLDER_SYMBOL: &str = "symbol.folder";
const FAILURE_SYMBOL: &str = "symbol.xmark.circle";
/// The text of a row's title, and of the line under it.
const TITLE_SIZE: f32 = 13.0;
const DETAIL_SIZE: f32 = 11.0;

/// The progress window, while `jobs` shows it.
pub(super) fn window(jobs: &Jobs) -> Option<Window<Message>> {
    if !jobs.visible() {
        return None;
    }
    let rows: Vec<Element<Message>> = jobs
        .running
        .iter()
        .map(running)
        .chain(jobs.failures.iter().map(failed))
        .collect();
    let shown = rows.len().clamp(1, MAX_ROWS) as f64;
    let size = Size::new(WIDTH, shown * ROW_HEIGHT);
    let list = Element::new(Stack {
        axis: Axis::Vertical,
        padding: Insets::ZERO,
        spacing: 0.0,
        align: Align::Stretch,
        children: rows,
    });
    Some(Window {
        key: WindowKey(Key::from(KEY)),
        spec: WindowSpec {
            title: title(jobs).into(),
            app_id: super::APP_ID.into(),
            size,
            min_size: Some(size),
            max_size: Some(size),
            decorations: DecorationMode::Preferred,
            foreign_parent: None,
            input_region: None,
        },
        shortcuts: CommandRegistry::new(),
        on_close: Some(Message::Job(JobEvent::Hide)),
        background: WindowBackground::Theme,
        popup: None,
        menus: None,
        content: Element::from(ScrollView {
            content: list,
            state: jobs.scroll,
            horizontal: false,
            vertical: true,
            scroller: Scroller::Overlay,
            on_scroll: Box::new(|state| Message::Job(JobEvent::Scroll(state))),
        })
        .key("operations"),
    })
}

/// The title: the kind every running operation shares ("Copy"), else [`TITLE`].
fn title(jobs: &Jobs) -> &'static str {
    let mut kinds = jobs.running.iter().map(|job| job.transfer.kind());
    match kinds.next() {
        Some(kind) if kinds.all(|k| k == kind) => kind,
        _ => TITLE,
    }
}

/// A running operation: what it does, its bar and Stop button, and how far it came.
fn running(job: &Job) -> Element<Message> {
    let transfer = &job.transfer;
    let title = transfer.title();
    let bar = Element::new(Stack {
        axis: Axis::Horizontal,
        padding: Insets::ZERO,
        spacing: SPACING,
        align: Align::Center,
        children: vec![
            Element::new(Progress {
                label: title.clone(),
                value: Some(transfer.fraction()),
            })
            .fill(),
            stop(job, "stop"),
        ],
    });
    row(
        FOLDER_SYMBOL,
        vec![
            line(title, TITLE_SIZE, TextRole::Body, false),
            bar,
            line(transfer.detail(), DETAIL_SIZE, TextRole::Caption, true),
        ],
    )
    .key(Key::from(format!("operation-{}", transfer.id)))
}

/// An operation that failed: what it did, and why.
fn failed(failure: &Failure) -> Element<Message> {
    row(
        FAILURE_SYMBOL,
        vec![
            line(failure.title.clone(), TITLE_SIZE, TextRole::Body, false),
            line(failure.reason.clone(), DETAIL_SIZE, TextRole::Caption, true),
        ],
    )
    .key(Key::from(format!("failure-{}", failure.id)))
}

fn row(symbol: &str, lines: Vec<Element<Message>>) -> Element<Message> {
    Element::new(Stack {
        axis: Axis::Horizontal,
        padding: ROW_INSETS,
        spacing: SPACING,
        align: Align::Center,
        children: vec![
            Element::new(Icon {
                name: symbol.into(),
                size: ICON,
                direction: IconDirection::Fixed,
                label: None,
                tint: None,
            }),
            Element::new(Stack {
                axis: Axis::Vertical,
                padding: Insets::ZERO,
                spacing: LINE_SPACING,
                align: Align::Stretch,
                children: lines,
            })
            .fill(),
        ],
    })
}

fn line(content: String, size: f32, role: TextRole, secondary: bool) -> Element<Message> {
    Element::new(Text {
        content: AttributedText::plain(content, TextStyle::new(size)),
        role,
        wrap: false,
        secondary,
        selected: false,
    })
}

/// The Stop button of `job`, keyed `key`: disabled once pressed.
pub(super) fn stop(job: &Job, key: &'static str) -> Element<Message> {
    Element::new(Button {
        label: "Stop".into(),
        role: ButtonRole::Normal,
        style: ButtonStyle::Bordered,
        enabled: !job.transfer.cancel.is_cancelled(),
        busy: false,
        on_press: Some(Message::Job(JobEvent::Stop(job.transfer.id))),
    })
    .key(key)
}

/// A browser window's progress row without its Stop button: a press shows the progress window. It reads as its
/// parts; the menu bar's Show Progress is the keyboard's way.
pub(super) struct ShowsProgress {
    pub spacing: f64,
    pub children: Vec<Element<Message>>,
}

impl Widget<Message> for ShowsProgress {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            flex: Flex::grow(1.0),
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: self.spacing,
                align: Align::Center,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, Message>) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        match event {
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) => {
                cx.emit(Message::Job(JobEvent::Show));
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}
