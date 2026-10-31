use super::*;
use crate::test_support::{group, Group};
use crate::test_support::{
    leaf, options, options_with, row, FakeAdapter, FakeClock, FakeSystem, Leaf, MAX_PIXELS,
};
use crate::{
    Change, EventCx, MeasureCx, PaintCx, Propagation, Role, SemanticsCx, Shortcut, Widget,
};
use cupertino_geometry::Rect;
use cupertino_ui_layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutSpec, Measured, Overflow, SizeSpec,
};
use cupertino_ui_platform::{
    Configure, DecorationMode, Mods, MouseButton, PointerEvent, WindowStates, WmCapabilities,
};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_theme::{Color, ColorRole, ColorTokens, StateVariant};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::Ordering;

/// Two buttons; pressing `plus` (or Primary+=) counts up, and a `wait` message starts a task.
struct Counter;

#[derive(Default)]
struct Model {
    count: u32,
    done: Vec<u32>,
    /// The window paints no background, as a panel that draws its own.
    panel: bool,
}

impl Application for Counter {
    type Model = Model;
    type Message = String;

    fn init() -> (Model, Vec<Command<String>>) {
        (Model::default(), Vec::new())
    }

    fn update(model: &mut Model, message: String) -> Vec<Command<String>> {
        match message.as_str() {
            "plus:Target:press" | "plus" => model.count += 1,
            "quit" => return vec![Command::Exit],
            "panel" => model.panel = !model.panel,
            "close" => return vec![Command::CloseWindow(WindowKey("main".into()))],
            "wait" => return vec![Command::Perform(Box::pin(async { "done 7".to_string() }))],
            m if m.starts_with("done ") => model.done.push(m[5..].parse().unwrap()),
            _ => {}
        }
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<String>> {
        vec![Window {
            key: WindowKey("main".into()),
            spec: WindowSpec {
                title: format!("Count {}", model.count),
                app_id: "test.counter".into(),
                size: Size::new(200.0, 100.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
            },
            shortcuts: CommandRegistry::new().with(
                Shortcut {
                    mods: Mods::PRIMARY,
                    keysym: Keysym(0x3d),
                },
                "plus".to_string(),
            ),
            on_close: None,
            popup: None,
            background: if model.panel {
                WindowBackground::None
            } else {
                WindowBackground::Theme
            },
            content: Element::new(row(vec![
                Element::new(leaf("plus")).key("plus"),
                Element::new(Leaf {
                    secret: true,
                    animated: true,
                    ..leaf("minus")
                })
                .key("minus"),
            ])),
        }]
    }
}

use crate::Element;

type Rt = Runtime<Counter, FakeSystem>;

fn steps() -> NonZeroU32 {
    NonZeroU32::new(16).unwrap()
}

fn start() -> (Rt, WindowId, Rc<RefCell<Vec<DamageRegion>>>) {
    start_with(|_| {})
}

fn start_with(
    setup: impl FnOnce(&mut RuntimeOptions),
) -> (Rt, WindowId, Rc<RefCell<Vec<DamageRegion>>>) {
    let damage = Rc::default();
    let mut options = options(&damage);
    setup(&mut options);
    let mut rt = Rt::new(FakeSystem::default(), options);
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    (rt, window, damage)
}

fn configure(rt: &mut Rt, window: WindowId, id: u32) {
    configure_at(rt, window, id, Size::new(200.0, 100.0));
}

fn configure_at<A: Application>(
    rt: &mut Runtime<A, FakeSystem>,
    window: WindowId,
    id: u32,
    size: Size,
) {
    rt.window_system_mut().send(
        window,
        WindowEvent::Configure(Configure {
            id: ConfigureId(id),
            size: Some(size),
            states: WindowStates {
                activated: true,
                ..WindowStates::default()
            },
            bounds: None,
            server_decorations: true,
            wm_capabilities: WmCapabilities {
                window_menu: true,
                maximize: true,
                fullscreen: true,
                minimize: true,
            },
        }),
    );
    rt.step(steps()).unwrap();
}

fn click<A: Application>(rt: &mut Runtime<A, FakeSystem>, window: WindowId, x: f64) {
    for pressed in [true, false] {
        rt.window_system_mut().send(
            window,
            WindowEvent::Pointer(
                PointerEvent::Button {
                    position: Point::new(x, 5.0),
                    button: MouseButton::Left,
                    pressed,
                    serial: 1,
                    mods: cupertino_ui_platform::Mods::NONE,
                },
                Timestamp(0),
            ),
        );
    }
    rt.step(steps()).unwrap();
}

fn key(rt: &mut Rt, window: WindowId, keysym: u32, mods: Mods) {
    rt.window_system_mut().send(
        window,
        WindowEvent::Key(
            KeyEvent {
                code: 0,
                keysym: Keysym(keysym),
                text: None,
                pressed: true,
                repeat: false,
                mods,
            },
            Timestamp(0),
        ),
    );
    rt.step(steps()).unwrap();
}

#[test]
fn a_window_is_drawn_only_after_its_first_configure_which_the_frame_acknowledges() {
    let (mut rt, window, damage) = start();
    assert_eq!(rt.window_system().created.len(), 1);
    assert!(rt.window_system().presented.is_empty());
    assert_eq!(
        rt.step(steps()).unwrap(),
        StepStatus::Waiting(Pending {
            frames: 1,
            ..Pending::default()
        })
    );

    configure(&mut rt, window, 9);

    assert_eq!(
        rt.window_system().presented,
        vec![(window, Some(ConfigureId(9)))]
    );
    assert_eq!(damage.borrow().as_slice(), [DamageRegion::Full]);
    assert_eq!(rt.step(steps()).unwrap(), StepStatus::Idle);
}

#[test]
fn a_refused_frame_waits_for_the_window_system_and_is_drawn_whole_when_it_resumes() {
    let (mut rt, window, damage) = start();
    rt.window_system_mut().refuse_next = true;
    configure(&mut rt, window, 3);

    assert!(rt.window_system().presented.is_empty());
    assert_eq!(
        rt.step(steps()).unwrap(),
        StepStatus::Waiting(Pending {
            frames: 1,
            ..Pending::default()
        }),
        "no retry until the window system says the window may draw"
    );

    rt.window_system_mut().release(window);
    rt.step(steps()).unwrap();

    assert_eq!(
        rt.window_system().presented,
        vec![(window, Some(ConfigureId(3)))]
    );
    assert_eq!(
        damage.borrow().as_slice(),
        [DamageRegion::Full, DamageRegion::Full]
    );
    assert_eq!(rt.step(steps()).unwrap(), StepStatus::Idle);
}

#[test]
fn a_click_updates_the_model_and_the_view_and_a_frame_with_no_damage_sends_nothing() {
    let (mut rt, window, _) = start();
    configure(&mut rt, window, 1);
    let plus = rt.find(window, &Key::from("plus")).unwrap();
    assert_eq!(rt.node_at(window, Point::new(5.0, 5.0)), Some(plus));

    click(&mut rt, window, 5.0);

    assert_eq!(rt.model().count, 1);
    assert_eq!(
        rt.window_system().requests,
        vec![(window, WindowRequest::SetTitle("Count 1".into()))]
    );
    assert_eq!(
        rt.find(window, &Key::from("plus")),
        Some(plus),
        "the node survives the update"
    );
    assert_eq!(
        rt.window_system().presented.len(),
        1,
        "no frame callback yet"
    );

    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(1)));
    rt.step(steps()).unwrap();
    assert_eq!(
        rt.window_system().presented.len(),
        1,
        "nothing visible changed, so no buffer is sent"
    );

