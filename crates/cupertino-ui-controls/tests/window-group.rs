use crate::support;

use cupertino_geometry::Size;
use cupertino_ui_controls::{GroupWindow, Spacer, WindowGroup};
use cupertino_ui_core::{
    Application, Command, CommandRegistry, Element, Window, WindowBackground, WindowKey,
};
use cupertino_ui_platform::{
    DecorationMode, Mods, WindowEvent, WindowId, WindowSpec, WindowStates,
};
use support::Harness;

const N: u32 = 0x6e;
const W: u32 = 0x77;

struct Documents;

#[derive(Debug)]
struct Model {
    open: Vec<u64>,
    next: u64,
}

#[derive(Clone, Debug, PartialEq)]
enum Message {
    New,
    Close(u64),
}

impl Application for Documents {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        (
            Model {
                open: vec![1],
                next: 2,
            },
            Vec::new(),
        )
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::New => {
                model.open.push(model.next);
                model.next += 1;
            }
            Message::Close(id) => model.open.retain(|&open| open != id),
        }
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        WindowGroup {
            key: "document".into(),
            windows: model
                .open
                .iter()
                .map(|&id| GroupWindow {
                    id,
                    spec: WindowSpec {
                        title: format!("Untitled {id}"),
                        app_id: "test.controls".into(),
                        size: Size::new(200.0, 100.0),
                        min_size: None,
                        max_size: None,
                        decorations: DecorationMode::Preferred,
                        foreign_parent: None,
                        input_region: None,
                    },
                    shortcuts: CommandRegistry::new(),
                    background: WindowBackground::Theme,
                    content: Element::new(Spacer { min: 0.0 }),
                })
                .collect(),
            on_new: Message::New,
            on_close: Box::new(Message::Close),
        }
        .windows()
    }
}

fn windows(h: &Harness<Documents>) -> Vec<(WindowKey, WindowId)> {
    h.rt.windows()
}

/// Moves the harness onto the group's window `id`.
fn enter(h: &mut Harness<Documents>, id: u64) -> WindowId {
    let key = WindowKey(format!("document/{id}").into());
    let window = windows(h)
        .into_iter()
        .find(|(k, _)| *k == key)
        .expect("open")
        .1;
    h.rt.window_system_mut()
        .configure(window, None, WindowStates::default())
        .unwrap();
    h.window = window;
    h.settle();
    window
}

/// Presses Primary and `keysym`; the release would go to a window the press may have closed.
fn primary(h: &mut Harness<Documents>, keysym: u32) {
    h.key_event(keysym, true, false, Mods::PRIMARY);
}

#[test]
fn primary_n_opens_another_window_and_primary_w_or_the_close_button_closes_the_one_it_is_pressed_in(
) {
    let mut h = Harness::<Documents>::new();
    let first = h.window;
    primary(&mut h, N);
    primary(&mut h, N);
    assert_eq!(h.model().open, [1, 2, 3]);
    assert_eq!(windows(&h).len(), 3);
    let second = enter(&mut h, 2);
    primary(&mut h, W);
    assert_eq!(h.model().open, [1, 3]);
    assert!(windows(&h).iter().all(|&(_, id)| id != second));
    enter(&mut h, 3);
    h.send(WindowEvent::CloseRequested);
    assert_eq!(h.model().open, [1]);
    assert_eq!(
        windows(&h),
        [(WindowKey("document/1".into()), first)],
        "the first window stayed itself"
    );
}
