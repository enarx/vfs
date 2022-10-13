use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use tokio::sync::RwLock;
use wasi_common::file::FileType;
use wasi_common::{Error, SystemTimeSpec};
use wasmtime_vfs_ledger::InodeId;

use super::link::Link;

pub enum Data {
    Directory(BTreeMap<String, Arc<Link>>),
    File(Vec<u8>),
}

impl Data {
    pub fn filetype(&self) -> FileType {
        match self {
            Self::Directory(..) => FileType::Directory,
            Self::File(..) => FileType::RegularFile,
        }
    }
}

pub struct Meta {
    pub create: SystemTime,
    pub access: SystemTime,
    pub modify: SystemTime,
}

impl Default for Meta {
    fn default() -> Self {
        let now = SystemTime::now();

        Self {
            create: now,
            access: now,
            modify: now,
        }
    }
}

pub struct Body {
    pub meta: Meta,
    pub data: Data,
}

impl From<Data> for Body {
    fn from(data: Data) -> Self {
        Self {
            meta: Meta::default(),
            data,
        }
    }
}

pub struct Inode {
    pub body: RwLock<Body>,
    pub id: InodeId,
}

impl Inode {
    // Update the timestamps of this inode.
    pub async fn update(
        &self,
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

        // Lock the inode body.
        let mut lock = self.body.write().await;

        // Set the access time if requested.
        if let Some(atime) = atime {
            lock.meta.access = match atime {
                SystemTimeSpec::SymbolicNow => now.unwrap(),
                SystemTimeSpec::Absolute(time) => time.into_std(),
            };
        }

        // Set the modification time if requested.
        if let Some(mtime) = mtime {
            lock.meta.modify = match mtime {
                SystemTimeSpec::SymbolicNow => now.unwrap(),
                SystemTimeSpec::Absolute(time) => time.into_std(),
            };
        }

        Ok(())
    }
}
