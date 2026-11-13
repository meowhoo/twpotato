use std::sync::Arc;
use twime_userdata::{Service, Writer};

/// Shared snapshots and the single asynchronous mutation queue.
pub struct UserLink {
    pub service: Arc<Service>,
    pub writer: Writer,
}
