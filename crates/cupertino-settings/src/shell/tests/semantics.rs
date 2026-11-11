//! What a screen reader announces of each pane and each sheet: the role, name, value and states of their nodes, read
//! from the accessibility tree the window builds headless. The panes that show only with a network or Bluetooth
//! service, or with a touchpad, open against fake daemons on a private bus and a fake sysfs.

use cupertino_filepicker::Mode;
use cupertino_kit::core::AccessibleValue;
use cupertino_ui_test::{semantic_outline, unannounced, whole_tree};

use super::*;
use crate::clipboard::App;
use crate::hardware::Sources;
use crate::sheet::Prompt;

// the fake daemons live with the integration tests, outside `src`
mod fakes {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fakes/semantics.rs"
    ));
}
use fakes::{FakeBluetooth, FakeNetwork};

fn tree(driver: &TestDriver<SettingsApp>, window: WindowId) -> AccessibilityUpdate {
    let snapshot = driver.runtime().snapshot(window).unwrap();
    whole_tree(window, snapshot.accessibility, snapshot.focus)
}

/// The nodes a screen reader would read a secret of: a password field, or a text field named as one, whose value is
/// text.
fn secrets_read(tree: &AccessibilityUpdate) -> Vec<String> {
    tree.nodes
        .iter()
        .filter(|(_, n)| {
            let named = n.name.as_deref().is_some_and(|name| {
                let name = name.to_lowercase();
                name.contains("password") || name.contains("passphrase")
            });
            (n.role == Role::PasswordInput || (n.role == Role::TextInput && named))
                && matches!(n.value, Some(AccessibleValue::Text { .. }))
        })
        .map(|(_, n)| cupertino_ui_test::describe(n))
        .collect()
}

/// Every line of `expected` is read by a node of the window, no node lacks a name and no secret is read out.
fn announces(driver: &TestDriver<SettingsApp>, window: WindowId, what: &str, expected: &[&str]) {
    let tree = tree(driver, window);
    let outline = semantic_outline(&tree).join("\n");
    assert_eq!(
        unannounced(&tree, expected),
        Vec::<&str>::new(),
        "{what} reads:\n{outline}"
    );
    assert_eq!(
        kit_gaps(&tree),
        Vec::<String>::new(),
        "{what} reads:\n{outline}"
    );
    assert_eq!(
        secrets_read(&tree),
        Vec::<String>::new(),
        "{what} reads:\n{outline}"
    );
}

/// What every pane reads besides its content: its sidebar row selected, the history buttons with no history, the
/// search field and the sidebar's splitter.
fn chrome(title: &str) -> [String; 5] {
    [
        format!("ListItem {title:?} focusable selected"),
        "Button \"Back\" disabled".to_owned(),
        "Button \"Forward\" disabled".to_owned(),
        "TextInput \"Search\" = \"\" focusable".to_owned(),
        "Splitter \"Sidebar\" = 232 of 0..738 focusable".to_owned(),
    ]
}

fn pane_announces(pane: PaneId, title: &str, content: &[&str], services: Option<Services>) {
    let dir = tempfile::tempdir().unwrap();
    let (mut driver, window) = match services {
        Some(services) => start_on(dir.path(), pane, None, services),
        None => start(dir.path(), pane),
    };
    settle(&mut driver, window);
    let chrome = chrome(title);
    let mut expected: Vec<&str> = chrome.iter().map(String::as_str).collect();
    expected.extend(content);
    announces(&driver, window, &format!("{pane:?}"), &expected);
}

