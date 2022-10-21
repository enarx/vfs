mod dir;
mod file;

use std::sync::Arc;

use tokio::sync::RwLock;
use wasi_common::{file::FdFlags, WasiDir, WasiFile};

use super::link::Link;
use crate::node::Node;

pub struct Open<L, D> {
    _root: Arc<dyn Node>,
    link: Arc<Link<L>>,
    data: D,
}

impl<T> Open<T, ()>
where
    Self: WasiDir,
    Link<T>: Node,
{
    pub fn dir(link: Arc<Link<T>>) -> Box<Self> {
        Box::new(Self {
            _root: link.clone().root(),
            link,
            data: (),
        })
    }
}

pub struct State {
    flags: FdFlags,
    pos: usize,
}

pub struct Data {
    state: RwLock<State>,
    write: bool,
    read: bool,
}

impl<T> Open<T, Data>
where
    Self: WasiFile,
    Link<T>: Node,
{
    pub fn file(link: Arc<Link<T>>, read: bool, write: bool, flags: FdFlags) -> Box<Self> {
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
