use std::collections::BTreeMap;
use std::sync::{Arc, Weak};

use wasi_common::{Error, ErrorExt, WasiDir};
use wasmtime_vfs_ledger::Ledger;

use super::inode::{Body, Data, Inode};
use super::link::Link;
use super::open::Open;

pub struct Builder(Arc<Link>);

impl From<Arc<Ledger>> for Builder {
    fn from(ledger: Arc<Ledger>) -> Self {
        Self(Arc::new(Link {
            parent: Weak::new(),
            inode: Arc::new(Inode {
                body: Body::from(Data::Directory(BTreeMap::new())).into(),
                id: ledger.create_device().create_inode(),
            }),
        }))
    }
}

impl Builder {
    pub async fn add(self, path: &str, data: impl Into<Option<Vec<u8>>>) -> Result<Self, Error> {
        let (parent, child) = self.0.walk(false, path).await?;

        match child {
            "" => Err(Error::invalid_argument()),
            "." => Err(Error::invalid_argument()),
            ".." => Err(Error::invalid_argument()),

            name => match &mut parent.inode.body.write().await.data {
                Data::File(..) => Err(Error::not_dir()),
                Data::Directory(dir) => match dir.get(name) {
                    Some(..) => Err(Error::exist()),
                    None => {
                        let data = match data.into() {
                            Some(content) => Data::File(content),
                            None => Data::Directory(BTreeMap::new()),
                        };

                        let inode = Arc::new(Inode {
                            body: Body::from(data).into(),
                            id: parent.inode.id.device().create_inode(),
                        });

                        let link = Arc::new(Link {
                            parent: Arc::downgrade(&parent),
                            inode,
                        });

                        dir.insert(name.to_string(), link);
                        Ok(self)
                    }
                },
            },
        }
    }

    pub fn build(self) -> Box<dyn WasiDir> {
        Box::new(Open::from(self.0))
    }
}
