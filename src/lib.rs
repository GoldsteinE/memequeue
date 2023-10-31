#![allow(dead_code)]

use std::{
    fs::File,
    io::{self, Write},
    mem, slice,
};

pub use crate::control::{Control, ShmemFutexControl};
use crate::{control::Side, mmap::Mmap};

mod control;
mod mmap;

pub struct MemeQueue<C> {
    control: C,
    left: Mmap,
    right: Mmap,
    // TODO: hide it behind generic somehow?
    file: File,
}

impl<C: Control> MemeQueue<C> {
    // TODO: should't exist
    /// # Safety
    /// none.
    pub unsafe fn from_file(file: File, queue_size: usize, master: bool) -> io::Result<Self> {
        let mmaps = if master {
            mmap::QueueMmaps::create_from_file(&file, queue_size)?
        } else {
            mmap::QueueMmaps::from_fd(&file, queue_size)?
        };
        let control = C::from_header(mmaps.header);
        Ok(Self {
            control,
            left: mmaps.left,
            right: mmaps.right,
            file,
        })
    }

    pub fn recv<R, F>(&self, cb: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
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
                self.control.commit_offset(
                    Side::Left,
                    left_offset + mem::size_of::<usize>() as u32 + slice.len() as u32,
                );
                drop(guard);
                self.control.notify(Side::Left);
                return res;
            } else {
                drop(guard);
                self.control.wait(Side::Right, right_offset);
            }
        }
    }

    pub fn send<R, E, F>(&self, cb: F) -> Result<R, E>
    where
        F: FnOnce(&mut MemeWriter<'_, C>) -> Result<R, E>,
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
            self.control.notify(Side::Right);
        }

        res
    }
}

pub struct MemeWriter<'a, C> {
    queue: &'a MemeQueue<C>,
    total_written: u32,
    right_offset: u32,
}

impl<C: Control> Write for MemeWriter<'_, C> {
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
                control.wait(Side::Left, left_offset);
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
