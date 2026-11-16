//! The bare field: a text field without chrome whose text is set at its own size.

use crate::support;

use cupertino_geometry::{Insets, Size};
use cupertino_ui_controls::{BareField, CaretBlink, Stack, TextChange, TextField};
use cupertino_ui_core::{Application, Command, Element, Role, Window};
use cupertino_ui_layout::{Align, Axis};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_text::ContentPurpose;
use cupertino_ui_theme::{ColorRole, InteractionVariant};
use support::{color, window, Harness};

thread_local! {
    /// The field holds its caret.
    static HOLD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The owner starts the field's blink ([`BareField::blink_start`]).
    static START: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
    /// The field's caret draws into a layer of its own ([`BareField::caret_layer`]) and tells its blink.
    static CARET_LAYER: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

#[derive(Clone, Debug, PartialEq)]
enum Message {
    Edit(TextChange),
    Blink(CaretBlink),
}

struct Model {
    query: String,
    revision: u64,
    /// What the field told of its blink, in order.
    blinks: Vec<CaretBlink>,
}

struct Query;

impl Application for Query {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        (
            Model {
                query: String::new(),
                revision: 0,
                blinks: Vec::new(),
            },
            Vec::new(),
        )
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Edit(change) => {
                model.query = change.text;
                model.revision = change.revision;
            }
            Message::Blink(blink) => model.blinks.push(blink),
        }
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        let field = Element::new(BareField {
            menu: true,
            field: TextField {
                label: "Search".into(),
                placeholder: "Search".into(),
                value: model.query.clone(),
                revision: model.revision,
                purpose: ContentPurpose::Normal,
                enabled: true,
                on_change: Some(Box::new(Message::Edit)),
                on_submit: None,
                selection: None,
                initial: true,
            },
            size: 26.0,
            hold_caret: HOLD.get(),
            blink_start: START.get(),
            caret_layer: CARET_LAYER.get(),
            on_blink: CARET_LAYER
                .get()
                .map(|_| Box::new(Message::Blink) as Box<dyn Fn(CaretBlink) -> Message>),
        })
        .key("query");
        window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Stretch,
            children: vec![field],
        }))
    }
}

fn harness() -> Harness<Query> {
    let mut h = Harness::<Query>::assistive();
    h.resize(Size::new(400.0, 100.0));
    h
}

#[test]
fn a_bare_field_is_one_line_of_its_own_type_tall() {
    let h = harness();
    // 26 pt text and room for its ascenders and descenders
    assert_eq!(h.rect("query").height(), 34.0);
}

#[test]
fn a_bare_field_paints_no_chrome_only_its_text() {
    let mut h = harness();
    h.type_text("dock");
    let shapes: Vec<PaintOp> = h
        .ops("query")
        .into_iter()
        .filter(|op| {
            matches!(
                op,
                PaintOp::Fill {
                    shape: Shape::RoundedRect { .. },
                    ..
                } | PaintOp::Stroke { .. }
            )
        })
        .collect();
    assert!(
        shapes.is_empty(),
        "no background, border or ring: {shapes:?}"
    );
    assert!(h
        .ops("query")
        .iter()
        .any(|op| matches!(op, PaintOp::Glyphs { .. })));
}

#[test]
fn a_bare_field_edits_and_reads_as_its_text_field() {
    let mut h = harness();
    h.type_text("calc");
    assert_eq!(h.model().query, "calc");
    let field = h.accessible("query");
    assert_eq!(
        (field.role, field.name.as_deref()),
        (Role::TextInput, Some("Search"))
    );
}

/// Standing alone in a larger surface, a bare field draws its caret in its text's colour, not the accent colour a
/// field with chrome uses.
#[test]
fn a_bare_field_s_caret_is_its_text_s_colour() {
    let mut h = harness();
    h.type_text("dock");
    let bars: Vec<_> = h
        .ops("query")
        .into_iter()
        .filter_map(|op| match op {
            PaintOp::Fill {
                shape: Shape::Rect(r),
                paint: Paint::Solid(c),
            } if r.width() <= 2.0 && r.height() > 4.0 => Some(c),
            _ => None,
        })
        .collect();
    let text = color(ColorRole::Text, InteractionVariant::Normal, false);
    assert_eq!(bars, vec![text], "one caret, in the text's colour");
}

/// The opacity of each caret the field drew last.
fn caret(h: &Harness<Query>) -> Vec<f32> {
    h.ops("query")
        .into_iter()
        .filter_map(|op| match op {
            PaintOp::Fill {
                shape: Shape::Rect(r),
                paint: Paint::Solid(c),
            } if r.width() <= 2.0 && r.height() > 4.0 => Some(c.a),
            _ => None,
        })
        .collect()
}

