use std::any::Any;
use std::collections::BTreeMap;
use std::io::{IoSlice, IoSliceMut, SeekFrom};
use std::ops::{Deref, DerefMut};
use std::path::{PathBuf, MAIN_SEPARATOR as SEP};
use std::sync::{Arc, Weak};

use wasi_common::dir::{ReaddirCursor, ReaddirEntity};
use wasi_common::file::{Advice, FdFlags, FileType, Filestat, OFlags};
use wasi_common::{Error, ErrorExt, SystemTimeSpec, WasiDir, WasiFile};
use wasmtime_vfs_ledger::{InodeId, Ledger};
use wasmtime_vfs_memory::{Inode, Link, Node};

use super::{File, Open, State};

pub struct Directory(Link<BTreeMap<String, Arc<dyn Node>>>);

impl Deref for Directory {
    type Target = Link<BTreeMap<String, Arc<dyn Node>>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Directory {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Directory {
    fn here(self: &Arc<Self>) -> Arc<dyn Node> {
        self.clone()
    }

    fn prev(self: &Arc<Self>) -> Arc<dyn Node> {
        match self.parent.upgrade() {
            Some(parent) => parent,
            None => self.here(),
        }
    }

    pub(crate) fn root(ledger: Arc<Ledger>) -> Arc<Self> {
        Arc::new(Self(Link {
            parent: Weak::<Self>::new(),
            inode: Arc::new(Inode::from(ledger.create_device().create_inode())),
        }))
    }

    pub(crate) fn new(parent: Arc<dyn Node>) -> Arc<Self> {
        let id = parent.id().device().create_inode();
        Arc::new(Self(Link {
            parent: Arc::downgrade(&parent),
            inode: Inode::from(id).into(),
        }))
    }
}

#[async_trait::async_trait]
impl Node for Directory {
    fn parent(&self) -> Option<Arc<dyn Node>> {
        self.parent.upgrade()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn id(&self) -> Arc<InodeId> {
        self.inode.id.clone()
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
        Ok(Box::new(Open {
            _root: self.root(),
            link: self,
            state: State::default().into(),
            write: false,
            read: false,
        }))
    }

    async fn open_file(
        self: Arc<Self>,
        _dir: bool,
        read: bool,
        write: bool,
        flags: FdFlags,
    ) -> Result<Box<dyn WasiFile>, Error> {
        Ok(Box::new(Open {
            _root: self.root(),
            link: self,
            state: State::from(flags).into(),
            write,
            read,
        }))
    }
}

#[async_trait::async_trait]
impl WasiDir for Open<Directory> {
    fn as_any(&self) -> &dyn Any {
        self
    }

    async fn open_file(
        &self,
        follow: bool,
        path: &str,
        oflags: OFlags,
        read: bool,
        write: bool,
        flags: FdFlags,
    ) -> Result<Box<dyn WasiFile>, Error> {
        const VALID_OFLAGS: &[u32] = &[
            OFlags::empty().bits(),
            OFlags::CREATE.bits(),
            OFlags::DIRECTORY.bits(),
            OFlags::TRUNCATE.bits(),
            OFlags::CREATE.bits() | OFlags::DIRECTORY.bits(),
            OFlags::CREATE.bits() | OFlags::EXCLUSIVE.bits(),
            OFlags::CREATE.bits() | OFlags::TRUNCATE.bits(),
            OFlags::CREATE.bits() | OFlags::DIRECTORY.bits() | OFlags::EXCLUSIVE.bits(),
        ];

        // Descend into the path.
        if let Some((lhs, rhs)) = path.split_once(SEP) {
            let child = self.open_dir(follow, lhs).await?;
            return child
                .open_file(follow, rhs, oflags, read, write, flags)
                .await;
        }

        // Check the validity of the flags.
        if !VALID_OFLAGS.contains(&oflags.bits()) {
            return Err(Error::invalid_argument());
        }

        // Truncate can only be used with write.
        if oflags.contains(OFlags::TRUNCATE) && !write {
            return Err(Error::invalid_argument()); // FIXME
        }

        let odir = oflags.contains(OFlags::DIRECTORY);

        // Find or create the child.
        match path {
            "." if oflags.contains(OFlags::EXCLUSIVE) => Err(Error::exist()),
            "." if oflags.contains(OFlags::TRUNCATE) => Err(Error::io()), // FIXME
            "." | "" => self.link.here().open_file(odir, read, write, flags).await,

            ".." if oflags.contains(OFlags::EXCLUSIVE) => Err(Error::exist()),
            ".." if oflags.contains(OFlags::TRUNCATE) => Err(Error::io()), // FIXME
            ".." => self.link.prev().open_file(odir, read, write, flags).await,

            name => {
                let mut ilock = self.link.inode.data.write().await;
                let child = ilock.content.get(name).cloned();
                match (child, oflags.contains(OFlags::CREATE)) {
                    // If the file exists and we're creating it, then we have an error.
                    (Some(_), true) if oflags.contains(OFlags::EXCLUSIVE) => Err(Error::exist()),

                    // If the file doesn't exist and we're not creating it, then we have an error.
                    (None, false) => Err(Error::not_found()),

                    // If the file doesn't exist, create it.
                    (None, true) => {
                        let child: Arc<dyn Node> = match oflags.contains(OFlags::DIRECTORY) {
                            true => Directory::new(self.link.clone()),
                            false => File::new(self.link.clone()),
                        };

                        ilock.content.insert(name.into(), child.clone());
                        child.open_file(odir, read, write, flags).await
                    }

                    // Truncate the file.
                    (Some(child), _) if oflags.contains(OFlags::TRUNCATE) => {
                        let mut open = child.open_file(odir, false, true, FdFlags::empty()).await?;
                        open.set_filestat_size(0).await?;
                        Ok(open)
                    }

                    // Open the file.
                    (Some(child), _) => child.open_file(odir, read, write, flags).await,
                }
            }
        }
    }

    async fn open_dir(&self, follow: bool, path: &str) -> Result<Box<dyn WasiDir>, Error> {
        if let Some((lhs, rhs)) = path.split_once(SEP) {
            let child = self.open_dir(follow, lhs).await?;
            return child.open_dir(follow, rhs).await;
        }

        match path {
            "" => Err(Error::invalid_argument()),
            "." => self.link.here().open_dir().await,
            ".." => self.link.prev().open_dir().await,

            name => {
                let ilock = self.link.inode.data.read().await;
                let child = ilock.content.get(name).ok_or_else(Error::not_found)?;
                child.clone().open_dir().await
            }
        }
    }

    async fn create_dir(&self, path: &str) -> Result<(), Error> {
        if let Some((lhs, rhs)) = path.split_once(SEP) {
            let child = self.open_dir(true, lhs).await?;
            return child.create_dir(rhs).await;
        }

        match path {
            "" | "." | ".." => Err(Error::invalid_argument()),
            name => {
                let mut ilock = self.link.inode.data.write().await;
                match ilock.content.contains_key(name) {
                    true => Err(Error::exist()),
                    false => {
                        let child = Directory::new(self.link.clone());
                        ilock.content.insert(name.into(), child);
                        Ok(())
                    }
                }
            }
        }
    }

    async fn readdir(
        &self,
        cursor: ReaddirCursor,
    ) -> Result<Box<dyn Iterator<Item = Result<ReaddirEntity, Error>> + Send>, Error> {
        let cursor: usize = u64::from(cursor)
            .try_into()
            .map_err(|_| Error::invalid_argument())?;

        // Get the directory reference.
        let ilock = self.link.inode.data.read().await;

        // Add the single dot entries.
        let mut entries = vec![
            Ok(self.link.here().entity(".".into(), 1.into())),
            Ok(self.link.prev().entity("..".into(), 2.into())),
        ];

        // Add all of the child entries.
        for (k, v) in ilock.content.iter() {
            let next = entries.len() as u64 + 1;
            entries.push(Ok(v.entity(k.clone(), next.into())));
        }

        Ok(Box::new(entries.into_iter().skip(cursor)))
    }

    async fn symlink(&self, old_path: &str, new_path: &str) -> Result<(), Error> {
        if let Some((lhs, rhs)) = new_path.split_once(SEP) {
            let child = self.open_dir(true, lhs).await?;
            return child.symlink(old_path, rhs).await;
        }

        Err(Error::not_supported())
    }

    // Some notes on this code are in order.
    //
    // POSIX requires that a directory be empty before it can be removed.
    // However, we cannot do this check across filesystem boundaries without
    // a race condition. Even if we could, we probably shouldn't because the
    // behavior would be odd. Therefore, we only remove child directories if
    // the child is also a `tmpfs` AND has the same device id.
    async fn remove_dir(&self, path: &str) -> Result<(), Error> {
        if let Some((lhs, rhs)) = path.split_once(SEP) {
            let child = self.open_dir(true, lhs).await?;
            return child.remove_dir(rhs).await;
        }

        match path {
            "" | "." | ".." => Err(Error::invalid_argument()),

            name => {
                let mut plock = self.link.inode.data.write().await;

                {
                    let cnode = plock.content.get(name).ok_or_else(Error::not_found)?;

                    let clink = cnode
                        .as_any()
                        .downcast_ref::<Link<Vec<u8>>>()
                        .ok_or_else(Error::io)?; // FIXME: ENOTFILE?
                    if self.link.inode.id.device() != clink.inode.id.device() {
                        return Err(Error::io()); // FIXME: EXDEV?
                    }

                    let clock = clink.inode.data.read().await;
                    if clock.content.is_empty() {
                        return Err(Error::io()); // FIXME: ENOTEMPTY
                    }
                }

                plock.content.remove(name);
                Ok(())
            }
        }
    }

    // The same comments for `remove_dir` apply here.
    async fn unlink_file(&self, path: &str) -> Result<(), Error> {
        if let Some((lhs, rhs)) = path.split_once(SEP) {
            let child = self.open_dir(true, lhs).await?;
            return child.unlink_file(rhs).await;
        }

        match path {
            "" | "." | ".." => Err(Error::invalid_argument()),

            name => {
                let mut plock = self.link.inode.data.write().await;
                let cnode = plock.content.get(name).ok_or_else(Error::not_found)?;

                let clink = cnode
                    .as_any()
                    .downcast_ref::<Link<Vec<u8>>>()
                    .ok_or_else(Error::io)?; // FIXME: ENOTFILE?
                if self.link.inode.id.device() != clink.inode.id.device() {
                    return Err(Error::io()); // FIXME: EXDEV?
                }

                plock.content.remove(name);
                Ok(())
            }
        }
    }

    async fn read_link(&self, path: &str) -> Result<PathBuf, Error> {
        if let Some((lhs, rhs)) = path.split_once(SEP) {
            let child = self.open_dir(true, lhs).await?;
            return child.read_link(rhs).await;
        }

        Err(Error::not_supported())
    }

    async fn get_filestat(&self) -> Result<Filestat, Error> {
        let ilock = self.link.inode.data.read().await;

        Ok(Filestat {
            device_id: **self.link.inode.id.device(),
            inode: **self.link.inode.id,
            filetype: FileType::RegularFile,
            nlink: Arc::strong_count(&self.link.inode) as u64 * 2,
            size: 0, // FIXME
            atim: Some(ilock.access),
            mtim: Some(ilock.modify),
            ctim: Some(ilock.create),
        })
    }

    async fn get_path_filestat(&self, path: &str, follow: bool) -> Result<Filestat, Error> {
        if let Some((lhs, rhs)) = path.split_once(SEP) {
            let child = self.open_dir(true, lhs).await?;
            return child.get_path_filestat(rhs, follow).await;
        }

        match path {
            "." | "" => self.get_filestat().await,
            ".." => self.open_dir(true, "..").await?.get_filestat().await,

            name => {
                let flags = FdFlags::empty();
                let ilock = self.link.inode.data.read().await;
                let child = ilock.content.get(name).ok_or_else(Error::not_found)?;
                let mut file = child.clone().open_file(false, false, false, flags).await?;
                file.get_filestat().await
            }
        }
    }

    async fn rename(
        &self,
        path: &str,
        dest_dir: &dyn WasiDir,
        dest_path: &str,
    ) -> Result<(), Error> {
        if let Some((lhs, rhs)) = path.split_once(SEP) {
            let child = self.open_dir(true, lhs).await?;
            return child.rename(rhs, dest_dir, dest_path).await;
        }

        Err(Error::not_supported())
    }

    async fn hard_link(
        &self,
        path: &str,
        target_dir: &dyn WasiDir,
        target_path: &str,
    ) -> Result<(), Error> {
        if let Some((lhs, rhs)) = path.split_once(SEP) {
            let child = self.open_dir(true, lhs).await?;
            return child.hard_link(rhs, target_dir, target_path).await;
        }

        Err(Error::not_supported())
    }

    async fn set_times(
        &self,
        path: &str,
        atime: Option<SystemTimeSpec>,
        mtime: Option<SystemTimeSpec>,
        follow: bool,
    ) -> Result<(), Error> {
        if let Some((lhs, rhs)) = path.split_once(SEP) {
            let child = self.open_dir(true, lhs).await?;
            return child.set_times(rhs, atime, mtime, follow).await;
        }

        match path {
            "." | "" => self.link.inode.data.write().await.set_times(atime, mtime),
            ".." => {
                let dir = self.open_dir(true, "..").await?;
                dir.set_times(".", atime, mtime, follow).await
            }

            name => {
                let flags = FdFlags::empty();
                let ilock = self.link.inode.data.read().await;
                let child = ilock.content.get(name).ok_or_else(Error::not_found)?;
                let mut file = child.clone().open_file(false, false, false, flags).await?;
                file.set_times(atime, mtime).await
            }
        }
    }
}

#[async_trait::async_trait]
impl WasiFile for Open<Directory> {
    fn as_any(&self) -> &dyn Any {
        self
    }

    async fn get_filetype(&mut self) -> Result<FileType, Error> {
        Ok(FileType::Directory)
    }

    async fn datasync(&mut self) -> Result<(), Error> {
        Ok(())
    }

    async fn sync(&mut self) -> Result<(), Error> {
        Ok(())
    }

    async fn get_fdflags(&mut self) -> Result<FdFlags, Error> {
        Err(Error::not_supported())
    }

    async fn set_fdflags(&mut self, _flags: FdFlags) -> Result<(), Error> {
        Err(Error::not_supported())
    }

    async fn get_filestat(&mut self) -> Result<Filestat, Error> {
        let ilock = self.link.inode.data.read().await;

        Ok(Filestat {
            device_id: **self.link.inode.id.device(),
            inode: **self.link.inode.id,
            filetype: FileType::Directory,
            nlink: Arc::strong_count(&self.link.inode) as u64,
            size: ilock.content.len() as u64,
            atim: Some(ilock.access),
            mtim: Some(ilock.modify),
            ctim: Some(ilock.create),
        })
    }

    async fn set_filestat_size(&mut self, _size: u64) -> Result<(), Error> {
        Err(Error::not_supported())
    }

    async fn advise(&mut self, _offset: u64, _len: u64, _advice: Advice) -> Result<(), Error> {
        Err(Error::not_supported())
    }

    async fn allocate(&mut self, _offset: u64, _len: u64) -> Result<(), Error> {
        Err(Error::not_supported())
    }

    async fn set_times(
        &mut self,
        atime: Option<SystemTimeSpec>,
        mtime: Option<SystemTimeSpec>,
    ) -> Result<(), Error> {
        if !self.write {
            return Err(Error::io()); // FIXME: errorno
        }

        self.link.inode.data.write().await.set_times(atime, mtime)
    }

    async fn read_vectored<'a>(&mut self, _bufs: &mut [IoSliceMut<'a>]) -> Result<u64, Error> {
        Err(Error::not_supported())
    }

    async fn read_vectored_at<'a>(
        &mut self,
        _bufs: &mut [IoSliceMut<'a>],
        _offset: u64,
    ) -> Result<u64, Error> {
        Err(Error::not_supported())
    }

    async fn write_vectored<'a>(&mut self, _bufs: &[IoSlice<'a>]) -> Result<u64, Error> {
        Err(Error::not_supported())
    }

    // FIXME: we need to decide on a behavior for O_APPEND. WASI doesn't
    // specify a behavior. POSIX defines one behavior. Linux has a different
    // one. See: https://linux.die.net/man/2/pwrite
    async fn write_vectored_at<'a>(
        &mut self,
        _bufs: &[IoSlice<'a>],
        _offset: u64,
    ) -> Result<u64, Error> {
        Err(Error::not_supported())
    }

    async fn seek(&mut self, _pos: SeekFrom) -> Result<u64, Error> {
        Err(Error::not_supported())
    }

    async fn peek(&mut self, _buf: &mut [u8]) -> Result<u64, Error> {
        Err(Error::not_supported())
    }

    async fn num_ready_bytes(&self) -> Result<u64, Error> {
        Err(Error::not_supported())
    }

    async fn readable(&self) -> Result<(), Error> {
        Err(Error::not_supported())
    }

    async fn writable(&self) -> Result<(), Error> {
        Err(Error::not_supported())
    }
}
