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
}

#[test]
fn enumeration_lists_only_card_nodes_lowest_first() {
    let sys = Sys::new("enumerate-drm");
    // created out of order; connectors and render nodes are not cards
    sys.drm_card("0000:00:03.0", 10, None, None);
    sys.drm_card("0000:00:02.0", 2, Some("1\n"), Some(128));
    sys.class_link(
        "drm",
        "card2-DP-1",
        "devices/pci0000:00/0000:00:02.0/drm/card2/card2-DP-1",
    );
    sys.class_link("drm", "version", "devices/other");
    let cards = enumerate(&sys.root, Subsystem::Drm).unwrap();
    let names: Vec<_> = cards.iter().map(Device::sysname).collect();
    assert_eq!(names, ["card2", "card10"]);
}

#[test]
fn enumeration_of_input_lists_only_event_nodes() {
    let sys = Sys::new("enumerate-input");
    sys.input(
        3,
        "kbd",
        ["120013\n", "fffffffffffffffe\n", "0\n", "0\n", "0\n"],
        "0011\n",
    );
    sys.class_link(
        "input",
        "mouse0",
        "devices/platform/i8042/serio0/input/input3/mouse0",
    );
    sys.class_link(
        "input",
        "input3",
        "devices/platform/i8042/serio0/input/input3",
    );
    sys.class_link("input", "eventual", "devices/x");
    let found = enumerate(&sys.root, Subsystem::Input).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].sysname(), "event3");
}

#[test]
fn a_subsystem_has_a_class_directory_name() {
    assert_eq!(Subsystem::Input.as_str(), "input");
    assert_eq!(Subsystem::Drm.as_str(), "drm");
}

