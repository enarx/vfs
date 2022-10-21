use std::any::Any;
use std::io::{IoSlice, IoSliceMut};

use anyhow::ensure;
use async_trait::async_trait;
use tokio::join;
use wasi_common::file::{Advice, FdFlags, FileType, Filestat, SdFlags, SiFlags};
use wasi_common::{SystemTimeSpec, WasiFile};

pub struct Tee<T, W, R> {
    pub inner: T,
    pub write: W,
    pub read: R,
}

#[async_trait]
impl<T, W, R> WasiFile for Tee<T, W, R>
where
    T: WasiFile + 'static,
    W: WasiFile + 'static,
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
        let mut buf = vec![0; bufs.iter().map(|b| b.len()).sum()];
        let x = self
            .inner
            .read_vectored(&mut [IoSliceMut::new(&mut buf)])
            .await?;
        let (buf, _) = buf.split_at(x as usize);
        let y = self.read.write_vectored(&[IoSlice::new(buf)]).await?;
        ensure!(x == y, "inconsistent amount of bytes read: {x} != {y}");
        Ok(x)
    }

    async fn read_vectored_at<'a>(
        &mut self,
        bufs: &mut [IoSliceMut<'a>],
        offset: u64,
    ) -> anyhow::Result<u64> {
        let mut buf = vec![0; bufs.iter().map(|b| b.len()).sum()];
        let x = self
            .inner
            .read_vectored_at(&mut [IoSliceMut::new(&mut buf)], offset)
            .await?;
        let (buf, _) = buf.split_at(x as usize);
        let y = self
            .read
            .write_vectored_at(&[IoSlice::new(buf)], offset)
            .await?;
        ensure!(x == y, "inconsistent amount of bytes read: {x} != {y}");
        Ok(x)
    }

    async fn write_vectored<'a>(&mut self, bufs: &[IoSlice<'a>]) -> anyhow::Result<u64> {
        let (x, y) = join!(
            self.write.write_vectored(bufs),
            self.inner.write_vectored(bufs)
        );
        let x = x?;
        let y = y?;
        ensure!(x == y, "inconsistent amount of bytes written: {x} != {y}");
        Ok(x)
    }

    async fn write_vectored_at<'a>(
        &mut self,
        bufs: &[IoSlice<'a>],
        offset: u64,
    ) -> anyhow::Result<u64> {
        let (x, y) = join!(
            self.write.write_vectored_at(bufs, offset),
            self.inner.write_vectored_at(bufs, offset)
        );
        let x = x?;
        let y = y?;
        ensure!(x == y, "inconsistent amount of bytes written: {x} != {y}");
        Ok(x)
    }

    async fn peek(&mut self, buf: &mut [u8]) -> anyhow::Result<u64> {
        self.inner.peek(buf).await
    }

    async fn num_ready_bytes(&self) -> anyhow::Result<u64> {
        self.inner.num_ready_bytes().await
    }

    async fn readable(&self) -> anyhow::Result<()> {
        self.inner.readable().await
    }

    async fn writable(&self) -> anyhow::Result<()> {
        self.inner.writable().await
    }
}