    configure(&mut rt, window, 2);
    assert_eq!(
        rt.window_system().presented[1],
        (window, Some(ConfigureId(2)))
    );
}

#[test]
fn tab_moves_a_visible_focus_and_a_shortcut_sends_its_message() {
    let (mut rt, window, _) = start();
    configure(&mut rt, window, 1);

    key(&mut rt, window, 0xff09, Mods::NONE);
    let plus = rt.find(window, &Key::from("plus")).unwrap();
    let snapshot = rt.snapshot(window).unwrap();
    assert_eq!(snapshot.focus, Some(plus));
    let (_, node) = snapshot
        .accessibility
        .iter()
        .find(|(id, _)| *id == plus)
        .unwrap();
    assert!(node.states.focused);
    assert_eq!(node.role, Role::Button);

    key(&mut rt, window, 0xfe20, Mods::SHIFT);
    assert_eq!(
        rt.snapshot(window).unwrap().focus,
        rt.find(window, &Key::from("minus")),
        "Shift+Tab wraps backwards"
    );

    key(&mut rt, window, 0x3d, Mods::PRIMARY);
    assert_eq!(rt.model().count, 1);
}

#[test]
fn a_task_reports_back_as_a_message_and_a_close_request_ends_the_application() {
    let (mut rt, window, _) = start();
    configure(&mut rt, window, 1);
    rt.messages.push_back("wait".into());
    rt.step(steps()).unwrap();
    assert_eq!(rt.model().done, vec![7]);
    assert_eq!(rt.pending().tasks, 0);

    rt.window_system_mut()
        .send(window, WindowEvent::CloseRequested);
    rt.step(steps()).unwrap();
    assert!(rt.exited());
}

#[test]
fn the_snapshot_lists_the_nodes_in_reading_order_with_their_window_rects() {
    let (mut rt, window, _) = start();
    configure(&mut rt, window, 1);
    let snapshot = rt.snapshot(window).unwrap();
    let rects: Vec<_> = snapshot.nodes.iter().map(|n| n.rect).collect();
    assert_eq!(
        rects,
        [
            cupertino_geometry::Rect::new(0.0, 0.0, 200.0, 100.0),
            cupertino_geometry::Rect::new(0.0, 0.0, 10.0, 10.0),
            cupertino_geometry::Rect::new(10.0, 0.0, 10.0, 10.0),
        ]
    );
    assert_eq!(snapshot.display_list.nodes.len(), 3);
    assert_eq!(
        snapshot.nodes[0].children,
        vec![snapshot.nodes[1].id, snapshot.nodes[2].id]
    );
}

#[test]
fn a_buffer_is_sized_as_the_window_system_rounds_a_fractional_scale() {
    assert_eq!(
        [3.0, 2.0, 1.0].map(|n| physical(n, 1.25)),
        [4, 3, 1],
        "3.75 and 2.5 round up, 1.25 down"
    );
    assert_eq!(physical(100.0, 1.5), 150);
}

#[test]
fn a_closed_window_is_dropped_by_the_accessibility_adapter() {
    let adapter = FakeAdapter::default();
    let (mut rt, window, _) = start_with(|o| o.accessibility = Some(Box::new(adapter.clone())));
    configure(&mut rt, window, 1);
    rt.messages.push_back("close".into());
    rt.step(steps()).unwrap();
    assert_eq!(*adapter.closed.borrow(), [window]);
}

