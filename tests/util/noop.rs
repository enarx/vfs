use std::any::Any;

use async_trait::async_trait;
use wasi_common::file::FileType;
use wasi_common::WasiFile;

pub struct Noop;

#[async_trait]
impl WasiFile for Noop {
    fn as_any(&self) -> &dyn Any {
        self
    }

    async fn get_filetype(&mut self) -> anyhow::Result<FileType> {
        Ok(FileType::Unknown)
    }
}
