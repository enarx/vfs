use std::sync::Arc;
use std::time::SystemTime;

use tokio::sync::RwLock;
use wasi_common::{Error, SystemTimeSpec};
use wasmtime_vfs_ledger::InodeId;

pub struct Data<T> {
    pub create: SystemTime,
    pub access: SystemTime,
    pub modify: SystemTime,
    pub content: T,
}

impl<T: Default> Default for Data<T> {
    fn default() -> Self {
        let now = SystemTime::now();

        Self {
            create: now,
            access: now,
            modify: now,
            content: T::default(),
        }
    }
}

impl<T> Data<T> {
    // Update the timestamps of this inode.
    pub fn set_times(
        &mut self,
        atime: impl Into<Option<SystemTimeSpec>>,
        mtime: impl Into<Option<SystemTimeSpec>>,
    ) -> Result<(), Error> {
        let atime = atime.into();
        let mtime = mtime.into();

        // If either input wants the current time, get it.
        let now = match (&atime, &mtime) {
            (Some(SystemTimeSpec::SymbolicNow), _) => Some(SystemTime::now()),
            (_, Some(SystemTimeSpec::SymbolicNow)) => Some(SystemTime::now()),
            _ => None,
        };

        // Set the access time if requested.
        if let Some(atime) = atime {
            self.access = match atime {
                SystemTimeSpec::SymbolicNow => now.unwrap(),
                SystemTimeSpec::Absolute(time) => time.into_std(),
            };
        }

        // Set the modification time if requested.
        if let Some(mtime) = mtime {
            self.modify = match mtime {
                SystemTimeSpec::SymbolicNow => now.unwrap(),
                SystemTimeSpec::Absolute(time) => time.into_std(),
            };
        }

        Ok(())
    }
}

pub struct Inode<T> {
    pub data: RwLock<Data<T>>,
    pub id: Arc<InodeId>,
}
