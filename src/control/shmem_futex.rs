use std::{
    ptr,
    sync::atomic::{AtomicU32, Ordering},
};

use crate::{
    control::{Control, Side},
    mmap::Mmap,
};

// Aligned to cache line to improve cache hits.
#[repr(C, align(128))]
struct Half {
    offset: AtomicU32,
    lock: AtomicU32,
    cached_other_offset: AtomicU32,
}

#[repr(C)]
struct Header {
    left: Half,
    right: Half,
    // Waiters live outside of both cache lines, because they're commonly
    // needed by both sides.
    left_waiters: AtomicU32,
    right_waiters: AtomicU32,
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

    fn half(&self, side: Side) -> &Half {
        let header = self.header();
        match side {
            Side::Left => &header.left,
            Side::Right => &header.right,
        }
    }

    fn waiters(&self, side: Side) -> &AtomicU32 {
        let header = self.header();
        match side {
            Side::Left => &header.left_waiters,
            Side::Right => &header.right_waiters,
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
        let this = Self { header };
        let header = this.header();
        header
            .left
            .cached_other_offset
            .store(u32::MAX, Ordering::Relaxed);
        header
            .right
            .cached_other_offset
            .store(u32::MAX, Ordering::Relaxed);
        this
    }

    fn lock(&self, side: Side) -> Self::Guard<'_> {
        let futex = &self.half(side).lock;

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
        let half = self.half(side);
        let waiters = self.waiters(side);
        waiters.fetch_add(1, Ordering::AcqRel); // TODO: ordering
        futex_wait(&half.offset, expected);
        waiters.fetch_sub(1, Ordering::Release);
    }

    fn notify(&self, side: Side) {
        let half = self.half(side);
        // TODO: ordering
        if self.waiters(side).load(Ordering::Acquire) != 0 {
            futex_wake(&half.offset, 1);
        }
    }

    fn load_offset(&self, side: Side) -> u32 {
        self.half(side).offset.load(Ordering::Relaxed)
    }

    fn sync_load_offset(&self, side: Side) -> u32 {
        let res = self.half(side).offset.load(Ordering::Acquire);
        self.half(side.other())
            .cached_other_offset
            .store(res, Ordering::Relaxed);
        res
    }

    fn cached_offset(&self, side: Side) -> Option<u32> {
        let cached = self
            .half(side.other())
            .cached_other_offset
            .load(Ordering::Relaxed);

        (cached != u32::MAX).then_some(cached)
    }

    fn commit_offset(&self, side: Side, offset: u32) {
        self.half(side).offset.store(offset, Ordering::Release)
    }

    fn fix_offsets(&self, left_offset: u32, right_offset: u32) {
        let header = self.header();
        header.left.offset.store(left_offset, Ordering::Relaxed);
        header.right.offset.store(right_offset, Ordering::Relaxed);
        header
            .left
            .cached_other_offset
            .store(right_offset, Ordering::Relaxed);
        header
            .right
            .cached_other_offset
            .store(left_offset, Ordering::Relaxed);
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
