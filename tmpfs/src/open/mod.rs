mod dir;
mod file;

use std::sync::Arc;

use tokio::sync::RwLock;
use wasi_common::file::FdFlags;

use super::link::Link;

pub struct State {
    flags: FdFlags,
    pos: usize,
}

impl From<FdFlags> for State {
    fn from(flags: FdFlags) -> Self {
        Self { flags, pos: 0 }
    }
}

impl Default for State {
    fn default() -> Self {
        FdFlags::empty().into()
    }
}

pub struct Open {
    _root: Arc<Link>,
    link: Arc<Link>,
    state: RwLock<State>,
    write: bool,
    read: bool,
}

impl Open {
    pub fn new(link: Arc<Link>, read: bool, write: bool, flags: FdFlags) -> Self {
        let mut _root = link.clone();

        loop {
            _root = match _root.parent.upgrade() {
                Some(parent) => parent,
                None => break,
            };
        }

        Self {
            _root,
            link,
            state: State::from(flags).into(),
            write,
            read,
        }
    }
}

impl From<Arc<Link>> for Open {
    fn from(link: Arc<Link>) -> Self {
        Self::new(link, false, false, FdFlags::empty())
    }
}

impl From<&Arc<Link>> for Open {
    fn from(link: &Arc<Link>) -> Self {
        link.clone().into()
    }
}
