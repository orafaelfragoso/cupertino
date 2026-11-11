//! What the person decided about an app decides each request at once. Observing, semantic actions, pointer and keyboard
//! input need the app's [`Service::Control`]; images of the screen need its [`Service::Capture`]. A service the person
//! turned off refuses the request without asking, all services turned on start the run without asking, and otherwise
//! the person is asked for the services not yet decided.

use crate::{Capabilities, Operation};

/// A permission the person gives an app, asked and remembered on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    /// Observe windows and their controls, perform semantic actions, send pointer and keyboard input.
    Control,
    /// Take images of the screen.
    Capture,
}

impl Service {
    /// Every service, in a fixed order.
    pub const ALL: [Service; 2] = [Service::Control, Service::Capture];

    const fn bit(self) -> u8 {
        match self {
            Service::Control => 1,
            Service::Capture => 2,
        }
    }
}

/// A set of services. Its bits are also the wire encoding of the consent authority (control 1, capture 2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Services(u8);

impl Services {
    pub const NONE: Self = Self(0);

    /// The services that `capabilities` need.
    pub fn needed_by(capabilities: Capabilities) -> Self {
        let control = [
            Operation::Observe,
            Operation::Pointer,
            Operation::Keyboard,
            Operation::Semantic,
        ]
        .into_iter()
        .any(|op| capabilities.allows(op));
        let mut services = Self::NONE;
        if control {
            services.insert(Service::Control);
        }
        if capabilities.allows(Operation::Capture) {
            services.insert(Service::Capture);
        }
        services
    }

    pub fn insert(&mut self, service: Service) {
        self.0 |= service.bit();
    }

    pub fn contains(self, service: Service) -> bool {
        self.0 & service.bit() != 0
    }

    pub fn is_empty(self) -> bool {
        self == Self::NONE
    }

    /// The services in either set.
    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub fn iter(self) -> impl Iterator<Item = Service> {
        Service::ALL.into_iter().filter(move |s| self.contains(*s))
    }

    /// The wire bits: control 1, capture 2.
    pub fn bits(self) -> u32 {
        u32::from(self.0)
    }
}

impl FromIterator<Service> for Services {
    fn from_iter<I: IntoIterator<Item = Service>>(services: I) -> Self {
        let mut set = Self::NONE;
        for service in services {
            set.insert(service);
        }
        set
    }
}

/// The person's remembered answer for one app and service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
}

/// What a request becomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Every needed service is allowed: the run starts without asking.
    Allowed,
    /// A needed service is turned off: the request is refused without asking.
    Denied,
    /// Nothing needed is turned off; the person is asked for these undecided services.
    Ask(Services),
}

/// The verdict for a request needing `needed`, given each service's remembered `decision` for its app.
pub fn decide(needed: Services, decision: impl Fn(Service) -> Option<Decision>) -> Verdict {
    let mut undecided = Services::NONE;
    for service in needed.iter() {
        match decision(service) {
            Some(Decision::Deny) => return Verdict::Denied,
            Some(Decision::Allow) => {}
            None => undecided.insert(service),
        }
    }
    if undecided.is_empty() {
        Verdict::Allowed
    } else {
        Verdict::Ask(undecided)
    }
}

/// Whether a run that holds `held` from remembered answers lost one of them: it is now turned off or forgotten.
pub fn lost(held: Services, decision: impl Fn(Service) -> Option<Decision>) -> bool {
    held.iter()
        .any(|service| decision(service) != Some(Decision::Allow))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(ops: &[Operation]) -> Capabilities {
        Capabilities::of(ops.iter().copied())
    }

    #[test]
    fn capture_needs_its_own_service_and_everything_else_needs_control() {
        for op in [
            Operation::Observe,
            Operation::Pointer,
            Operation::Keyboard,
            Operation::Semantic,
        ] {
            assert_eq!(
                Services::needed_by(caps(&[op])),
                [Service::Control].into_iter().collect()
            );
        }
        assert_eq!(
            Services::needed_by(caps(&[Operation::Capture])),
            [Service::Capture].into_iter().collect()
        );
        let both = Services::needed_by(caps(&[Operation::Observe, Operation::Capture]));
        assert_eq!(both.bits(), 3);
        assert_eq!(both.iter().collect::<Vec<_>>(), Service::ALL);
    }

    #[test]
    fn one_service_turned_off_refuses_and_all_turned_on_allow() {
        let both: Services = Service::ALL.into_iter().collect();
        assert_eq!(decide(both, |_| Some(Decision::Allow)), Verdict::Allowed);
        assert_eq!(
            decide(both, |s| (s == Service::Capture).then_some(Decision::Deny)),
            Verdict::Denied,
            "a denial wins over an undecided service"
        );
        assert_eq!(
            decide(both, |s| Some(if s == Service::Control {
                Decision::Allow
            } else {
                Decision::Deny
            })),
            Verdict::Denied
        );
    }

    #[test]
    fn only_the_undecided_services_are_asked() {
        let both: Services = Service::ALL.into_iter().collect();
        assert_eq!(decide(both, |_| None), Verdict::Ask(both));
        assert_eq!(
            decide(both, |s| (s == Service::Control).then_some(Decision::Allow)),
            Verdict::Ask([Service::Capture].into_iter().collect())
        );
        let control: Services = [Service::Control].into_iter().collect();
        assert_eq!(
            decide(control, |s| (s == Service::Capture)
                .then_some(Decision::Deny)),
            Verdict::Ask(control),
            "a service the request does not need decides nothing"
        );
    }

    #[test]
    fn a_run_loses_its_service_when_it_is_turned_off_or_forgotten_only() {
        let control: Services = [Service::Control].into_iter().collect();
        assert!(!lost(control, |_| Some(Decision::Allow)));
        assert!(lost(control, |_| Some(Decision::Deny)));
        assert!(lost(control, |_| None));
        assert!(
            !lost(control, |s| (s == Service::Control)
                .then_some(Decision::Allow)),
            "a change to another service keeps the run"
        );
        assert!(
            !lost(Services::NONE, |_| None),
            "an answer given for this run only"
        );
    }
}
