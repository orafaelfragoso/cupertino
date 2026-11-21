//! Work that takes turns (`inflight::Turns`): the Wallpaper pane's thumbnails and folders of pictures start a few at
//! a time, in the order they were asked for, so that however many are asked for the services keep a thread of the
//! window's share of Kit's I/O pool.

use cupertino_kit::task::IO_SHARE;
use cupertino_settings::inflight::{Turns, PICTURE_FOLDER_TURNS, THUMBNAIL_TURNS};

#[test]
fn many_thumbnails_and_folders_leave_the_services_a_thread() {
    let mut thumbnails = Turns::new(THUMBNAIL_TURNS);
    let mut folders = Turns::new(PICTURE_FOLDER_TURNS);
    let mut started = Vec::new();
    started.extend((0..200).filter_map(|n| thumbnails.ask(n)));
    started.extend((1000..1020).filter_map(|n| folders.ask(n)));
    // the task of the services and one more of the window's (a file, a program) still find a thread
    assert!(thumbnails.running() + folders.running() + 2 <= IO_SHARE);
    assert_eq!(started, vec![0, 1000]);
    assert_eq!((thumbnails.waiting(), folders.waiting()), (199, 19));

    // each one finished starts the next in the order asked for, and the last leaves nothing running
    let mut order = Vec::new();
    while let Some(next) = thumbnails.finished() {
        order.push(next);
    }
    assert_eq!(order, (1..200).collect::<Vec<_>>());
    assert_eq!((thumbnails.running(), thumbnails.waiting()), (0, 0));
    assert_eq!(
        thumbnails.ask(7),
        Some(7),
        "a thumbnail asked for later starts at once"
    );
}
