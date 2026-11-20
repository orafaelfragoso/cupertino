//! Public-boundary tests for `cupertino-kit-shell` (W5.10):
//! - Transaction limits matching `cupertino_protocols::transaction`, design values, component table,
//!   entry registration, helper thread `host::spawn` hygiene, `run` lifecycle, and safe no-op
//!   requests when called outside a component thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cupertino_kit::core::{Application, Command, Environment, Window, WindowKey};
use cupertino_kit_shell::bus::{Message, OsdKind};
use cupertino_kit_shell::hosting::{Host, EXIT_VERSION};
use cupertino_kit_shell::{
    design, holds, host, outputs, run, session, transaction, Component, ComponentName, Entry,
    Error, Event, Program, Role, Surface,
};

static INIT_CALLED: AtomicBool = AtomicBool::new(false);

struct TestProgram;

impl Application for TestProgram {
    type Model = ();
    type Message = ();

    fn init() -> (Self::Model, Vec<Command<Self::Message>>) {
        INIT_CALLED.store(true, Ordering::SeqCst);
        let _env: Environment = cupertino_kit_shell::environment();
        assert!(!session::locked());
        assert!(outputs::all().is_empty());
        assert!(holds().is_empty());
        ((), Vec::new())
    }

    fn update(_: &mut Self::Model, _: Self::Message) -> Vec<Command<Self::Message>> {
        Vec::new()
    }

    fn view(_: &Self::Model) -> Vec<Window<Self::Message>> {
        Vec::new()
    }
}

impl Program for TestProgram {
    fn surface(_: &Self::Model, _: &WindowKey) -> Surface {
        Surface {
            role: Role::Window,
            persistent: None,
            holes: Vec::new(),
        }
    }

    fn event(_: Event) -> Option<Self::Message> {
        None
    }
}

impl Component for TestProgram {
    const NAME: ComponentName = ComponentName::OSD;
}

#[test]
fn design_limits_entries_and_outside_thread_safety() {
    assert_eq!(EXIT_VERSION, 76);
    assert_eq!(
        transaction::MAX_DEADLINE,
        Duration::from_millis(cupertino_protocols::transaction::MAX_DEADLINE_MS as u64)
    );
    // the menu bar's values are the theme data's
    let mb = design::menu_bar();
    let theme = cupertino_assets::theme::load().expect("theme data");
    let typography = theme.tokens.typography();
    assert_eq!(mb.height, theme.shapes.menu_bar_height);
    assert_eq!(
        mb.selection_radius,
        theme.shapes.menu_bar_selection_corner_radius
    );
    assert_eq!(mb.item.size, typography.menu_bar_item().size as f32);
    assert_eq!(
        (mb.item.weight.0, mb.app_name.weight.0),
        (
            typography.menu_bar_item().weight,
            typography.menu_bar_app_name().weight
        )
    );
    assert_eq!(
        mb.cap_height,
        typography.metrics.cap_height / typography.metrics.units_per_em
    );

    static ENTRY: Entry = Entry::of::<TestProgram>();
    assert_eq!(ENTRY.name(), ComponentName::OSD);

    // Every component in the canonical table round-trips through ComponentName::from_name.
    for name in [
        "menubar",
        "dock",
        "desktop",
        "windowmanager",
        "overview",
        "launcher",
        "notifications",
        "controlcenter",
        "osd",
        "agent",
        "dialogs",
    ] {
        let c = ComponentName::from_name(name).expect("known component");
        assert_eq!(c.as_str(), name);
    }
    assert!(ComponentName::from_name("unknown").is_none());

    // Calling request/query APIs outside a component thread does not panic.
    assert!(!session::locked());
    assert!(outputs::all().is_empty());
    assert!(holds().is_empty());
    session::activate_app(1, "org.example.App");
    cupertino_kit_shell::bus::publish(Message::ShowOsd {
        kind: OsdKind::Volume,
        level: 0.5,
    });

    // Host::connect fails cleanly when WAYLAND_SOCKET is unset.
    std::env::remove_var("WAYLAND_SOCKET");
    assert!(matches!(Host::connect(), Err(Error::Protocol(_))));
}

#[test]
fn standalone_program_run_and_spawn_work() {
    let env = cupertino_kit::app::environment().expect("toolkit env");
    run::<TestProgram>(env).expect("program runs");
    assert!(INIT_CALLED.load(Ordering::SeqCst));

    let mut cmd = std::process::Command::new("/bin/true");
    assert!(host::spawn(cmd));
    cmd = std::process::Command::new("/nonexistent/cupertino-test-binary");
    assert!(!host::spawn(cmd));
}
