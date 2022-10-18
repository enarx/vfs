use std::any::Any;
use std::cmp::min;
use std::io::{IoSlice, IoSliceMut, SeekFrom};
use std::sync::Arc;

use tokio::sync::RwLock;
use wasi_common::file::{Advice, FdFlags, FileType, Filestat};
use wasi_common::{Error, ErrorExt, ErrorKind, SystemTimeSpec, WasiFile};

use crate::link::Link;
use crate::node::Node;

use super::Open;

pub struct State {
    flags: FdFlags,
    pos: usize,
}

pub struct Data {
    state: RwLock<State>,
    write: bool,
    read: bool,
}

impl Open<Vec<u8>, Data> {
    pub fn file(link: Arc<Link<Vec<u8>>>, read: bool, write: bool, flags: FdFlags) -> Box<Self> {
        Box::new(Self {
            _root: link.clone().root(),
            link,
            data: Data {
                state: RwLock::new(State { flags, pos: 0 }),
                write,
                read,
            },
        })
    }
}

#[async_trait::async_trait]
impl WasiFile for Open<Vec<u8>, Data> {
    fn as_any(&self) -> &dyn Any {
        self
    }

    async fn get_filetype(&mut self) -> Result<FileType, Error> {
        Ok(FileType::RegularFile)
    }

    async fn datasync(&mut self) -> Result<(), Error> {
        Ok(())
    }

    async fn sync(&mut self) -> Result<(), Error> {
        Ok(())
    }

    async fn get_fdflags(&mut self) -> Result<FdFlags, Error> {
        Ok(self.data.state.read().await.flags)
    }

    async fn set_fdflags(&mut self, flags: FdFlags) -> Result<(), Error> {
        if !self.data.write {
            return Err(Error::io()); // FIXME: errorno
        }

        self.data.state.write().await.flags = flags;
        Ok(())
    }

    async fn get_filestat(&mut self) -> Result<Filestat, Error> {
        let ilock = self.link.inode.data.read().await;

        Ok(Filestat {
            device_id: **self.link.inode.id.device(),
            inode: **self.link.inode.id,
            filetype: FileType::RegularFile,
            nlink: Arc::strong_count(&self.link.inode) as u64,
            size: ilock.content.len() as u64,
            atim: Some(ilock.access),
            mtim: Some(ilock.modify),
            ctim: Some(ilock.create),
        })
    }

    async fn set_filestat_size(&mut self, size: u64) -> Result<(), Error> {
        let size: usize = size.try_into().map_err(|_| Error::invalid_argument())?;

        if !self.data.write {
            return Err(Error::io()); // FIXME: errorno
        }

        self.link.inode.data.write().await.content.resize(size, 0);
        Ok(())
    }

    async fn advise(&mut self, _offset: u64, _len: u64, _advice: Advice) -> Result<(), Error> {
        Ok(())
    }

    async fn allocate(&mut self, offset: u64, len: u64) -> Result<(), Error> {
        if !self.data.write {
            return Err(Error::io()); // FIXME: errorno
        }

        let offset: usize = offset.try_into().map_err(|_| Error::invalid_argument())?;
        let len: usize = len.try_into().map_err(|_| Error::invalid_argument())?;
        offset
            .checked_add(len)
            .ok_or_else(Error::invalid_argument)?;
        Ok(())
    }

    async fn set_times(
        &mut self,
        atime: Option<SystemTimeSpec>,
        mtime: Option<SystemTimeSpec>,
    ) -> Result<(), Error> {
        if !self.data.write {
            return Err(Error::io()); // FIXME: errorno
        }

        self.link.inode.data.write().await.set_times(atime, mtime)
    }

