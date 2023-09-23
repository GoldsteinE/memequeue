use std::{
    marker::PhantomData,
    sync::atomic::{AtomicU32, Ordering},
};

pub(crate) struct ShmemRawMutex {
    futex: *const AtomicU32,
}

pub(crate) struct ShmemRawMutexGuard<'a> {
    mutex: &'a ShmemRawMutex,
}

impl ShmemRawMutex {
    // SAFETY: `futex` must be a pointer valid for the lifetime of `Self`
    pub(crate) unsafe fn new(futex: *const AtomicU32) -> Self {
        Self { futex }
    }

    fn futex(&self) -> &AtomicU32 {
        // SAFETY: pointer must be valid as per `::new()` contract
        unsafe { &*self.futex }
    }

    pub(crate) fn lock(&self) -> ShmemRawMutexGuard<'_> {
        // SAFETY: pointer must be valid as per `::new()` contract
        let futex = self.futex();
        loop {
            if futex
                .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return ShmemRawMutexGuard { mutex: self };
            }
            futex_wait(futex, 1);
        }
    }

    pub(crate) fn wait(&self) {
        let futex = self.futex();
        loop {
            if futex.load(Ordering::Relaxed) == 0 {
                return;
            }
            futex_wait(futex, 1);
        }
    }
}

impl Drop for ShmemRawMutexGuard<'_> {
    fn drop(&mut self) {
        let futex = self.mutex.futex();
        futex.store(0, Ordering::Release);
        // TODO: maybe don't wake all?
        futex_wake(futex, u32::MAX);
    }
}

fn futex_wake(futex: &AtomicU32, count: u32) {
    // SAFETY: we're passing valid params
    unsafe {
        libc::syscall(libc::SYS_futex, futex, libc::FUTEX_WAKE, count);
    }
}

fn futex_wait(futex: &AtomicU32, expected: u32) {
    // SAFETY: we're passing valid params
    unsafe {
        libc::syscall(
            libc::SYS_futex,
            futex,
            libc::FUTEX_WAIT,
            expected,
            std::ptr::null::<libc::timespec>(),
        );
    }
}
