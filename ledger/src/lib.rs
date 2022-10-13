use std::collections::BTreeSet;
use std::ops::{Deref, Range};
use std::sync::{Arc, Mutex};

/// A ledger of filesystem devices.
pub struct Ledger(Mutex<(BTreeSet<u64>, Range<u64>)>);

impl Ledger {
    /// Create a new ledger.
    pub fn new() -> Arc<Ledger> {
        Arc::new(Self(Mutex::new((BTreeSet::new(), 0..u64::MAX))))
    }

    /// Allocate a new device.
    pub fn create_device(self: Arc<Self>) -> Arc<DeviceId> {
        Arc::new(DeviceId {
            id: {
                let (free, next) = &mut *self.0.lock().unwrap();
                let id = free.iter().cloned().chain(next).next().expect("out of ids");
                free.remove(&id);
                id
            },
            inodes: Mutex::new((BTreeSet::new(), 0..u64::MAX)),
            devices: self,
        })
    }
}

/// A filesystem device identifier.
pub struct DeviceId {
    devices: Arc<Ledger>,
    inodes: Mutex<(BTreeSet<u64>, Range<u64>)>,
    id: u64,
}

impl Drop for DeviceId {
    fn drop(&mut self) {
        self.devices.0.lock().unwrap().0.insert(self.id);
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
    pub fn create_inode(self: Arc<Self>) -> InodeId {
        InodeId {
            id: {
                let (free, next) = &mut *self.inodes.lock().unwrap();
                let id = free.iter().cloned().chain(next).next().expect("out of ids");
                free.remove(&id);
                id
            },
            device: self,
        }
    }
}

/// A filesystem inode identifier.
pub struct InodeId {
    device: Arc<DeviceId>,
    id: u64,
}

impl Drop for InodeId {
    fn drop(&mut self) {
        self.device.inodes.lock().unwrap().0.insert(self.id);
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