/// A held caret's blink draws nothing: through a fade out, the hidden stretch and a fade in the field draws no frame,
/// so it shows what it showed when the hold began; let go, the caret blinks again.
#[test]
fn a_held_caret_draws_no_frame_until_it_is_let_go() {
    use cupertino_ui_platform::Timestamp;
    const MS: u64 = 1_000_000;
    let blink = |h: &mut Harness<Query>, from: u64, to: u64| {
        for ms in (from..=to).step_by(16) {
            h.clock().advance_to(Timestamp(ms * MS)).unwrap();
            h.settle();
        }
    };
    let mut h = harness();
    h.type_text("doc");
    // the edit that takes the hold is the last to restart the blink
    HOLD.set(true);
    h.type_text("k");
    let frames = h.rt.window_system().frames(h.window).unwrap().len();
    blink(&mut h, 16, 1100);
    assert_eq!(
        h.rt.window_system().frames(h.window).unwrap().len(),
        frames,
        "the held caret drew no frame"
    );
    HOLD.set(false);
    h.type_text("s");
    let frames = h.rt.window_system().frames(h.window).unwrap().len();
    blink(&mut h, 1100, 2200);
    assert!(
        h.rt.window_system().frames(h.window).unwrap().len() > frames,
        "let go, the caret blinks"
    );
}

/// A held caret the field paints again for another reason part way through its fade (the window drawn anew) is drawn as
/// it was when the hold began, not where its cycle has come to, so what the compositor shows of a surface it moves does
/// not change.
#[test]
fn a_held_caret_painted_again_part_way_through_its_fade_is_drawn_as_the_hold_began() {
    use cupertino_ui_controls::caret_opacity;
    use cupertino_ui_core::Clock;
    use cupertino_ui_platform::{Timestamp, WindowEvent};
    const MS: u64 = 1_000_000;
    let mut h = harness();
    HOLD.set(true);
    // the edit restarts the blink, the caret shown
    let since = h.clock().now();
    h.type_text("dock");
    let env = h.env();
    let text = color(ColorRole::Text, InteractionVariant::Normal, false).a;
    assert_eq!(caret(&h), vec![text], "shown");
    // a moment part way through the fade out, on the field's own curve
    let fading = (0..2000)
        .map(|ms| Timestamp(since.0 + ms * MS))
        .find(|&t| (0.25..0.75).contains(&caret_opacity(&env, since, t)))
        .expect("the caret fades");
    let frames = h.rt.window_system().frames(h.window).unwrap().len();
    let mut t = since.0;
    while t < fading.0 {
        t = (t + 16 * MS).min(fading.0);
        h.clock().advance_to(Timestamp(t)).unwrap();
        h.settle();
    }
    assert_eq!(
        h.rt.window_system().frames(h.window).unwrap().len(),
        frames,
        "the held caret drew no frame"
    );
    h.send(WindowEvent::Scale(2.0));
    h.settle();
    assert!(
        h.rt.window_system().frames(h.window).unwrap().len() > frames,
        "the window drew again"
    );
    assert_eq!(caret(&h), vec![text], "held as the hold began");
    assert!(
        caret_opacity(&env, since, fading) < 0.75,
        "where its cycle has come to"
    );
    HOLD.set(false);
}

/// Moves the clock in display frames to `to` milliseconds, drawing what the field asks for.
fn run(h: &mut Harness<Query>, to: u64) {
    use cupertino_ui_core::Clock;
    use cupertino_ui_platform::Timestamp;
    let to = Timestamp(to * 1_000_000);
    while h.clock().now() < to {
        let next = Timestamp((h.clock().now().0 + 16_000_000).min(to.0));
        h.clock().advance_to(next).unwrap();
        h.settle();
    }
}

/// The window loses the keyboard and gets it back.
fn reactivate(h: &mut Harness<Query>) {
    use cupertino_ui_platform::WindowStates;
    for activated in [false, true] {
        let states = WindowStates {
            activated,
            ..WindowStates::default()
        };
        h.rt.window_system_mut()
            .configure(h.window, Some(Size::new(400.0, 100.0)), states)
            .unwrap();
        h.settle();
    }
}

