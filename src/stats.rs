use std::sync::atomic::AtomicUsize;

#[derive(Debug, Default)]
pub struct Stats {
    pub left_notify_yields_to_os: AtomicUsize,
    pub right_notify_yields_to_os: AtomicUsize,
    pub left_wait_yields_to_os: AtomicUsize,
    pub right_wait_yields_to_os: AtomicUsize,
}
