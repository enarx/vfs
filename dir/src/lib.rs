use std::any::Any;
use std::collections::BTreeMap;
use std::io::{IoSlice, IoSliceMut, SeekFrom};
use std::ops::{Deref, DerefMut};
use std::path::{PathBuf, MAIN_SEPARATOR as SEP};
use std::sync::{Arc, Weak};

use wasi_common::dir::{ReaddirCursor, ReaddirEntity};
use wasi_common::file::{Advice, FdFlags, FileType, Filestat, OFlags};
use wasi_common::{Error, ErrorExt, SystemTimeSpec, WasiDir, WasiFile};
use wasmtime_vfs_ledger::{DeviceId, InodeId, Ledger};
use wasmtime_vfs_memory::{Link, Node, Open, State};

type NodeConstructor = Arc<dyn Fn(Arc<dyn Node>) -> Arc<dyn Node> + Send + Sync>;

/// A directory generic in file [`Node`] constructor
pub struct Directory {
    nodes: Link<BTreeMap<String, Arc<dyn Node>>>,
    create_file: Option<NodeConstructor>,
}

impl Deref for Directory {
    type Target = Link<BTreeMap<String, Arc<dyn Node>>>;

    fn deref(&self) -> &Self::Target {
        &self.nodes
    }
}

impl DerefMut for Directory {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.nodes
    }
}

impl Directory {
    fn new_at(
        parent: Weak<dyn Node>,
        device_id: Arc<DeviceId>,
        create_file: Option<NodeConstructor>,
    ) -> Arc<Self> {
        let nodes = Link {
            parent,
            inode: Arc::new(device_id.create_inode().into()),
        };
        Self { nodes, create_file }.into()
    }

    fn prev(self: &Arc<Self>) -> Arc<dyn Node> {
        match self.parent.upgrade() {
            Some(parent) => parent,
            None => self.clone(),
        }
    }

    pub fn device(parent: Arc<dyn Node>, create_file: Option<NodeConstructor>) -> Arc<Self> {
        Self::new_at(
            Arc::downgrade(&parent),
            parent.id().device().ledger().create_device(),
            create_file,
        )
    }

    pub fn root(ledger: Arc<Ledger>, create_file: Option<NodeConstructor>) -> Arc<Self> {
        Self::new_at(Weak::<Self>::new(), ledger.create_device(), create_file)
    }

    pub fn new(parent: Arc<dyn Node>, create_file: Option<NodeConstructor>) -> Arc<Self> {
        Self::new_at(Arc::downgrade(&parent), parent.id().device(), create_file)
    }

    pub async fn get(self: &Arc<Self>, path: &str) -> Result<Arc<dyn Node>, Error> {
        let mut this: Arc<dyn Node> = self.clone();

        for seg in path.trim_end_matches(SEP).split(SEP) {
            this = match seg {
                "" | "." => continue,
                ".." => self.prev(),
                seg => {
                    let any = this.to_any();
                    let dir = any.downcast::<Directory>().map_err(|_| Error::not_dir())?;
                    let ilock = dir.inode.data.read().await;
                    ilock.content.get(seg).ok_or_else(Error::not_found)?.clone()
                }
            };
        }

        Ok(this)
    }

    pub async fn attach(self: &Arc<Self>, path: &str, node: Arc<dyn Node>) -> Result<(), Error> {
        let path = path.trim_end_matches(SEP);
        let (this, name) = match path.rsplit_once(SEP) {
            None => (self.clone(), path),
            Some((lhs, rhs)) => {
                let any = self.get(lhs).await?.to_any();
                let dir = any.downcast::<Directory>().map_err(|_| Error::not_dir())?;
                (dir, rhs)
            }
        };

        let mut ilock = this.inode.data.write().await;

        match name {
            "" | "." | ".." => Err(Error::invalid_argument()),
            name if ilock.content.contains_key(name) => Err(Error::exist()),
            name => {
                ilock.content.insert(name.to_owned(), node);
                Ok(())
            }
        }
    }
}

