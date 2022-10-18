use std::any::Any;
use std::collections::BTreeMap;
use std::sync::Arc;

use wasi_common::dir::{ReaddirCursor, ReaddirEntity};
use wasi_common::file::{FdFlags, FileType};
use wasi_common::{Error, ErrorExt, WasiDir, WasiFile};

use crate::link::Link;
use crate::open::Open;

#[async_trait::async_trait]
pub trait Node: 'static + Send + Sync {
    fn parent(&self) -> Option<Arc<dyn Node>>;
    fn as_any(&self) -> &dyn Any;

    fn entity(&self, name: String, next: ReaddirCursor) -> ReaddirEntity;

    async fn open_dir(self: Arc<Self>) -> Result<Box<dyn WasiDir>, Error>;

    async fn open_file(
        self: Arc<Self>,
        dir: bool,
        read: bool,
        write: bool,
        flags: FdFlags,
    ) -> Result<Box<dyn WasiFile>, Error>;

    fn root(self: Arc<Self>) -> Arc<dyn Node>
    where
        Self: Sized,
    {
        let mut root: Arc<dyn Node> = self;

        while let Some(parent) = root.parent() {
            root = parent;
        }

        root
    }
}

#[async_trait::async_trait]
impl Node for Link<BTreeMap<String, Arc<dyn Node>>> {
    fn parent(&self) -> Option<Arc<dyn Node>> {
        self.parent.upgrade()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn entity(&self, name: String, next: ReaddirCursor) -> ReaddirEntity {
        ReaddirEntity {
            name,
            filetype: FileType::Directory,
            inode: **self.inode.id,
            next,
        }
    }

    async fn open_dir(self: Arc<Self>) -> Result<Box<dyn WasiDir>, Error> {
        Ok(Open::dir(self))
    }

    async fn open_file(
        self: Arc<Self>,
        _dir: bool,
        _read: bool,
        _write: bool,
        _flags: FdFlags,
    ) -> Result<Box<dyn WasiFile>, Error> {
        Err(Error::io()) // FIXME
    }
}

#[async_trait::async_trait]
impl Node for Link<Vec<u8>> {
    fn parent(&self) -> Option<Arc<dyn Node>> {
        self.parent.upgrade()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn entity(&self, name: String, next: ReaddirCursor) -> ReaddirEntity {
        ReaddirEntity {
            name,
            filetype: FileType::RegularFile,
            inode: **self.inode.id,
            next,
        }
    }

    async fn open_dir(self: Arc<Self>) -> Result<Box<dyn WasiDir>, Error> {
        Err(Error::not_dir())
    }

    async fn open_file(
        self: Arc<Self>,
        dir: bool,
        read: bool,
        write: bool,
        flags: FdFlags,
    ) -> Result<Box<dyn WasiFile>, Error> {
        if dir {
            return Err(Error::not_dir());
        }

        Ok(Open::file(self, read, write, flags))
    }
}