#[test]
fn an_assistive_technology_that_starts_after_the_app_gets_the_whole_tree_and_then_only_changes() {
    let adapter = FakeAdapter::default();
    let (mut rt, window, _) = start_with(|o| o.accessibility = Some(Box::new(adapter.clone())));
    configure(&mut rt, window, 1);
    assert!(adapter.updates.borrow().is_empty());

    adapter.active.set(true);
    rt.step(steps()).unwrap();

    let snapshot = rt.snapshot(window).unwrap();
    assert_eq!(
        *adapter.updates.borrow(),
        [AccessibilityUpdate {
            window,
            root: snapshot.nodes[0].id,
            focus: None,
            nodes: snapshot.accessibility.clone(),
            removed: Vec::new(),
            full: true,
        }]
    );

    key(&mut rt, window, 0xff09, Mods::NONE);
    let plus = rt.find(window, &Key::from("plus")).unwrap();
    {
        let updates = adapter.updates.borrow();
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[1].focus, Some(plus));
        assert!(!updates[1].full);
        assert!(
            updates[1].nodes.len() < snapshot.accessibility.len(),
            "a diff, not the tree again"
        );
    }

    adapter.active.set(false);
    rt.step(steps()).unwrap();
    adapter.active.set(true);
    rt.step(steps()).unwrap();
    let updates = adapter.updates.borrow();
    assert_eq!(updates.len(), 3);
    assert!(
        updates[2].full,
        "an adapter back from inactive replaces its tree"
    );
    assert_eq!(updates[2].nodes, rt.snapshot(window).unwrap().accessibility);
}

#[test]
fn a_window_whose_content_was_too_large_to_draw_draws_again_on_its_next_change_without_a_configure()
{
    let damage = Rc::default();
    let broken = Rc::new(Cell::new(false));
    let errors = Rc::new(RefCell::new(Vec::new()));
    let seen = Rc::clone(&errors);
    let mut options = options_with(&damage, &broken);
    options.on_window_error = Some(Box::new(move |w, e| seen.borrow_mut().push((w, e.clone()))));
    let mut rt = Rt::new(FakeSystem::default(), options);
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    configure(&mut rt, window, 1);
    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(1)));
    rt.step(steps()).unwrap();

    broken.set(true);
    key(&mut rt, window, 0xff09, Mods::NONE);
    assert_eq!(
        *errors.borrow(),
        [(
            window,
            UiError::Render(cupertino_ui_render::RenderError::ResourceLimit)
        )]
    );
    assert_eq!(
        rt.step(steps()).unwrap(),
        StepStatus::Idle,
        "no retry until something changes"
    );

    broken.set(false);
    key(&mut rt, window, 0xff09, Mods::NONE);
    assert_eq!(rt.window_system().presented.len(), 2);
    assert_eq!(damage.borrow().last(), Some(&DamageRegion::Full));
}

#[test]
fn a_window_the_renderer_cannot_draw_is_reported_and_draws_again_after_a_drawable_configure() {
    let errors = Rc::new(RefCell::new(Vec::new()));
    let seen = Rc::clone(&errors);
    let (mut rt, window, damage) = start_with(|o| {
        o.on_window_error = Some(Box::new(move |w, e| seen.borrow_mut().push((w, e.clone()))));
    });
    configure(&mut rt, window, 1);
    let side = (MAX_PIXELS as f64).sqrt() + 1.0;

    configure_at(&mut rt, window, 2, Size::new(side, side));

    assert!(!rt.exited());
    assert_eq!(
        *errors.borrow(),
        [(
            window,
            UiError::Render(cupertino_ui_render::RenderError::ResourceLimit)
        )]
    );
    assert_eq!(
        rt.step(steps()).unwrap(),
        StepStatus::Idle,
        "no retry until something changes"
    );
    key(&mut rt, window, 0xff09, Mods::NONE);
    assert_eq!(
        errors.borrow().len(),
        1,
        "a change tries again, and the same error is not reported again"
    );
    assert!(!rt.exited());

    configure(&mut rt, window, 3);
    assert_eq!(
        rt.window_system().presented,
        vec![
            (window, Some(ConfigureId(1))),
            (window, Some(ConfigureId(3)))
        ]
    );
    assert_eq!(damage.borrow().last(), Some(&DamageRegion::Full));
    assert_eq!(errors.borrow().len(), 1);
}

#[test]
fn a_protected_field_never_shows_its_text_to_the_input_method() {
    let (mut rt, window, _) = start();
    configure(&mut rt, window, 1);

    key(&mut rt, window, 0xff09, Mods::NONE);
    key(&mut rt, window, 0xff09, Mods::NONE);

    let minus = rt.find(window, &Key::from("minus")).unwrap();
    assert_eq!(rt.snapshot(window).unwrap().text_input, Some(minus));
    let sent = rt
        .window_system()
        .text_input
        .last()
        .cloned()
        .flatten()
        .unwrap();
    assert_eq!(sent.surrounding, None);
    assert_eq!(sent.purpose, cupertino_ui_text::ContentPurpose::Password);
}

#[test]
fn an_assistive_technology_listening_from_the_start_gets_each_window_once() {
    let adapter = FakeAdapter::default();
    adapter.active.set(true);
    let (mut rt, window, _) = start_with(|o| o.accessibility = Some(Box::new(adapter.clone())));
    configure(&mut rt, window, 1);

    let updates = adapter.updates.borrow();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].nodes, rt.snapshot(window).unwrap().accessibility);
}

#[test]
fn the_adapter_learns_each_lock_state_the_window_system_reports() {
    let adapter = FakeAdapter::default();
    let (mut rt, _, _) = start_with(|o| o.accessibility = Some(Box::new(adapter.clone())));
    for locked in [false, true, false] {
        rt.window_system_mut()
            .queue
            .push_back(PlatformEvent::SessionLock(locked));
    }
    rt.step(steps()).unwrap();
    assert_eq!(*adapter.locked.borrow(), [false, true, false]);
}