#[async_trait::async_trait]
impl Node for Directory {
    fn to_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self
    }

    fn parent(&self) -> Option<Arc<dyn Node>> {
        self.parent.upgrade()
    }

    fn filetype(&self) -> FileType {
        FileType::Directory
    }

    fn id(&self) -> Arc<InodeId> {
        self.inode.id.clone()
    }

    async fn open_dir(self: Arc<Self>) -> Result<Box<dyn WasiDir>, Error> {
        Ok(Box::new(OpenDir(Open {
            root: self.root(),
            link: self,
            state: State::default().into(),
            write: false,
            read: false,
        })))
    }

    async fn open_file(
        self: Arc<Self>,
        _path: &str,
        _dir: bool,
        read: bool,
        write: bool,
        flags: FdFlags,
    ) -> Result<Box<dyn WasiFile>, Error> {
        Ok(Box::new(OpenDir(Open {
            root: self.root(),
            link: self,
            state: State::from(flags).into(),
            write,
            read,
        })))
    }
}

struct OpenDir(Open<Directory>);

impl Deref for OpenDir {
    type Target = Open<Directory>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[async_trait::async_trait]
impl WasiDir for OpenDir {
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
            "." | "" => {
                let link = self.link.clone();
                link.open_file(path, odir, read, write, flags).await
            }

            ".." if oflags.contains(OFlags::EXCLUSIVE) => Err(Error::exist()),
            ".." if oflags.contains(OFlags::TRUNCATE) => Err(Error::io()), // FIXME
            ".." => {
                let link = self.link.prev();
                link.open_file(path, odir, read, write, flags).await
            }

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
                        let link = self.link.clone();
                        let child: Arc<dyn Node> = if oflags.contains(OFlags::DIRECTORY) {
                            Directory::new(link, self.link.create_file.clone())
                        } else if let Some(ref create_file) = self.link.create_file {
                            create_file(link)
                        } else {
                            return Err(Error::not_supported());
                        };

                        ilock.content.insert(name.into(), child.clone());
                        child.open_file(path, odir, read, write, flags).await
                    }

                    // Truncate the file.
                    (Some(child), _) if oflags.contains(OFlags::TRUNCATE) => {
                        let mut open = child
                            .open_file(path, odir, false, true, FdFlags::empty())
                            .await?;
                        open.set_filestat_size(0).await?;
                        Ok(open)
                    }

