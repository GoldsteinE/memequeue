use std::io;

use crate::mmap::Mmap;

mod shmem_futex;
pub use shmem_futex::{ShmemFutexControl, ShmemFutexControlConfig};

mod eventfd;
pub use eventfd::{EventFdControl, EventFdControlConfig};

#[derive(Debug, Clone, Copy)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    pub fn other(self) -> Self {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
}

pub trait Control<H>: Sized {
    type Config;
    type LockGuard<'a>
    where
        Self: 'a;

    #[cfg(feature = "stats")]
    fn stats(&self) -> &crate::stats::Stats;

    fn new(config: Self::Config, header: Mmap, handshake_result: &mut H) -> io::Result<Self>;
    fn lock(&self, side: Side) -> Self::LockGuard<'_>;
    // TODO: more flexible errors?
    fn wait(&self, side: Side, expected: u32) -> io::Result<()>;
    fn notify(&self, side: Side) -> io::Result<()>;

    fn load_offset(&self, side: Side) -> u32;
    fn sync_load_offset(&self, side: Side) -> u32;
    fn cached_offset(&self, side: Side) -> Option<u32>;
    fn commit_offset(&self, side: Side, offset: u32);
    fn fix_offsets(&self, left_offset: u32, right_offset: u32);
}
