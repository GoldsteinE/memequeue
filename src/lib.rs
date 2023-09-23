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

    fn alloc_for_write(&self, size: usize, reason: &'static str) -> *mut u8 {
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
                #[cfg(skip)]
                println!(
                    "writing {size} bytes of {reason} to {}..{}",
                    right,
                    right + size,
                );
                // SAFETY: offset should be in bounds
                return unsafe { self.left.ptr().as_ptr().add(right) };
            }
            self.left_lock.wait();
        }
    }

    pub fn write<R>(&self, f: impl FnOnce(&mut MemeWriter<'_>) -> R) -> R {
        let _right_guard = self.right_lock.lock();
        let len_ptr = self.alloc_for_write(8, "size");
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
        self.header()
            .right
            .fetch_add(writer.total_written, Ordering::Relaxed);

        res
    }

    pub fn read<R>(&self, f: impl FnOnce(&[u8]) -> R) -> R {
        let _left_guard = self.left_lock.lock();
        loop {
            let header = self.header();
            let left = header.left.load(Ordering::Relaxed);
            let right = header.right.load(Ordering::Relaxed);
            if right.saturating_sub(left) > 0 {
                debug_assert!(right - left >= 8);
                #[cfg(skip)]
                println!("reading size from {left}..{}", left + 8);
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
                #[cfg(skip)]
                println!(
                    "reading {size} bytes of data from {}..{}",
                    left + 8,
                    left + 8 + size,
                );
                header.left.store(left + 8 + size, Ordering::Relaxed);
                return f(data);
            }
            self.right_lock.wait();
        }
    }

    pub fn export_state(&self) -> MemeState {
        let mut buf = vec![0_u8; self.size];
        let mut right_buf = vec![0_u8; self.size];
        unsafe {
            ptr::copy_nonoverlapping(
                self.left.ptr().as_ptr().cast_const(),
                buf.as_mut_ptr(),
                self.size,
            );
            ptr::copy_nonoverlapping(
                self.right.ptr().as_ptr().cast_const(),
                right_buf.as_mut_ptr(),
                self.size,
            );
        }
        let left_pointer = self.header().left.load(Ordering::Relaxed);
        let right_pointer = self.header().left.load(Ordering::Relaxed);
        MemeState {
            buf,
            right_buf,
            left_pointer,
            right_pointer,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MemeState {
    pub buf: Vec<u8>,
    #[serde(skip)]
    pub right_buf: Vec<u8>,
    pub left_pointer: usize,
    pub right_pointer: usize,
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

        // SAFETY: staying inside of our allocation
        let dest_ptr = unsafe {
            self.queue
                .alloc_for_write(buf.len(), "data")
                .add(self.total_written)
        };
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

    #[derive(Debug, serde::Serialize)]
    enum Action {
        Read,
        Write(Vec<u8>),
    }

    #[test]
    fn known_bad() {
        let actions = [
            Action::Write(vec![0; 16]),
            Action::Write(vec![1; 0]),
            Action::Write(vec![2; 94]),
            Action::Write(vec![3; 35]),
            Action::Write(vec![4; 15]),
            Action::Write(vec![5; 66]),
            Action::Write(vec![6; 36]),
            Action::Write(vec![7; 88]),
            Action::Write(vec![8; 38]),
            Action::Write(vec![9; 52]),
            Action::Write(vec![10; 99]),
            Action::Write(vec![11; 56]),
            Action::Write(vec![12; 54]),
            Action::Write(vec![13; 69]),
            Action::Write(vec![14; 96]),
            Action::Write(vec![15; 52]),
            Action::Write(vec![16; 82]),
            Action::Write(vec![17; 42]),
            Action::Write(vec![18; 77]),
            Action::Write(vec![19; 35]),
            Action::Write(vec![20; 80]),
            Action::Write(vec![21; 58]),
            Action::Write(vec![22; 25]),
            Action::Write(vec![23; 73]),
            Action::Write(vec![24; 78]),
            Action::Write(vec![25; 43]),
            Action::Write(vec![26; 1]),
            Action::Write(vec![27; 64]),
            Action::Write(vec![28; 0]),
            Action::Write(vec![29; 9]),
            Action::Write(vec![30; 10]),
            Action::Write(vec![31; 49]),
            Action::Write(vec![32; 89]),
            Action::Write(vec![33; 18]),
            Action::Write(vec![34; 20]),
            Action::Write(vec![35; 40]),
            Action::Write(vec![36; 71]),
            Action::Write(vec![37; 27]),
            Action::Write(vec![38; 92]),
            Action::Write(vec![39; 45]),
            Action::Write(vec![40; 81]),
            Action::Write(vec![41; 87]),
            Action::Write(vec![42; 22]),
            Action::Write(vec![43; 1]),
            Action::Write(vec![44; 26]),
            Action::Write(vec![45; 7]),
            Action::Write(vec![46; 3]),
            Action::Write(vec![47; 56]),
            Action::Write(vec![48; 2]),
            Action::Write(vec![49; 82]),
            Action::Write(vec![50; 49]),
            Action::Write(vec![51; 96]),
            Action::Write(vec![52; 87]),
            Action::Write(vec![53; 97]),
            Action::Write(vec![54; 42]),
            Action::Write(vec![55; 95]),
            Action::Write(vec![56; 30]),
            Action::Write(vec![57; 84]),
            Action::Write(vec![58; 22]),
            Action::Write(vec![59; 75]),
            Action::Write(vec![60; 41]),
            Action::Write(vec![61; 49]),
            Action::Write(vec![62; 48]),
            Action::Write(vec![63; 97]),
            Action::Write(vec![64; 81]),
            Action::Write(vec![65; 82]),
            Action::Write(vec![66; 46]),
            Action::Write(vec![67; 71]),
        ];

        let file = tempfile::NamedTempFile::new().unwrap();
        let mut to_read = VecDeque::new();
        let queue = MemeQueue::from_path(file.path(), 200).unwrap();
        #[cfg(skip)]
        println!("{}", &serde_json::to_string(&queue.export_state()).unwrap());
        for (idx, action) in actions.into_iter().enumerate() {
            #[cfg(skip)]
            println!("{}", &serde_json::to_string(&action).unwrap());
            match action {
                Action::Read => {
                    let Some(expected) = to_read.pop_front() else {
                        continue;
                    };
                    let data = queue.read(|buf| buf.to_owned());
                    assert_eq!(data, expected, "failed on action {idx}");
                }
                Action::Write(buf) => {
                    queue.write(|writer| writer.write_all(&buf)).unwrap();
                    to_read.push_back(buf);
                }
            }
            #[cfg(skip)]
            println!("{}", serde_json::to_string(&queue.export_state()).unwrap());
            assert!(
                queue.export_state().buf == queue.export_state().right_buf,
                "failed on action {idx}"
            );
        }
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
                        let data = queue.read(|buf| buf.to_owned());
                        available_space += data.len() + 8;
                        prop_assert_eq!(data, expected);
                    },
                    Action::Write(buf) => {
                        if buf.len() + 8 > available_space {
                            continue;
                        }

                        queue.write(|writer| writer.write_all(&buf)).unwrap();
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
