use std::collections::BTreeSet;
use std::ops::{Deref, Range};
use std::sync::{Arc, Mutex};

/// A potentially infinite stream of unique `u64` ids.
///
/// You can call `.next()` to allocate a new identifier. This will return
/// `None` if the stream is exhausted. However, unused identifiers can be
/// returned to the stream with `.free()` and will be reused.
struct Reusable {
    // A set of all free, discontiguous identifiers.
    free: BTreeSet<u64>,

    // A set of all free, contiguous identifiers.
    next: Range<u64>,
}

impl Default for Reusable {
    fn default() -> Self {
        Reusable {
            free: BTreeSet::new(),
            next: 0..u64::MAX,
        }
    }
}

impl Iterator for Reusable {
    type Item = u64;

    fn next(&mut self) -> Option<Self::Item> {
        // Try to reuse an identifier from the discontiguous set.
        // Fall back to allocating from the contiguous range.
        self.free.pop_first().or_else(|| self.next.next())
    }
}

impl Reusable {
    fn free(&mut self, id: u64) {
        // Detect double-free conditions.
        debug_assert!(id < self.next.start);
        debug_assert!(!self.free.contains(&id));

        // Insert the freed id into the discontiguous set.
        self.free.insert(id);

        // Attempt to move the discontiguous set into the contiguous range.
        while self.next.start > 0 {
            let prev = self.next.start - 1;
            if !self.free.remove(&prev) {
                break;
            }

            self.next.start = prev;
        }
    }
}

/// A ledger of filesystem devices.
pub struct Ledger(Mutex<Reusable>);

impl Ledger {
    /// Create a new ledger.
    pub fn new() -> Arc<Ledger> {
        Arc::new(Ledger(Default::default()))
    }

    /// Allocate a new device.
    pub fn create_device(self: Arc<Self>) -> Arc<DeviceId> {
        let id = self.0.lock().unwrap().next().expect("out of devices");
        Arc::new(DeviceId {
            id,
            inodes: Default::default(),
            devices: self,
        })
    }
}

/// A filesystem device identifier.
pub struct DeviceId {
    devices: Arc<Ledger>,
    inodes: Mutex<Reusable>,
    id: u64,
}

impl Drop for DeviceId {
    fn drop(&mut self) {
        self.devices.0.lock().unwrap().free(self.id);
    }
}

impl Deref for DeviceId {
    type Target = u64;

    fn deref(&self) -> &Self::Target {
        &self.id
    }
}

impl Eq for DeviceId {}
impl PartialEq for DeviceId {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl DeviceId {
    /// Get a reference to the ledger.
    pub fn ledger(&self) -> Arc<Ledger> {
        self.devices.clone()
    }

    /// Allocate a new inode.
    pub fn create_inode(self: Arc<Self>) -> Arc<InodeId> {
        let id = self.inodes.lock().unwrap().next().expect("out of inodes");
        Arc::new(InodeId { id, device: self })
    }
}

/// A filesystem inode identifier.
pub struct InodeId {
    device: Arc<DeviceId>,
    id: u64,
}

impl Drop for InodeId {
    fn drop(&mut self) {
        self.device.inodes.lock().unwrap().free(self.id);
    }
}

impl Deref for InodeId {
    type Target = u64;

    fn deref(&self) -> &Self::Target {
        &self.id
    }
}

impl Eq for InodeId {}
impl PartialEq for InodeId {
    fn eq(&self, other: &Self) -> bool {
        self.device == other.device && self.id == other.id
    }
}

impl InodeId {
    /// Get a reference to the device.
    pub fn device(&self) -> Arc<DeviceId> {
        self.device.clone()
    }
}

#[cfg(test)]
mod test {
    use crate::Ledger;

    #[test]
    fn reuse() {
        // Test the first inode number.
        let inode00 = Ledger::new().create_device().create_inode();
        assert_eq!(**inode00.device(), 0);
        assert_eq!(**inode00, 0);

        // Test the second inode number.
        let inode01 = inode00.device().create_inode();
        assert_eq!(**inode01.device(), 0);
        assert_eq!(**inode01, 1);

        // Test the first inode on a new device.
        let inode10 = inode00.device().ledger().create_device().create_inode();
        assert_eq!(**inode10.device(), 1);
        assert_eq!(**inode10, 0);

        // Test the third inode number.
        let inode02 = inode00.device().create_inode();
        assert_eq!(**inode02.device(), 0);
        assert_eq!(**inode02, 2);

        // Test the second inode on a new device.
        let inode11 = inode10.device().create_inode();
        assert_eq!(**inode11.device(), 1);
        assert_eq!(**inode11, 1);

        // Test the third inode on a new device.
        let inode12 = inode11.device().create_inode();
        assert_eq!(**inode12.device(), 1);
        assert_eq!(**inode12, 2);

        drop(inode01);
        drop(inode12);

        // Test inode reuse.
        let inode01 = inode00.device().create_inode();
        assert_eq!(**inode01.device(), 0);
        assert_eq!(**inode01, 1);

        // Test inode reuse on a new device.
        let inode12 = inode10.device().create_inode();
        assert_eq!(**inode12.device(), 1);
        assert_eq!(**inode12, 2);

        drop(inode00);
        drop(inode01);
        drop(inode02);

        // Test device reuse.
        let inode00 = inode10.device().ledger().create_device().create_inode();
        assert_eq!(**inode00.device(), 0);
        assert_eq!(**inode00, 0);
    }
}
