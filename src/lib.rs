#![allow(dead_code, unused_variables, unused_mut, unused_imports)]

mod shmem_mutex;

mod flock;
use flock::Flock;
use once_cell::sync::Lazy;

mod mmap;
use mmap::Mmap;

use std::{
    ffi::CString,
    fs::{self, File},
    io::{self, Write},
    mem::{self, MaybeUninit},
    os::{
        fd::{AsRawFd, IntoRawFd},
        unix::prelude::{FileExt, OsStringExt as _},
    },
    path::Path,
    ptr::{self, NonNull},
    slice,
    sync::atomic::{AtomicPtr, AtomicU32, AtomicUsize, Ordering},
};

use libc::{O_CREAT, O_EXCL, O_RDWR, O_TRUNC};

use crate::shmem_mutex::ShmemRawMutex;

// SAFETY: we're passing a valid param
static PAGE_SIZE: Lazy<usize> = Lazy::new(|| unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize });

macro_rules! debug_log {
    ($($t:tt)*) => {};
}

#[repr(C)]
struct Header {
    left_lock: AtomicU32,
    right_lock: AtomicU32,
    left: AtomicUsize,
    right: AtomicUsize,
}

pub struct MemeQueue {
    size: usize,
    left_lock: ShmemRawMutex,
    right_lock: ShmemRawMutex,
    header: Mmap,
    left: Mmap,
    right: Mmap,
    file: File,
}

impl MemeQueue {
    pub fn from_path(path: impl AsRef<Path>, size: usize) -> io::Result<Self> {
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(path)?;

        let _flock = Flock::lock(&file)?;

        let header_size = mem::size_of::<Header>().max(*PAGE_SIZE);
        let need_to_init = file.metadata()?.len() == 0;
        // This file is not prepared to be a queue, initialize it
        if need_to_init {
            // Queue size must be multiple of page size.
            let size = size.next_multiple_of(*PAGE_SIZE);
            let file_len = (header_size + size) as u64;
            file.set_len(file_len)?;
        }

        let queue_size = (file.metadata()?.len() as usize).saturating_sub(header_size);
        if queue_size % *PAGE_SIZE != 0 || queue_size == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "passed a non-empty file of wrong size",
            ));
        }

        let header = Mmap::new_file(header_size, 0, file.as_raw_fd())?;
        if need_to_init {
            // SAFETY: pointer is valid for the whole mmap
            unsafe {
                header.ptr().as_ptr().write_bytes(0, header_size);
            }
        }
        let big_mmap = Mmap::new_file(queue_size * 2, header_size, file.as_raw_fd())?;
        let left = Mmap::new_file_at(big_mmap.ptr(), queue_size, header_size, file.as_raw_fd())?;
        // SAFETY: we offset into the second part of `big_mmap`
        let right_ptr = unsafe { NonNull::new_unchecked(big_mmap.ptr().as_ptr().add(queue_size)) };
        let right = Mmap::new_file_at(right_ptr, queue_size, header_size, file.as_raw_fd())?;
        // Now completely covered by left and right mmaps.
        mem::forget(big_mmap);

        // SAFETY: pointer into map is valid + all values are valid for Header
        let header_ref = unsafe { &*header.ptr().as_ptr().cast::<Header>() };
        // SAFETY: pointer will be valid for lifetime of `Self`
        let left_lock = unsafe { ShmemRawMutex::new(&header_ref.left_lock) };
        let right_lock = unsafe { ShmemRawMutex::new(&header_ref.right_lock) };

        Ok(Self {
            size: queue_size,
            left_lock,
            right_lock,
            header,
            left,
            right,
            file,
        })
    }

    fn header(&self) -> &Header {
        // SAFETY: pointer into map is valid + all values are valid for Header
        unsafe { &*self.header.ptr().as_ptr().cast::<Header>() }
    }

    fn alloc_for_write(&self, size: usize) -> WritePermission {
        let header = self.header();

        loop {
            let left = header.left.load(Ordering::Relaxed);
            if left > self.size {
                debug_log!("acquiring left lock for maintenance");
                let _left_lock = self.left_lock.lock();
                header.left.fetch_sub(self.size, Ordering::Relaxed);
                header.right.fetch_sub(self.size, Ordering::Relaxed);
                debug_log!("releasing left lock for maintenance");
                continue;
            }
            let window_end = left + self.size;
            let right = header.right.load(Ordering::Relaxed);
            let space_available = window_end.saturating_sub(right);
            if space_available >= size {
                debug_log!("allowing write for {size} bytes; window = {left}..{}; write window = {right}..{}", left + self.size, right + size);
                return WritePermission {
                    start: right,
                    end: right + size,
                };
            }
            self.left_lock.wait();
        }
    }

    pub fn send<R>(&self, f: impl FnOnce(&mut MemeWriter<'_>) -> R) -> R {
        debug_log!("acquiring right lock for write");
        let _right_guard = self.right_lock.lock();
        let perm = self.alloc_for_write(8);
        let right = self.header().right.load(Ordering::Relaxed);
        // SAFETY: copying into memory we just alloced
        let mut writer = MemeWriter {
            queue: self,
            total_written: 8,
        };
        let res = f(&mut writer);
        // TODO: error handling!
        writer.flush().unwrap();
        let written = writer.total_written - 8;
        debug_log!("writing size = {written} to {}..{}", right, right + 8);
        // SAFETY: copying into memory we just alloced
        perm.write(
            self.left.ptr(),
            (written as u64).to_le_bytes().as_ptr(),
            right,
            8,
        );
        self.header()
            .right
            .fetch_add(writer.total_written, Ordering::Relaxed);

        debug_log!("releasing write lock for right");
        res
    }

    pub fn recv<R>(&self, f: impl FnOnce(&[u8]) -> R) -> R {
        loop {
            debug_log!("acquiring left lock for read");
            let left_guard = self.left_lock.lock();
            let header = self.header();
            let left = header.left.load(Ordering::Relaxed);
            let right = header.right.load(Ordering::Relaxed);
            if right.saturating_sub(left) > 0 {
                debug_assert!(right - left >= 8);

                debug_log!("reading size from {left}..{}", left + 8);
                let size = {
                    let mut buf = [0; 8];
                    let left_ptr = unsafe { self.left.ptr().as_ptr().add(left) };
                    // SAFETY: copying from valid memory to a stack array
                    unsafe {
                        ptr::copy_nonoverlapping(left_ptr.cast_const(), buf.as_mut_ptr(), 8);
                    }
                    u64::from_le_bytes(buf) as usize
                };
                // SAFETY: reading data we previously written
                let data = unsafe {
                    slice::from_raw_parts(self.left.ptr().as_ptr().add(left).add(8), size)
                };

                debug_log!(
                    "reading {size} bytes of data from {}..{}",
                    left + 8,
                    left + 8 + size,
                );
                header.left.store(left + 8 + size, Ordering::Relaxed);
                debug_log!("releasing left lock for read");
                return f(data);
            }
            debug_log!("releasing left lock for read, waiting for message");
            drop(left_guard);
            self.right_lock.wait();
        }
    }
}

