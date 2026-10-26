//! Opens an evdev node read-only and prints its capabilities, then every frame as it arrives.
//!
//! Usage: `cargo run -p cupertino-evdev --example print-frames -- /dev/input/event5`

use cupertino_evdev::Device;
use rustix::event::{poll, PollFd, PollFlags};
use rustix::fs::{open, Mode, OFlags};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(path), None) = (args.next(), args.next()) else {
        return Err("usage: print-frames /dev/input/eventN".into());
    };
    let fd = open(&path, OFlags::RDONLY | OFlags::CLOEXEC, Mode::empty())
        .map_err(|errno| format!("open {path}: {errno}"))?;
    let mut device = Device::open(fd)?;
    println!("{:#?}", device.info());

    let mut frames = Vec::new();
    loop {
        device.read_frames(&mut frames)?;
        for frame in frames.drain(..) {
            println!("{frame:?}");
        }
        let mut fds = [PollFd::new(&device, PollFlags::IN)];
        poll(&mut fds, None)?;
    }
}