#[test]
fn each_pane_without_a_service_reads_its_controls_by_role_name_value_and_state() {
    let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    let panes: [(PaneId, &str, &[&str]); 14] = [
        (
            PaneId::Battery,
            "Energy Saver",
            &[
                "StaticText \"System Sleep\"",
                "PopUpButton \"Put the computer to sleep after\" = \"Never\" focusable collapsed",
            ],
        ),
        (
            PaneId::Accessibility,
            "Accessibility",
            &[
                "Button \"Display\" focusable",
                "Button \"Pointer\" focusable",
                "Button \"Keyboard\" focusable",
            ],
        ),
        (
            PaneId::Appearance,
            "Appearance",
            &[
                "RadioGroup \"Appearance\"",
                "RadioButton \"Light\" on focusable",
                "RadioButton \"Dark\" off",
            ],
        ),
        (
            PaneId::ControlPanel,
            "Control Panel",
            &[
                "StaticText \"Clock Options\"",
                "PopUpButton \"Show date\" = \"Never\" focusable collapsed",
                "Switch \"Show the day of the week\" off focusable",
                "Switch \"Use a 24-hour clock\" off focusable",
                "Switch \"Show AM/PM\" on focusable",
                "Switch \"Flash the time separators\" off focusable",
                "Switch \"Display the time with seconds\" off focusable",
            ],
        ),
        (
            PaneId::DesktopDock,
            "Desktop & Dock",
            &[
                "StaticText \"Dock\"",
                "Slider \"Size\" = 0.42857142857142855 of 0..1 focusable",
                "Slider \"Magnification\" = 0 of 0..1 focusable",
                "RadioGroup \"Position on screen\"",
                "RadioButton \"Bottom\" on focusable",
                "RadioButton \"Left\" off",
                "RadioButton \"Right\" off",
                "PopUpButton \"Minimize windows using\" = \"Warp\" focusable collapsed",
                "Switch \"Automatically hide and show the Dock\" off focusable",
                "Switch \"Show suggested and recent apps in Dock\" on focusable",
                "Button \"Hot Corners\" focusable",
            ],
        ),
        (
            PaneId::Displays,
            "Displays",
            // the section is the display's name when the test runs inside a Wayland session
            &["StaticText \"Resolution\""],
        ),
        (
            PaneId::Wallpaper,
            "Wallpaper",
            &[
                "StaticText \"Current\"",
                "StaticText \"Pictures\"",
                "StaticText \"Add pictures\"",
                "Button \"Add Folder…\" focusable",
                "Button \"Add Photo…\" focusable",
            ],
        ),
        // the level and the mute depend on the machine's mixer: only the section is the same everywhere
        (PaneId::Sound, "Sound", &["StaticText \"Output\""]),
        (
            PaneId::Notifications,
            "Notifications",
            &[
                "StaticText \"Lock Screen\"",
                "Switch \"Show on lock screen\" on focusable",
                "PopUpButton \"Show previews\" = \"When Unlocked\" focusable collapsed",
                "StaticText \"Without previews, a notification shows only its app and the word Notification.\"",
                "StaticText \"Application Notifications\"",
                "StaticText \"No applications\"",
                "StaticText \"Apps appear here once they send a notification. Default follows the settings above.\"",
            ],
        ),
        (
            PaneId::Focus,
            "Focus",
            &[
                "StaticText \"Do Not Disturb\"",
                "StaticText \"The notification centre is not running\"",
                "Switch \"Do Not Disturb\" off disabled",
            ],
        ),
        (
            PaneId::LockScreen,
            "Lock Screen",
            &[
                "PopUpButton \"Turn display off when inactive\" = \"10 minutes\" focusable collapsed",
                "PopUpButton \"Require password after the display is turned off\" = \"5 minutes\" focusable collapsed",
            ],
        ),
        (
            PaneId::Privacy,
            "Privacy & Security",
            &[
                "Button \"Clipboard Access\" focusable",
                "Button \"Screen Sharing\" focusable",
                "Button \"Desktop Control\" focusable",
                "Button \"Screen Capture\" focusable",
            ],
        ),
        (
            PaneId::Keyboard,
            "Keyboard",
            &[
                "StaticText \"Key Repeat\"",
                "Slider \"Key repeat rate\" = 0.8571428571428571 of 0..1 focusable",
                "Slider \"Delay until repeat\" = 0.6 of 0..1 focusable",
                "StaticText \"Modifier Keys\"",
                "PopUpButton \"Command key\" = \"Super is the command key\" focusable collapsed",
                "Switch \"Delay caps lock\" on focusable",
                "Switch \"Caps lock switches input sources\" off focusable",
                "PopUpButton \"Press fn key to\" = \"Change Input Source\" focusable collapsed",
                "Switch \"Show accents for held keys\" on focusable",
            ],
        ),
        (
            PaneId::Mouse,
            "Mouse",
            &[
                "Slider \"Tracking speed\" = 0.3333333333333333 of 0..1 focusable",
                "Switch \"Pointer acceleration\" on focusable",
                "Switch \"Natural scrolling\" off focusable",
            ],
        ),
    ];
    for (pane, title, content) in panes {
        pane_announces(pane, title, content, None);
    }
}

/// What a sub-page reads besides its content: its pane's sidebar row still selected, its own title in the toolbar,
/// and a history to go back through.
fn sub_page_chrome(pane: &str, title: &str) -> [String; 6] {
    [
        format!("ListItem {pane:?} focusable selected"),
        format!("StaticText {title:?}"),
        "Button \"Back\" focusable".to_owned(),
        "Button \"Forward\" disabled".to_owned(),
        "TextInput \"Search\" = \"\" focusable".to_owned(),
        "Splitter \"Sidebar\" = 232 of 0..738 focusable".to_owned(),
    ]
}