pub struct MemeWriter<'a> {
    queue: &'a MemeQueue,
    total_written: usize,
}

impl Write for MemeWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let header = self.queue.header();

        if buf.len() > self.total_written + self.queue.size {
            return Err(io::Error::new(
                // TODO: should be StorageFull?
                io::ErrorKind::Other,
                "message is longer than the queue",
            ));
        }

        let perm = self.queue.alloc_for_write(self.total_written + buf.len());
        let right = header.right.load(Ordering::Relaxed);
        // SAFETY: staying inside of our allocation
        let dest_ptr = unsafe {
            self.queue
                .left
                .ptr()
                .as_ptr()
                .add(right + self.total_written)
        };
        debug_log!(
            "writing {} bytes of data to {}..{}",
            buf.len(),
            right + self.total_written,
            right + self.total_written + buf.len(),
        );
        // SAFETY:
        // 1. ptr should be in bounds
        // 2. buf is external, so can't overlap
        // 3. even if buf comes from read, it's the different part of the queue
        perm.write(
            self.queue.left.ptr(),
            buf.as_ptr(),
            right + self.total_written,
            buf.len(),
        );
        self.total_written += buf.len();

        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if unsafe {
            libc::msync(
                self.queue.left.ptr().as_ptr().cast(),
                self.queue.size * 2,
                libc::MS_SYNC,
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

struct WritePermission {
    start: usize,
    end: usize,
}

impl WritePermission {
    #[inline]
    fn write(&self, base_ptr: NonNull<u8>, source: *const u8, offset: usize, count: usize) {
        debug_assert!(offset >= self.start);
        debug_assert!(offset + count <= self.end);
        unsafe { ptr::copy_nonoverlapping(source, base_ptr.as_ptr().add(offset), count) }
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, io::Write as _};

    use proptest::{prelude::ProptestConfig, prop_assert_eq, prop_compose, proptest};

    use super::MemeQueue;

    #[derive(Debug)]
    enum Action {
        Read,
        Write(Vec<u8>),
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            timeout: 100,
            ..ProptestConfig::default()
        })]

        #[test]
        fn simple(actions in proptest::collection::vec(action(), 0..100)) {
            let file = tempfile::NamedTempFile::new().unwrap();
            let mut to_read = VecDeque::new();
            let queue = MemeQueue::from_path(file.path(), 4096).unwrap();
            assert_eq!(queue.size, 4096);
            let mut available_space = 4096;
            for action in actions {
                match action {
                    Action::Read => {
                        let Some(expected) = to_read.pop_front() else { continue };
                        let data = queue.recv(|buf| buf.to_owned());
                        available_space += data.len() + 8;
                        prop_assert_eq!(data, expected);
                    },
                    Action::Write(buf) => {
                        if buf.len() + 8 > available_space {
                            continue;
                        }

                        queue.send(|writer| writer.write_all(&buf)).unwrap();
                        available_space -= buf.len() + 8;
                        to_read.push_back(buf);
                    }
                }
            }
        }
    }

    prop_compose! {
        fn action()(opt in proptest::option::of(0_usize..1000)) -> Action {
            match opt {
                None => Action::Read,
                Some(x) => Action::Write(vec![0; x]),
            }
        }
    }
}