#[test]
fn a_failing_window_reports_each_error_once_and_its_animation_waits_for_a_frame_that_presents() {
    let damage = Rc::default();
    let broken = Rc::new(Cell::new(false));
    let errors = Rc::new(RefCell::new(Vec::new()));
    let seen = Rc::clone(&errors);
    let mut options = options_with(&damage, &broken);
    options.on_window_error = Some(Box::new(move |w, e| seen.borrow_mut().push((w, e.clone()))));
    let mut rt = Rt::new(FakeSystem::default(), options);
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    configure(&mut rt, window, 1);
    key(&mut rt, window, 0xff09, Mods::NONE);
    key(&mut rt, window, 0xff09, Mods::NONE);
    let frame = |rt: &mut Rt| {
        rt.window_system_mut()
            .send(window, WindowEvent::Frame(Timestamp(1)));
        rt.step(steps()).unwrap();
    };
    frame(&mut rt);
    let presented = rt.window_system().presented.len();

    broken.set(true);
    key(&mut rt, window, 0x61, Mods::NONE);
    frame(&mut rt);
    assert_eq!(errors.borrow().len(), 1);
    let requested = rt.window_system().frames_requested;
    frame(&mut rt);
    frame(&mut rt);
    assert_eq!(errors.borrow().len(), 1, "the same error is reported once");
    assert_eq!(
        rt.window_system().frames_requested,
        requested,
        "the animation asks for no frames while the window fails"
    );

    broken.set(false);
    key(&mut rt, window, 0x61, Mods::NONE);
    assert_eq!(rt.window_system().presented.len(), presented + 1);
    assert!(rt.window_system().frames_requested > requested);
    broken.set(true);
    frame(&mut rt);
    assert_eq!(errors.borrow().len(), 2, "a failure after a success is new");
}

/// One node that takes every key, logging what reaches it.
struct Holder;

impl Application for Holder {
    type Model = Vec<String>;
    type Message = String;

    fn init() -> (Vec<String>, Vec<Command<String>>) {
        (Vec::new(), Vec::new())
    }

    fn update(log: &mut Vec<String>, message: String) -> Vec<Command<String>> {
        log.push(message);
        Vec::new()
    }

    fn view(_: &Vec<String>) -> Vec<Window<String>> {
        vec![Window {
            key: WindowKey("main".into()),
            spec: WindowSpec {
                title: "Holder".into(),
                app_id: "test.holder".into(),
                size: Size::new(200.0, 100.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::Theme,
            popup: None,
            content: Element::new(Leaf {
                stop: true,
                ..leaf("hold")
            }),
        }]
    }
}

#[test]
fn a_release_ends_its_press_by_physical_key_even_when_its_symbol_changed() {
    let damage = Rc::default();
    let mut rt = Runtime::<Holder, _>::new(FakeSystem::default(), options(&damage));
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    configure_at(&mut rt, window, 1, Size::new(200.0, 100.0));
    let send = |rt: &mut Runtime<Holder, FakeSystem>, event| {
        rt.window_system_mut().send(window, event);
        rt.step(steps()).unwrap();
    };
    let key = |keysym, pressed| {
        WindowEvent::Key(
            KeyEvent {
                code: 30,
                keysym: Keysym(keysym),
                text: None,
                pressed,
                repeat: false,
                mods: Mods::NONE,
            },
            Timestamp(0),
        )
    };
    // `a` pressed, then Shift goes down and the same key is released as `A`
    send(&mut rt, key(0x61, true));
    send(&mut rt, key(0x41, false));
    send(&mut rt, WindowEvent::Focus(false));
    assert!(
        !rt.model().iter().any(|m| m.ends_with(":cancel")),
        "the released key holds nothing: {:?}",
        rt.model()
    );

    send(&mut rt, key(0x61, true));
    send(&mut rt, WindowEvent::Focus(true));
    send(&mut rt, WindowEvent::Focus(false));
    assert_eq!(
        rt.model().iter().filter(|m| m.ends_with(":cancel")).count(),
        1,
        "a key still held is cancelled"
    );
}

#[test]
fn an_appearance_change_redraws_open_windows_with_the_new_theme_and_keeps_the_focus() {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let (mut rt, window, _) = start();
    let dark = Arc::new(ThemeTokens {
        preferences: cupertino_ui_theme::ThemePreferences {
            appearance: cupertino_ui_theme::Appearance::Dark,
            reduce_transparency: false,
            reduce_motion: true,
        },
        ..(*rt.options.theme).clone()
    });
    let made = Arc::clone(&dark);
    let asked = Rc::clone(&seen);
    rt = rt.with_theme_for(Box::new(move |appearance: &SystemAppearance| {
        asked.borrow_mut().push(*appearance);
        Arc::clone(&made)
    }));
    configure(&mut rt, window, 1);
    key(&mut rt, window, 0xff09, Mods::NONE);
    let focus = rt.snapshot(window).unwrap().focus;
    assert!(focus.is_some());
    let frame = |rt: &mut Rt| {
        rt.window_system_mut()
            .send(window, WindowEvent::Frame(Timestamp(1)));
        rt.step(steps()).unwrap();
    };
    frame(&mut rt);
    let presented = rt.window_system().presented.len();

    let appearance = SystemAppearance {
        dark: true,
        reduce_motion: true,
        ..SystemAppearance::default()
    };
    rt.window_system_mut()
        .queue
        .push_back(PlatformEvent::Appearance(appearance));
    rt.step(steps()).unwrap();
    frame(&mut rt);

    assert_eq!(*seen.borrow(), [appearance]);
    assert!(
        Arc::ptr_eq(&rt.windows[0].env.theme, &dark),
        "the open window draws with it"
    );
    assert!(
        Arc::ptr_eq(&rt.options.theme, &dark),
        "a new window would too"
    );
    assert!(
        rt.window_system().presented.len() > presented,
        "the window draws again"
    );
    assert_eq!(rt.snapshot(window).unwrap().focus, focus);
}

#[test]
fn without_a_theme_maker_an_appearance_change_keeps_the_theme() {
    let (mut rt, window, _) = start();
    configure(&mut rt, window, 1);
    let before = Arc::clone(&rt.windows[0].env.theme);
    rt.window_system_mut()
        .queue
        .push_back(PlatformEvent::Appearance(SystemAppearance {
            dark: true,
            ..SystemAppearance::default()
        }));
    rt.step(steps()).unwrap();
    assert!(Arc::ptr_eq(&rt.windows[0].env.theme, &before));
}

#[test]
fn a_theme_equal_to_the_current_one_lays_nothing_out_again() {
    let (mut rt, window, _) = start();
    configure(&mut rt, window, 1);
    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(1)));
    rt.step(steps()).unwrap();
    assert!(!rt.windows[0].dirty);
    let current = Arc::clone(&rt.options.theme);

