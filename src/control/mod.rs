use crate::mmap::Mmap;

mod shmem_futex;
pub use shmem_futex::ShmemFutexControl;

#[derive(Clone, Copy)]
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

pub trait Control {
    type Guard<'a>
    where
        Self: 'a;

    fn from_header(header: Mmap) -> Self;
    fn lock(&self, side: Side) -> Self::Guard<'_>;
    fn wait(&self, side: Side, expected: u32);
    fn notify(&self, side: Side);

    fn load_offset(&self, side: Side) -> u32;
    fn sync_load_offset(&self, side: Side) -> u32;
    fn commit_offset(&self, side: Side, offset: u32);
    fn fix_offsets(&self, left_offset: u32, right_offset: u32);
}
