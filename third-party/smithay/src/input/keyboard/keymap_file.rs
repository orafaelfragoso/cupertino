use std::ffi::CString;
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::os::unix::io::{AsFd, BorrowedFd};
use std::path::PathBuf;

use sha2::{Digest, Sha256};
use tracing::error;
use xkbcommon::xkb::{self, Keymap, KEYMAP_FORMAT_TEXT_V1};

use crate::utils::SealedFile;

/// Unique ID for a keymap
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct KeymapFileId([u8; 32]);

impl KeymapFileId {
    fn for_keymap(keymap: &str) -> Self {
        // Use a hash, so `keymap` events aren't sent when keymap hasn't changed, particularly
        // with `virtual-keyboard-unstable-v1`.
        Self(Sha256::digest(keymap).as_slice().try_into().unwrap())
    }
}

/// Prepare an unsealed file descriptor for legacy `wl_keyboard` (< v7) and input-method recipients
/// when the keymap is created or changed, so delivery never writes a temporary file on the loop.
/// On Linux/Android this uses an unsealed memfd re-opened read-only via `/proc/self/fd`, falling
/// back to an unlinked temporary file.
fn build_legacy_file(keymap: &str) -> Result<File, std::io::Error> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use std::os::unix::io::AsRawFd;

        if let Ok(fd) = rustix::fs::memfd_create(c"smithay-keymap-legacy", rustix::fs::MemfdFlags::CLOEXEC) {
            let mut file: File = fd.into();
            if file.write_all(keymap.as_bytes()).is_ok()
                && file.flush().is_ok()
                && file.seek(SeekFrom::Start(0)).is_ok()
            {
                if let Ok(readonly) = File::open(format!("/proc/self/fd/{}", file.as_raw_fd())) {
                    return Ok(readonly);
                }
            }
        }
    }

    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);

    let mut file = tempfile::tempfile_in(dir)?;
    file.write_all(keymap.as_bytes())?;
    file.flush()?;
    file.seek(SeekFrom::Start(0))?;
    Ok(file)
}

/// Wraps an XKB keymap into a sealed file and a prepared legacy file for sending to WlKeyboard over an fd
#[derive(Debug)]
pub struct KeymapFile {
    sealed: Option<SealedFile>,
    legacy: Option<File>,
    keymap: String,
    id: KeymapFileId,
}

impl KeymapFile {
    /// Turn the keymap into a string using KEYMAP_FORMAT_TEXT_V1, create sealed and legacy files for it, and store the string
    pub fn new(keymap: &Keymap) -> Self {
        let name = c"smithay-keymap";
        let keymap = keymap.get_as_string(KEYMAP_FORMAT_TEXT_V1);
        let sealed = SealedFile::with_content(name, &CString::new(keymap.as_str()).unwrap());

        if let Err(err) = sealed.as_ref() {
            error!("Error when creating sealed keymap file: {}", err);
        }

        let legacy = build_legacy_file(&keymap);
        if let Err(err) = legacy.as_ref() {
            error!("Error when creating legacy keymap file: {}", err);
        }

        Self {
            id: KeymapFileId::for_keymap(&keymap),
            sealed: sealed.ok(),
            legacy: legacy.ok(),
            keymap,
        }
    }

    #[cfg(feature = "wayland_frontend")]
    pub(crate) fn change_keymap(&mut self, keymap: &Keymap) {
        let keymap = keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);

        let name = c"smithay-keymap-file";
        let sealed = SealedFile::with_content(name, &CString::new(keymap.clone()).unwrap());

        if let Err(err) = sealed.as_ref() {
            error!("Error when creating sealed keymap file: {}", err);
        }

        let legacy = build_legacy_file(&keymap);
        if let Err(err) = legacy.as_ref() {
            error!("Error when creating legacy keymap file: {}", err);
        }

        self.id = KeymapFileId::for_keymap(&keymap);
        self.sealed = sealed.ok();
        self.legacy = legacy.ok();
        self.keymap = keymap;
    }

    #[cfg(feature = "wayland_frontend")]
    /// Run a closure with the file descriptor to ensure safety
    pub fn with_fd<F>(&self, supports_sealed: bool, cb: F) -> Result<(), std::io::Error>
    where
        F: FnOnce(BorrowedFd<'_>, usize),
    {
        if let Some(file) = supports_sealed.then_some(self.sealed.as_ref()).flatten() {
            cb(file.as_fd(), file.size());
        } else if let Some(file) = self.legacy.as_ref() {
            cb(file.as_fd(), self.keymap.len());
        } else {
            return Err(std::io::Error::other("legacy keymap file unavailable"));
        }
        Ok(())
    }

    /// Send the keymap contained within to a WlKeyboard
    pub fn send(
        &self,
        keyboard: &wayland_server::protocol::wl_keyboard::WlKeyboard,
    ) -> Result<(), std::io::Error> {
        use wayland_server::{protocol::wl_keyboard::KeymapFormat, Resource};

        self.with_fd(keyboard.version() >= 7, |fd, size| {
            keyboard.keymap(KeymapFormat::XkbV1, fd, size as u32);
        })
    }

    /// Get this keymap's unique ID.
    pub(crate) fn id(&self) -> KeymapFileId {
        self.id
    }
}