    rt.set_theme(Arc::new((*current).clone()));
    assert!(!rt.windows[0].dirty, "an equal theme is no change");
    assert!(Arc::ptr_eq(&rt.windows[0].env.theme, &current));

    let mut other = (*current).clone();
    other.motion.reduce = !other.motion.reduce;
    rt.set_theme(Arc::new(other));
    assert!(
        rt.windows[0].dirty,
        "a different theme lays the window out again"
    );
}

/// One node that takes keys and asks to be woken 500 ns after each, logging what reaches it; `hide` unmounts it.
struct Sleeper;

impl Application for Sleeper {
    type Model = Vec<String>;
    type Message = String;

    fn init() -> (Vec<String>, Vec<Command<String>>) {
        (Vec::new(), Vec::new())
    }

    fn update(log: &mut Vec<String>, message: String) -> Vec<Command<String>> {
        log.push(message);
        Vec::new()
    }

    fn view(log: &Vec<String>) -> Vec<Window<String>> {
        let content = if log.iter().any(|m| m == "hide") {
            row(Vec::new())
        } else {
            row(vec![Element::new(Leaf {
                stop: true,
                wake_after: Some(500),
                ..leaf("sleeper")
            })])
        };
        vec![Window {
            key: WindowKey("main".into()),
            spec: WindowSpec {
                title: "Sleeper".into(),
                app_id: "test.sleeper".into(),
                size: Size::new(200.0, 100.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::Theme,
            popup: None,
            content: Element::new(content),
        }]
    }
}

type Sleepy = Runtime<Sleeper, FakeSystem>;

/// A sleeper focused by Tab, with a clock the test moves.
fn sleeper() -> (Sleepy, WindowId, Arc<FakeClock>) {
    let clock = Arc::new(FakeClock::default());
    let mut options = options(&Rc::default());
    options.clock = clock.clone();
    let mut rt = Sleepy::new(FakeSystem::default(), options);
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    configure_at(&mut rt, window, 1, Size::new(200.0, 100.0));
    tap(&mut rt, window, 0xff09);
    (rt, window, clock)
}

fn tap(rt: &mut Sleepy, window: WindowId, keysym: u32) {
    rt.window_system_mut().send(
        window,
        WindowEvent::Key(
            KeyEvent {
                code: 0,
                keysym: Keysym(keysym),
                text: None,
                pressed: true,
                repeat: false,
                mods: Mods::NONE,
            },
            Timestamp(0),
        ),
    );
    rt.step(steps()).unwrap();
}

fn wakes(rt: &Sleepy) -> usize {
    rt.model().iter().filter(|m| m.ends_with(":wake")).count()
}

#[test]
fn a_node_is_woken_once_the_clock_reaches_the_time_it_asked_for() {
    let (mut rt, window, clock) = sleeper();
    clock.0.store(100, Ordering::SeqCst);
    tap(&mut rt, window, 0x61);
    assert_eq!(rt.pending().next_wake, Some(Timestamp(600)));

    clock.0.store(599, Ordering::SeqCst);
    rt.step(steps()).unwrap();
    assert_eq!(wakes(&rt), 0, "not before its time");

    clock.0.store(650, Ordering::SeqCst);
    rt.step(steps()).unwrap();
    assert_eq!(wakes(&rt), 1);
    assert_eq!(rt.pending().next_wake, None, "a wake is delivered once");
}

#[test]
fn a_new_request_moves_the_wake_and_an_unmounted_node_loses_it() {
    let (mut rt, window, clock) = sleeper();
    tap(&mut rt, window, 0x61);
    clock.0.store(300, Ordering::SeqCst);
    tap(&mut rt, window, 0x61);
    assert_eq!(rt.pending().next_wake, Some(Timestamp(800)));
    clock.0.store(700, Ordering::SeqCst);
    rt.step(steps()).unwrap();
    assert_eq!(wakes(&rt), 0, "the first request was replaced");

    rt.messages.push_back("hide".into());
    rt.step(steps()).unwrap();
    assert_eq!(rt.pending().next_wake, None);
}

#[test]
fn the_loop_waits_only_until_the_earliest_wake() {
    let (mut rt, window, clock) = sleeper();
    tap(&mut rt, window, 0x61);
    clock.0.store(200, Ordering::SeqCst);
    rt.window_system_mut().close_on_wait = true;
    let waits = Rc::clone(&rt.window_system().waits);
    rt.run().unwrap();
    // steps poll without waiting; the first wait that may block is `run`'s
    let blocking = waits
        .borrow()
        .iter()
        .copied()
        .find(|&w| w != Some(Duration::ZERO));
    assert_eq!(
        blocking,
        Some(Some(Duration::from_nanos(300))),
        "the wake due at 500 bounds the wait at 200: {:?}",
        waits.borrow()
    );
}

#[test]
fn a_theme_switch_repaints_what_draws_in_the_themes_colours() {
    use cupertino_ui_render::{Paint, PaintOp};
    use cupertino_ui_theme::{Appearance, ColorRole, ColorTokens, StateVariant};
    let themed = |appearance| {
        move |theme: &ThemeTokens| {
            Arc::new(ThemeTokens {
                colors: ColorTokens::neutral(appearance),
                ..theme.clone()
            })
        }
    };
    let (mut rt, window, _) = start_with(|o| o.theme = themed(Appearance::Light)(&o.theme));
    configure(&mut rt, window, 1);
    let frame = |rt: &mut Rt| {
        rt.window_system_mut()
            .send(window, WindowEvent::Frame(Timestamp(1)));
        rt.step(steps()).unwrap();
    };
    frame(&mut rt);
    // the window's background, the first thing its root node paints
    let background = |rt: &Rt| match &rt.snapshot(window).unwrap().display_list.nodes[0].ops[0] {
        PaintOp::Fill {
            paint: Paint::Solid(color),
            ..
        } => *color,
        other => panic!("the root paints its background first, not {other:?}"),
    };
    let light = background(&rt);
    let dark = themed(Appearance::Dark)(&rt.options.theme);
    let expected = dark
        .colors
        .get(ColorRole::WindowBackground, StateVariant::NORMAL)
        .unwrap();
    assert_ne!(
        light, expected,
        "the two themes differ in the window background"
    );

    rt.set_theme(dark);
    frame(&mut rt);
    assert_eq!(
        background(&rt),
        expected,
        "the same-size root paints in the new theme"
    );
}

#[test]
fn a_window_paints_the_theme_background_unless_its_content_draws_its_own() {
    let background = Color::new(0.9, 0.9, 0.9, 1.0);
    let (mut rt, window, _) = start_with(|o| {
        let mut theme = (*o.theme).clone();
        theme.colors = ColorTokens::new([(
            (ColorRole::WindowBackground, StateVariant::NORMAL),
            background,
        )]);
        o.theme = Arc::new(theme);
    });
    configure(&mut rt, window, 1);
    let first_op = |rt: &Rt| {
        rt.snapshot(window).unwrap().display_list.nodes[0]
            .ops
            .first()
            .cloned()
    };
    let fill = Some(PaintOp::Fill {
        shape: Shape::Rect(cupertino_geometry::Rect::new(0.0, 0.0, 200.0, 100.0)),
        paint: Paint::Solid(background),
    });
    assert_eq!(first_op(&rt), fill);

    rt.messages.push_back("panel".into());
    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(1)));
    rt.step(steps()).unwrap();
    assert_eq!(first_op(&rt), None, "a panel paints its own surface");

