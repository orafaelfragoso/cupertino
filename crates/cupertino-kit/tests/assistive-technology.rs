//! What assistive technology asks of an application's controls reaches them through the runtime: a value set on a
//! text field, a focus move, a click; and a keyed control keeps its node, which its AT-SPI object path is made of, when
//! the view is built again around it.

use crate::support;

use std::cell::RefCell;
use std::rc::Rc;

use cupertino_kit::controls::WINDOW_CONTENT_INSET;
use cupertino_kit::core::{
    AccessibilityAdapter, AccessibilityUpdate, Action, ActionData, ActionRequest, CommandRegistry,
    NodeId,
};
use cupertino_kit::geometry::Size;
use cupertino_kit::platform::{DecorationMode, PlatformEvent, WindowId};
use cupertino_kit::prelude::*;
use cupertino_kit::text::{AttributedText, ContentPurpose, TextStyle};
use support::Run;

struct Form;

#[derive(Default)]
struct Model {
    name: String,
    revision: u64,
    changes: usize,
    presses: usize,
}

#[derive(Clone, Debug, PartialEq)]
enum Message {
    Name(TextChange),
    Press,
}

impl Application for Form {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        (Model::default(), Vec::new())
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Name(change) => {
                model.name = change.text;
                model.revision = change.revision;
                model.changes += 1;
            }
            Message::Press => model.presses += 1,
        }
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        let pressed = (model.presses > 0).then(|| {
            Element::new(Text {
                content: AttributedText::plain("Pressed", TextStyle::new(13.0)),
                role: cupertino_kit::controls::TextRole::Body,
                wrap: false,
                secondary: false,
                selected: false,
            })
        });
        let children = pressed
            .into_iter()
            .chain([
                Element::new(TextField {
                    label: "Name".into(),
                    placeholder: "Your name".into(),
                    value: model.name.clone(),
                    revision: model.revision,
                    purpose: ContentPurpose::Name,
                    enabled: true,
                    on_change: Some(Box::new(Message::Name)),
                    on_submit: None,
                    selection: None,
                    initial: false,
                })
                .key("name"),
                Element::new(Button {
                    label: format!("OK {}", model.presses),
                    role: ButtonRole::Default,
                    style: ButtonStyle::Bordered,
                    enabled: true,
                    busy: false,
                    on_press: Some(Message::Press),
                })
                .key("ok"),
            ])
            .collect();
        vec![Window {
            key: WindowKey("form".into()),
            spec: WindowSpec {
                title: "Form".into(),
                app_id: "org.example.Form".into(),
                size: Size::new(280.0, 160.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::Theme,
            popup: None,
            content: Element::new(Stack {
                axis: Axis::Vertical,
                padding: WINDOW_CONTENT_INSET,
                spacing: 8.0,
                align: Align::Stretch,
                children,
            }),
            menus: None,
        }]
    }
}

/// An assistive technology that is always listening: it keeps what it is told and hands over what it asks for.
#[derive(Clone, Default)]
struct Assistive {
    updates: Rc<RefCell<Vec<AccessibilityUpdate>>>,
    asked: Rc<RefCell<Vec<ActionRequest>>>,
}

impl AccessibilityAdapter for Assistive {
    fn is_active(&self) -> bool {
        true
    }

    fn window_focus(&mut self, _: WindowId, _: bool) {}

    fn update(&mut self, update: &AccessibilityUpdate) {
        self.updates.borrow_mut().push(update.clone());
    }

    fn drain_actions(&mut self, out: &mut Vec<ActionRequest>) {
        out.append(&mut self.asked.borrow_mut());
    }
}

impl Assistive {
    /// Asks `target` for `action`, and wakes the application as the real adapter does.
    fn ask(&self, run: &mut Run<Form>, target: NodeId, action: Action, data: Option<ActionData>) {
        self.asked.borrow_mut().push(ActionRequest {
            target,
            action,
            data,
        });
        run.driver
            .window_system_mut()
            .enqueue(PlatformEvent::Wake)
            .unwrap();
        run.settle();
    }

    /// The name the latest update gave `node`.
    fn name(&self, node: NodeId) -> Option<String> {
        self.updates
            .borrow()
            .iter()
            .rev()
            .find_map(|u| u.nodes.iter().find(|(id, _)| *id == node))
            .and_then(|(_, n)| n.name.clone())
    }
}

fn node(run: &Run<Form>, key: &'static str) -> NodeId {
    run.driver
        .runtime()
        .find(run.window, &Key::from(key))
        .unwrap()
}

#[test]
fn a_value_set_by_assistive_technology_changes_the_model_once_and_its_focus_request_focuses_the_field(
) {
    let at = Assistive::default();
    let mut run = Run::<Form>::with_accessibility(Some(Box::new(at.clone())));
    let field = node(&run, "name");

    at.ask(
        &mut run,
        field,
        Action::SetValue,
        Some(ActionData::Text("Grace".into())),
    );
    assert_eq!(
        (run.model().name.as_str(), run.model().changes),
        ("Grace", 1)
    );

    assert_ne!(run.snapshot().focus, Some(field));
    at.ask(&mut run, field, Action::Focus, None);
    assert_eq!(run.snapshot().focus, Some(field));
    assert_eq!(run.model().changes, 1, "focus changes no value");
}

#[test]
fn a_keyed_button_keeps_its_node_when_its_click_rebuilds_the_view_around_it() {
    let at = Assistive::default();
    let mut run = Run::<Form>::with_accessibility(Some(Box::new(at.clone())));
    let ok = node(&run, "ok");
    assert_eq!(at.name(ok).as_deref(), Some("OK 0"));

    at.ask(&mut run, ok, Action::Click, None);
    assert_eq!(run.model().presses, 1);
    // a text now comes before the button, and the button's label changed
    assert_eq!(node(&run, "ok"), ok);
    assert_eq!(at.name(ok).as_deref(), Some("OK 1"));
}
