//! The shared and separate hosts must interpret a bridged publication identically.

use cupertino_kit_shell::bus::{Message, Sender, Topic};
use cupertino_kit_shell::ComponentName;
use serde_json::{from_value, json, to_value, Value};

#[test]
fn a_bridged_publication_keeps_its_payload_topic_and_delivery_kind() {
    for (wire, topic, retained) in [
        (
            json!({"type":"ComponentStopped", "component":"menubar"}),
            Topic::Host,
            false,
        ),
        (
            json!({"type":"TogglePanel", "panel":"ControlPanel", "output":null}),
            Topic::Panels,
            false,
        ),
        (
            json!({"type":"PanelShown", "panel":"ControlPanel", "shown":true, "output":17}),
            Topic::Panels,
            true,
        ),
        (
            json!({"type":"SetDoNotDisturb", "on":true, "lifetime":null}),
            Topic::DoNotDisturb,
            false,
        ),
        (
            json!({"type":"DoNotDisturbChanged", "on":true, "lifetimes":[["hour","One hour"]]}),
            Topic::DoNotDisturb,
            true,
        ),
        (
            json!({"type":"SessionMenu","user_name":"A","lock":true,"log_out":true,"switch_user":false,"sleep":true,"restart":false,"shut_down":true}),
            Topic::Session,
            true,
        ),
        (
            json!({"type":"ShowOsd","kind":"Brightness","level":0.5}),
            Topic::Osd,
            false,
        ),
        (
            json!({"type":"Transaction","id":{"component":"overview","instance":2,"serial":8},"output":17,"scene":"OverviewOpening"}),
            Topic::Transactions,
            false,
        ),
        (
            json!({"type":"OverviewShown","output":17,"shown":true}),
            Topic::Overview,
            true,
        ),
    ] {
        let message: Message = from_value(wire.clone()).unwrap();
        assert_eq!((message.topic(), message.retained()), (topic, retained));
        assert_eq!(to_value(message).unwrap(), wire);
    }
}

#[test]
fn an_overview_said_shown_is_said_gone_when_its_publisher_stops_and_nothing_else_is_said_again() {
    let shown: Message =
        from_value(json!({"type":"OverviewShown","output":17,"shown":true})).unwrap();
    assert_eq!(
        shown.withdrawn().map(|m| to_value(m).unwrap()),
        Some(json!({"type":"OverviewShown","output":17,"shown":false}))
    );
    for wire in [
        json!({"type":"OverviewShown","output":17,"shown":false}),
        json!({"type":"PanelShown", "panel":"ControlPanel", "shown":true, "output":17}),
        json!({"type":"DoNotDisturbChanged", "on":true, "lifetimes":[]}),
        json!({"type":"ShowOsd","kind":"Brightness","level":0.5}),
    ] {
        let message: Message = from_value(wire).unwrap();
        assert_eq!(message.withdrawn(), None, "{message:?}");
    }
}

#[test]
fn an_unknown_message_or_field_cannot_be_treated_as_a_known_publication() {
    for wire in [
        json!({"type":"OpenWindows"}),
        json!({"type":"ComponentStopped","component":"no_such_component"}),
        json!({"type":"ComponentStopped","component":"osd","from":"host"}),
        json!({"type":"Transaction","id":{"component":"overview","instance":2,"serial":8,"other":true},"output":17,"scene":"StageSwitch"}),
    ] {
        assert!(from_value::<Message>(wire).is_err());
    }
}

#[test]
fn a_bridge_names_its_publisher_separately_from_the_message() {
    for (name, publisher) in [
        ("host", Sender::Host),
        ("agent", Sender::Component(ComponentName::AGENT)),
        ("dialogs", Sender::Component(ComponentName::DIALOGS)),
    ] {
        assert_eq!(from_value::<Sender>(json!(name)).unwrap(), publisher);
        assert_eq!(to_value(publisher).unwrap(), name);
    }
    for name in [json!("cupertino-login"), json!("Host"), Value::Null] {
        assert!(from_value::<Sender>(name).is_err());
    }
}
