//! Live settings reloads and legacy keymap delivery never block the compositor's loop:
//! - `KeymapFile::with_fd(false, ...)` delivers a pre-built read-only descriptor without writing to the filesystem.
//! - A settings file that blocks when opened (a FIFO) does not stall the event loop, while other settings files still
//!   reload off the loop and apply cleanly.

use std::ffi::CString;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cupertino_assets::Assets;
use cupertino_defaults::power::PowerSettings;
use cupertino_server::config::SettingsFiles;
use cupertino_server::live::SettingsPaths;
use cupertino_server::{Config, Cupertino};
use smithay::input::keyboard::{xkb, KeymapFile, XkbConfig};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;

/// Upper bound for waiting for an off-loop reload on a loaded builder.
const SETTLE: Duration = Duration::from_secs(5);
/// Maximum time a single event-loop dispatch should ever take while a settings file is blocked.
const DISPATCH_BOUND: Duration = Duration::from_millis(500);

#[test]
fn legacy_keymap_delivery_uses_prepared_readonly_fd_without_writing_files() {
    let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    let xkb_cfg = XkbConfig::default();
    let keymap = xkb::Keymap::new_from_names(
        &ctx,
        &xkb_cfg.rules,
        &xkb_cfg.model,
        &xkb_cfg.layout,
        &xkb_cfg.variant,
        xkb_cfg.options,
        xkb::COMPILE_NO_FLAGS,
    )
    .expect("default US keymap compiles");
    let expected = keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
    let keymap_file = KeymapFile::new(&keymap);

    // Even if XDG_RUNTIME_DIR points to a nonexistent path, legacy delivery must not write a tempfile.
    let prior = std::env::var_os("XDG_RUNTIME_DIR");
    std::env::set_var("XDG_RUNTIME_DIR", "/nonexistent/cupertino-keymap-test-dir");

    let mut read_back = Vec::new();
    let mut write_failed = false;
    let res = keymap_file.with_fd(false, |fd, size| {
        assert_eq!(size, expected.len());
        // Safety: mmap of valid fd for `size` bytes read-only, unmapped immediately after copy.
        unsafe {
            let ptr = libc::mmap(
                std::ptr::null_mut(),
                size,
                libc::PROT_READ,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            );
            assert_ne!(ptr, libc::MAP_FAILED, "MAP_SHARED read-only mmap must work");
            let slice = std::slice::from_raw_parts(ptr.cast::<u8>(), size);
            read_back.extend_from_slice(slice);
            libc::munmap(ptr, size);

            let byte = 0u8;
            write_failed = libc::write(fd.as_raw_fd(), (&byte as *const u8).cast(), 1) < 0;
        }
    });

    match prior {
        Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
        None => std::env::remove_var("XDG_RUNTIME_DIR"),
    }

    res.expect("legacy keymap delivery must succeed without filesystem writes");
    assert_eq!(read_back, expected.as_bytes());
    assert!(write_failed, "prepared legacy keymap fd must be read-only");
}

#[test]
fn blocked_settings_file_does_not_block_loop_and_other_files_still_reload() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("live-settings-deadlines-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let compositor_path = dir.join("compositor.json");
    let power_path = dir.join("power.conf");
    std::fs::write(&compositor_path, "{}\n").unwrap();
    let initial_power = PowerSettings {
        dim_before_display_sleep: true,
        ..PowerSettings::default()
    };
    initial_power.store(&power_path).unwrap();

    let mut event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().unwrap();
    let display: Display<Cupertino> = Display::new().unwrap();
    let mut state = Cupertino::new(
        display,
        event_loop.handle(),
        event_loop.get_signal(),
        Config::default(),
        false,
        &Assets::builtin(),
    )
    .expect("server starts");
    let paths = SettingsPaths {
        compositor: Some(compositor_path.clone()),
        power: Some(power_path.clone()),
        ..SettingsPaths::default()
    };
    let files = SettingsFiles {
        power: initial_power,
        ..SettingsFiles::default()
    };
    state.start_settings_watch(files, paths);

    // Replace compositor.json with a FIFO so any blocking open on it stalls until a writer opens it.
    std::fs::remove_file(&compositor_path).unwrap();
    let c_fifo = CString::new(compositor_path.to_str().unwrap()).unwrap();
    // Safety: valid NUL-terminated path string.
    assert_eq!(unsafe { libc::mkfifo(c_fifo.as_ptr(), 0o600) }, 0);

    // Concurrently update power.conf with a real setting change.
    let updated_power = PowerSettings {
        dim_before_display_sleep: false,
        ..PowerSettings::default()
    };
    updated_power.store(&power_path).unwrap();

    let deadline = Instant::now() + SETTLE;
    while state.config.power.dim_before_display_sleep {
        assert!(
            Instant::now() < deadline,
            "power.conf did not reload while compositor.json was blocked on a FIFO"
        );
        let step = Instant::now();
        event_loop
            .dispatch(Some(Duration::from_millis(20)), &mut state)
            .unwrap();
        assert!(
            step.elapsed() < DISPATCH_BOUND,
            "event loop dispatch stalled for {:?}",
            step.elapsed()
        );
    }
    assert!(state.live.reloads >= 1);

    // Unblock the off-loop compositor.json reader cleanly so no background thread lingers.
    let unblock_path: PathBuf = compositor_path.clone();
    let unblocker = std::thread::spawn(move || {
        use std::io::Write;
        if let Ok(mut w) = std::fs::OpenOptions::new().write(true).open(&unblock_path) {
            let _ = w.write_all(b"{}\n");
        }
    });
    let _ = unblocker.join();
}
