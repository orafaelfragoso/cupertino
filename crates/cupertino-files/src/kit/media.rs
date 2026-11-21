//! The removable media of the system bus ([`crate::udisks`]) in the windows: the sidebar's rows follow the devices
//! udisks reports, a click mounts a file system or asks for a container's passphrase, the eject button ejects a drive.
//! The connection is made by `cupertino_kit::bus` and ticked by the UI loop; the calls are futures on it, a mount
//! whose file system is not there yet is tried again by a scheduled wake, and the process table a busy file system's
//! notice reads is read by an I/O task.

use std::path::Path;
use std::sync::Arc;

use cupertino_kit::prelude::*;

use super::{woken, Handed, Message, Model, Unlock};
use crate::udisks::{self, Called, Passphrase, Report, Volumes};
use cupertino_kit::files::places::removable_holding;

/// The future of a call on the removable media, as the message that hands its end over.
fn called(call: impl std::future::Future<Output = Called> + Send + 'static) -> Command<Message> {
    Command::Perform(Box::pin(async move {
        Message::MediaCalled(Handed::new(call.await))
    }))
}

impl Model {
    /// The system bus connected: the removable media are listened for on it.
    pub(super) fn system_bus(
        &mut self,
        connection: zbus::Result<zbus::Connection>,
    ) -> Vec<Command<Message>> {
        match connection {
            Ok(system) => {
                let wake = Arc::clone(&self.session.media_wake);
                vec![Command::Perform(Box::pin(async move {
                    Message::MediaWatched(Handed::new(Volumes::watch(system, wake).await))
                }))]
            }
            Err(e) => {
                eprintln!("cupertino-files: no removable media: {e}");
                Vec::new()
            }
        }
    }

    /// The removable media are listened for: they are read now and whenever udisks changes them.
    pub(super) fn media_watched(
        &mut self,
        volumes: zbus::Result<Volumes>,
    ) -> Vec<Command<Message>> {
        let mut volumes = match volumes {
            Ok(volumes) => volumes,
            Err(e) => {
                eprintln!("cupertino-files: no removable media: {e}");
                return Vec::new();
            }
        };
        let mut commands: Vec<Command<Message>> = volumes
            .follow()
            .map(|follow| {
                Command::Perform(Box::pin(async move {
                    follow.await;
                    Message::Worked
                }))
            })
            .into_iter()
            .collect();
        commands.push(woken(&self.session.media_woken, Message::Media));
        self.session.volumes = Some(volumes);
        commands
    }

    /// The removable media reported: what they reported is taken, and the next report waited for.
    pub(super) fn follow_media(&mut self) -> Vec<Command<Message>> {
        let reports = self
            .session
            .volumes
            .as_ref()
            .map(Volumes::poll)
            .unwrap_or_default();
        self.take_media(reports);
        vec![woken(&self.session.media_woken, Message::Media)]
    }

    /// Take what the removable media reported: the rows follow the devices, a window on a file system that went away
    /// goes home, a passphrase sheet of a container that went away closes, and a finished mount goes to its folder in
    /// the window that asked for it.
    fn take_media(&mut self, reports: Vec<Report>) {
        if reports.is_empty() {
            return;
        }
        let session = &mut self.session;
        let held: Vec<bool> = self
            .windows
            .values()
            .map(|w| removable_holding(w.browser.dir(), &session.places).is_some())
            .collect();
        let mut arrived = Vec::new();
        for report in reports {
            let object = match &report {
                Report::Mounted { object, .. } | Report::Done { object } => {
                    session.mounting.remove(object)
                }
                Report::Devices(_) => None,
            };
            if let Some(at) = session.media.take(report) {
                arrived.extend(object.map(|window| (window, at)));
            }
        }
        session.show_media();
        for ((&id, window), held) in self.windows.iter_mut().zip(held) {
            if held && !session.media.holds(window.browser.dir()) {
                window.go(session.home.clone());
            }
            if let Some((_, at)) = arrived.iter().rev().find(|(w, _)| *w == id) {
                window.go(at.clone());
            }
            let gone = window
                .unlock
                .as_ref()
                .is_some_and(|u| session.media.index_of(&u.object).is_none());
            if gone {
                window.unlock = None;
            }
        }
    }

