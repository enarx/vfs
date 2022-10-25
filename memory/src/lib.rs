use std::sync::Weak;
use std::time::SystemTime;
use std::{any::Any, sync::Arc};

use tokio::sync::RwLock;
use wasi_common::file::{FdFlags, FileType};
use wasi_common::{Error, SystemTimeSpec, WasiDir, WasiFile};
use wasmtime_vfs_ledger::InodeId;

#[async_trait::async_trait]
pub trait Node: 'static + Any + Send + Sync {
    fn to_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync>;
    fn parent(&self) -> Option<Arc<dyn Node>>;
    fn filetype(&self) -> FileType;
    fn id(&self) -> Arc<InodeId>;

    async fn open_dir(self: Arc<Self>) -> Result<Box<dyn WasiDir>, Error>;

    async fn open_file(
        self: Arc<Self>,
        dir: bool,
        read: bool,
        write: bool,
        flags: FdFlags,
    ) -> Result<Box<dyn WasiFile>, Error>;

    fn root(self: &Arc<Self>) -> Arc<dyn Node>
    where
        Self: Sized,
    {
        let mut root: Arc<dyn Node> = self.clone();

        while let Some(parent) = root.parent() {
            root = parent;
        }

        root
    }
}

pub struct Data<T> {
    pub create: SystemTime,
    pub access: SystemTime,
    pub modify: SystemTime,
    pub content: T,
}

pub struct Inode<T> {
    pub data: RwLock<Data<T>>,
    pub id: Arc<InodeId>,
}

pub struct Link<T> {
    pub parent: Weak<dyn Node>,
    pub inode: Arc<Inode<T>>,
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

impl<T: Default> From<Arc<InodeId>> for Inode<T> {
    fn from(id: Arc<InodeId>) -> Self {
        let data = Data::default().into();
        Self { data, id }
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
