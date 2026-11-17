//! Enumeration, parents, classification, GPUs and the preflight report, over a fixture sysfs tree.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use cupertino_devices::preflight;
use cupertino_devices::{
    classify_device, enumerate, gpus, primary_gpu, select_primary, select_render, DevT, Device,
    Error, ParseError, RenderChoice, Subsystem,
};

/// A sysfs tree in the build's scratch directory, one per test.
struct Sys {
    root: PathBuf,
}

const KEYBOARD: [&str; 5] = ["120013\n", "fffffffffffffffe\n", "0\n", "0\n", "0\n"];

impl Sys {
    fn new(name: &str) -> Self {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("sysfs-{name}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("devices")).unwrap();
        fs::create_dir_all(root.join("class/input")).unwrap();
        fs::create_dir_all(root.join("class/drm")).unwrap();
        let root = fs::canonicalize(&root).unwrap();
        Self { root }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    fn write(&self, relative: &str, text: &str) {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// `class/<subsystem>/<name>` pointing at the device directory `target` (relative to the root).
    fn class_link(&self, subsystem: &str, name: &str, target: &str) {
        fs::create_dir_all(self.path(target)).unwrap();
        symlink(
            self.path(target),
            self.path(&format!("class/{subsystem}/{name}")),
        )
        .unwrap();
    }

    fn drm_card(&self, pci: &str, card: u32, boot_vga: Option<&str>, render: Option<u32>) {
        let dev = format!("devices/pci0000:00/{pci}");
        self.write(
            &format!("{dev}/uevent"),
            "DRIVER=fixture\nPCI_SLOT_NAME=x\n",
        );
        if let Some(value) = boot_vga {
            self.write(&format!("{dev}/boot_vga"), value);
        }
        let node = format!("{dev}/drm/card{card}");
        self.write(
            &format!("{node}/uevent"),
            &format!("MAJOR=226\nMINOR={card}\nDEVNAME=dri/card{card}\nDEVTYPE=drm_minor\n"),
        );
        self.class_link("drm", &format!("card{card}"), &node);
        if let Some(number) = render {
            let node = format!("{dev}/drm/renderD{number}");
            self.write(
                &format!("{node}/uevent"),
                &format!("MAJOR=226\nMINOR={number}\nDEVNAME=dri/renderD{number}\n"),
            );
            self.class_link("drm", &format!("renderD{number}"), &node);
        }
    }

    /// An input device `inputN` under a platform parent, with its `eventN` node and capability files.
    fn input(&self, n: u32, name: &str, caps: [&str; 5], bustype: &str) {
        let parent = "devices/platform/i8042/serio0";
        self.write(&format!("{parent}/uevent"), "DRIVER=atkbd\n");
        let dir = format!("{parent}/input/input{n}");
        self.write(
            &format!("{dir}/uevent"),
            &format!("PRODUCT=11/1/1/ab\nNAME=\"{name}\"\n"),
        );
        for (file, text) in ["ev", "key", "rel", "abs"].iter().zip(caps) {
            self.write(&format!("{dir}/capabilities/{file}"), text);
        }
        self.write(&format!("{dir}/properties"), caps[4]);
        self.write(&format!("{dir}/id/bustype"), bustype);
        let node = format!("{dir}/event{n}");
        self.write(
            &format!("{node}/uevent"),
            &format!("MAJOR=13\nMINOR={}\nDEVNAME=input/event{n}\n", 64 + n),
        );
        self.class_link("input", &format!("event{n}"), &node);
    }

    fn first(&self, subsystem: Subsystem) -> Device {
        enumerate(&self.root, subsystem).unwrap().remove(0)
    }
}

#[test]
fn enumeration_lists_only_the_device_nodes_of_a_subsystem_lowest_first() {
    let drm = Sys::new("enumerate-drm");
    // created out of order; connectors and render nodes are not cards
    drm.drm_card("0000:00:03.0", 10, None, None);
    drm.drm_card("0000:00:02.0", 2, Some("1\n"), Some(128));
    drm.class_link(
        "drm",
        "card2-DP-1",
        "devices/pci0000:00/0000:00:02.0/drm/card2/card2-DP-1",
    );
    drm.class_link("drm", "version", "devices/other");

    let input = Sys::new("enumerate-input");
    input.input(3, "kbd", KEYBOARD, "0011\n");
    input.class_link(
        "input",
        "mouse0",
        "devices/platform/i8042/serio0/input/input3/mouse0",
    );
    input.class_link(
        "input",
        "input3",
        "devices/platform/i8042/serio0/input/input3",
    );
    input.class_link("input", "eventual", "devices/x");

    let empty = Sys::new("enumerate-empty");
    for (name, sys, subsystem, expected) in [
        ("drm", &drm, Subsystem::Drm, &["card2", "card10"][..]),
        ("input", &input, Subsystem::Input, &["event3"]),
        ("empty", &empty, Subsystem::Input, &[]),
    ] {
        let found = enumerate(&sys.root, subsystem).unwrap();
        let names: Vec<_> = found.iter().map(Device::sysname).collect();
        assert_eq!(names, expected, "{name}");
    }
    assert!(gpus(&empty.root).unwrap().is_empty());
    assert!(primary_gpu(&empty.root).unwrap().is_none());

    fs::remove_dir_all(empty.path("class/drm")).unwrap();
    match enumerate(&empty.root, Subsystem::Drm) {
        Err(Error::Io { path, .. }) => assert_eq!(path, empty.path("class/drm")),
        other => panic!("without a class directory: {other:?}"),
    }
}

#[test]
fn a_device_reads_its_uevent_properties_and_seat() {
    let sys = Sys::new("device");
    sys.drm_card("0000:00:02.0", 0, Some("1\n"), Some(128));
    let device = sys.first(Subsystem::Drm);
    assert_eq!(device.sysname(), "card0");
    assert_eq!(device.devnode(), Some(Path::new("/dev/dri/card0")));
    assert_eq!(device.devnum(), Some(DevT::new(226, 0)));
    assert_eq!(device.property("DEVTYPE"), Some("drm_minor"));
    assert_eq!(device.property("NOPE"), None);
    assert_eq!(device.properties().len(), 4);
    assert!(device.syspath().ends_with("drm/card0"));
    assert_eq!(device.seat(), "seat0");

    sys.write("devices/a/b/uevent", "ID_SEAT=seat1\n");
    let device = Device::from_syspath(&sys.path("devices/a/b")).unwrap();
    assert_eq!(
        (device.seat(), device.devnode(), device.devnum()),
        ("seat1", None, None)
    );
}

#[test]
fn a_device_without_a_readable_uevent_names_the_file() {
    let sys = Sys::new("bad-uevent");
    fs::create_dir_all(sys.path("devices/none")).unwrap();
    sys.write("devices/a/uevent", "MAJOR=13\nbroken line\n");
    sys.write("devices/b/uevent", "MAJOR=13\n");
    for (dir, expected) in [
        ("devices/none", None),
        (
            "devices/a",
            Some(ParseError::MalformedProperty("broken line".into())),
        ),
        ("devices/b", Some(ParseError::IncompleteDevnum)),
    ] {
        let uevent = sys.path(&format!("{dir}/uevent"));
        match (Device::from_syspath(&sys.path(dir)), expected) {
            (Err(Error::Io { path, .. }), None) => assert_eq!(path, uevent),
            (Err(Error::Parse { origin, source }), Some(expected)) => {
                assert!(origin.ends_with(&format!("{dir}/uevent")), "{origin}");
                assert_eq!(source, expected);
            }
            (other, _) => panic!("{dir}: {other:?}"),
        }
    }
}

#[test]
fn parents_skip_container_directories_and_stop_below_the_devices_root() {
    let sys = Sys::new("parents");
    sys.input(3, "kbd", KEYBOARD, "0011\n");
    let event = sys.first(Subsystem::Input);
    let parent = event.parent(&sys.root).unwrap().unwrap();
    // `input/` has no uevent and is skipped; the nearest device is input3
    assert_eq!(parent.sysname(), "input3");
    assert_eq!(parent.property("NAME"), Some("\"kbd\""));
    let chain: Vec<_> = event
        .parents(&sys.root)
        .unwrap()
        .iter()
        .map(|d| d.sysname().to_owned())
        .collect();
    assert_eq!(chain, ["input3", "serio0"]);
    let top = parent.parent(&sys.root).unwrap().unwrap();
    assert_eq!(top.sysname(), "serio0");
    assert!(top.parent(&sys.root).unwrap().is_none());
}

#[test]
fn a_node_is_classified_from_the_capabilities_above_it() {
    let keyboard = Sys::new("classify-keyboard");
    keyboard.input(3, "kbd", KEYBOARD, "0011\n");
    let class = classify_device(&keyboard.first(Subsystem::Input), &keyboard.root)
        .unwrap()
        .unwrap();
    assert!(class.keyboard && class.key);
    assert!(!class.mouse && !class.touchpad && !class.switch && !class.clickpad);

    let touchpad = Sys::new("classify-touchpad");
    // key: BTN_LEFT, BTN_TOOL_FINGER and BTN_TOUCH; abs: X, Y, MT slot, MT position; props: pointer, buttonpad
    let mut words = vec!["0000000000000000".to_owned(); 12];
    // bits 0x110 (BTN_LEFT), 0x145 (BTN_TOOL_FINGER) and 0x14a (BTN_TOUCH): word 4 holds 0x100..0x13f, word 5 0x140..
    words[12 - 1 - 4] = format!("{:016x}", 1u64 << (0x110 - 0x100));
    words[12 - 1 - 5] = format!("{:016x}", (1u64 << 5) | (1u64 << 10));
    let key = format!("{}\n", words.join(" "));
    // ABS_X, ABS_Y, ABS_MT_SLOT (0x2f), ABS_MT_POSITION_X/Y (0x35, 0x36)
    let abs = format!(
        "{:x}\n",
        (1u64 << 0) | (1 << 1) | (1 << 0x2f) | (1 << 0x35) | (1 << 0x36)
    );
    touchpad.input(5, "pad", ["b\n", &key, "0\n", &abs, "5\n"], "0018\n");
    let class = classify_device(&touchpad.first(Subsystem::Input), &touchpad.root)
        .unwrap()
        .unwrap();
    assert!(class.touchpad && class.clickpad, "{class:?}");
    assert!(!class.touchscreen && !class.mouse);

    let card = Sys::new("classify-none");
    card.drm_card("0000:00:02.0", 0, None, None);
    assert!(classify_device(&card.first(Subsystem::Drm), &card.root)
        .unwrap()
        .is_none());
}

#[test]
fn a_capability_file_that_is_not_hex_or_a_missing_bustype_fails_the_classification() {
    let bad = Sys::new("classify-bad");
    bad.input(
        3,
        "kbd",
        ["120013\n", "not-hex\n", "0\n", "0\n", "0\n"],
        "0011\n",
    );
    match classify_device(&bad.first(Subsystem::Input), &bad.root) {
        Err(Error::Parse { origin, source }) => {
            assert!(origin.ends_with("capabilities/key"));
            assert!(matches!(source, ParseError::InvalidNumber { .. }));
        }
        other => panic!("{other:?}"),
    }

    let no_bustype = Sys::new("classify-no-bustype");
    no_bustype.input(3, "kbd", KEYBOARD, "0011\n");
    fs::remove_file(no_bustype.path("devices/platform/i8042/serio0/input/input3/id/bustype"))
        .unwrap();
    assert!(matches!(
        classify_device(&no_bustype.first(Subsystem::Input), &no_bustype.root),
        Err(Error::Io { .. })
    ));
}

#[test]
fn the_primary_gpu_is_the_boot_gpu_or_else_the_first_card_and_each_gpu_knows_its_render_node() {
    let boot = Sys::new("primary-boot");
    boot.drm_card("0000:00:02.0", 0, Some("0\n"), Some(128));
    boot.drm_card("0000:00:03.0", 1, Some("1\n"), None);
    let all = gpus(&boot.root).unwrap();
    assert_eq!(
        all.iter().map(|g| g.boot_vga).collect::<Vec<_>>(),
        [false, true]
    );
    assert_eq!(
        all[0].render_node.as_deref(),
        Some(Path::new("/dev/dri/renderD128"))
    );
    assert_eq!(all[1].render_node, None);
    assert_eq!(
        primary_gpu(&boot.root).unwrap().unwrap().device.sysname(),
        "card1"
    );

    let first = Sys::new("primary-first");
    first.drm_card("0000:00:02.0", 0, None, Some(128));
    first.drm_card("0000:00:03.0", 1, Some("0\n"), None);
    let all = gpus(&first.root).unwrap();
    assert!(all.iter().all(|g| !g.boot_vga));
    assert_eq!(
        primary_gpu(&first.root).unwrap().unwrap().device.sysname(),
        "card0"
    );
    assert_eq!(select_primary(all).unwrap().device.sysname(), "card0");
    assert!(select_primary(Vec::new()).is_none());

    let bad = Sys::new("boot-vga-bad");
    bad.drm_card("0000:00:02.0", 0, Some("maybe\n"), None);
    match gpus(&bad.root) {
        Err(Error::Parse { origin, source }) => {
            assert!(origin.ends_with("0000:00:02.0/boot_vga"), "{origin}");
            assert_eq!(
                source,
                ParseError::InvalidNumber {
                    key: "boot_vga",
                    value: "maybe".into()
                }
            );
        }
        other => panic!("a boot_vga that is not 0 or 1: {other:?}"),
    }
}

#[test]
fn the_render_override_picks_a_card_by_either_node_and_otherwise_falls_back_to_the_primary() {
    let sys = Sys::new("render-select");
    sys.drm_card("0000:00:02.0", 0, Some("1\n"), Some(128));
    sys.drm_card("0000:00:03.0", 1, None, Some(129));
    let all = gpus(&sys.root).unwrap();
    for (name, chosen, card, how) in [
        (
            "a card node",
            Some("/dev/dri/card1"),
            "card1",
            RenderChoice::Named,
        ),
        (
            "a render node",
            Some("/dev/dri/renderD129"),
            "card1",
            RenderChoice::Named,
        ),
        ("unset", None, "card0", RenderChoice::Primary),
        ("empty", Some(""), "card0", RenderChoice::Primary),
        (
            "unmatched",
            Some("/dev/dri/card9"),
            "card0",
            RenderChoice::Unmatched(PathBuf::from("/dev/dri/card9")),
        ),
    ] {
        let (gpu, got) = select_render(all.clone(), chosen.map(Path::new)).unwrap();
        assert_eq!((gpu.device.sysname(), got), (card, how), "{name}");
    }
    assert!(select_render(Vec::new(), Some(Path::new("/dev/dri/card0"))).is_none());
    assert!(select_render(Vec::new(), None).is_none());
}

/// A GPU on `driver`, bound through `card/device/driver`.
fn gpu_with_driver(name: &str, driver: Option<&str>) -> Sys {
    let sys = Sys::new(name);
    sys.drm_card("0000:00:02.0", 0, Some("1\n"), None);
    if let Some(driver) = driver {
        let pci = sys.path("devices/pci0000:00/0000:00:02.0");
        let target = sys.path(&format!("bus/pci/drivers/{driver}"));
        fs::create_dir_all(&target).unwrap();
        symlink(&target, pci.join("driver")).unwrap();
        symlink(&pci, pci.join("drm/card0/device")).unwrap();
    }
    sys
}

#[test]
fn preflight_names_the_primary_gpus_driver_and_warns_when_none_is_bound() {
    assert!(preflight::run(&Sys::new("preflight-none").root)
        .unwrap()
        .is_none());

    let sys = gpu_with_driver("preflight-i915", Some("i915"));
    let report = preflight::run(&sys.root).unwrap().unwrap();
    assert_eq!(report.driver.as_deref(), Some("i915"));
    assert_eq!(report.notes, ["primary GPU card0 uses the i915 driver"]);
    assert!(report.problems.is_empty() && report.warnings.is_empty());
    assert!(report.is_ok());
    assert_eq!(
        report.to_string(),
        "note: primary GPU card0 uses the i915 driver\n"
    );

    let sys = gpu_with_driver("preflight-unbound", None);
    let report = preflight::run(&sys.root).unwrap().unwrap();
    assert_eq!(report.driver, None);
    assert_eq!(report.warnings.len(), 1);
    assert!(report.warnings[0].what.contains("no driver bound"));
    assert!(report.is_ok(), "a warning is not a problem");
    let text = report.to_string();
    assert!(
        text.starts_with("warning: primary GPU card0 has no driver bound ("),
        "{text}"
    );
}

#[test]
fn preflight_finds_nvidia_without_drm_modesetting_a_problem_with_its_fix() {
    let sys = gpu_with_driver("preflight-nvidia-none", Some("nvidia"));
    sys.write("module/nvidia/version", "570.86.16\n");
    let report = preflight::run(&sys.root).unwrap().unwrap();
    assert_eq!(report.driver.as_deref(), Some("nvidia"));
    assert!(report.notes.contains(&"nvidia driver 570.86.16".to_owned()));
    assert!(!report.is_ok());
    assert!(report.problems[0]
        .what
        .contains("nvidia_drm kernel module is not loaded"));
    assert!(report.problems[0]
        .fix
        .contains("modprobe nvidia_drm modeset=1"));
    assert!(report
        .to_string()
        .contains("problem: the nvidia_drm kernel module"));

    let sys = gpu_with_driver("preflight-nvidia-off", Some("nvidia"));
    sys.write("module/nvidia_drm/parameters/modeset", "N\n");
    let report = preflight::run(&sys.root).unwrap().unwrap();
    assert_eq!(report.problems.len(), 1);
    assert!(report.problems[0].what.contains("modesetting is off"));
    assert!(report.problems[0].fix.contains("nvidia-drm.modeset=1"));

    for (index, (value, shown)) in [
        ("1\n", "on"),
        ("N\n", "off"),
        ("0\n", "off"),
        ("Y\n", "on"),
        ("weird\n", "weird"),
    ]
    .into_iter()
    .enumerate()
    {
        let sys = gpu_with_driver(&format!("preflight-fbdev-{index}"), Some("nvidia"));
        sys.write("module/nvidia_drm/parameters/modeset", "Y\n");
        sys.write("module/nvidia_drm/parameters/fbdev", value);
        let report = preflight::run(&sys.root).unwrap().unwrap();
        assert!(report.problems.is_empty(), "{value:?}: {report}");
        assert!(
            report
                .notes
                .contains(&format!("nvidia-drm fbdev is {shown}")),
            "{value:?}: {:?}",
            report.notes
        );
    }
}
