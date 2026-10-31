//! Runs the public API against the fake sysfs tree in `tests/fixtures/sys`.

use std::path::{Path, PathBuf};

use cupertino_devices::{
    classify_device, enumerate, gpus, primary_gpu, select_primary, select_render, DevT, Device,
    Error, InputClass, Subsystem,
};

fn sysfs() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sys")
}

fn input_devices() -> Vec<Device> {
    enumerate(&sysfs(), Subsystem::Input).unwrap()
}

fn class_of(sysname: &str) -> InputClass {
    let device = input_devices()
        .into_iter()
        .find(|d| d.sysname() == sysname)
        .unwrap();
    classify_device(&device, &sysfs()).unwrap().unwrap()
}

#[test]
fn input_enumeration_keeps_only_event_nodes_in_order() {
    let devices = input_devices();
    let names: Vec<&str> = devices.iter().map(|d| d.sysname()).collect();
    assert_eq!(names, ["event3", "event5", "event6", "event7"]);
    assert_eq!(devices[0].devnode(), Some(Path::new("/dev/input/event3")));
    assert_eq!(devices[0].devnum(), Some(DevT::new(13, 67)));
    assert_eq!(devices[2].devnum(), Some(DevT::new(13, 70)));
    assert_eq!(devices[0].seat(), "seat0");
}

#[test]
fn parent_chain_skips_container_directories() {
    let touchpad = &input_devices()[2];
    let parents = touchpad.parents(&sysfs()).unwrap();
    let names: Vec<&str> = parents.iter().map(|d| d.sysname()).collect();
    // `input/` has no uevent and is skipped; the chain ends below `/devices`.
    assert_eq!(names, ["input8", "0018:06CB:CD8B.0002"]);
    assert_eq!(
        parents[0].property("NAME"),
        Some("\"SYNA8004:00 06CB:CD8B Touchpad\"")
    );
}

#[test]
fn keyboard() {
    let class = class_of("event3");
    assert_eq!(
        class,
        InputClass {
            key: true,
            keyboard: true,
            ..InputClass::default()
        }
    );
}

#[test]
fn mouse() {
    assert_eq!(
        class_of("event5"),
        InputClass {
            mouse: true,
            ..InputClass::default()
        }
    );
}

#[test]
fn clickpad_touchpad() {
    let class = class_of("event6");
    assert_eq!(
        class,
        InputClass {
            touchpad: true,
            clickpad: true,
            ..InputClass::default()
        }
    );
}

#[test]
fn touchscreen() {
    assert_eq!(
        class_of("event7"),
        InputClass {
            touchscreen: true,
            ..InputClass::default()
        }
    );
}

#[test]
fn drm_enumeration_keeps_only_card_nodes() {
    let names: Vec<String> = gpus(&sysfs())
        .unwrap()
        .iter()
        .map(|gpu| gpu.device.sysname().to_owned())
        .collect();
    assert_eq!(names, ["card0", "card1"]);
}

#[test]
fn primary_gpu_is_the_boot_vga_card() {
    let primary = primary_gpu(&sysfs()).unwrap().unwrap();
    assert_eq!(primary.device.sysname(), "card1");
    assert_eq!(primary.device.devnode(), Some(Path::new("/dev/dri/card1")));
    assert_eq!(primary.device.devnum(), Some(DevT::new(226, 1)));
}

#[test]
fn primary_gpu_without_boot_vga_is_the_first_card() {
    let mut all = gpus(&sysfs()).unwrap();
    for gpu in &mut all {
        gpu.boot_vga = false;
    }
    assert_eq!(select_primary(all).unwrap().device.sysname(), "card0");
}

#[test]
fn the_render_gpu_is_the_named_card_else_the_primary_one() {
    let all = gpus(&sysfs()).unwrap();
    let render = |wanted: Option<&str>| {
        select_render(all.clone(), wanted.map(Path::new)).map(|gpu| gpu.device.sysname().to_owned())
    };
    assert_eq!(render(None).as_deref(), Some("card1"));
    assert_eq!(render(Some("/dev/dri/card0")).as_deref(), Some("card0"));
    assert_eq!(render(Some("/dev/dri/card7")).as_deref(), Some("card1"));
    assert_eq!(
        select_render(Vec::new(), Some(Path::new("/dev/dri/card0"))),
        None
    );
}

#[test]
fn missing_class_directory_is_an_error() {
    let result = enumerate(&sysfs().join("nowhere"), Subsystem::Input);
    assert!(matches!(result, Err(Error::Io { .. })));
}