#[test]
fn enumerating_without_a_class_directory_names_the_path() {
    let sys = Sys::new("no-class");
    fs::remove_dir_all(sys.path("class/drm")).unwrap();
    match enumerate(&sys.root, Subsystem::Drm) {
        Err(Error::Io { path, .. }) => assert_eq!(path, sys.path("class/drm")),
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_empty_class_directory_lists_nothing() {
    let sys = Sys::new("empty");
    assert!(enumerate(&sys.root, Subsystem::Input).unwrap().is_empty());
    assert!(gpus(&sys.root).unwrap().is_empty());
    assert!(primary_gpu(&sys.root).unwrap().is_none());
}

#[test]
fn a_device_reads_its_uevent_properties() {
    let sys = Sys::new("device");
    sys.drm_card("0000:00:02.0", 0, Some("1\n"), Some(128));
    let device = &enumerate(&sys.root, Subsystem::Drm).unwrap()[0];
    assert_eq!(device.sysname(), "card0");
    assert_eq!(device.devnode(), Some(Path::new("/dev/dri/card0")));
    assert_eq!(device.devnum(), Some(DevT::new(226, 0)));
    assert_eq!(device.property("DEVTYPE"), Some("drm_minor"));
    assert_eq!(device.property("NOPE"), None);
    assert_eq!(device.properties().len(), 4);
    assert!(device.syspath().ends_with("drm/card0"));
    assert_eq!(device.seat(), "seat0");
}

#[test]
fn a_device_with_an_id_seat_belongs_to_that_seat() {
    let sys = Sys::new("seat");
    sys.write("devices/a/b/uevent", "ID_SEAT=seat1\n");
    let device = Device::from_syspath(&sys.path("devices/a/b")).unwrap();
    assert_eq!(device.seat(), "seat1");
    assert_eq!(device.devnode(), None);
    assert_eq!(device.devnum(), None);
}

#[test]
fn a_device_without_a_uevent_file_is_an_io_error() {
    let sys = Sys::new("no-uevent");
    fs::create_dir_all(sys.path("devices/a")).unwrap();
    match Device::from_syspath(&sys.path("devices/a")) {
        Err(Error::Io { path, .. }) => assert_eq!(path, sys.path("devices/a/uevent")),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_malformed_uevent_is_a_parse_error_naming_the_file() {
    let sys = Sys::new("bad-uevent");
    sys.write("devices/a/uevent", "MAJOR=13\nbroken line\n");
    match Device::from_syspath(&sys.path("devices/a")) {
        Err(Error::Parse { origin, source }) => {
            assert!(origin.ends_with("devices/a/uevent"), "{origin}");
            assert_eq!(source, ParseError::MalformedProperty("broken line".into()));
        }
        other => panic!("{other:?}"),
    }
    sys.write("devices/b/uevent", "MAJOR=13\n");
    assert!(matches!(
        Device::from_syspath(&sys.path("devices/b")),
        Err(Error::Parse {
            source: ParseError::IncompleteDevnum,
            ..
        })
    ));
}

#[test]
fn parents_skip_container_directories_and_stop_below_the_devices_root() {
    let sys = Sys::new("parents");
    sys.input(
        3,
        "kbd",
        ["120013\n", "fffffffffffffffe\n", "0\n", "0\n", "0\n"],
        "0011\n",
    );
    let event = &enumerate(&sys.root, Subsystem::Input).unwrap()[0];
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
fn a_keyboard_node_is_classified_from_the_capabilities_above_it() {
    let sys = Sys::new("classify-keyboard");
    sys.input(
        3,
        "kbd",
        ["120013\n", "fffffffffffffffe\n", "0\n", "0\n", "0\n"],
        "0011\n",
    );
    let event = &enumerate(&sys.root, Subsystem::Input).unwrap()[0];
    let class = classify_device(event, &sys.root).unwrap().unwrap();
    assert!(class.keyboard && class.key);
    assert!(!class.mouse && !class.touchpad && !class.switch && !class.clickpad);
}

#[test]
fn a_touchpad_node_with_a_button_pad_is_a_clickpad() {
    let sys = Sys::new("classify-touchpad");
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
    sys.input(5, "pad", ["b\n", &key, "0\n", &abs, "5\n"], "0018\n");
    let event = &enumerate(&sys.root, Subsystem::Input).unwrap()[0];
    let class = classify_device(event, &sys.root).unwrap().unwrap();
    assert!(class.touchpad, "{class:?}");
    assert!(class.clickpad);
    assert!(!class.touchscreen && !class.mouse);
}

#[test]
fn a_node_with_no_capabilities_above_it_is_not_an_input_device() {
    let sys = Sys::new("classify-none");
    sys.drm_card("0000:00:02.0", 0, None, None);
    let card = &enumerate(&sys.root, Subsystem::Drm).unwrap()[0];
    assert!(classify_device(card, &sys.root).unwrap().is_none());
}

#[test]
fn a_capability_file_that_is_not_hex_is_a_parse_error() {
    let sys = Sys::new("classify-bad");
    sys.input(
        3,
        "kbd",
        ["120013\n", "not-hex\n", "0\n", "0\n", "0\n"],
        "0011\n",
    );
    let event = &enumerate(&sys.root, Subsystem::Input).unwrap()[0];
    match classify_device(event, &sys.root) {
        Err(Error::Parse { origin, source }) => {
            assert!(origin.ends_with("capabilities/key"));
            assert!(matches!(source, ParseError::InvalidNumber { .. }));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_missing_bustype_is_an_io_error() {
    let sys = Sys::new("classify-no-bustype");
    sys.input(
        3,
        "kbd",
        ["120013\n", "fffffffffffffffe\n", "0\n", "0\n", "0\n"],
        "0011\n",
    );
    fs::remove_file(sys.path("devices/platform/i8042/serio0/input/input3/id/bustype")).unwrap();
    let event = &enumerate(&sys.root, Subsystem::Input).unwrap()[0];
    assert!(matches!(
        classify_device(event, &sys.root),
        Err(Error::Io { .. })
    ));
}

#[test]
fn the_boot_gpu_is_the_primary_one_wherever_it_sits() {
    let sys = Sys::new("primary-boot");
    sys.drm_card("0000:00:02.0", 0, Some("0\n"), Some(128));
    sys.drm_card("0000:00:03.0", 1, Some("1\n"), None);
    let all = gpus(&sys.root).unwrap();
    assert_eq!(all.len(), 2);
    assert!(!all[0].boot_vga && all[1].boot_vga);
    assert_eq!(
        primary_gpu(&sys.root).unwrap().unwrap().device.sysname(),
        "card1"
    );
}

#[test]
fn without_a_boot_gpu_the_first_card_is_primary() {
    let sys = Sys::new("primary-first");
    sys.drm_card("0000:00:02.0", 0, None, Some(128));
    sys.drm_card("0000:00:03.0", 1, Some("0\n"), None);
    let all = gpus(&sys.root).unwrap();
    assert!(all.iter().all(|g| !g.boot_vga));
    assert_eq!(
        primary_gpu(&sys.root).unwrap().unwrap().device.sysname(),
        "card0"
    );
    assert_eq!(
        select_primary(all.clone()).unwrap().device.sysname(),
        "card0"
    );
    assert!(select_primary(Vec::new()).is_none());
}

#[test]
fn a_gpu_knows_its_render_node() {
    let sys = Sys::new("render-node");
    sys.drm_card("0000:00:02.0", 0, Some("1\n"), Some(128));
    sys.drm_card("0000:00:03.0", 1, None, None);
    let all = gpus(&sys.root).unwrap();
    assert_eq!(
        all[0].render_node.as_deref(),
        Some(Path::new("/dev/dri/renderD128"))
    );
    assert_eq!(all[1].render_node, None);
}

#[test]
fn a_boot_vga_attribute_that_is_not_zero_or_one_is_a_parse_error() {
    let sys = Sys::new("boot-vga-bad");
    sys.drm_card("0000:00:02.0", 0, Some("maybe\n"), None);
    match gpus(&sys.root) {
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
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_render_override_picks_a_card_by_its_node_or_its_render_node() {
    let sys = Sys::new("render-select");
    sys.drm_card("0000:00:02.0", 0, Some("1\n"), Some(128));
    sys.drm_card("0000:00:03.0", 1, None, Some(129));
    let all = gpus(&sys.root).unwrap();

    let (gpu, how) = select_render(all.clone(), Some(Path::new("/dev/dri/card1"))).unwrap();
    assert_eq!((gpu.device.sysname(), how), ("card1", RenderChoice::Named));
    let (gpu, how) = select_render(all.clone(), Some(Path::new("/dev/dri/renderD129"))).unwrap();
    assert_eq!((gpu.device.sysname(), how), ("card1", RenderChoice::Named));
}

#[test]
fn an_unset_empty_or_unmatched_override_falls_back_to_the_primary() {
    let sys = Sys::new("render-fallback");
    sys.drm_card("0000:00:02.0", 0, Some("1\n"), Some(128));
    sys.drm_card("0000:00:03.0", 1, None, None);
    let all = gpus(&sys.root).unwrap();

    let (gpu, how) = select_render(all.clone(), None).unwrap();
    assert_eq!(
        (gpu.device.sysname(), how),
        ("card0", RenderChoice::Primary)
    );
    let (_, how) = select_render(all.clone(), Some(Path::new(""))).unwrap();
    assert_eq!(how, RenderChoice::Primary);
    let (gpu, how) = select_render(all.clone(), Some(Path::new("/dev/dri/card9"))).unwrap();
    assert_eq!(gpu.device.sysname(), "card0");
    assert_eq!(
        how,
        RenderChoice::Unmatched(PathBuf::from("/dev/dri/card9"))
    );
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
fn preflight_without_a_gpu_reports_nothing() {
    let sys = Sys::new("preflight-none");
    assert!(preflight::run(&sys.root).unwrap().is_none());
}

#[test]
fn preflight_names_the_driver_of_the_primary_gpu() {
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
}

#[test]
fn preflight_warns_when_no_driver_is_bound() {
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
fn preflight_finds_nvidia_without_the_drm_module_a_problem() {
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
}

#[test]
fn preflight_finds_nvidia_modesetting_off_a_problem_and_on_fine() {
    let sys = gpu_with_driver("preflight-nvidia-off", Some("nvidia"));
    sys.write("module/nvidia_drm/parameters/modeset", "N\n");
    let report = preflight::run(&sys.root).unwrap().unwrap();
    assert_eq!(report.problems.len(), 1);
    assert!(report.problems[0].what.contains("modesetting is off"));
    assert!(report.problems[0].fix.contains("nvidia-drm.modeset=1"));

    let sys = gpu_with_driver("preflight-nvidia-on", Some("nvidia"));
    sys.write("module/nvidia_drm/parameters/modeset", "Y\n");
    sys.write("module/nvidia_drm/parameters/fbdev", "1\n");
    let report = preflight::run(&sys.root).unwrap().unwrap();
    assert!(report.problems.is_empty(), "{report}");
    assert!(report.notes.contains(&"nvidia-drm fbdev is on".to_owned()));
}

#[test]
fn preflight_reads_the_fbdev_flag_in_both_spellings() {
    for (value, shown) in [
        ("N\n", "off"),
        ("0\n", "off"),
        ("Y\n", "on"),
        ("weird\n", "weird"),
    ] {
        let sys = gpu_with_driver(
            &format!("preflight-fbdev-{}", shown.len() + value.len()),
            Some("nvidia"),
        );
        sys.write("module/nvidia_drm/parameters/modeset", "Y\n");
        sys.write("module/nvidia_drm/parameters/fbdev", value);
        let report = preflight::run(&sys.root).unwrap().unwrap();
        assert!(
            report
                .notes
                .contains(&format!("nvidia-drm fbdev is {shown}")),
            "{value:?}: {:?}",
            report.notes
        );
    }
}
