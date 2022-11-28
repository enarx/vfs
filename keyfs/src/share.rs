use std::any::Any;
use std::cmp::min;
use std::io::IoSliceMut;
use std::sync::Arc;

use wasi_common::file::{FdFlags, FileType, Filestat};
use wasi_common::{Error, ErrorExt, WasiDir, WasiFile};
use wasmtime_vfs_ledger::InodeId;
use wasmtime_vfs_memory::{Data, Inode, Link, Node, Open, State};

pub struct Share(Link<Vec<u8>>);

#[async_trait::async_trait]
impl Node for Share {
    fn to_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self
    }

    fn parent(&self) -> Option<Arc<dyn Node>> {
        self.0.parent.upgrade()
    }

    fn filetype(&self) -> FileType {
        FileType::SocketDgram
    }

    fn id(&self) -> Arc<InodeId> {
        self.0.inode.id.clone()
    }

    async fn open_dir(self: Arc<Self>) -> Result<Box<dyn WasiDir>, Error> {
        Err(Error::not_dir())
    }

    async fn open_file(
        self: Arc<Self>,
        _path: &str,
        dir: bool,
        read: bool,
        write: bool,
        flags: FdFlags,
    ) -> Result<Box<dyn WasiFile>, Error> {
        if dir {
            return Err(Error::not_dir());
        }

        if !read || write || !flags.is_empty() {
            return Err(Error::perm());
        }

        Ok(Box::new(OpenShare(Open {
            root: self.root(),
            link: self,
            state: State::from(flags).into(),
            write,
            read,
        })))
    }
}

impl Share {
    pub fn new(parent: Arc<dyn Node>, data: impl Into<Vec<u8>>) -> Arc<Self> {
        let id = parent.id().device().create_inode();

        let inode = Inode {
            data: Data::from(data.into()).into(),
            id,
        };

        Arc::new(Self(Link {
            parent: Arc::downgrade(&parent),
            inode: inode.into(),
        }))
    }
}

struct OpenShare(Open<Share>);

#[async_trait::async_trait]
impl WasiFile for OpenShare {
    fn as_any(&self) -> &dyn Any {
        self
    }

    async fn get_filetype(&mut self) -> Result<FileType, Error> {
        Ok(FileType::SocketDgram)
    }

    async fn get_filestat(&mut self) -> Result<Filestat, Error> {
        let ilock = self.0.link.0.inode.data.read().await;

        Ok(Filestat {
            device_id: **self.0.link.0.inode.id.device(),
            inode: **self.0.link.0.inode.id,
            filetype: FileType::SocketDgram,
            nlink: Arc::strong_count(&self.0.link.0.inode) as u64,
            size: ilock.content.len() as u64,
            atim: Some(ilock.access),
            mtim: Some(ilock.modify),
            ctim: Some(ilock.create),
        })
    }

    async fn read_vectored<'a>(&mut self, bufs: &mut [IoSliceMut<'a>]) -> Result<u64, Error> {
        let ilock = self.0.link.0.inode.data.read().await;

        if ilock.content.len() > bufs.iter().map(|x| x.len()).sum() {
            return Err(Error::too_big());
        }

        let mut total = 0;

        for buf in bufs {
            let len = min(buf.len(), ilock.content.len() - total);
            buf[..len].copy_from_slice(&ilock.content[total..][..len]);
            total += len;
        }

        Ok(total.try_into()?)
    }

    async fn num_ready_bytes(&self) -> Result<u64, Error> {
        let ilock = self.0.link.0.inode.data.read().await;
        Ok(ilock.content.len() as u64)
    }

    async fn readable(&self) -> Result<(), Error> {
        Ok(())
    }
}
