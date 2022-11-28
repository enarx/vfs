use std::any::Any;
use std::io::IoSlice;
use std::marker::PhantomData;
use std::sync::Arc;

use digest::Digest;
use signature::{DigestVerifier, Signature};
use wasi_common::file::{FdFlags, FileType, Filestat, SiFlags};
use wasi_common::{Error, ErrorExt, ErrorKind, WasiDir, WasiFile};
use wasmtime_vfs_ledger::InodeId;
use wasmtime_vfs_memory::{Data, Inode, Link, Node};

struct VerifyingKey<K, D, S> {
    ignore: PhantomData<S>,
    digest: PhantomData<D>,
    public: Arc<K>,
}

pub struct Verify<K, D, S>(Link<VerifyingKey<K, D, S>>);

#[async_trait::async_trait]
impl<K, D, S> Node for Verify<K, D, S>
where
    K: DigestVerifier<D, S> + Send + Sync + 'static,
    D: Digest + Clone + Send + Sync + 'static,
    S: Signature + Send + Sync + 'static,
{
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

        if read || !write {
            return Err(Error::perm()); // FIXME: errno
        }

        if !flags.is_empty() {
            return Err(Error::invalid_argument()); // FIXME: errno
        }

        Ok(Box::new(OpenVerify {
            _root: self.root(),
            link: self,
            hash: D::new(),
        }))
    }
}

impl<K, D, S> Verify<K, D, S> {
    pub fn new(parent: Arc<dyn Node>, key: impl Into<Arc<K>>) -> Arc<Self> {
        let id = parent.id().device().create_inode();

        let key = VerifyingKey {
            ignore: PhantomData,
            digest: PhantomData,
            public: key.into(),
        };

        let inode = Inode {
            data: Data::from(key).into(),
            id,
        };

        Arc::new(Self(Link {
            parent: Arc::downgrade(&parent),
            inode: inode.into(),
        }))
    }
}

struct OpenVerify<K, D, S> {
    _root: Arc<dyn Node>,
    link: Arc<Verify<K, D, S>>,
    hash: D,
}

#[async_trait::async_trait]
impl<K, D, S> WasiFile for OpenVerify<K, D, S>
where
    K: DigestVerifier<D, S> + Send + Sync + 'static,
    D: Digest + Clone + Send + Sync + 'static,
    S: Signature + Send + Sync + 'static,
{
    fn as_any(&self) -> &dyn Any {
        self
    }

    async fn get_filetype(&mut self) -> Result<FileType, Error> {
        Ok(FileType::SocketDgram)
    }

    async fn get_filestat(&mut self) -> Result<Filestat, Error> {
        let ilock = self.link.0.inode.data.read().await;

        Ok(Filestat {
            device_id: **self.link.0.inode.id.device(),
            inode: **self.link.0.inode.id,
            filetype: FileType::SocketDgram,
            nlink: Arc::strong_count(&self.link.0.inode) as u64,
            size: 0,
            atim: Some(ilock.access),
            mtim: Some(ilock.modify),
            ctim: Some(ilock.create),
        })
    }

    async fn sock_send<'a>(
        &mut self,
        bufs: &[std::io::IoSlice<'a>],
        _flags: SiFlags,
    ) -> Result<u64, Error> {
        self.write_vectored(bufs).await
    }

    async fn write_vectored<'a>(&mut self, bufs: &[IoSlice<'a>]) -> Result<u64, Error> {
        let mut total = 0;

        for buf in bufs {
            self.hash.update(buf.as_ref());
            total += buf.len();
        }

        Ok(total as u64)
    }

    async fn write_vectored_at<'a>(
        &mut self,
        bufs: &[IoSlice<'a>],
        offset: u64,
    ) -> Result<u64, Error> {
        if offset != u64::MAX {
            return Err(Error::invalid_argument());
        }

        if bufs.len() != 1 {
            return Err(Error::invalid_argument());
        }

        let hash = self.hash.clone();
        let sig = S::from_bytes(bufs[0].as_ref()).map_err(|_| Error::invalid_argument())?;

        let ilock = self.link.0.inode.data.read().await;
        match ilock.content.public.verify_digest(hash, &sig) {
            Ok(()) => Ok(bufs[0].len() as u64),
            Err(_) => Err(ErrorKind::Ilseq.into()),
        }
    }

    async fn writable(&self) -> Result<(), Error> {
        Ok(())
    }
}
