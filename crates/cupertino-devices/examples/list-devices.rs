//! Opt-in check against the real `/sys` of this machine:
//! `cargo run -p cupertino-devices --example list-devices`.

use std::path::Path;

use cupertino_devices::{
    classify_device, enumerate, gpus, primary_gpu, InputClass, Monitor, Subsystem,
};

const SYSFS: &str = "/sys";

/// The names of the set flags, e.g. `keyboard,key`.
fn flags(class: &InputClass) -> String {
    let all = [
        ("key", class.key),
        ("keyboard", class.keyboard),
        ("mouse", class.mouse),
        ("touchpad", class.touchpad),
        ("touchscreen", class.touchscreen),
        ("tablet", class.tablet),
        ("tablet-pad", class.tablet_pad),
        ("joystick", class.joystick),
        ("pointing-stick", class.pointing_stick),
        ("accelerometer", class.accelerometer),
        ("switch", class.switch),
        ("clickpad", class.clickpad),
    ];
    let names: Vec<&str> = all
        .iter()
        .filter(|(_, set)| *set)
        .map(|(name, _)| *name)
        .collect();
    if names.is_empty() {
        "-".to_owned()
    } else {
        names.join(",")
    }
}

fn main() {
    let root = Path::new(SYSFS);

    println!("input devices:");
    for device in enumerate(root, Subsystem::Input).expect("enumerate input") {
        let class = classify_device(&device, root).expect("classify");
        let name = device
            .parent(root)
            .expect("parent")
            .and_then(|parent| parent.property("NAME").map(str::to_owned))
            .unwrap_or_default();
        let devnode = device
            .devnode()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let devnum = device.devnum().map(|n| n.to_string()).unwrap_or_default();
        println!(
            "  {devnode} ({devnum}) {name} -> {}",
            class.map_or("not an input device".to_owned(), |c| flags(&c))
        );
    }

    println!("gpus:");
    for gpu in gpus(root).expect("gpus") {
        let devnode = gpu
            .device
            .devnode()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        println!(
            "  {devnode} boot_vga={} seat={}",
            gpu.boot_vga,
            gpu.device.seat()
        );
    }
    let primary = primary_gpu(root).expect("primary gpu");
    println!(
        "primary: {}",
        primary.map_or("none".to_owned(), |gpu| gpu.device.sysname().to_owned())
    );

    let monitor = Monitor::new().expect("open uevent socket");
    let queued = monitor.drain().expect("drain");
    println!(
        "monitor: bound to kernel group 1, {} queued events, {} malformed, overflowed: {}",
        queued.events.len(),
        queued.errors.len(),
        queued.overflowed
    );
}