#[test]
fn each_sub_page_reads_its_controls_by_role_name_value_and_state() {
    let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    let pages: [(PaneId, &str, SubPage, &[&str]); 7] = [
        (
            PaneId::Accessibility,
            "Accessibility",
            SubPage::Display,
            &[
                "Switch \"Reduce motion\" off focusable",
                "Switch \"Reduce transparency\" off focusable",
                "Switch \"Increase contrast\" off focusable",
                "Switch \"Differentiate without colour\" off focusable",
                "Switch \"Invert colours\" off focusable",
                "Slider \"Display contrast\" = 0 of 0..1 focusable",
                "PopUpButton \"Colour filter\" = \"None\" focusable collapsed",
            ],
        ),
        (
            PaneId::Accessibility,
            "Accessibility",
            SubPage::Pointer,
            &[
                "Slider \"Pointer size\" = 0 of 0..1 focusable",
                "Switch \"Shake mouse pointer to locate\" on focusable",
            ],
        ),
        (
            PaneId::Accessibility,
            "Accessibility",
            SubPage::Keyboard,
            &[
                "Switch \"Sticky keys\" off focusable",
                "Switch \"Press Shift five times to toggle sticky keys\" off focusable",
                "Switch \"Slow keys\" off focusable",
                "Switch \"Mouse keys\" off focusable",
                "Switch \"Press Alt five times to toggle mouse keys\" off focusable",
            ],
        ),
        (
            PaneId::DesktopDock,
            "Desktop & Dock",
            SubPage::HotCorners,
            &[
                "PopUpButton \"Top Left\" = \"None\" focusable collapsed",
                "PopUpButton \"Top Right\" = \"None\" focusable collapsed",
                "PopUpButton \"Bottom Left\" = \"None\" focusable collapsed",
                "PopUpButton \"Bottom Right\" = \"None\" focusable collapsed",
            ],
        ),
        (
            PaneId::Privacy,
            "Privacy & Security",
            SubPage::ClipboardAccess,
            &[
                "StaticText \"No applications\"",
                "Button \"Add App…\" focusable",
                "Button \"Browse…\" focusable",
                "StaticText \"These apps can read and change what you copy, at any time.\"",
            ],
        ),
        (
            PaneId::Privacy,
            "Privacy & Security",
            SubPage::DesktopControl,
            &[
                "StaticText \"No applications\"",
                "StaticText \"Apps show here after they ask to control the desktop.\"",
                "StaticText \"Apps that are on can see your windows and their controls and use the pointer and keyboard. Turning one off stops it at once; Forget one to be asked next time.\"",
            ],
        ),
        (
            PaneId::Privacy,
            "Privacy & Security",
            SubPage::ScreenCapture,
            &[
                "StaticText \"No applications\"",
                "StaticText \"Apps show here after they ask to capture the screen.\"",
                "StaticText \"Apps that are on can take pictures of your screen. Turning one off stops it at once; Forget one to be asked next time.\"",
            ],
        ),
    ];
    for (pane, pane_title, sub, content) in pages {
        let dir = tempfile::tempdir().unwrap();
        let (mut driver, window) = start(dir.path(), pane);
        click(&mut driver, window, &format!("open-{sub:?}"));
        let chrome = sub_page_chrome(pane_title, sub.title());
        let mut expected: Vec<&str> = chrome.iter().map(String::as_str).collect();
        expected.extend(content);
        announces(&driver, window, &format!("{sub:?}"), &expected);
    }
}

#[test]
fn a_committed_switch_reads_its_new_state() {
    let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let (mut driver, window) = start(dir.path(), PaneId::Mouse);
    let switch = node(&driver, window, Role::Switch, "Natural scrolling");
    click_node(&mut driver, window, switch);
    announces(
        &driver,
        window,
        "Mouse after the click",
        &["Switch \"Natural scrolling\" on focusable"],
    );
}

// ---- the panes of a service or a device ----------------------------------------------------------------------

