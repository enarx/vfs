use std::any::Any;
use std::io::{IoSlice, IoSliceMut};

use anyhow::bail;
use async_trait::async_trait;
use wasi_common::file::{Advice, FdFlags, FileType, Filestat, SdFlags, SiFlags};
use wasi_common::{SystemTimeSpec, WasiFile};

pub struct Surround<L, T, R> {
    pub left: L,
    pub inner: T,
    pub right: R,
}

#[async_trait]
impl<L, T, R> WasiFile for Surround<L, T, R>
where
    L: WasiFile + 'static,
    T: WasiFile + 'static,
    R: WasiFile + 'static,
{
    fn as_any(&self) -> &dyn Any {
        self
    }

    async fn get_filetype(&mut self) -> anyhow::Result<FileType> {
        self.inner.get_filetype().await
    }

    #[cfg(unix)]
    fn pollable(&self) -> Option<rustix::fd::BorrowedFd> {
        self.inner.pollable()
    }

    #[cfg(windows)]
    fn pollable(&self) -> Option<io_extras::os::windows::RawHandleOrSocket> {
        self.inner.pollable()
    }

    fn isatty(&mut self) -> bool {
        self.inner.isatty()
    }

    async fn sock_accept(&mut self, fdflags: FdFlags) -> anyhow::Result<Box<dyn WasiFile>> {
        self.inner.sock_accept(fdflags).await
    }

    async fn sock_send<'a>(
        &mut self,
        si_data: &[IoSlice<'a>],
        si_flags: SiFlags,
    ) -> anyhow::Result<u64> {
        self.inner.sock_send(si_data, si_flags).await
    }

    async fn sock_shutdown(&mut self, how: SdFlags) -> anyhow::Result<()> {
        self.inner.sock_shutdown(how).await
    }

    async fn datasync(&mut self) -> anyhow::Result<()> {
        self.inner.datasync().await
    }

    async fn sync(&mut self) -> anyhow::Result<()> {
        self.inner.sync().await
    }

    async fn get_fdflags(&mut self) -> anyhow::Result<FdFlags> {
        self.inner.get_fdflags().await
    }

    async fn set_fdflags(&mut self, flags: FdFlags) -> anyhow::Result<()> {
        self.inner.set_fdflags(flags).await
    }

    async fn get_filestat(&mut self) -> anyhow::Result<Filestat> {
        self.inner.get_filestat().await
    }

    async fn set_filestat_size(&mut self, size: u64) -> anyhow::Result<()> {
        self.inner.set_filestat_size(size).await
    }

    async fn advise(&mut self, offset: u64, len: u64, advice: Advice) -> anyhow::Result<()> {
        self.inner.advise(offset, len, advice).await
    }

    async fn allocate(&mut self, offset: u64, len: u64) -> anyhow::Result<()> {
        self.inner.allocate(offset, len).await
    }

    async fn set_times(
        &mut self,
        atime: Option<SystemTimeSpec>,
        mtime: Option<SystemTimeSpec>,
    ) -> anyhow::Result<()> {
        self.inner.set_times(atime, mtime).await
    }

    async fn read_vectored<'a>(&mut self, bufs: &mut [IoSliceMut<'a>]) -> anyhow::Result<u64> {
        // TODO: Check for EOF?
        match self.left.read_vectored(bufs).await {
            Ok(0) => {}
            Ok(n) => return Ok(n),
            Err(e) => bail!(e),
        }
        match self.inner.read_vectored(bufs).await {
            Ok(0) => {}
            Ok(n) => return Ok(n),
            Err(e) => bail!(e),
        }
        self.right.read_vectored(bufs).await
    }

    async fn write_vectored<'a>(&mut self, bufs: &[IoSlice<'a>]) -> anyhow::Result<u64> {
        self.inner.write_vectored(bufs).await
    }

    async fn write_vectored_at<'a>(
        &mut self,
        bufs: &[IoSlice<'a>],
        offset: u64,
    ) -> anyhow::Result<u64> {
        self.inner.write_vectored_at(bufs, offset).await
    }

    async fn readable(&self) -> anyhow::Result<()> {
        self.inner.readable().await
    }

    async fn writable(&self) -> anyhow::Result<()> {
        self.inner.writable().await
    }
}
