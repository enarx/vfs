use std::any::Any;
use std::collections::BTreeMap;
use std::path::{PathBuf, MAIN_SEPARATOR as SEP};
use std::sync::Arc;

use wasi_common::dir::{ReaddirCursor, ReaddirEntity};
use wasi_common::file::{FdFlags, FileType, Filestat, OFlags};
use wasi_common::{Error, ErrorExt, SystemTimeSpec, WasiDir, WasiFile};

use super::Open;
use crate::inode::{Data, Inode};
use crate::link::Link;
use crate::node::Node;

impl Link<BTreeMap<String, Arc<dyn Node>>> {
    pub fn mkdir(self: &Arc<Self>) -> Arc<dyn Node> {
        Arc::new(Link {
            parent: Arc::downgrade(&self.here()),
            inode: Arc::new(Inode {
                id: self.inode.id.device().create_inode(),
                data: Data::<BTreeMap<_, _>>::default().into(),
            }),
        })
    }

    pub fn mkfile(self: &Arc<Self>) -> Arc<dyn Node> {
        Arc::new(Link {
            parent: Arc::downgrade(&self.here()),
            inode: Arc::new(Inode {
                id: self.inode.id.device().create_inode(),
                data: Data::<Vec<u8>>::default().into(),
            }),
        })
    }
}

impl Open<BTreeMap<String, Arc<dyn Node>>, ()> {
    pub fn dir(link: Arc<Link<BTreeMap<String, Arc<dyn Node>>>>) -> Box<Self> {
        Box::new(Self {
            _root: link.clone().root(),
            link,
            data: (),
        })
    }
}

#[async_trait::async_trait]
impl WasiDir for Open<BTreeMap<String, Arc<dyn Node>>, ()> {
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
                        let child = match oflags.contains(OFlags::DIRECTORY) {
                            true => self.link.mkdir(),
                            false => self.link.mkfile(),
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
                        let child = self.link.mkdir();
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
