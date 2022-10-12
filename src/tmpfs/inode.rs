use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use tokio::sync::RwLock;
use wasi_common::file::FileType;
use wasi_common::{Error, SystemTimeSpec};

use super::link::Link;
use crate::InodeId;

pub enum Data {
    Directory(BTreeMap<String, Arc<Link>>),
    File(Vec<u8>),
}

impl From<BTreeMap<String, Arc<Link>>> for Data {
    fn from(directory: BTreeMap<String, Arc<Link>>) -> Self {
        Self::Directory(directory)
    }
}

impl From<Vec<u8>> for Data {
    fn from(file: Vec<u8>) -> Self {
        Self::File(file)
    }
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
    pub ctime: SystemTime,
    pub atime: SystemTime,
    pub mtime: SystemTime,
}

impl Default for Meta {
    fn default() -> Self {
        let now = SystemTime::now();

        Self {
            ctime: now,
            atime: now,
            mtime: now,
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

impl From<BTreeMap<String, Arc<Link>>> for Body {
    fn from(directory: BTreeMap<String, Arc<Link>>) -> Self {
        Data::from(directory).into()
    }
}

impl From<Vec<u8>> for Body {
    fn from(file: Vec<u8>) -> Self {
        Data::from(file).into()
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
            lock.meta.atime = match atime {
                SystemTimeSpec::SymbolicNow => now.unwrap(),
                SystemTimeSpec::Absolute(time) => time.into_std(),
            };
        }

        // Set the modification time if requested.
        if let Some(mtime) = mtime {
            lock.meta.mtime = match mtime {
                SystemTimeSpec::SymbolicNow => now.unwrap(),
                SystemTimeSpec::Absolute(time) => time.into_std(),
            };
        }

        Ok(())
    }
}
