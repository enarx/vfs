use std::io::IoSlice;
use std::sync::Arc;

use wasi_common::file::{FdFlags, OFlags};
use wasi_common::{Error, ErrorExt, WasiDir};
use wasmtime_vfs_ledger::Ledger;

use super::link::Link;
use super::open::Open;

pub struct Builder(Box<dyn WasiDir>);

impl From<Arc<Ledger>> for Builder {
    fn from(ledger: Arc<Ledger>) -> Self {
        Self(Open::dir(Link::new(ledger).into()))
    }
}

impl Builder {
    pub async fn add(self, path: &str, data: impl Into<Option<Vec<u8>>>) -> Result<Self, Error> {
        match data.into() {
            None => self.0.create_dir(path).await?,
            Some(data) => {
                let of = OFlags::CREATE | OFlags::EXCLUSIVE;
                let f = FdFlags::empty();
                let mut file = self.0.open_file(true, path, of, true, true, f).await?;
                let bufs = IoSlice::new(&data);
                if file.write_vectored(&[bufs]).await? != data.len() as u64 {
                    return Err(Error::io()); // FIXME
                }
            }
        }

        Ok(self)
    }

    pub fn build(self) -> Box<dyn WasiDir> {
        self.0
    }
}
