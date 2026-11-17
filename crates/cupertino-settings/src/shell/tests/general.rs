//! The General pane's About page: it opens at once, and the graphics memory only the driver's tool reports, read on
//! a thread, fills in when it arrives.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use cupertino_kit::platform::PlatformEvent;

use super::*;
use crate::about::PciAddress;

/// How long the test waits for the reading thread's answer to reach the window.
const THREAD_WAIT: Duration = Duration::from_secs(10);

fn write(root: &Path, path: &str, value: &str) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, value).unwrap();
}

/// One graphics device whose memory sysfs does not report, as its driver leaves it.
fn graphics(root: &Path) {
    write(
        root,
        "usr/share/hwdata/pci.ids",
        "10de  Example Vendor\n\t2504  GA106 [Example Graphics Lite Hash Rate]\n",
    );
    let device = "sys/devices/pci0000:00/0000:05:00.0";
    write(root, &format!("{device}/vendor"), "0x10de\n");
    write(root, &format!("{device}/device"), "0x2504\n");
    let card = root.join("sys/class/drm/card1");
    std::fs::create_dir_all(&card).unwrap();
    std::os::unix::fs::symlink(root.join(device), card.join("device")).unwrap();
}

/// Whether the window shows `text` as a name or a value.
fn shows(driver: &TestDriver<SettingsApp>, window: WindowId, text: &str) -> bool {
    let quoted = format!("{text:?}");
    driver
        .runtime()
        .snapshot(window)
        .unwrap()
        .accessibility
        .iter()
        .any(|(_, node)| {
            node.name.as_deref() == Some(text) || format!("{:?}", node.value).contains(&quoted)
        })
}
