use std::collections::BTreeSet;
use std::ops::{Deref, Range};
use std::sync::{Arc, Mutex};

/// A ledger of filesystem devices.
#[derive(Debug)]
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

#[test]
fn ledger_create_device() {
    let ledger = Ledger::new();
    let mut set = BTreeSet::new();

    let device_id = {
        let device_id = ledger.clone().create_device();
        assert_eq!(device_id.id, 0);
        assert_eq!(device_id.inodes.lock().unwrap().0.len(), 0);
        assert_eq!(device_id.devices.0.lock().unwrap().0, set);
        device_id.id
    };

    set.insert(device_id);
    assert_eq!(ledger.0.lock().unwrap().0, set);
}

/// A filesystem device identifier.
#[derive(Debug)]
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

#[test]
fn device_id_deref() {
    let device = Ledger::new().create_device();
    assert_eq!(**device, 0);
}

impl Eq for DeviceId {}
impl PartialEq for DeviceId {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

#[test]
fn device_id_partial_eq() {
    let ledger = Ledger::new();
    let device_0 = ledger.clone().create_device();
    let device_1 = ledger.create_device();
    assert_eq!(device_0.id, 0);
    assert_eq!(device_1.id, 1);
    assert_eq!(device_0, device_0);
    assert_ne!(device_0, device_1);
    assert_eq!(device_1, device_1);
}

impl DeviceId {
    /// Get a reference to the ledger.
    pub fn ledger(&self) -> Arc<Ledger> {
        self.devices.clone()
    }

    /// Allocate a new inode.
    pub fn create_inode(self: Arc<Self>) -> Arc<InodeId> {
        Arc::new(InodeId {
            id: {
                let (free, next) = &mut *self.inodes.lock().unwrap();
                let id = free.iter().cloned().chain(next).next().expect("out of ids");
                free.remove(&id);
                id
            },
            device: self,
        })
    }
}

#[test]
fn device_id_ledger() {
    let device = Ledger::new().create_device();
    let _: Arc<Ledger> = device.ledger();
}

#[test]
fn device_id_create_inode() {
    let device = Ledger::new().create_device();
    assert_eq!(device.inodes.lock().unwrap().0.len(), 0);
    let mut set = BTreeSet::new();
    set.insert(device.clone().create_inode().id);
    assert_eq!(device.inodes.lock().unwrap().0, set);
}

/// A filesystem inode identifier.
#[derive(Debug)]
pub struct InodeId {
    device: Arc<DeviceId>,
    id: u64,
}

#[test]
fn inode_id_debug() {
    let inode_id = Ledger::new().create_device().create_inode();
    // TODO: consider testing this more
    format!("{:?}", inode_id);
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

#[test]
fn inode_id_partial_eq() {
    let ledger = Ledger::new();
    let device_0 = ledger.clone().create_device();
    let device_1 = ledger.create_device();
    let d0i0 = device_0.clone().create_inode();
    let d0i1 = device_0.create_inode();
    let d1i0 = device_1.create_inode();
    assert_eq!((d0i0.device().id, d0i0.id), (0, 0));
    assert_eq!((d0i1.device().id, d0i1.id), (0, 1));
    assert_eq!((d1i0.device().id, d1i0.id), (1, 0));
    assert_eq!(d0i0, d0i0);
    assert_ne!(d0i1, d0i0);
    assert_ne!(d1i0, d0i0);
}

impl InodeId {
    /// Get a reference to the device.
    pub fn device(&self) -> Arc<DeviceId> {
        self.device.clone()
    }
}

#[test]
fn inode_id_device() {
    let device_id = Ledger::new().create_device();
    assert_eq!(device_id.create_inode().device().id, 0);
}