#[test]
fn the_network_bluetooth_and_trackpad_panes_read_their_controls_by_role_name_value_and_state() {
    let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    let bus = cupertino_test_bus::PrivateBus::start()
        .expect("dbus-daemon is needed for the panes of a service");
    let _network = bus
        .blocking_builder()
        .serve_at("/org/cupertino/Network1", FakeNetwork)
        .unwrap()
        .name("org.cupertino.Network1")
        .unwrap()
        .build()
        .unwrap();
    let _bluetooth = bus
        .blocking_builder()
        .serve_at("/org/cupertino/Bluetooth1", FakeBluetooth)
        .unwrap()
        .name("org.cupertino.Bluetooth1")
        .unwrap()
        .build()
        .unwrap();
    // an input event node udev calls a touchpad
    let hardware = tempfile::tempdir().unwrap();
    let event = hardware.path().join("sys/class/input/event3");
    std::fs::create_dir_all(&event).unwrap();
    std::fs::write(event.join("dev"), "13:67\n").unwrap();
    std::fs::create_dir_all(hardware.path().join("udev")).unwrap();
    std::fs::write(
        hardware.path().join("udev/c13:67"),
        "E:ID_INPUT_TOUCHPAD=1\n",
    )
    .unwrap();
    let services = || {
        Services::on(Bus::at(bus.connect()), Bus::at(bus.connect())).with_hardware(Sources {
            sysfs: hardware.path().join("sys"),
            udev_data: hardware.path().join("udev"),
        })
    };
    let panes: [(PaneId, &str, &[&str]); 4] = [
        (
            PaneId::Wifi,
            "Wi-Fi",
            &[
                "Switch \"Wi-Fi\" on focusable",
                "StaticText \"Known Networks\"",
                "StaticText \"home (secured)\"",
                "Button \"Disconnect\" focusable",
                "StaticText \"Other Networks\"",
                "StaticText \"cafe\"",
                "Button \"Join\" focusable",
            ],
        ),
        (
            PaneId::Bluetooth,
            "Bluetooth",
            &[
                "Switch \"Bluetooth\" on focusable",
                "StaticText \"My Devices\"",
                "StaticText \"Headphones\"",
                "Button \"Disconnect\" focusable",
                "Button \"Forget…\" focusable",
                "StaticText \"Nearby Devices\"",
                "StaticText \"Keyboard\"",
                "Button \"Pair\" focusable",
            ],
        ),
        (
            PaneId::Network,
            "Network",
            &[
                "StaticText \"Status: connected\"",
                "StaticText \"eth0 (ethernet)\"",
                "StaticText \"connected · 10.0.0.3\"",
                "StaticText \"wlan0 (wifi)\"",
                "StaticText \"connected · home · 10.0.0.2\"",
            ],
        ),
        (
            PaneId::Trackpad,
            "Trackpad",
            &[
                "Slider \"Tracking speed\" = 0.3333333333333333 of 0..1 focusable",
                "Switch \"Natural scrolling\" on focusable",
            ],
        ),
    ];
    for (pane, title, content) in panes {
        pane_announces(pane, title, content, Some(services()));
    }
}

// ---- sheets ---------------------------------------------------------------------------------------------------

/// The whole tree of the window with `sheet` up: the sheet alone, a modal dialog over a content no reader reaches.
fn sheet_reads(dir: &Path, sheet: Sheet, what: &str) -> Vec<String> {
    let (driver, window) = start_with(dir, PaneId::Wifi, Some(sheet));
    let tree = tree(&driver, window);
    assert_eq!(kit_gaps(&tree), Vec::<String>::new(), "{what}");
    assert_eq!(secrets_read(&tree), Vec::<String>::new(), "{what}");
    semantic_outline(&tree)
}

fn lines(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|l| (*l).to_owned()).collect()
}

