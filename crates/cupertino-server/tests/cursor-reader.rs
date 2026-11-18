use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use cupertino_assets::CursorTheme;
use cupertino_cursor::xcursor::{self, XCursorImage};
use cupertino_server::cursor::{CursorColors, CursorLoader};
use rustix::event::{poll, PollFd, PollFlags, Timespec};
use smithay::input::pointer::CursorIcon;
use smithay::reexports::calloop::EventLoop;

fn image(width: u32) -> Vec<u8> {
    xcursor::write(&[XCursorImage {
        nominal_size: 24,
        width,
        height: 1,
        xhot: 0,
        yhot: 0,
        delay_ms: 0,
        pixels: vec![0xff000000; width as usize],
    }])
    .unwrap()
}

#[test]
fn a_slow_cursor_file_leaves_the_arrow_visible_then_wakes_the_loop_to_redraw() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("cursor-{}", std::process::id()));
    let cursors = dir.join("Slow/cursors");
    fs::create_dir_all(&cursors).unwrap();
    fs::write(cursors.join("default"), image(1)).unwrap();
    let path = cursors.join("text");
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        &path,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )
    .unwrap();
    let events = EventLoop::try_new().unwrap();
    let mut loader = CursorLoader::load(
        Some(CursorTheme {
            name: "Slow".into(),
            search_dir: dir.clone(),
        }),
        CursorColors::DEFAULT,
        &events.handle(),
    );

    // Nobody has opened the FIFO for writing: a synchronous load would not return.
    assert_eq!(
        loader.frame(CursorIcon::Text, 24, Duration::ZERO).pixels,
        (1, 1)
    );
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .write_all(&image(3))
        .unwrap();
    let mut fds = [PollFd::new(&events, PollFlags::IN)];
    let timeout = Timespec {
        tv_sec: 3,
        tv_nsec: 0,
    };
    assert_eq!(poll(&mut fds, Some(&timeout)).unwrap(), 1);
    assert_eq!(
        loader.frame(CursorIcon::Text, 24, Duration::ZERO).pixels,
        (3, 1)
    );
    fs::remove_dir_all(dir).unwrap();
}
