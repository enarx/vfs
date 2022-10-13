use std::any::Any;
use std::collections::BTreeMap;
use std::path::{PathBuf, MAIN_SEPARATOR as SEP};
use std::sync::Arc;

use wasi_common::dir::{ReaddirCursor, ReaddirEntity};
use wasi_common::file::{FdFlags, FileType, Filestat, OFlags};
use wasi_common::{Error, ErrorExt, SystemTimeSpec, WasiDir, WasiFile};

use super::Open;
use crate::inode::{Body, Data, Inode};
use crate::link::Link;

const VALID_OFLAGS: &[u32] = &[
    OFlags::empty().bits(),
    OFlags::CREATE.bits(),
    OFlags::DIRECTORY.bits(),
    OFlags::TRUNCATE.bits(),
    OFlags::CREATE.bits() | OFlags::DIRECTORY.bits(),
    OFlags::CREATE.bits() | OFlags::EXCLUSIVE.bits(),
    OFlags::CREATE.bits() | OFlags::DIRECTORY.bits() | OFlags::EXCLUSIVE.bits(),
];

#[async_trait::async_trait]
impl WasiDir for Open {
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
        // Recurse to the parent directory.
        if let Some((lhs, rhs)) = path.rsplit_once(SEP) {
            return self
                .open_dir(follow, lhs)
                .await?
                .open_file(follow, rhs, oflags, read, write, flags)
                .await;
        }

        // Check the validity of the flags.
        VALID_OFLAGS
            .iter()
            .find(|o| **o == oflags.bits())
            .ok_or_else(Error::invalid_argument)?;

        // Truncate can only be used with write.
        if oflags.contains(OFlags::TRUNCATE) && !write {
            return Err(Error::io()); // FIXME
        }

        // Lock the parent's inode.
        let mut ilock = self.link.inode.body.write().await;

        // Find or create the child.
        let child = match path {
            "" => Err(Error::invalid_argument()),

            "." if oflags.contains(OFlags::EXCLUSIVE) => Err(Error::exist()),
            "." if oflags.contains(OFlags::TRUNCATE) => Err(Error::io()), // FIXME
            "." => Ok(self.link.clone()),

            ".." if oflags.contains(OFlags::EXCLUSIVE) => Err(Error::exist()),
            ".." if oflags.contains(OFlags::TRUNCATE) => Err(Error::io()), // FIXME
            ".." => Ok(self.link.parent()?),

            name => match &mut ilock.data {
                Data::File(_) => Err(Error::not_dir()),
                Data::Directory(dir) => match (dir.get(name), oflags.contains(OFlags::CREATE)) {
                    // If the file exists and we're creating it, then we have an error.
                    (Some(_), true) if oflags.contains(OFlags::EXCLUSIVE) => Err(Error::exist()),

                    // If the file doesn't exist and we're not creating it, then we have an error.
                    (None, false) => Err(Error::not_found()),

                    // If the file doesn't exist, create it.
                    (None, _) => {
                        let child = Arc::new(Link {
                            parent: Arc::downgrade(&self.link),
                            inode: Arc::new(Inode {
                                id: self.link.inode.id.device().create_inode(),
                                body: match oflags.contains(OFlags::DIRECTORY) {
                                    true => Body::from(BTreeMap::new()).into(),
                                    false => Body::from(Vec::new()).into(),
                                },
                            }),
                        });

                        dir.insert(name.into(), child.clone());
                        Ok(child)
                    }

                    // Truncate the file.
                    (Some(child), _) if oflags.contains(OFlags::TRUNCATE) => {
                        let mut ilock = child.inode.body.write().await;

                        match &mut ilock.data {
                            Data::Directory(_) => return Err(Error::io()), // FIXME
                            Data::File(_) => ilock.data = Vec::new().into(),
                        }

                        Ok(child.clone())
                    }

                    // Open the directory.
                    (Some(child), _) if oflags.contains(OFlags::DIRECTORY) => {
                        match &child.inode.body.read().await.data {
                            Data::Directory(..) => Ok(child.clone()),
                            Data::File(..) => Err(Error::not_dir()),
                        }
                    }

                    // Open the file.
                    (Some(child), _) => Ok(child.clone()),
                },
            },
        }?;

