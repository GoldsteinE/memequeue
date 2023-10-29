use std::{
    ptr,
    sync::atomic::{AtomicU32, Ordering},
};

use crate::{
    control::{Control, Side},
    mmap::Mmap,
};

#[repr(C)]
struct Header {
    left_offset: AtomicU32,
    right_offset: AtomicU32,
    left_lock: AtomicU32,
    right_lock: AtomicU32,
}

pub struct ShmemFutexControl {
    header: Mmap,
}

impl ShmemFutexControl {
    fn header(&self) -> &Header {
        // SAFETY:
        // 1. mmaps are page-aligned
        // 2. all values are valid for u32
        unsafe { &*self.header.as_ptr().cast() }
    }

    fn offset(&self, side: Side) -> &AtomicU32 {
        let header = self.header();
        match side {
            Side::Left => &header.left_offset,
            Side::Right => &header.right_offset,
        }
    }

    fn futex(&self, side: Side) -> &AtomicU32 {
        let header = self.header();
        match side {
            Side::Left => &header.left_lock,
            Side::Right => &header.right_lock,
        }
    }
}

pub struct ShmemFutexGuard<'a> {
    futex: &'a AtomicU32,
}

impl Control for ShmemFutexControl {
    type Guard<'a> = ShmemFutexGuard<'a>
    where
        Self: 'a;

    fn from_header(header: Mmap) -> Self {
        Self { header }
    }

    fn lock(&self, side: Side) -> Self::Guard<'_> {
        let futex = self.futex(side);

        if futex
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            while futex.swap(2, Ordering::Acquire) != 0 {
                futex_wait(futex, 2);
            }
        }

        ShmemFutexGuard { futex }
    }

    fn wait(&self, side: Side, expected: u32) {
        futex_wait(self.offset(side), expected);
    }

    fn notify(&self, side: Side) {
        futex_wake(self.offset(side), 1);
    }

    fn load_offset(&self, side: Side) -> u32 {
        self.offset(side).load(Ordering::Relaxed)
    }

    fn sync_load_offset(&self, side: Side) -> u32 {
        self.offset(side).load(Ordering::Acquire)
    }

    fn store_offset(&self, side: Side, offset: u32) {
        self.offset(side).store(offset, Ordering::Relaxed)
    }

    fn commit_offset(&self, side: Side, offset: u32) {
        self.offset(side).store(offset, Ordering::Release)
    }
}

impl Drop for ShmemFutexGuard<'_> {
    fn drop(&mut self) {
        if self.futex.swap(0, Ordering::Release) == 2 {
            futex_wake(self.futex, 1);
        }
    }
}

fn futex_wake(futex: &AtomicU32, count: u32) {
    // SAFETY: futex operations are safe and we're passing all the right arguments.
    unsafe {
        libc::syscall(libc::SYS_futex, futex, libc::FUTEX_WAKE, count);
    }
}

fn futex_wait(futex: &AtomicU32, expected: u32) {
    // SAFETY: futex operations are safe and we're passing all the right arguments.
    unsafe {
        libc::syscall(
            libc::SYS_futex,
            futex,
            libc::FUTEX_WAIT,
            expected,
            ptr::null::<libc::timespec>(),
        );
    }
}
