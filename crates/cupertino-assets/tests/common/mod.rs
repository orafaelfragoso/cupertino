//! Helpers shared by the integration tests: pack files in a temp dir and a restoring environment guard.
#![allow(dead_code)]

use std::ffi::OsString;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

/// Write `content` to `root/rel`, creating the directories.
pub fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

/// Write a pack manifest.
pub fn manifest(root: &Path, json: &str) {
    write(root, "manifest.json", json);
}

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Holds the process-wide environment lock and puts every variable it changed back when dropped, so tests that run
/// as threads of one process neither race nor leak values.
pub struct Env {
    saved: Vec<(String, Option<OsString>)>,
    _lock: MutexGuard<'static, ()>,
}

impl Env {
    pub fn new() -> Self {
        Self {
            saved: Vec::new(),
            _lock: ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner()),
        }
    }

    fn remember(&mut self, name: &str) {
        if !self.saved.iter().any(|(n, _)| n == name) {
            self.saved.push((name.to_string(), std::env::var_os(name)));
        }
    }

    pub fn set(&mut self, name: &str, value: impl AsRef<std::ffi::OsStr>) -> &mut Self {
        self.remember(name);
        // SAFETY: the environment lock is held and tests in this process change variables only through it.
        unsafe { std::env::set_var(name, value) };
        self
    }

    pub fn unset(&mut self, name: &str) -> &mut Self {
        self.remember(name);
        // SAFETY: as in `set`.
        unsafe { std::env::remove_var(name) };
        self
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        for (name, value) in self.saved.drain(..).rev() {
            // SAFETY: as in `set`; the lock is still held.
            unsafe {
                match value {
                    Some(v) => std::env::set_var(&name, v),
                    None => std::env::remove_var(&name),
                }
            }
        }
    }
}
