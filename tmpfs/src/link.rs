use std::sync::{Arc, Weak};

use wasmtime_vfs_ledger::Ledger;

use crate::node::Node;

use super::inode::{Data, Inode};

pub struct Link<T> {
    pub parent: Weak<dyn Node>,
    pub inode: Arc<Inode<T>>,
}

impl<T> Link<T>
where
    Self: Node,
{
    pub fn here(self: &Arc<Self>) -> Arc<dyn Node> {
        self.clone()
    }

    pub fn prev(self: &Arc<Self>) -> Arc<dyn Node> {
        match self.parent() {
            Some(root) => root,
            None => self.clone(),
        }
    }
}

impl<T> Link<T>
where
    Self: Node,
    T: Default,
{
    pub fn new(ledger: Arc<Ledger>) -> Self {
        let inode = Inode {
            id: ledger.create_device().create_inode(),
            data: Data::default().into(),
        };

        Self {
            parent: Weak::<Self>::new(),
            inode: inode.into(),
        }
    }
}
