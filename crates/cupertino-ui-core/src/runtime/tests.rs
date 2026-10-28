use super::*;
use crate::test_support::{leaf, options, row, FakeSystem};
use crate::{Role, Shortcut};
use cupertino_ui_platform::{
    Configure, DecorationMode, Mods, MouseButton, PointerEvent, WindowStates, WmCapabilities,
};
use std::cell::RefCell;
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
                Element::new(leaf("minus")).key("minus"),
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
    let damage = Rc::default();
    let mut rt = Rt::new(FakeSystem::default(), options(&damage));
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    (rt, window, damage)
}

fn configure(rt: &mut Rt, window: WindowId, id: u32) {
    rt.window_system_mut().send(
        window,
        WindowEvent::Configure(Configure {
            id: ConfigureId(id),
            size: Some(Size::new(200.0, 100.0)),
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
