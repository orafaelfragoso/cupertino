//! A request is decided from what the person remembered for the app, without asking when everything is settled.

use cupertino_control_model::permission::{decide, lost, Decision, Service, Services, Verdict};
use cupertino_control_model::{Capabilities, Operation};

fn remembered(
    control: Option<Decision>,
    capture: Option<Decision>,
) -> impl Fn(Service) -> Option<Decision> {
    move |s| match s {
        Service::Control => control,
        Service::Capture => capture,
    }
}

fn services(list: &[Service]) -> Services {
    list.iter().copied().collect()
}

#[test]
fn services_follow_from_the_operations_asked_for() {
    use Operation::*;
    for (ops, expected) in [
        (&[Observe][..], &[Service::Control][..]),
        (&[Pointer], &[Service::Control]),
        (&[Keyboard], &[Service::Control]),
        (&[Semantic], &[Service::Control]),
        (&[Capture], &[Service::Capture]),
        (&[Observe, Capture], &[Service::Control, Service::Capture]),
        (&[], &[]),
    ] {
        let needed = Services::needed_by(Capabilities::of(ops.iter().copied()));
        assert_eq!(needed.iter().collect::<Vec<_>>(), expected, "{ops:?}");
    }
}

#[test]
fn the_wire_bits_are_control_one_capture_two() {
    for (list, bits) in [
        (&[][..], 0),
        (&[Service::Control], 1),
        (&[Service::Capture], 2),
        (&[Service::Capture, Service::Control, Service::Control], 3),
    ] {
        assert_eq!(services(list).bits(), bits, "{list:?}");
    }
    let control = services(&[Service::Control]);
    assert_eq!(control.union(services(&[Service::Capture])).bits(), 3);
    assert_eq!(control.union(control), control);
    assert_eq!(control.union(Services::NONE), control);
}

#[test]
fn a_request_is_allowed_denied_or_asks_only_for_what_is_undecided() {
    let allow = Some(Decision::Allow);
    let deny = Some(Decision::Deny);
    let both = services(&[Service::Control, Service::Capture]);
    let control = services(&[Service::Control]);
    for (name, needed, control_choice, capture_choice, expected) in [
        ("all allowed", both, allow, allow, Verdict::Allowed),
        (
            "one turned off while another is undecided",
            both,
            None,
            deny,
            Verdict::Denied,
        ),
        ("one turned off", both, deny, allow, Verdict::Denied),
        (
            "one undecided",
            both,
            allow,
            None,
            Verdict::Ask(services(&[Service::Capture])),
        ),
        (
            "a denied service that is not needed",
            control,
            allow,
            deny,
            Verdict::Allowed,
        ),
        (
            "nothing needed",
            Services::NONE,
            None,
            None,
            Verdict::Allowed,
        ),
    ] {
        assert_eq!(
            decide(needed, remembered(control_choice, capture_choice)),
            expected,
            "{name}"
        );
    }
}

#[test]
fn a_run_loses_a_service_turned_off_or_forgotten() {
    let allow = Some(Decision::Allow);
    let deny = Some(Decision::Deny);
    let all: Services = Service::ALL.into_iter().collect();
    for (name, held, control, capture, expected) in [
        ("all still allowed", all, allow, allow, false),
        ("one turned off", all, allow, deny, true),
        ("one forgotten", all, None, allow, true),
        ("nothing held", Services::NONE, None, None, false),
        (
            "an unheld service forgotten",
            services(&[Service::Control]),
            allow,
            None,
            false,
        ),
    ] {
        assert_eq!(lost(held, remembered(control, capture)), expected, "{name}");
    }
}
