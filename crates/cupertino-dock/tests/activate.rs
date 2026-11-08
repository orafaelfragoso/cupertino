//! Asking a resident application for a window over D-Bus, against a fake `org.freedesktop.Application` on a private
//! session bus: the call reaches the object path of the desktop file ID with the activation token in its platform
//! data, a name nobody owns fails, and an application that does not answer fails once the call's time is up.
#![cfg(feature = "app")]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use std::time::{Duration, Instant};

use cupertino_dock::activate::{activate, bounded, object_path};
use cupertino_test_bus::PrivateBus;
use zbus::zvariant::OwnedValue;

const APP: &str = "org.example.Notes";

/// The platform data of each `Activate` call the fake application took.
#[derive(Clone, Default)]
struct Calls(Arc<Mutex<Vec<HashMap<String, OwnedValue>>>>);

struct FakeApplication(Calls);

#[zbus::interface(name = "org.freedesktop.Application")]
impl FakeApplication {
    fn activate(&self, platform_data: HashMap<String, OwnedValue>) {
        self.0 .0.lock().unwrap().push(platform_data);
    }

    fn open(&self, _uris: Vec<String>, _platform_data: HashMap<String, OwnedValue>) {}

    fn activate_action(
        &self,
        _name: String,
        _parameter: Vec<OwnedValue>,
        _platform_data: HashMap<String, OwnedValue>,
    ) {
    }
}

fn text(data: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    data.get(key).and_then(|v| String::try_from(v.clone()).ok())
}

#[test]
fn activate_reaches_the_application_with_the_token_and_fails_for_a_name_nobody_owns() {
    let Some(bus) = PrivateBus::start() else {
        return;
    };
    let calls = Calls::default();
    let served = calls.clone();
    async_io::block_on(async {
        let _application = bus
            .builder()
            .name(APP)
            .unwrap()
            .serve_at(object_path(APP), FakeApplication(served))
            .unwrap()
            .build()
            .await
            .unwrap();
        let dock = bus.connect_async().await;

        activate(&dock, APP, Some("token-1")).await.unwrap();
        activate(&dock, APP, None).await.unwrap();
        let taken = calls.0.lock().unwrap().clone();
        assert_eq!(taken.len(), 2);
        assert_eq!(
            text(&taken[0], "activation-token").as_deref(),
            Some("token-1")
        );
        assert_eq!(
            text(&taken[0], "desktop-startup-id").as_deref(),
            Some("token-1")
        );
        assert!(taken[1].is_empty(), "no token, no platform data");

        assert!(
            activate(&dock, "org.example.Missing", Some("token-2"))
                .await
                .is_err(),
            "nobody owns the name"
        );
    });
}

const HUNG: &str = "org.example.Hung";

/// An application that takes `Activate` calls and never answers them.
struct HungApplication;

#[zbus::interface(name = "org.freedesktop.Application")]
impl HungApplication {
    async fn activate(&self, _platform_data: HashMap<String, OwnedValue>) {
        std::future::pending::<()>().await;
    }
}

#[test]
fn an_application_that_does_not_answer_fails_the_call_when_its_time_is_up() {
    let Some(bus) = PrivateBus::start() else {
        return;
    };
    async_io::block_on(async {
        let _application = bus
            .builder()
            .name(HUNG)
            .unwrap()
            .serve_at(object_path(HUNG), HungApplication)
            .unwrap()
            .build()
            .await
            .unwrap();
        let timeout = Duration::from_millis(300);
        let dock = bounded(bus.builder(), timeout).build().await.unwrap();
        let asked = Instant::now();
        let outcome = activate(&dock, HUNG, Some("token-3")).await;
        assert!(outcome.is_err(), "{outcome:?}");
        let waited = asked.elapsed();
        assert!(
            waited >= timeout && waited < Duration::from_secs(10),
            "{waited:?}"
        );
    });
}
