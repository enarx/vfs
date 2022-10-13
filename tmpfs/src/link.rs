use std::path::MAIN_SEPARATOR as SEP;
use std::sync::{Arc, Weak};

use wasi_common::file::{FileType, Filestat};
use wasi_common::{Error, ErrorExt};

use super::inode::{Data, Inode};

pub struct Link {
    pub parent: Weak<Link>,
    pub inode: Arc<Inode>,
}

impl Link {
    pub fn parent(self: &Arc<Self>) -> Result<Arc<Link>, Error> {
        self.parent.upgrade().ok_or_else(Error::not_found)
    }

    pub async fn get(self: &Arc<Self>, name: &str) -> Result<Option<Arc<Self>>, Error> {
        if name.contains(SEP) {
            return Err(Error::invalid_argument());
        }

        match &self.inode.body.read().await.data {
            Data::File(..) => Err(Error::not_dir()),

            Data::Directory(dir) => Ok(match (name, self.parent.upgrade()) {
                ("", _) => Some(self.clone()),
                (".", _) => Some(self.clone()),
                ("..", Some(parent)) => Some(parent),
                (name, _) => dir.get(name).cloned(),
            }),
        }
    }

    pub async fn req(self: &Arc<Self>, name: &str) -> Result<Arc<Self>, Error> {
        self.get(name).await?.ok_or_else(Error::not_found)
    }

    #[async_recursion::async_recursion]
    pub async fn walk(
        self: &Arc<Self>,
        follow: bool,
        path: &str,
    ) -> Result<(Arc<Link>, &str), Error> {
        // Validate input.
        if path.starts_with(SEP) {
            return Err(Error::invalid_argument());
        }

        // Recurse while there are multiple segments in the path.
        if let Some((lhs, rhs)) = path.split_once(SEP) {
            return self.req(lhs).await?.walk(follow, rhs).await;
        }

        Ok((self.clone(), path))
    }

    pub async fn stat(self: &Arc<Self>) -> Filestat {
        let ilock = self.inode.body.read().await;

        let filetype = match ilock.data {
            Data::File(..) => FileType::RegularFile,
            Data::Directory(..) => FileType::Directory,
        };

        let nlink = Arc::strong_count(&self.inode) as u64
            * match ilock.data {
                Data::Directory(..) => 2,
                Data::File(..) => 1,
            };

        let size = match ilock.data {
            Data::File(ref data) => data.len() as u64,
            Data::Directory(..) => 0, // FIXME
        };

        Filestat {
            device_id: **self.inode.id.device(),
            inode: *self.inode.id,
            filetype,
            nlink,
            size,
            atim: Some(ilock.meta.access),
            mtim: Some(ilock.meta.modify),
            ctim: Some(ilock.meta.create),
        }
    }
}
