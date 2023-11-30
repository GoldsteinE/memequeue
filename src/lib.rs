#![allow(dead_code)]

use std::{
    io::{self, Write},
    mem, slice,
};

pub use crate::control::{
    Control, EventFdControl, EventFdControlConfig, ShmemFutexControl, ShmemFutexControlConfig,
};
use crate::{control::Side, handshake::HandshakeResult, mmap::Mmap};

mod control;
pub mod handshake;
mod mmap;

#[cfg(feature = "stats")]
pub mod stats;

#[macro_export]
macro_rules! debug_output {
    // ($($t:tt)*) => { eprintln!($($t)*) }
    ($($t:tt)*) => {}
    // ($($t:tt)*) => {
    //     unsafe {
    //         use std::fmt::Write as _;
    //         $crate::DEBUG.clear();
    //         $crate::DEBUG.write_fmt(format_args!($($t)*)).unwrap();
    //     }
    // }
}

pub struct MemeQueue<H, C> {
    // Note: field order is important, as it ensures proper drop order.
    control: C,
    left: Mmap,
    right: Mmap,
    handshake_result: H,
}

impl<H: HandshakeResult, C: Control<H>> MemeQueue<H, C> {
    pub fn new(handshake_result: H) -> io::Result<Self>
    where
        C::Config: Default,
    {
        Self::with_config(handshake_result, C::Config::default())
    }

    pub fn with_config(mut handshake_result: H, config: C::Config) -> io::Result<Self> {
        // SAFETY: guaranteed by `HandshakeResult`s contract.
        let mmap::QueueMmaps {
            left,
            right,
            header,
        } = unsafe {
            mmap::QueueMmaps::from_fd(&handshake_result.shmem_fd(), handshake_result.queue_size())?
        };
        let control = C::new(config, header, &mut handshake_result)?;
        handshake_result.mark_ready()?;
        Ok(Self {
            control,
            left,
            right,
            handshake_result,
        })
    }
}

impl<H, C: Control<H>> MemeQueue<H, C> {
    #[cfg(feature = "stats")]
    pub fn stats(&self) -> &crate::stats::Stats {
        self.control.stats()
    }

    pub fn recv<R, E, F>(&self, cb: F) -> Result<R, E>
    where
        F: FnOnce(&[u8]) -> Result<R, E>,
        E: From<io::Error>,
    {
        loop {
            let guard = self.control.lock(Side::Left);
            let left_offset = self.control.load_offset(Side::Left);
            let right_offset = {
                let cached = self.control.cached_offset(Side::Right);
                match cached {
                    Some(cached) if cached > left_offset => cached,
                    _ => self.control.sync_load_offset(Side::Right),
                }
            };

            if right_offset > left_offset {
                debug_assert!((right_offset - left_offset) as usize > mem::size_of::<usize>());
                // SAFETY: we keep offsets in-bounds
                let slice = unsafe {
                    let left_ptr = self.left.as_ptr().add(left_offset as usize);
                    let size = left_ptr.cast::<usize>().read_unaligned();
                    let data_ptr = left_ptr.add(mem::size_of::<usize>());
                    let slice = slice::from_raw_parts(data_ptr, size);
                    slice
                };
                let res = cb(slice);
                // TODO: should we commit offset if callback failed?
                self.control.commit_offset(
                    Side::Left,
                    left_offset + mem::size_of::<usize>() as u32 + slice.len() as u32,
                );
                drop(guard);
                debug_output!("notifying left about {}", left_offset + mem::size_of::<usize>() as u32 + slice.len() as u32);
                // Error safety: we already commited offset and will return soon regardless.
                self.control.notify(Side::Left)?;
                return res;
            } else {
                drop(guard);
                // Error safety: we're not in the middle of some operation,
                // so failing is OK.
                self.control.wait(Side::Right, right_offset)?;
            }
        }
    }

    pub fn send<R, E, F>(&self, cb: F) -> Result<R, E>
    where
        F: FnOnce(&mut MemeWriter<'_, H, C>) -> Result<R, E>,
        E: From<io::Error>,
    {
        let _guard = self.control.lock(Side::Right);
        let mut writer = MemeWriter {
            queue: self,
            total_written: 0,
            right_offset: self.control.load_offset(Side::Right),
        };
        // Space for size
        writer.write_all(&[0; mem::size_of::<usize>()])?;
        let res = cb(&mut writer);

        if res.is_ok() {
            let message_size = writer.total_written as usize - mem::size_of::<usize>();
            let right_offset = writer.right_offset;
            // SAFETY: we keep offsets in bounds
            unsafe {
                let right_ptr = self.left.as_ptr().add(right_offset as usize);
                right_ptr.cast::<usize>().write_unaligned(message_size);
            };
            self.control
                .commit_offset(Side::Right, right_offset + writer.total_written);
            // Error safety: we commited offset and will return soon regardless
            debug_output!("notifying right about {}", right_offset + writer.total_written);
            self.control.notify(Side::Right)?;
        }

        res
    }
}

pub struct MemeWriter<'a, H, C> {
    queue: &'a MemeQueue<H, C>,
    total_written: u32,
    right_offset: u32,
}

impl<H, C: Control<H>> Write for MemeWriter<'_, H, C> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let next_total_written = self.total_written as u64 + buf.len() as u64;
        if next_total_written > u32::MAX as u64
            || next_total_written > (self.queue.left.size() - mem::size_of::<usize>()) as u64
        {
            // TODO: maybe Ok(0)?
            return Err(io::Error::new(
                // TODO: should be `StorageFull`
                io::ErrorKind::Other,
                "tried to write too much",
            ));
        }

        let control = &self.queue.control;
        let left = &self.queue.left;
        let right = &self.queue.right;

        loop {
            let right_offset = self.right_offset + self.total_written;
            let left_offset = match control.cached_offset(Side::Left) {
                Some(offset)
                    if offset as usize + left.size() > right_offset as usize + buf.len() =>
                {
                    offset
                }
                _ => control.sync_load_offset(Side::Left),
            };

            let end = left
                .as_ptr()
                .wrapping_add(left_offset as usize + left.size());
            // SAFETY: one past the end
            let right_bound = unsafe { right.as_ptr().add(right.size()) };
            let end = end.min(right_bound);

            // SAFETY: should be in bounds
            let right_ptr = unsafe { left.as_ptr().add(right_offset as usize) };
            let space_left = (end as usize) - (right_ptr as usize);

            if space_left > 0 {
                let buf_part = &buf[..space_left.min(buf.len())];
                // SAFETY: we currently own all the space after the right pointer.
                // There is no way `buf` could overlap it. Even if it's from the same ringbuffer,
                // it would be from the reader part, not the writer part.
                unsafe {
                    std::ptr::copy_nonoverlapping(buf_part.as_ptr(), right_ptr, buf_part.len());
                }
                self.total_written += buf_part.len() as u32;
                return Ok(buf_part.len());
            } else if left_offset as usize >= left.size() {
                let _left_guard = control.lock(Side::Left);
                let left_offset = control.load_offset(Side::Left);
                let new_left_offset = left_offset - left.size() as u32;
                let new_right_offset = right_offset - self.total_written - left.size() as u32;
                control.fix_offsets(new_left_offset, new_right_offset);
                self.right_offset = new_right_offset;
            } else {
                // Error safety: there're two cases.
                // 1. If caller propagates the error, we won't commit anything, so it's safe.
                // 2. If caller hides the error, we will commit everything we've written.
                //    Size is calculated by `.total_written`, which is synchronized with actual
                //    bytes written, so it's ok, although the message will obviously be malformed.
                control.wait(Side::Left, left_offset)?;
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