        Ok(Box::new(Open::new(child, read, write, flags)))
    }

    async fn open_dir(&self, follow: bool, path: &str) -> Result<Box<dyn WasiDir>, Error> {
        // Get the file link.
        let (parent, name) = self.link.walk(follow, path).await?;
        let dir = parent.req(name).await?;

        // Open the directory.
        let ilock = dir.inode.body.read().await;
        match &ilock.data {
            Data::Directory(..) => Ok(Box::new(Open::from(&dir))),
            Data::File(..) => Err(Error::not_dir()),
        }
    }

    async fn create_dir(&self, path: &str) -> Result<(), Error> {
        let (parent, name) = self.link.walk(true, path).await?;
        let mut ilock = parent.inode.body.write().await;
        match &mut ilock.data {
            Data::File(..) => Err(Error::not_dir()),
            Data::Directory(dir) if dir.contains_key(name) => Err(Error::exist()),
            Data::Directory(dir) => {
                let child = Arc::new(Link {
                    parent: Arc::downgrade(&parent),
                    inode: Arc::new(Inode {
                        id: parent.inode.id.device().create_inode(),
                        body: Body::from(BTreeMap::new()).into(),
                    }),
                });

                dir.insert(name.into(), child);
                Ok(())
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
        let ilock = self.link.inode.body.read().await;
        let dir = match &ilock.data {
            Data::Directory(dir) => Ok(dir),
            Data::File(..) => Err(Error::not_dir()),
        }?;

        // Add the single dot entry.
        let mut entries = vec![Ok(ReaddirEntity {
            filetype: FileType::Directory,
            inode: *self.link.inode.id,
            name: ".".to_string(),
            next: 1.into(),
        })];

        // If there is a parent, add the double dot entry.
        if let Some(parent) = self.link.parent.upgrade() {
            entries.push(Ok(ReaddirEntity {
                filetype: FileType::Directory,
                inode: *parent.inode.id,
                name: "..".to_string(),
                next: 2.into(),
            }));
        }

        // Add all of the child entries.
        for (i, (k, v)) in dir.iter().enumerate() {
            entries.push(Ok(ReaddirEntity {
                filetype: match v.inode.body.read().await.data {
                    Data::Directory(..) => FileType::Directory,
                    Data::File(..) => FileType::RegularFile,
                },
                inode: *v.inode.id,
                name: k.to_string(),
                next: (i as u64 + entries.len() as u64 + 1).into(),
            }));
        }

        Ok(Box::new(entries.into_iter().skip(cursor)))
    }

    async fn symlink(&self, _old_path: &str, _new_path: &str) -> Result<(), Error> {
        Err(Error::not_supported())
    }

    async fn remove_dir(&self, path: &str) -> Result<(), Error> {
        // Get the file link.
        let (parent, name) = self.link.walk(true, path).await?;

        // Look up the child.
        let child = match name {
            "" => Err(Error::invalid_argument()),
            "." | ".." => Err(Error::io()), // FIXME
            name => parent.req(name).await,
        }?;

        // Lock the parent and child.
        let mut plock = parent.inode.body.write().await;
        let clock = child.inode.body.read().await;

        match &clock.data {
            Data::File(..) => Err(Error::not_dir()),
            Data::Directory(dir) if !dir.is_empty() => Err(Error::io()), // FIXME
            Data::Directory(..) => {
                let dir = match &mut plock.data {
                    Data::Directory(dir) => dir,
                    Data::File(..) => return Err(Error::not_dir()),
                };

                dir.remove(name);
                Ok(())
            }
        }
    }

    async fn unlink_file(&self, path: &str) -> Result<(), Error> {
        // Get the file link.
        let (parent, name) = self.link.walk(true, path).await?;

        // Look up the child.
        let child = match name {
            "" => Err(Error::invalid_argument()),
            "." | ".." => Err(Error::io()), // FIXME
            name => parent.req(name).await,
        }?;

        // Lock the parent and child.
        let mut plock = parent.inode.body.write().await;
        let clock = child.inode.body.read().await;

        match (&mut plock.data, &clock.data) {
            (Data::Directory(dir), Data::File(..)) => {
                dir.remove(name);
                Ok(())
            }

            _ => Err(Error::io()), // FIXME
        }
    }

    async fn read_link(&self, _path: &str) -> Result<PathBuf, Error> {
        Err(Error::not_supported())
    }

    async fn get_filestat(&self) -> Result<Filestat, Error> {
        Ok(self.link.stat().await)
    }

    async fn get_path_filestat(&self, path: &str, follow: bool) -> Result<Filestat, Error> {
        let (parent, name) = self.link.walk(follow, path).await?;
        Ok(parent.req(name).await?.stat().await)
    }

    async fn rename(
        &self,
        _path: &str,
        _dest_dir: &dyn WasiDir,
        _dest_path: &str,
    ) -> Result<(), Error> {
        Err(Error::not_supported())
    }

    async fn hard_link(
        &self,
        _path: &str,
        _target_dir: &dyn WasiDir,
        _target_path: &str,
    ) -> Result<(), Error> {
        Err(Error::not_supported())
    }

    async fn set_times(
        &self,
        path: &str,
        atime: Option<SystemTimeSpec>,
        mtime: Option<SystemTimeSpec>,
        follow: bool,
    ) -> Result<(), Error> {
        let (parent, name) = self.link.walk(follow, path).await?;
        let entry = parent.req(name).await?;
        entry.inode.update(atime, mtime).await
    }
}
