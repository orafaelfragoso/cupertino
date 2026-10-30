use super::*;
use crate::test_support::{
    leaf, options, options_with, row, FakeAdapter, FakeSystem, Leaf, MAX_PIXELS,
};
use crate::{Role, Shortcut};
use cupertino_ui_platform::{
    Configure, DecorationMode, Mods, MouseButton, PointerEvent, WindowStates, WmCapabilities,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Two buttons; pressing `plus` (or Primary+=) counts up, and a `wait` message starts a task.
struct Counter;

#[derive(Default)]
struct Model {
    count: u32,
    done: Vec<u32>,
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

fn configure_at(rt: &mut Rt, window: WindowId, id: u32, size: Size) {
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

fn click(rt: &mut Rt, window: WindowId, x: f64) {
    for pressed in [true, false] {
        rt.window_system_mut().send(
            window,
            WindowEvent::Pointer(
                PointerEvent::Button {
                    position: Point::new(x, 5.0),
                    button: MouseButton::Left,
                    pressed,
                    serial: 1,
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
