use std::fs;
use std::path::Path;
use std::time::Instant;

use cupertino_sessiond::upgrade::{decide, Decision, Installed, Watch, SETTLE};

#[test]
fn the_manager_waits_for_one_settled_image_and_the_last_session_to_end() {
    let dir =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("upgrade-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let path = dir.join("manager");
    let replace = || {
        let new = dir.join("replacement");
        fs::write(&new, b"image").unwrap();
        fs::rename(new, &path).unwrap();
    };
    replace();
    // Keep the original inode allocated, as a running manager does.
    let _running = fs::File::open(&path).unwrap();
    let mut watch = Watch::of_file(&path).unwrap();
    let start = Instant::now();
    assert_eq!(watch.look(start), (Installed::Same, false));
    replace();
    assert_eq!(watch.look(start), (Installed::Replaced, false));
    replace();
    let next = start + SETTLE;
    assert_eq!(watch.look(next), (Installed::Replaced, false));
    let (installed, settled) = watch.look(next + SETTLE);
    assert_eq!(decide(installed, settled, true), Decision::Keep);
    assert_eq!(decide(installed, settled, false), Decision::HandOver);
    fs::remove_dir_all(dir).unwrap();
}
