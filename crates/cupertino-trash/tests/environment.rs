//! Where the default trash is, from the environment.
//!
//! This file changes environment variables, so its tests hold the lock and restore them.

use cupertino_trash::Trash;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Mutex;

static LOCK: Mutex<()> = Mutex::new(());

fn with_env(data_home: Option<&str>, home: Option<&str>, check: impl FnOnce()) {
    let _held = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let saved: Vec<(&str, Option<OsString>)> = ["XDG_DATA_HOME", "HOME"]
        .iter()
        .map(|v| (*v, std::env::var_os(v)))
        .collect();
    for (name, value) in [("XDG_DATA_HOME", data_home), ("HOME", home)] {
        match value {
            Some(v) => std::env::set_var(name, v),
            None => std::env::remove_var(name),
        }
    }
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(check));
    for (name, value) in saved {
        match value {
            Some(v) => std::env::set_var(name, v),
            None => std::env::remove_var(name),
        }
    }
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

#[test]
fn the_default_trash_is_in_the_data_home() {
    with_env(Some("/data-home"), Some("/home/u"), || {
        assert_eq!(Trash::new().home_trash(), PathBuf::from("/data-home/Trash"));
    });
}

#[test]
fn without_a_data_home_the_trash_is_in_the_local_share_of_the_home() {
    with_env(None, Some("/home/u"), || {
        assert_eq!(
            Trash::new().home_trash(),
            PathBuf::from("/home/u/.local/share/Trash")
        );
        assert_eq!(Trash::default().home_trash(), Trash::new().home_trash());
    });
}

#[test]
fn without_either_the_trash_is_beside_the_working_folder() {
    with_env(None, None, || {
        assert_eq!(Trash::new().home_trash(), PathBuf::from("./Trash"));
    });
}