    async fn read_vectored<'a>(&mut self, bufs: &mut [IoSliceMut<'a>]) -> Result<u64, Error> {
        if !self.data.read {
            return Err(Error::io()); // FIXME: errorno
        }

        let mut total = 0;

        let mut olock = self.data.state.write().await;
        let ilock = self.link.inode.data.read().await;
        for buf in bufs {
            let len = min(buf.len(), ilock.content.len() - olock.pos);
            buf.copy_from_slice(&ilock.content[olock.pos..][..len]);
            total += len as u64;
            olock.pos += len;
        }

        Ok(total)
    }

    async fn read_vectored_at<'a>(
        &mut self,
        bufs: &mut [IoSliceMut<'a>],
        offset: u64,
    ) -> Result<u64, Error> {
        if !self.data.read {
            return Err(Error::io()); // FIXME: errorno
        }

        let mut position: usize = offset.try_into().map_err(|_| Error::invalid_argument())?;
        let mut total = 0;

        let data = &self.link.inode.data.read().await.content[..];
        for buf in bufs {
            let len = min(buf.len(), data.len() - position);
            buf.copy_from_slice(&data[position..][..len]);
            total += len as u64;
            position += len;
        }

        Ok(total)
    }

    async fn write_vectored<'a>(&mut self, bufs: &[IoSlice<'a>]) -> Result<u64, Error> {
        if !self.data.write {
            return Err(Error::io()); // FIXME: errorno
        }

        let mut total = 0;

        let mut olock = self.data.state.write().await;
        let mut ilock = self.link.inode.data.write().await;
        for buf in bufs {
            let pos = match olock.flags.contains(FdFlags::APPEND) {
                true => ilock.content.len(),
                false => olock.pos,
            };

            if pos + buf.len() > ilock.content.len() {
                ilock.content.resize(pos + buf.len(), 0);
            }

            ilock.content[pos..][..buf.len()].copy_from_slice(buf);
            total += buf.len() as u64;

            if !olock.flags.contains(FdFlags::APPEND) {
                olock.pos += buf.len();
            }
        }

        Ok(total)
    }

    // FIXME: we need to decide on a behavior for O_APPEND. WASI doesn't
    // specify a behavior. POSIX defines one behavior. Linux has a different
    // one. See: https://linux.die.net/man/2/pwrite
    async fn write_vectored_at<'a>(
        &mut self,
        bufs: &[IoSlice<'a>],
        offset: u64,
    ) -> Result<u64, Error> {
        if !self.data.write {
            return Err(Error::io()); // FIXME: errorno
        }

        let mut pos: usize = offset.try_into().map_err(|_| Error::invalid_argument())?;
        let mut total = 0;

        let mut ilock = self.link.inode.data.write().await;
        for buf in bufs {
            if pos + buf.len() > ilock.content.len() {
                ilock.content.resize(pos + buf.len(), 0);
            }

            ilock.content[pos..][..buf.len()].copy_from_slice(buf);
            total += buf.len() as u64;
            pos += buf.len();
        }

        Ok(total)
    }

    async fn seek(&mut self, pos: SeekFrom) -> Result<u64, Error> {
        let mut olock = self.data.state.write().await;
        let ilock = self.link.inode.data.read().await;

        let cur = match pos {
            SeekFrom::Current(_) => i64::try_from(olock.pos),
            SeekFrom::Start(_) => Ok(0),
            SeekFrom::End(_) => i64::try_from(ilock.content.len()),
        };

        let off = match pos {
            SeekFrom::Current(off) => Ok(off),
            SeekFrom::Start(off) => i64::try_from(off),
            SeekFrom::End(off) => Ok(off),
        };

        let pos = cur.map_err(|_| ErrorKind::Inval)? + off.map_err(|_| ErrorKind::Inval)?;
        let pos = usize::try_from(pos).map_err(|_| ErrorKind::Inval)?;
        olock.pos = pos;

        Ok(pos as u64)
    }

    async fn peek(&mut self, buf: &mut [u8]) -> Result<u64, Error> {
        if !self.data.read {
            return Err(Error::io()); // FIXME: errorno
        }

        let mut total = 0;

        let olock = self.data.state.read().await;
        let ilock = self.link.inode.data.read().await;
        let len = min(buf.len(), ilock.content.len() - olock.pos);
        buf.copy_from_slice(&ilock.content[olock.pos..][..len]);
        total += len as u64;

        Ok(total)
    }

    async fn num_ready_bytes(&self) -> Result<u64, Error> {
        if !self.data.read {
            return Err(Error::io()); // FIXME: errorno
        }

        let olock = self.data.state.read().await;
        let ilock = self.link.inode.data.read().await;
        let len = min(ilock.content.len(), olock.pos);
        let len = ilock.content.len() - len;
        Ok(len as u64)
    }

    async fn readable(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn writable(&self) -> Result<(), Error> {
        Ok(())
    }
}