#[test]
fn each_sheet_is_a_modal_dialog_that_reads_its_title_message_field_and_buttons() {
    let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let app = |name: &str, program: &str| App {
        name: name.into(),
        icon: None,
        program: Some(program.into()),
        reason: None,
    };
    let sheets: [(Sheet, &[&str]); 8] = [
        (
            Sheet::passphrase("home", "Home"),
            &[
                "Window",
                "  Dialog \"Enter the password for “Home”\" modal",
                "    StaticText \"Enter the password for “Home”\"",
                "    StaticText \"The network needs a password to join.\"",
                "    PasswordInput \"Password\" = [protected] focusable focused",
                "    Button \"Cancel\" focusable",
                "    Button \"Join\" focusable",
            ],
        ),
        (
            Sheet::forget("kb", "Keyboard"),
            &[
                "Window",
                "  AlertDialog \"Forget “Keyboard”?\" modal",
                "    StaticText \"Forget “Keyboard”?\"",
                "    StaticText \"The device will need to be paired again to be used.\"",
                "    Button \"Forget Device\" focusable",
                "    Button \"Cancel\" focusable focused",
            ],
        ),
        (
            Sheet::apps(vec![
                app("Clip Daemon", "/usr/bin/clipd"),
                app("Notes", "/usr/bin/notes"),
            ]),
            &[
                "Window",
                "  Dialog \"Allow an Application to Use the Clipboard\" modal",
                "    StaticText \"Allow an Application to Use the Clipboard\"",
                "    StaticText \"It can read and change what you copy at any time.\"",
                "    Button \"Clip Daemon\" focusable focused",
                "    Button \"Notes\" focusable",
                "    Button \"Cancel\" focusable",
                "    Button \"Allow\" focusable",
            ],
        ),
        (
            Sheet::unallow(Path::new("/usr/bin/clipd"), "clipd"),
            &[
                "Window",
                "  AlertDialog \"Remove “clipd” from Clipboard Access?\" modal",
                "    StaticText \"Remove “clipd” from Clipboard Access?\"",
                "    StaticText \"If it's open, it's disconnected and has to be opened again.\"",
                "    Button \"Cancel\" focusable focused",
                "    Button \"Remove\" focusable",
            ],
        ),
        (
            Sheet::resolution("1:1:Mouse", "Mouse"),
            &[
                "Window",
                "  Dialog \"Resolution of “Mouse”\" modal",
                "    StaticText \"Resolution of “Mouse”\"",
                "    StaticText \"Type the resolution in dots per inch, from 50 to 32000.\"",
                "    TextInput \"Resolution of “Mouse”\" = \"\" focusable focused",
                "    Button \"Cancel\" focusable",
                "    Button \"Set\" focusable",
            ],
        ),
        (
            Sheet::pairing(1, "Headphones", Prompt::Confirm { passkey: Some(123456) }),
            &[
                "Window",
                "  Dialog \"Pair with “Headphones”?\" modal",
                "    StaticText \"Pair with “Headphones”?\"",
                "    StaticText \"Check that the device shows 123456.\"",
                "    Button \"Cancel\" focusable focused",
                "    Button \"Pair\" focusable",
            ],
        ),
        (
            Sheet::pairing(2, "Headphones", Prompt::Request),
            &[
                "Window",
                "  Dialog \"Enter the code shown on “Headphones”\" modal",
                "    StaticText \"Enter the code shown on “Headphones”\"",
                "    StaticText \"Type the six-digit code.\"",
                "    TextInput \"Enter the code shown on “Headphones”\" = \"\" focusable focused",
                "    Button \"Cancel\" focusable",
                "    Button \"Pair\" focusable",
            ],
        ),
        (
            Sheet::pairing(
                3,
                "Headphones",
                Prompt::Display {
                    passkey: 123456,
                    entered: 2,
                },
            ),
            &[
                "Window",
                "  Dialog \"Pairing with “Headphones”\" modal",
                "    StaticText \"Pairing with “Headphones”\"",
                "    StaticText \"Type 123456 on the device, then press Return on it. 2 of 6 digits typed.\"",
                "    Button \"Cancel\" focusable focused",
                "    Button \"OK\" focusable",
            ],
        ),
    ];
    for (sheet, expected) in sheets {
        assert_eq!(sheet_reads(dir.path(), sheet, expected[1]), lines(expected));
    }

    // the choosers list the folder's entries their mode shows, under the folder's path
    let pictures = tempfile::tempdir().unwrap();
    std::fs::create_dir(pictures.path().join("Photos")).unwrap();
    std::fs::write(pictures.path().join("me.png"), "x").unwrap();
    std::fs::write(pictures.path().join("notes.txt"), "x").unwrap();
    let folder = format!("    StaticText {:?}", pictures.path().display().to_string());
    let choosers: [(Mode, Vec<&str>); 2] = [
        (
            Mode::Folder,
            vec![
                "Window",
                "  Dialog \"Add a Folder\" modal",
                "    StaticText \"Add a Folder\"",
                &folder,
                "    Button \"Enclosing Folder\" focusable focused",
                "    Button \"Photos\" focusable",
                "    Button \"Cancel\" focusable",
                "    Button \"Choose\" focusable",
            ],
        ),
        (
            Mode::Photo,
            vec![
                "Window",
                "  Dialog \"Add a Photo\" modal",
                "    StaticText \"Add a Photo\"",
                &folder,
                "    Button \"Enclosing Folder\" focusable focused",
                "    Button \"Photos\" focusable",
                "    Button \"me.png\" focusable",
                "    Button \"Cancel\" focusable",
                "    Button \"Add\" focusable",
            ],
        ),
    ];
    for (mode, expected) in choosers {
        let sheet = Sheet::picker(pictures.path(), mode);
        assert_eq!(
            sheet_reads(dir.path(), sheet, expected[1]),
            lines(&expected)
        );
    }
}