                    // Open the file.
                    (Some(child), _) => child.open_file(path, odir, read, write, flags).await,
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
            "." => self.link.clone().open_dir().await,
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
                        let child =
                            Directory::new(self.link.clone(), self.link.create_file.clone());
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
            Ok(ReaddirEntity {
                name: ".".into(),
                next: 1.into(),
                inode: **self.link.id(),
                filetype: self.link.filetype(),
            }),
            Ok(ReaddirEntity {
                name: "..".into(),
                next: 2.into(),
                inode: **self.link.prev().id(),
                filetype: self.link.prev().filetype(),
            }),
        ];

        // Add all of the child entries.
        for (k, v) in ilock.content.iter() {
            let next = entries.len() as u64 + 1;
            entries.push(Ok(ReaddirEntity {
                name: k.into(),
                next: next.into(),
                inode: **v.id(),
                filetype: v.filetype(),
            }));
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
    // the child is also a `Directory` AND has the same device id.
    async fn remove_dir(&self, path: &str) -> Result<(), Error> {
        if let Some((lhs, rhs)) = path.split_once(SEP) {
            let child = self.open_dir(true, lhs).await?;
            return child.remove_dir(rhs).await;
        }

        match path {
            "" | "." | ".." => Err(Error::invalid_argument()),

            name => {
                let mut plock = self.link.inode.data.write().await;

                let cnode = plock.content.get(name).ok_or_else(Error::not_found)?;
                if self.link.id().device() != cnode.id().device() {
                    return Err(Error::io()); // FIXME: EXDEV?
                }

                let clink = cnode
                    .clone()
                    .to_any()
                    .downcast::<Directory>()
                    .map_err(|_| Error::not_dir())?;

                let clock = clink.inode.data.read().await;
                if clock.content.is_empty() {
                    return Err(Error::io()); // FIXME: ENOTEMPTY
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

                if cnode.filetype() == FileType::Directory {
                    return Err(Error::io()); // FIXME: ENOTFILE?
                }

                if self.link.id().device() != cnode.id().device() {
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
            filetype: FileType::Directory,
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
                let child = child.clone();
                let mut file = child.open_file(path, false, false, false, flags).await?;
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
                let child = child.clone();
                let mut file = child.open_file(path, false, false, false, flags).await?;
                file.set_times(atime, mtime).await
            }
        }
    }
}

#[async_trait::async_trait]
impl WasiFile for OpenDir {
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

#[cfg(test)]
mod test {
    use super::*;

    use std::io::IoSliceMut;
    use std::path::MAIN_SEPARATOR as SEP;
    use std::sync::Arc;

    use wasi_common::file::{FdFlags, FileType, OFlags};
    use wasmtime_vfs_file::File;
    use wasmtime_vfs_ledger::Ledger;
    use wasmtime_vfs_memory::Node;

    #[tokio::test]
    async fn test() {
        const FILES: &[(&str, Option<&[u8]>)] = &[
            ("/foo", None),
            ("/foo/bar", Some(b"abc")),
            ("/foo/baz", Some(b"abc")),
            ("/foo/bat", None),
            ("/foo/bat/qux", Some(b"abc")),
            ("/ack", None),
            ("/ack/act", Some(b"abc")),
            ("/zip", Some(b"abc")),
        ];

        let dir = Directory::root(Ledger::new(), None);
        for (path, data) in FILES {
            let parent = dir.get(path.rsplit_once(SEP).unwrap().0).await.unwrap();
            let child: Arc<dyn Node> = match data {
                Some(data) => File::with_data(parent, *data),
                None => Directory::new(parent, Some(Arc::new(File::new))),
            };

            dir.attach(path, child).await.unwrap()
        }
        let treefs = dir.open_dir().await.unwrap();

        let top: Vec<Result<_, _>> = treefs.readdir(0.into()).await.unwrap().collect();

        assert_eq!(top[0].as_ref().unwrap().name, ".");
        assert_eq!(top[0].as_ref().unwrap().filetype, FileType::Directory);
        assert_eq!(top[0].as_ref().unwrap().inode, 0);

        assert_eq!(top[1].as_ref().unwrap().name, "..");
        assert_eq!(top[1].as_ref().unwrap().filetype, FileType::Directory);
        assert_eq!(top[1].as_ref().unwrap().inode, 0);

        assert_eq!(top[2].as_ref().unwrap().name, "ack");
        assert_eq!(top[2].as_ref().unwrap().filetype, FileType::Directory);
        assert_eq!(top[2].as_ref().unwrap().inode, 6);

        assert_eq!(top[3].as_ref().unwrap().name, "foo");
        assert_eq!(top[3].as_ref().unwrap().filetype, FileType::Directory);
        assert_eq!(top[3].as_ref().unwrap().inode, 1);

        assert_eq!(top[4].as_ref().unwrap().name, "zip");
        assert_eq!(top[4].as_ref().unwrap().filetype, FileType::RegularFile);
        assert_eq!(top[4].as_ref().unwrap().inode, 8);

        let foo = treefs.open_dir(false, "foo").await.unwrap();
        let foo: Vec<Result<_, _>> = foo.readdir(0.into()).await.unwrap().collect();

        assert_eq!(foo[0].as_ref().unwrap().name, ".");
        assert_eq!(foo[0].as_ref().unwrap().filetype, FileType::Directory);
        assert_eq!(foo[0].as_ref().unwrap().inode, 1);

        assert_eq!(foo[1].as_ref().unwrap().name, "..");
        assert_eq!(foo[1].as_ref().unwrap().filetype, FileType::Directory);
        assert_eq!(foo[1].as_ref().unwrap().inode, 0);

        assert_eq!(foo[2].as_ref().unwrap().name, "bar");
        assert_eq!(foo[2].as_ref().unwrap().filetype, FileType::RegularFile);
        assert_eq!(foo[2].as_ref().unwrap().inode, 2);

        assert_eq!(foo[3].as_ref().unwrap().name, "bat");
        assert_eq!(foo[3].as_ref().unwrap().filetype, FileType::Directory);
        assert_eq!(foo[3].as_ref().unwrap().inode, 4);

        assert_eq!(foo[4].as_ref().unwrap().name, "baz");
        assert_eq!(foo[4].as_ref().unwrap().filetype, FileType::RegularFile);
        assert_eq!(foo[4].as_ref().unwrap().inode, 3);

        let mut qux = treefs
            .open_file(
                false,
                "foo/bat/qux",
                OFlags::empty(),
                true,
                false,
                FdFlags::empty(),
            )
            .await
            .unwrap();

        let mut buf = [0u8; 3];
        let mut bufs = [IoSliceMut::new(&mut buf)];
        let len = qux.read_vectored(&mut bufs).await.unwrap();
        assert_eq!(len, 3);
        assert_eq!(&buf, b"abc");
    }
}