    /// A call on the removable media ended, or has more to do: its failure is a notice, a mount whose file system is
    /// not there yet is tried again after [`udisks::CLEARTEXT_RETRY`], and a busy file system's notice names the
    /// programs that hold it, read on an I/O task.
    pub(super) fn media_called(&mut self, called: Called) -> Vec<Command<Message>> {
        match called {
            Called::Ended(report, notice) => {
                if let Some(notice) = notice {
                    self.session.notices.post(notice);
                }
                self.take_media(vec![report]);
                Vec::new()
            }
            Called::Later(pending) => vec![Command::After {
                delay: udisks::CLEARTEXT_RETRY,
                message: Message::MountAgain(Handed::new(pending)),
            }],
            Called::Busy(busy) => {
                let mount = busy.mount().to_path_buf();
                vec![cupertino_kit::task::io(
                    udisks::HOLDERS_DEADLINE,
                    move || udisks::holders(Path::new("/proc"), &mount),
                    move |holders| Message::MediaCalled(Handed::new(busy.ended(&holders))),
                )]
            }
        }
    }

    /// Try a mount again whose file system was not there.
    pub(super) fn mount_again(&mut self, pending: udisks::Pending) -> Vec<Command<Message>> {
        match &self.session.volumes {
            Some(volumes) => vec![called(volumes.mount_again(pending))],
            None => Vec::new(),
        }
    }

    /// Open removable device `row` in the window `id`: a file system that is not mounted is mounted (the window goes
    /// there once it is), a locked container asks for its passphrase, and a row with a call under way does nothing.
    pub(super) fn open_device(&mut self, id: u64, row: usize) -> Vec<Command<Message>> {
        let session = &mut self.session;
        let Some(window) = self.windows.get_mut(&id) else {
            return Vec::new();
        };
        match session.media.device(row) {
            Some((device, false)) if device.locked => {
                window.unlock = Some(Unlock {
                    object: device.object.clone(),
                    name: device.name.clone(),
                    passphrase: Passphrase::default(),
                });
                Vec::new()
            }
            Some((_, false)) => {
                let (Some(volumes), Some(device)) = (&session.volumes, session.media.start(row))
                else {
                    return Vec::new();
                };
                session.mounting.insert(device.object.clone(), id);
                let call = called(volumes.mount(&device));
                session.show_media();
                vec![call]
            }
            _ => Vec::new(),
        }
    }

    /// Unlock the container of the passphrase sheet of the window `id` with the passphrase typed; the window goes to
    /// what it holds once that is mounted. An empty passphrase keeps the sheet open.
    pub(super) fn unlock(&mut self, id: u64) -> Vec<Command<Message>> {
        let session = &mut self.session;
        let Some(window) = self.windows.get_mut(&id) else {
            return Vec::new();
        };
        if window
            .unlock
            .as_ref()
            .is_none_or(|u| u.passphrase.is_empty())
        {
            return Vec::new();
        }
        let Some(unlock) = window.unlock.take() else {
            return Vec::new();
        };
        let started = session
            .media
            .index_of(&unlock.object)
            .and_then(|row| session.media.start(row));
        let (Some(volumes), Some(device)) = (&session.volumes, started) else {
            return Vec::new();
        };
        session.mounting.insert(device.object.clone(), id);
        let call = called(volumes.unlock(&device, unlock.passphrase));
        session.show_media();
        vec![call]
    }

    /// A press on the accessory of the sidebar row `key`: the eject button of a mounted removable file system.
    pub(super) fn accessory(&mut self, key: &Key) -> Vec<Command<Message>> {
        let session = &self.session;
        let row = (0..session.places.len())
            .find(|&i| super::view::place_key(session, i) == *key)
            .filter(|&i| session.places[i].ejectable());
        row.map(|index| self.eject(index)).unwrap_or_default()
    }

    /// Eject the drive of the removable file system of place `index`: the windows on its file systems go home first,
    /// so they hold none of them.
    pub(super) fn eject(&mut self, index: usize) -> Vec<Command<Message>> {
        let session = &mut self.session;
        let Some((device, filesystems)) = index
            .checked_sub(session.standard_places)
            .and_then(|i| session.media.eject(i))
        else {
            return Vec::new();
        };
        let mounts: Vec<&Path> = filesystems
            .iter()
            .filter_map(|d| d.mount.as_deref())
            .collect();
        for window in self.windows.values_mut() {
            if mounts.iter().any(|m| window.browser.dir().starts_with(m)) {
                window.go(session.home.clone());
            }
        }
        let call = session
            .volumes
            .as_ref()
            .map(|volumes| called(volumes.eject(&device, filesystems)));
        session.show_media();
        call.into_iter().collect()
    }
}
