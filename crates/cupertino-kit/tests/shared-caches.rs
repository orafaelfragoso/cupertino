//! The caches every Kit instance of a process shares (D5): instances starting at once on UI threads of their own, and
//! the process itself, start on one text system with its fonts and one set of the asset packs' icons, while each
//! keeps a theme of its own.

use cupertino_kit::app;
use cupertino_kit::core::Environment;
use cupertino_kit::Instance;
use std::sync::{Arc, Barrier};

#[test]
fn instances_start_on_the_processs_one_text_system_and_one_set_of_icons() {
    // the built-in font alone, whatever packs the machine has
    std::env::set_var(cupertino_assets::SEARCH_PATH_ENV, "");
    let together = Arc::new(Barrier::new(2));
    let threads = ["first", "second"].map(|area| {
        let together = Arc::clone(&together);
        std::thread::Builder::new()
            .name(format!("{area}-ui"))
            .spawn(move || {
                let _instance = Instance::new(area).enter();
                together.wait();
                app::environment().unwrap()
            })
            .unwrap()
    });
    let [first, second]: [Environment; 2] = threads.map(|t| t.join().unwrap());
    let process = app::environment().unwrap();
    for environment in [&first, &second] {
        assert!(Arc::ptr_eq(&environment.text, &process.text));
        assert!(Arc::ptr_eq(&environment.icons, &process.icons));
    }
    assert!(
        !Arc::ptr_eq(&first.theme, &second.theme),
        "each starts on a theme of its own"
    );
}
