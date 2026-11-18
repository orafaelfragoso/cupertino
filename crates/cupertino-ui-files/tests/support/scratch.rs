//! A folder of a test's own under the target directory, removed when the test is done, for the tests of what touches
//! the file system.

use std::path::{Path, PathBuf};

pub struct Scratch(PathBuf);

impl Scratch {
    /// An empty folder named after `test`.
    pub fn new(test: &str) -> Scratch {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("ui-files-{test}"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Scratch(root)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// A file `rel` below the folder holding `bytes`; the folders above it are made.
    pub fn file(&self, rel: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        path
    }

    /// A folder `rel` below the folder.
    pub fn folder(&self, rel: &str) -> PathBuf {
        let path = self.0.join(rel);
        std::fs::create_dir_all(&path).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
