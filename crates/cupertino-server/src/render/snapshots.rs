//! Window snapshots that outlive the frame that captured them (the last look of a closing window), behind
//! generational handles. The owner holds a snapshot as long as it should exist; a frame leases it while its
//! elements may still be drawn. Dropping the owner retires the snapshot at once unless a lease is outstanding,
//! and the last lease to end retires it then, so a snapshot cannot vanish under a frame that was composed with it.

use std::cell::RefCell;
use std::rc::Rc;

use cupertino_scene::{ResourceKey, SnapshotId};
use smithay::backend::renderer::gles::GlesTexture;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotError {
    /// The handle names a snapshot that was never stored here, or one that has been retired.
    Stale(SnapshotId),
}

impl std::fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stale(id) => write!(f, "snapshot {id:?} is retired or unknown"),
        }
    }
}

impl std::error::Error for SnapshotError {}

struct Slot<T> {
    generation: u64,
    texture: Option<T>,
    leases: usize,
    owned: bool,
}

struct Inner<T> {
    slots: Vec<Slot<T>>,
    free: Vec<usize>,
}

impl<T> Inner<T> {
    fn retire_if_unused(&mut self, slot: usize) {
        let s = &mut self.slots[slot];
        if !s.owned && s.leases == 0 && s.texture.take().is_some() {
            s.generation += 1;
            self.free.push(slot);
        }
    }
}

/// The store of retained snapshots; clones share it.
pub struct Snapshots<T = GlesTexture>(Rc<RefCell<Inner<T>>>);

impl<T> Clone for Snapshots<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Default for Snapshots<T> {
    fn default() -> Self {
        Self(Rc::new(RefCell::new(Inner {
            slots: Vec::new(),
            free: Vec::new(),
        })))
    }
}

/// Keeps a snapshot alive; the snapshot is retired when this goes, once no lease is left.
pub struct SnapshotOwner<T = GlesTexture> {
    store: Snapshots<T>,
    id: SnapshotId,
}

/// A frame's claim on a snapshot while its elements may be drawn.
pub struct SnapshotLease<T = GlesTexture> {
    store: Snapshots<T>,
    id: SnapshotId,
}

impl<T: Clone> Snapshots<T> {
    pub fn insert(&self, texture: T) -> SnapshotOwner<T> {
        let mut inner = self.0.borrow_mut();
        let slot = match inner.free.pop() {
            Some(slot) => slot,
            None => {
                inner.slots.push(Slot {
                    generation: 0,
                    texture: None,
                    leases: 0,
                    owned: false,
                });
                inner.slots.len() - 1
            }
        };
        let s = &mut inner.slots[slot];
        s.texture = Some(texture);
        s.leases = 0;
        s.owned = true;
        let id = SnapshotId(ResourceKey {
            slot: slot as u64,
            generation: s.generation,
        });
        SnapshotOwner {
            store: self.clone(),
            id,
        }
    }

    /// The texture of `id` and a lease that keeps it until the lease ends. A retired or unknown handle is an error,
    /// also when its slot has been reused since.
    pub fn lease(&self, id: SnapshotId) -> Result<(T, SnapshotLease<T>), SnapshotError> {
        let mut inner = self.0.borrow_mut();
        let slot = inner
            .slots
            .get_mut(id.0.slot as usize)
            .filter(|s| s.generation == id.0.generation && s.owned)
            .ok_or(SnapshotError::Stale(id))?;
        let texture = slot.texture.clone().ok_or(SnapshotError::Stale(id))?;
        slot.leases += 1;
        Ok((
            texture,
            SnapshotLease {
                store: self.clone(),
                id,
            },
        ))
    }

    /// Pin the snapshot `id` without taking its texture: a lease that keeps it until the lease ends.
    pub fn pin(&self, id: SnapshotId) -> Result<SnapshotLease<T>, SnapshotError> {
        self.lease(id).map(|(_, lease)| lease)
    }

    /// How many snapshots are stored (not yet retired).
    pub fn live(&self) -> usize {
        self.0
            .borrow()
            .slots
            .iter()
            .filter(|s| s.texture.is_some())
            .count()
    }
}

impl<T> SnapshotLease<T> {
    pub fn id(&self) -> SnapshotId {
        self.id
    }
}

impl<T> SnapshotOwner<T> {
    pub fn id(&self) -> SnapshotId {
        self.id
    }
}

impl<T> Drop for SnapshotOwner<T> {
    fn drop(&mut self) {
        let mut inner = self.store.0.borrow_mut();
        let slot = self.id.0.slot as usize;
        inner.slots[slot].owned = false;
        inner.retire_if_unused(slot);
    }
}

impl<T> Drop for SnapshotLease<T> {
    fn drop(&mut self) {
        let mut inner = self.store.0.borrow_mut();
        let slot = self.id.0.slot as usize;
        inner.slots[slot].leases -= 1;
        inner.retire_if_unused(slot);
    }
}

impl<T> std::fmt::Debug for SnapshotOwner<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SnapshotOwner").field(&self.id).finish()
    }
}

impl<T> std::fmt::Debug for SnapshotLease<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SnapshotLease").field(&self.id).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_snapshot_survives_unmap_until_render_completion() {
        let store: Snapshots<u32> = Snapshots::default();
        let ghost = store.insert(7);
        let id = ghost.id();
        let (texture, lease) = store.lease(id).unwrap();
        assert_eq!(texture, 7);
        // the window is unmapped and its ghost is gone while the frame composed with it is still in flight
        drop(ghost);
        assert_eq!(store.live(), 1, "an outstanding lease keeps the snapshot");
        assert_eq!(store.lease(id).err(), Some(SnapshotError::Stale(id)));
        // the frame completes
        drop(lease);
        assert_eq!(store.live(), 0, "the last lease to end retires it");
    }

    #[test]
    fn a_snapshot_with_no_lease_retires_when_its_owner_goes() {
        let store: Snapshots<u32> = Snapshots::default();
        drop(store.insert(1));
        assert_eq!(store.live(), 0);
    }

    #[test]
    fn invalid_snapshot_handle_returns_error() {
        let store: Snapshots<u32> = Snapshots::default();
        let unknown = SnapshotId(ResourceKey {
            slot: 3,
            generation: 0,
        });
        assert_eq!(
            store.lease(unknown).err(),
            Some(SnapshotError::Stale(unknown))
        );
        let first = store.insert(1);
        let stale = first.id();
        drop(first);
        let second = store.insert(2);
        assert_eq!(second.id().0.slot, stale.0.slot, "the slot is reused");
        assert_ne!(second.id(), stale, "under a new generation");
        assert_eq!(store.lease(stale).err(), Some(SnapshotError::Stale(stale)));
        assert_eq!(store.lease(second.id()).unwrap().0, 2);
    }
}