    rt.messages.push_back("panel".into());
    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(1)));
    rt.step(steps()).unwrap();
    assert_eq!(
        first_op(&rt),
        fill,
        "the root repaints when the background changes back"
    );
}

/// One of four choices in a roving group; only the selected one is a Tab stop. Arrows, Home and End choose another and
/// move the focus to it.
struct Choice {
    index: usize,
    selected: bool,
}

const CHOICES: usize = 4;

impl Widget<String> for Choice {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(20.0),
            height: SizeSpec::Fixed(20.0),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.selected != old.selected {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(20.0, 20.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, String>) -> Propagation {
        let Event::Key(key, _) = event else {
            return Propagation::Continue;
        };
        if !key.pressed || cx.node() != cx.target() {
            return Propagation::Continue;
        }
        let (target, index) = match key.keysym.0 {
            0xff53 => (FocusTarget::Next, (self.index + 1) % CHOICES),
            0xff51 => (FocusTarget::Previous, (self.index + CHOICES - 1) % CHOICES),
            0xff50 => (FocusTarget::First, 0),
            0xff57 => (FocusTarget::Last, CHOICES - 1),
            _ => return Propagation::Continue,
        };
        cx.emit(format!("choose:{index}"));
        cx.focus_item(target);
        Propagation::Stop
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::RadioButton);
        cx.roving_item();
        if self.selected {
            cx.focusable();
        }
    }
}

/// A plain button, a roving group of four choices, and the model's selection; the model refuses choice 2.
struct Choices;

impl Application for Choices {
    type Model = usize;
    type Message = String;

    fn init() -> (usize, Vec<Command<String>>) {
        (0, Vec::new())
    }

    fn update(selected: &mut usize, message: String) -> Vec<Command<String>> {
        if let Some(index) = message.strip_prefix("choose:") {
            let index: usize = index.parse().unwrap();
            if index != 2 {
                *selected = index;
            }
        }
        Vec::new()
    }

