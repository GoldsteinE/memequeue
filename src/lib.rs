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
            let file_len = (header_size + size * 2) as u64;
            file.set_len(file_len)?;
        }

        let queue_part_size = (file.metadata()?.len() as usize).saturating_sub(header_size);
        if queue_part_size % 2 != 0
            || (queue_part_size / 2) % *PAGE_SIZE != 0
            || queue_part_size == 0
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "passed a non-empty file of wrong size",
            ));
        }
        let queue_size = queue_part_size / 2;

        let header = Mmap::new_file(header_size, 0, file.as_raw_fd())?;
        if need_to_init {
            // SAFETY: pointer is valid for the whole mmap
            unsafe {
                header.ptr().as_ptr().write_bytes(0, header_size);
            }
        }
        let big_mmap = Mmap::new_file(queue_part_size, header_size, file.as_raw_fd())?;
        let left = Mmap::new_file_at(big_mmap.ptr(), queue_size, header_size, file.as_raw_fd())?;
        // SAFETY: we offset into the second part of `big_mmap`
        let right_ptr = unsafe { NonNull::new_unchecked(big_mmap.ptr().as_ptr().add(queue_size)) };
        let right = Mmap::new_file_at(
            right_ptr,
            queue_size,
            header_size + queue_size,
            file.as_raw_fd(),
        )?;
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

    fn alloc_for_write(&self, size: usize) -> *mut u8 {
        let header = self.header();

        loop {
            let left = header.left.load(Ordering::Relaxed);
            if left > self.size {
                let _left_lock = self.left_lock.lock();
                header.left.fetch_sub(self.size, Ordering::Relaxed);
                header.right.fetch_sub(self.size, Ordering::Relaxed);
            }
            let window_end = left + self.size;
            let right = header.right.load(Ordering::Relaxed);
            let space_available = window_end.saturating_sub(right);
            if space_available >= size {
                header.right.fetch_add(size, Ordering::Relaxed);
                // SAFETY: offset should be in bounds
                return unsafe { self.left.ptr().as_ptr().add(right) };
            }
            self.left_lock.wait();
        }
    }

    pub fn write<R>(&self, f: impl FnOnce(&mut MemeWriter<'_>) -> R) -> R {
        let _right_guard = self.right_lock.lock();
        let len_ptr = self.alloc_for_write(8);
        // SAFETY: copying into memory we just alloced
        unsafe { ptr::copy_nonoverlapping([0; 8].as_ptr(), len_ptr, 8) };
        let mut writer = MemeWriter {
            queue: self,
            total_written: 8,
        };
        let mut_writer = &mut writer;
        let res = f(mut_writer);
        let written = writer.total_written - 8;
        // SAFETY: copying into memory we just alloced
        unsafe { ptr::copy_nonoverlapping((written as u64).to_le_bytes().as_ptr(), len_ptr, 8) };

        res
    }

    pub fn read<R>(&self, f: impl FnOnce(&[u8]) -> R) -> R {
        loop {
            let _left_guard = self.left_lock.lock();
            let header = self.header();
            let left = header.left.load(Ordering::Relaxed);
            let right = header.right.load(Ordering::Relaxed);
            if right.saturating_sub(left) > 0 {
                debug_assert!(right - left >= 8);
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
                header.left.store(left + 8 + size, Ordering::Relaxed);
                return f(data);
            }
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

        let dest_ptr = self.queue.alloc_for_write(buf.len());
        // SAFETY:
        // 1. ptr should be in bounds
        // 2. buf is external, so can't overlap
        // 3. even if buf comes from read, it's the different part of the queue
        unsafe { ptr::copy_nonoverlapping(buf.as_ptr(), dest_ptr, buf.len()) }
        self.total_written += buf.len();

        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
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

    #[test]
    fn known_bad() {
        let actions = [
            Action::Write(vec![0; 30]),
            Action::Read,
            Action::Write(vec![0; 81]),
            Action::Write(vec![0; 46]),
            Action::Write(vec![0; 67]),
            Action::Read,
            Action::Read,
            Action::Read,
            Action::Write(vec![0; 68]),
            Action::Read,
            Action::Write(vec![0; 72]),
            Action::Write(vec![0; 99]),
            Action::Write(vec![0; 46]),
            Action::Write(vec![0; 88]),
            Action::Write(vec![0; 76]),
            Action::Read,
            Action::Write(vec![0; 32]),
            Action::Read,
            Action::Read,
            Action::Read,
            Action::Write(vec![0; 26]),
            Action::Write(vec![0; 37]),
            Action::Write(vec![0; 46]),
            Action::Write(vec![0; 10]),
            Action::Read,
            Action::Write(vec![0; 63]),
            Action::Write(vec![0; 22]),
            Action::Write(vec![0; 49]),
            Action::Write(vec![0; 59]),
            Action::Write(vec![0; 41]),
            Action::Write(vec![0; 45]),
            Action::Write(vec![0; 97]),
            Action::Write(vec![0; 18]),
            Action::Write(vec![0; 51]),
            Action::Read,
            Action::Read,
            Action::Write(vec![0; 64]),
            Action::Write(vec![0; 83]),
            Action::Read,
            Action::Write(vec![0; 11]),
            Action::Read,
            Action::Write(vec![0; 60]),
            Action::Write(vec![0; 23]),
            Action::Read,
            Action::Read,
            Action::Read,
            Action::Write(vec![0; 96]),
            Action::Read,
            Action::Write(vec![0; 70]),
            Action::Read,
            Action::Read,
            Action::Write(vec![0; 64]),
            Action::Write(vec![0; 26]),
            Action::Write(vec![0; 34]),
            Action::Read,
            Action::Write(vec![0; 42]),
            Action::Read,
            Action::Write(vec![0; 92]),
            Action::Write(vec![0; 75]),
            Action::Read,
            Action::Read,
            Action::Write(vec![0; 68]),
            Action::Write(vec![0; 47]),
            Action::Read,
            Action::Read,
            Action::Read,
            Action::Read,
            Action::Read,
            Action::Read,
            Action::Read,
            Action::Read,
            Action::Write(vec![0; 59]),
            Action::Write(vec![0; 27]),
            Action::Read,
            Action::Write(vec![0; 49]),
            Action::Write(vec![0; 90]),
            Action::Write(vec![0; 74]),
            Action::Write(vec![0; 24]),
            Action::Write(vec![0; 95]),
            Action::Write(vec![0; 70]),
            Action::Write(vec![0; 28]),
            Action::Read,
            Action::Write(vec![0; 37]),
            Action::Write(vec![0; 91]),
            Action::Read,
            Action::Write(vec![0; 26]),
            Action::Write(vec![0; 8]),
            Action::Read,
            Action::Write(vec![0; 44]),
            Action::Read,
            Action::Read,
            Action::Write(vec![0; 91]),
            Action::Read,
            Action::Read,
            Action::Read,
            Action::Write(vec![0; 59]),
            Action::Read,
            Action::Write(vec![0; 73]),
            Action::Write(vec![0; 30]),
            Action::Read,
            Action::Read,
            Action::Read,
            Action::Write(vec![0; 55]),
            Action::Write(vec![0; 25]),
            Action::Write(vec![0; 75]),
            Action::Read,
            Action::Read,
            Action::Read,
            Action::Read,
            Action::Read,
            Action::Read,
            Action::Read,
            Action::Write(vec![0; 0]),
            Action::Read,
            Action::Read,
            Action::Read,
            Action::Read,
            Action::Read,
            Action::Read,
            Action::Read,
            Action::Read,
            Action::Write(vec![0; 86]),
            Action::Write(vec![0; 39]),
            Action::Read,
            Action::Write(vec![0; 83]),
            Action::Write(vec![0; 51]),
            Action::Read,
            Action::Write(vec![0; 56]),
            Action::Write(vec![0; 1]),
            Action::Read,
            Action::Read,
            Action::Read,
            Action::Read,
            Action::Write(vec![0; 0]),
            Action::Read,
        ];

        let file = tempfile::NamedTempFile::new().unwrap();
        let mut to_read = VecDeque::new();
        let queue = MemeQueue::from_path(file.path(), 200).unwrap();
        for action in actions {
            match action {
                Action::Read => {
                    let Some(expected) = to_read.pop_front() else {
                        continue;
                    };
                    let data = queue.read(|buf| buf.to_owned());
                    assert_eq!(data, expected);
                }
                Action::Write(buf) => {
                    queue.write(|writer| writer.write_all(&buf)).unwrap();
                    to_read.push_back(buf);
                }
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            timeout: 100,
            ..ProptestConfig::default()
        })]

        #[test]
        fn simple(actions in proptest::collection::vec(action(), 0..1000)) {
            let file = tempfile::NamedTempFile::new().unwrap();
            let mut to_read = VecDeque::new();
            let queue = MemeQueue::from_path(file.path(), 200).unwrap();
            for action in actions {
                match action {
                    Action::Read => {
                        let Some(expected) = to_read.pop_front() else { continue };
                        let data = queue.read(|buf| buf.to_owned());
                        prop_assert_eq!(data, expected);
                    },
                    Action::Write(buf) => {
                        queue.write(|writer| writer.write_all(&buf)).unwrap();
                        to_read.push_back(buf);
                    }
                }
            }
        }
    }

    prop_compose! {
        fn action()(opt in proptest::option::of(0_usize..100)) -> Action {
            match opt {
                None => Action::Read,
                Some(x) => Action::Write(vec![0; x]),
            }
        }
    }
}
