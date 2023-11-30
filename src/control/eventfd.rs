use std::{
    io,
    os::fd::RawFd,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{
    control::shmem_futex::ShmemFutexGuard,
    control::Side,
    handshake::{ExchangeFd, HandshakeResult},
    mmap::Mmap,
    Control, ShmemFutexControl, ShmemFutexControlConfig,
};

#[derive(Debug, Default, Clone)]
pub struct EventFdControlConfig {}

pub struct EventFdControl {
    // TODO: abstract the locks + offsets part?
    shmem_futex: ShmemFutexControl,
    left_event: RawFd,
    right_event: RawFd,
    last_notify: AtomicU64,
    clock: quanta::Clock,
}

pub struct EventFdGuard<'a>(ShmemFutexGuard<'a>);

impl EventFdControl {
    fn event(&self, side: Side) -> RawFd {
        match side {
            Side::Left => self.left_event,
            Side::Right => self.right_event,
        }
    }
}

impl<H: HandshakeResult + ExchangeFd> Control<H> for EventFdControl {
    type Config = EventFdControlConfig;

    type LockGuard<'a> = EventFdGuard<'a>
    where
        Self: 'a;

    #[cfg(feature = "stats")]
    fn stats(&self) -> &crate::stats::Stats {
        Control::<H>::stats(&self.shmem_futex)
    }

    fn new(_config: Self::Config, header: Mmap, handshake_result: &mut H) -> io::Result<Self> {
        let (left_event, right_event) = if handshake_result.is_owner() {
            let left_event = unsafe { libc::eventfd(0, 0) };
            if left_event < 0 {
                return Err(io::Error::last_os_error());
            }

            let right_event = unsafe { libc::eventfd(0, 0) };
            if right_event < 0 {
                return Err(io::Error::last_os_error());
            }

            handshake_result.send_fd(left_event)?;
            handshake_result.send_fd(right_event)?;

            (left_event, right_event)
        } else {
            (handshake_result.recv_fd()?, handshake_result.recv_fd()?)
        };

        // TODO: translate meaningful config options
        let shmem_futex =
            ShmemFutexControl::new(ShmemFutexControlConfig::default(), header, handshake_result)?;

        let clock = quanta::Clock::new();
        clock.now(); // heat it up
        Ok(Self {
            shmem_futex,
            left_event,
            right_event,
            last_notify: AtomicU64::new(0),
            clock,
        })
    }

    fn lock(&self, side: Side) -> Self::LockGuard<'_> {
        EventFdGuard(Control::<H>::lock(&self.shmem_futex, side))
    }

    #[inline(never)]
    fn wait(&self, side: Side, expected: u32) -> io::Result<()> {
        let half = self.shmem_futex.half(side);

        let before_inc = self.clock.raw();
        self.shmem_futex
            .waiters(side)
            .fetch_add(1, Ordering::SeqCst); // TODO: ordering
        let after_inc = self.clock.raw();
        if half.offset.load(Ordering::SeqCst) == expected {
            let inside_if = self.clock.raw();
            #[cfg(feature = "stats")]
            match side {
                Side::Left => Control::<H>::stats(self)
                    .left_wait_yields_to_os
                    .fetch_add(1, Ordering::Relaxed),
                Side::Right => Control::<H>::stats(self)
                    .right_wait_yields_to_os
                    .fetch_add(1, Ordering::Relaxed),
            };

            crate::debug_output!("waiting for {side:?} to change from {expected:?}");

            let mut pfd = libc::pollfd {
                fd: self.event(side),
                events: libc::POLLIN,
                revents: 0,
            };
            let res = unsafe { libc::poll(&mut pfd, 1, 5000 + 1000 * side as i32) };
            if res < 1 {
                println!("We waited for {side:?} to change from {expected}, but...");
                println!("Oh no, we deadlocked (or at least `poll()` from {} returned {res}). That's bad.", self.event(side));
                println!("revents is {}, btw", pfd.revents);
                println!(
                    "We should send the other side 0xDEAD so they can laugh at our common demise."
                );
                let res = unsafe {
                    libc::write(
                        self.event(side.other()),
                        &0xDEAD_u64.to_ne_bytes() as *const _ as *const _,
                        8,
                    )
                };
                println!(
                    "Nothing will save us. Even write to {}, which resulted in {res}.",
                    self.event(side.other()),
                );
                println!("Timings, for your dark amusement:");
                dbg!(
                    before_inc,
                    after_inc,
                    inside_if,
                    self.last_notify.load(Ordering::Relaxed)
                );
                println!(
                    "My last word would be this: {:?}",
                    self.shmem_futex.header()
                );
                panic!("Goodbye, cruel world.");
            }

            let mut buf = [0_u8; 8];
            // SAFETY: we're passing a valid length-8 buffer
            let res = unsafe { libc::read(self.event(side), buf.as_mut_ptr().cast(), 8) };
            if res < 0 {
                return Err(io::Error::last_os_error());
            }
            if u64::from_ne_bytes(buf) == 0xDEAD {
                println!("Lmao, the other side deadlocked. What a loser. Surely it's their fault.");
                println!("We waited for {side:?} to change from {expected}, but guess that'll never happen now.");
                println!("Well, here's your header, maybe you'll find out why they're such a fuckup: {:?}", self.shmem_futex.header());
                println!("You could also use some timings ig:");
                dbg!(
                    before_inc,
                    after_inc,
                    inside_if,
                    self.last_notify.load(Ordering::Relaxed)
                );
                panic!("Welp, nothing we can do about it.");
            }
        }
        self.shmem_futex
            .waiters(side)
            .fetch_sub(1, Ordering::SeqCst);

        Ok(())
    }

    #[inline(never)]
    fn notify(&self, side: Side) -> io::Result<()> {
        // TODO: ordering
        if self.shmem_futex.waiters(side).load(Ordering::SeqCst) != 0 {
            crate::debug_output!("sending notification to {side:?}");
            #[cfg(feature = "stats")]
            match side {
                Side::Left => Control::<H>::stats(self)
                    .left_notify_yields_to_os
                    .fetch_add(1, Ordering::Relaxed),
                Side::Right => Control::<H>::stats(self)
                    .right_notify_yields_to_os
                    .fetch_add(1, Ordering::Relaxed),
            };
            self.last_notify.store(self.clock.raw(), Ordering::Relaxed);
            // SAFETY: we're passing a valid length-8 buffer
            let res =
                unsafe { libc::write(self.event(side), 1_u64.to_ne_bytes().as_ptr().cast(), 8) };
            if res < 0 {
                return Err(io::Error::last_os_error());
            }
        }

        Ok(())
    }

    fn load_offset(&self, side: Side) -> u32 {
        Control::<H>::load_offset(&self.shmem_futex, side)
    }

    fn sync_load_offset(&self, side: Side) -> u32 {
        Control::<H>::sync_load_offset(&self.shmem_futex, side)
    }

    fn cached_offset(&self, side: Side) -> Option<u32> {
        Control::<H>::cached_offset(&self.shmem_futex, side)
    }

    fn commit_offset(&self, side: Side, offset: u32) {
        Control::<H>::commit_offset(&self.shmem_futex, side, offset)
    }

    fn fix_offsets(&self, left_offset: u32, right_offset: u32) {
        Control::<H>::fix_offsets(&self.shmem_futex, left_offset, right_offset);
    }
}