    fn view(selected: &usize) -> Vec<Window<String>> {
        let choices = (0..CHOICES)
            .map(|index| {
                Element::new(Choice {
                    index,
                    selected: index == *selected,
                })
                .key(Key::from(format!("choice{index}")))
            })
            .collect();
        vec![Window {
            key: WindowKey("main".into()),
            spec: WindowSpec {
                title: "Choices".into(),
                app_id: "test.choices".into(),
                size: Size::new(200.0, 100.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::Theme,
            popup: None,
            content: Element::new(row(vec![
                Element::new(leaf("before")).key("before"),
                Element::new(Group {
                    roving: true,
                    ..row(choices)
                }),
            ])),
        }]
    }
}

#[test]
fn arrows_move_the_focus_with_the_selection_in_a_roving_group_and_a_refused_one_keeps_it() {
    let damage = Rc::default();
    let mut rt = Runtime::<Choices, _>::new(FakeSystem::default(), options(&damage));
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    configure_at(&mut rt, window, 1, Size::new(200.0, 100.0));
    let key = |rt: &mut Runtime<Choices, FakeSystem>, keysym: u32| {
        rt.window_system_mut().send(
            window,
            WindowEvent::Key(
                KeyEvent {
                    code: 0,
                    keysym: Keysym(keysym),
                    text: None,
                    pressed: true,
                    repeat: false,
                    mods: Mods::NONE,
                },
                Timestamp(0),
            ),
        );
        rt.step(steps()).unwrap();
        rt.window_system_mut()
            .send(window, WindowEvent::Frame(Timestamp(1)));
        rt.step(steps()).unwrap();
    };
    let focused = |rt: &Runtime<Choices, FakeSystem>| {
        let focus = rt.snapshot(window).unwrap().focus;
        (0..CHOICES).find(|i| focus == rt.find(window, &Key::from(format!("choice{i}"))))
    };

    key(&mut rt, 0xff09);
    key(&mut rt, 0xff09);
    assert_eq!(
        focused(&rt),
        Some(0),
        "Tab stops once in the group, on its selected item"
    );
    key(&mut rt, 0xff53);
    assert_eq!((focused(&rt), *rt.model()), (Some(1), 1));
    assert!(
        rt.windows[0].focus.visible(),
        "the ring shows on the moved focus"
    );

    key(&mut rt, 0xff53);
    assert_eq!(
        (focused(&rt), *rt.model()),
        (Some(1), 1),
        "a refused choice keeps the focus"
    );

    key(&mut rt, 0xff57);
    assert_eq!((focused(&rt), *rt.model()), (Some(3), 3));
    key(&mut rt, 0xff53);
    assert_eq!(
        (focused(&rt), *rt.model()),
        (Some(0), 0),
        "Next wraps to the first"
    );
    key(&mut rt, 0xff51);
    assert_eq!(focused(&rt), Some(3), "Previous wraps to the last");
}

/// A vertical scroll container whose offset is the model's; it reports its geometry and reveals as messages.
struct Pane {
    name: &'static str,
    height: SizeSpec,
    offset: f64,
    content: Option<Element<String>>,
}

impl Widget<String> for Pane {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: self.height,
            overflow: Overflow::Clip,
            ..LayoutSpec::new(ContainerKind::Scroll {
                horizontal: false,
                vertical: true,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<String>> {
        self.content.take().into_iter().collect()
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::Layout
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, 0.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, String>) -> Propagation {
        match event {
            Event::ScrollGeometry { viewport, content } => cx.emit(format!(
                "geometry:{}:{}:{}",
                self.name, viewport.height, content.height
            )),
            Event::Reveal(offset) => cx.emit(format!("reveal:{}:{}", self.name, offset.y)),
            _ => return Propagation::Continue,
        }
        Propagation::Stop
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.focus_fallback();
    }

    fn scroll_offset(&self, _: &()) -> Point {
        Point::new(0.0, self.offset)
    }
}

/// A block of a fixed height that a click focuses. An item of a roving group asks End to focus the item keyed `end`.
struct Block {
    height: f64,
    focusable: bool,
    end: Option<u64>,
}

fn block(height: f64, focusable: bool) -> Element<String> {
    Element::new(Block {
        height,
        focusable,
        end: None,
    })
}

impl Widget<String> for Block {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(50.0),
            height: SizeSpec::Fixed(self.height),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(50.0, self.height),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, String>) -> Propagation {
        match event {
            Event::Pointer(PointerEvent::Button { pressed: true, .. }, _) if self.focusable => {
                cx.request_focus();
            }
            Event::Key(key, _) if key.pressed && key.keysym.0 == 0xff57 => {
                let Some(end) = self.end else {
                    return Propagation::Continue;
                };
                cx.emit(format!("cursor:{end}"));
                cx.focus_item(FocusTarget::Key(Key::Int(end)));
            }
            _ => return Propagation::Continue,
        }
        Propagation::Stop
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        if self.focusable {
            cx.role(Role::Button);
            cx.focusable();
        }
        if self.end.is_some() {
            cx.roving_item();
        }
    }
}

fn column(children: Vec<Element<String>>) -> Element<String> {
    struct Column(Vec<Element<String>>);
    impl Widget<String> for Column {
        type State = ();
        fn layout(&self) -> LayoutSpec {
            LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: Align::Start,
                justify: Justify::Start,
            })
        }
        fn take_children(&mut self) -> Vec<Element<String>> {
            std::mem::take(&mut self.0)
        }
        fn state(&self) {}
        fn update(&self, _: &Self, _: &mut ()) -> Change {
            Change::Layout
        }
        fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
            Measured {
                size: Size::new(0.0, 0.0),
                baseline: None,
            }
        }
        fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, String>) -> Propagation {
            Propagation::Continue
        }
        fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}
        fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
    }
    Element::new(Column(children))
}

/// An outer pane as tall as the window, scrolled 20 down: a focusable top block (30), a gap (200), an inner pane 40 tall around a
/// hidden field (100 + 20), one row of 20 per 20 of the outer viewport the model heard, and a roving list that builds
/// only its cursor item.
struct Scrolling;

struct Panes {
    outer: f64,
    inner: f64,
    viewport: f64,
    cursor: u64,
    heard: Vec<String>,
}

impl Application for Scrolling {
    type Model = Panes;
    type Message = String;

    fn init() -> (Panes, Vec<Command<String>>) {
        (
            Panes {
                outer: 20.0,
                inner: 0.0,
                viewport: 0.0,
                cursor: 0,
                heard: Vec::new(),
            },
            Vec::new(),
        )
    }

    fn update(model: &mut Panes, message: String) -> Vec<Command<String>> {
        let parts: Vec<&str> = message.split(':').collect();
        match parts[..] {
            ["geometry", name, viewport, content] => {
                if name == "outer" {
                    model.viewport = viewport.parse().unwrap();
                }
                model.heard.push(format!("{name}:{viewport}:{content}"));
            }
            ["reveal", "outer", y] => model.outer = y.parse().unwrap(),
            ["reveal", "inner", y] => model.inner = y.parse().unwrap(),
            ["cursor", i] => model.cursor = i.parse().unwrap(),
            _ => {}
        }
        Vec::new()
    }

