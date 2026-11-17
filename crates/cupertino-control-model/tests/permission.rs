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

#[test]
fn services_follow_from_the_operations_asked_for() {
    for op in [
        Operation::Observe,
        Operation::Pointer,
        Operation::Keyboard,
        Operation::Semantic,
    ] {
        let s = Services::needed_by(Capabilities::of([op]));
        assert_eq!(s.iter().collect::<Vec<_>>(), vec![Service::Control]);
    }
    let capture = Services::needed_by(Capabilities::of([Operation::Capture]));
    assert_eq!(capture.iter().collect::<Vec<_>>(), vec![Service::Capture]);
    let both = Services::needed_by(Capabilities::of([Operation::Observe, Operation::Capture]));
    assert!(both.contains(Service::Control) && both.contains(Service::Capture));
    assert!(Services::needed_by(Capabilities::NONE).is_empty());
}

#[test]
fn the_wire_bits_are_control_one_capture_two() {
    let mut s = Services::NONE;
    assert_eq!(s.bits(), 0);
    s.insert(Service::Capture);
    assert_eq!(s.bits(), 2);
    s.insert(Service::Control);
    assert_eq!(s.bits(), 3);
    s.insert(Service::Control);
    assert_eq!(s.bits(), 3, "insert is idempotent");
}

#[test]
fn union_and_collect_agree() {
    let a: Services = [Service::Control].into_iter().collect();
    let b: Services = [Service::Capture].into_iter().collect();
    assert_eq!(a.union(b).bits(), 3);
    assert_eq!(a.union(a), a);
    assert_eq!(a.union(Services::NONE), a);
}

#[test]
fn all_allowed_starts_without_asking() {
    let needed = Services::needed_by(Capabilities::of([Operation::Observe, Operation::Capture]));
    let all = remembered(Some(Decision::Allow), Some(Decision::Allow));
    assert_eq!(decide(needed, all), Verdict::Allowed);
}

#[test]
fn a_service_turned_off_refuses_even_when_another_is_undecided() {
    let needed = Services::needed_by(Capabilities::of([Operation::Observe, Operation::Capture]));
    assert_eq!(
        decide(needed, remembered(None, Some(Decision::Deny))),
        Verdict::Denied
    );
    assert_eq!(
        decide(
            needed,
            remembered(Some(Decision::Deny), Some(Decision::Allow))
        ),
        Verdict::Denied
    );
}

#[test]
fn only_the_undecided_services_are_asked() {
    let needed = Services::needed_by(Capabilities::of([Operation::Observe, Operation::Capture]));
    let Verdict::Ask(ask) = decide(needed, remembered(Some(Decision::Allow), None)) else {
        panic!("capture is undecided");
    };
    assert_eq!(ask.iter().collect::<Vec<_>>(), vec![Service::Capture]);
}

#[test]
fn a_denied_service_that_is_not_needed_does_not_matter() {
    let needed = Services::needed_by(Capabilities::of([Operation::Observe]));
    assert_eq!(
        decide(
            needed,
            remembered(Some(Decision::Allow), Some(Decision::Deny))
        ),
        Verdict::Allowed
    );
}

#[test]
fn a_request_needing_nothing_is_allowed() {
    assert_eq!(
        decide(Services::NONE, remembered(None, None)),
        Verdict::Allowed
    );
}

#[test]
fn a_run_loses_a_service_turned_off_or_forgotten() {
    let held: Services = Service::ALL.into_iter().collect();
    let allow = Some(Decision::Allow);
    assert!(!lost(held, remembered(allow, allow)));
    assert!(lost(held, remembered(allow, Some(Decision::Deny))));
    assert!(lost(held, remembered(None, allow)));
    assert!(!lost(Services::NONE, remembered(None, None)));
    let control: Services = [Service::Control].into_iter().collect();
    assert!(!lost(control, remembered(allow, None)));
}
