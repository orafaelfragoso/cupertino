//! Where the default trash is, from the environment.
//!
//! This file changes environment variables, so it has one test, which restores them.

use cupertino_trash::Trash;
use std::path::PathBuf;

#[test]
fn the_default_trash_is_in_the_data_home_else_in_the_home_else_beside_the_working_folder() {
    let saved: Vec<_> = ["XDG_DATA_HOME", "HOME"]
        .iter()
        .map(|v| (*v, std::env::var_os(v)))
        .collect();
    let outcome = std::panic::catch_unwind(|| {
        for (data_home, home, expected) in [
            (Some("/data-home"), Some("/home/u"), "/data-home/Trash"),
            (None, Some("/home/u"), "/home/u/.local/share/Trash"),
            (None, None, "./Trash"),
        ] {
            for (name, value) in [("XDG_DATA_HOME", data_home), ("HOME", home)] {
                match value {
                    Some(v) => std::env::set_var(name, v),
                    None => std::env::remove_var(name),
                }
            }
            assert_eq!(
                Trash::new().home_trash(),
                PathBuf::from(expected),
                "{data_home:?} {home:?}"
            );
            assert_eq!(Trash::default().home_trash(), PathBuf::from(expected));
        }
    });
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