    fn view(model: &Panes) -> Vec<Window<String>> {
        let rows = (0..(model.viewport / 20.0) as usize).map(|_| block(20.0, false));
        let list = Element::new(Group {
            roving: true,
            ..group(vec![Element::new(Block {
                height: 20.0,
                focusable: true,
                end: Some(9),
            })
            .key(Key::Int(model.cursor))])
        });
        let content = column(
            [
                block(30.0, true).key("top"),
                block(200.0, false),
                Element::new(Pane {
                    name: "inner",
                    height: SizeSpec::Fixed(40.0),
                    offset: model.inner,
                    content: Some(column(vec![
                        block(100.0, false),
                        block(20.0, true).key("field"),
                    ])),
                }),
            ]
            .into_iter()
            .chain(rows)
            .chain([list])
            .collect(),
        );
        vec![Window {
            key: WindowKey("main".into()),
            spec: WindowSpec {
                title: "Scrolling".into(),
                app_id: "test.scrolling".into(),
                size: Size::new(200.0, 100.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::Theme,
            popup: None,
            content: Element::new(Pane {
                name: "outer",
                height: SizeSpec::Fill,
                offset: model.outer,
                content: Some(content),
            }),
        }]
    }
}

fn scrolling_at(height: f64) -> (Runtime<Scrolling, FakeSystem>, WindowId) {
    let damage = Rc::default();
    let mut rt = Runtime::<Scrolling, FakeSystem>::new(FakeSystem::default(), options(&damage));
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    configure_at(&mut rt, window, 1, Size::new(200.0, height));
    (rt, window)
}

fn scrolling() -> (Runtime<Scrolling, FakeSystem>, WindowId) {
    scrolling_at(100.0)
}

fn press(rt: &mut Runtime<Scrolling, FakeSystem>, window: WindowId, keysym: u32) {
    rt.window_system_mut().send(
        window,
        WindowEvent::Key(
            KeyEvent {
                code: 0,
                keysym: Keysym(keysym),
                text: None,
                pressed: true,
                repeat: false,
                mods: Mods::NONE,
            },
            Timestamp(0),
        ),
    );
    rt.step(steps()).unwrap();
}

fn rect_of(rt: &Runtime<Scrolling, FakeSystem>, window: WindowId, key: &'static str) -> Rect {
    let node = rt.find(window, &key.into()).unwrap();
    let snapshot = rt.snapshot(window).unwrap();
    snapshot.nodes.iter().find(|n| n.id == node).unwrap().rect
}

#[test]
fn the_model_hears_a_scroll_geometry_before_the_first_frame_paints() {
    let (rt, window) = scrolling_at(150.0);
    assert_eq!(
        rt.model().heard,
        [
            "outer:100:290",
            "inner:40:120",
            "outer:100:390",
            "outer:150:390",
            "outer:150:430"
        ],
        "the configured size, then the rows for it, reach the model"
    );
    assert_eq!(
        rt.window_system()
            .presented
            .iter()
            .filter(|(w, _)| *w == window)
            .count(),
        1,
        "only the frame with the rows for the viewport paints"
    );
    assert_eq!(
        rt.snapshot(window).unwrap().pending.frames,
        0,
        "and nothing waits to paint after it"
    );
}

#[test]
fn keyboard_focus_reveals_a_node_through_nested_panes_and_a_click_does_not() {
    let (mut rt, window) = scrolling();
    click(&mut rt, window, 10.0);
    assert_eq!(
        rt.snapshot(window).unwrap().focus,
        rt.find(window, &"top".into())
    );
    assert_eq!(
        rt.model().outer,
        20.0,
        "a click scrolls nothing under the pointer"
    );

    press(&mut rt, window, 0xff09);
    assert_eq!(
        rt.snapshot(window).unwrap().focus,
        rt.find(window, &"field".into())
    );
    // the inner pane moves the field to its bottom edge; the outer pane shows the inner viewport's part of it
    assert_eq!((rt.model().inner, rt.model().outer), (80.0, 170.0));
    assert_eq!(rect_of(&rt, window, "field").origin.y, 80.0);
}

#[test]
fn a_roving_move_to_a_key_focuses_the_item_the_new_view_builds() {
    let (mut rt, window) = scrolling();
    for _ in 0..3 {
        press(&mut rt, window, 0xff09);
    }
    let first = rt.find(window, &Key::Int(0)).unwrap();
    assert_eq!(rt.snapshot(window).unwrap().focus, Some(first));
    press(&mut rt, window, 0xff57);
    assert_eq!(rt.model().cursor, 9);
    let end = rt.find(window, &Key::Int(9));
    assert!(end.is_some());
    assert_eq!(rt.snapshot(window).unwrap().focus, end);
}

/// An outer pane around an inner pane that holds nothing focusable.
struct NestedPanes;

impl Application for NestedPanes {
    type Model = ();
    type Message = String;

    fn init() -> ((), Vec<Command<String>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), _: String) -> Vec<Command<String>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<String>> {
        let inner = Element::new(Pane {
            name: "inner",
            height: SizeSpec::Fixed(40.0),
            offset: 0.0,
            content: Some(block(100.0, false)),
        })
        .key("inner");
        vec![Window {
            key: WindowKey("main".into()),
            spec: WindowSpec {
                title: "Nested".into(),
                app_id: "test.nested".into(),
                size: Size::new(200.0, 100.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::Theme,
            popup: None,
            content: Element::new(Pane {
                name: "outer",
                height: SizeSpec::Fill,
                offset: 0.0,
                content: Some(column(vec![inner, block(200.0, false)])),
            })
            .key("outer"),
        }]
    }
}

#[test]
fn an_inner_pane_that_became_a_tab_stop_keeps_the_outer_pane_from_being_one() {
    let damage = Rc::default();
    let mut rt = Runtime::<NestedPanes, FakeSystem>::new(FakeSystem::default(), options(&damage));
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    configure_at(&mut rt, window, 1, Size::new(200.0, 100.0));
    let inner = rt.find(window, &"inner".into());
    for _ in 0..2 {
        rt.window_system_mut().send(
            window,
            WindowEvent::Key(
                KeyEvent {
                    code: 0,
                    keysym: Keysym(0xff09),
                    text: None,
                    pressed: true,
                    repeat: false,
                    mods: Mods::NONE,
                },
                Timestamp(0),
            ),
        );
        rt.step(steps()).unwrap();
        assert_eq!(rt.snapshot(window).unwrap().focus, inner);
    }
}
