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

#[test]
fn general_opens_without_the_graphics_memory_and_shows_it_when_its_thread_answers() {
    let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    graphics(root.path());
    let (release, released) = mpsc::channel::<()>();
    let released = Mutex::new(released);
    let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = reads.clone();
    let services = Services::on(Bus::address(NOWHERE), Bus::address(NOWHERE));
    let about_root = root.path().to_owned();
    // opened on another pane, so General reads with the reader set here
    let (mut driver, window) =
        start_state(dir.path(), PaneId::Appearance, services, move |state| {
            state.about_root = about_root;
            state.read_video_memory = Arc::new(move || {
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                // the tool answers only when the test lets it
                let _ = released.lock().unwrap().recv();
                vec![(PciAddress::parse("00000000:05:00.0").unwrap(), 12 << 30)]
            });
        });
    click(&mut driver, window, "general");
    click(
        &mut driver,
        window,
        &format!("open-{:?}", crate::pages::SubPage::About),
    );
    assert!(
        shows(&driver, window, "Example Graphics Lite"),
        "the pane shows before the tool answers"
    );
    release.send(()).unwrap();
    let end = Instant::now() + THREAD_WAIT;
    while !shows(&driver, window, "Example Graphics Lite 12\u{a0}GB") {
        assert!(Instant::now() < end, "the memory fills in");
        std::thread::sleep(Duration::from_millis(20));
        let _ = driver.window_system_mut().enqueue(PlatformEvent::Wake);
        settle(&mut driver, window);
    }
    assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 1);
}
