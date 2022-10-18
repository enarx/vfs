mod dir;
mod file;

use std::sync::Arc;

use super::link::Link;
use crate::node::Node;

pub struct Open<L, D> {
    _root: Arc<dyn Node>,
    link: Arc<Link<L>>,
    data: D,
}