/// The blink a field tells its owner is the one it draws: its opacity at each moment, and where it next holds.
#[test]
fn the_blink_a_field_tells_is_the_one_it_draws() {
    use cupertino_ui_controls::caret_opacity;
    use cupertino_ui_platform::Timestamp;
    let env = harness().env();
    let blink = CaretBlink::of(&env, false);
    assert_eq!(blink, CaretBlink::default(), "the theme's own");
    let mut fades = 0;
    for ms in 0..3000_u64 {
        let t = ms as f64 / 1000.0;
        let drawn = caret_opacity(&env, Timestamp(0), Timestamp(ms * 1_000_000));
        assert_eq!(blink.opacity(t), drawn, "{ms} ms");
        let steady = blink.steady_from(t, 0.1);
        if drawn > 0.0 && drawn < 1.0 {
            fades += 1;
            assert!(steady > t, "{ms} ms: past the fade");
        }
        // it holds from there for a tenth of a second, shown or hidden, and is the first moment that does
        let held = blink.opacity(steady);
        assert!(held == 0.0 || held == 1.0, "{ms} ms: {held}");
        let holds = |from: f64| {
            (0..100).all(|step| (blink.opacity(from + step as f64 / 1000.0) - held).abs() < 1e-3)
        };
        assert!(holds(steady), "{ms} ms");
        if steady > t + 2e-3 {
            assert!(!holds(steady - 2e-3), "{ms} ms: an earlier moment holds");
        }
    }
    assert!(fades > 0, "it fades");
    let steady = CaretBlink {
        phase_ms: 0,
        restarted: false,
    };
    assert_eq!(
        (steady.opacity(0.56), steady.steady_from(0.56, 0.1)),
        (1.0, 0.56)
    );
}

/// A blink its owner starts runs from the moment the field took the start: the window losing the keyboard and
/// getting it back starts it over no more, as it does any other field's.
#[test]
fn a_blink_its_owner_starts_runs_on_whatever_the_window_s_activation_does() {
    use cupertino_ui_controls::caret_opacity;
    use cupertino_ui_platform::Timestamp;
    // how far the caret fades in a display frame, the most two fields drawing on frames apart differ by
    const FRAME_OF_FADE: f32 = 0.15;
    let text = color(ColorRole::Text, InteractionVariant::Normal, false).a;
    for start in [None, Some(1)] {
        START.set(start);
        let mut h = harness();
        let env = h.env();
        run(&mut h, 300);
        reactivate(&mut h);
        // a moment part way through a fade of the blink from the field's start, after the window got the
        // keyboard back
        let back = cupertino_ui_core::Clock::now(h.clock()).0 / 1_000_000;
        let fading = (back + 16..back + 2000)
            .find(|&ms| {
                (0.25..0.5).contains(&caret_opacity(
                    &env,
                    Timestamp(0),
                    Timestamp(ms * 1_000_000),
                ))
            })
            .expect("it fades");
        run(&mut h, fading);
        // the same field at the same moment, its window never losing the keyboard
        let mut kept = harness();
        run(&mut kept, fading);
        let (drawn, from_start) = (caret(&h), caret(&kept));
        assert_eq!(drawn.len(), 1, "{start:?}");
        assert!(from_start[0] < 0.6 * text, "part way: {from_start:?}");
        if start.is_some() {
            assert!(
                (drawn[0] - from_start[0]).abs() < FRAME_OF_FADE * text,
                "{drawn:?} against {from_start:?}"
            );
        } else {
            assert!(
                (drawn[0] - from_start[0]).abs() > 0.3 * text,
                "started over: {drawn:?}"
            );
        }
    }
    START.set(None);
}

/// A caret in a layer of its own is drawn fully shown, and its blink draws no frame: the owner shows the blink through
/// the layer. The field tells its blink as the caret shows again, an edit as a start over.
#[test]
fn a_caret_in_a_layer_of_its_own_is_drawn_fully_shown_and_its_blink_draws_nothing() {
    START.set(Some(1));
    CARET_LAYER.set(Some(6));
    let mut h = harness();
    let text = color(ColorRole::Text, InteractionVariant::Normal, false).a;
    let frames = h.rt.window_system().frames(h.window).unwrap().len();
    let now = cupertino_ui_core::Clock::now(h.clock()).0 / 1_000_000;
    run(&mut h, now + 1100);
    assert_eq!(
        h.rt.window_system().frames(h.window).unwrap().len(),
        frames,
        "its blink drew no frame"
    );
    assert_eq!(caret(&h), vec![text], "fully shown");
    reactivate(&mut h);
    let told = h.model().blinks.clone();
    assert!(
        told.iter().all(|b| !b.restarted && b.phase_ms > 0) && !told.is_empty(),
        "the window's activation starts nothing over: {told:?}"
    );
    h.type_text("d");
    assert_eq!(
        h.model().blinks.last(),
        Some(&CaretBlink {
            restarted: true,
            ..told[0]
        }),
        "an edit starts it over"
    );
    assert_eq!(caret(&h), vec![text]);
    START.set(None);
    CARET_LAYER.set(None);
}
