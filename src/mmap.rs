use std::{
    io,
    os::fd::{self, RawFd},
    ptr::{self, NonNull},
};

use libc::{MAP_ANONYMOUS, MAP_FAILED, MAP_FIXED, MAP_PRIVATE, MAP_SHARED, PROT_READ, PROT_WRITE};

pub(crate) struct Mmap {
    ptr: NonNull<u8>,
    size: usize,
}

impl Mmap {
    pub(crate) fn new_anon(size: usize) -> io::Result<Self> {
        // SAFETY: we're passing valid params
        let ptr = unsafe {
            libc::mmap(
                ptr::null_mut(),
                size,
                PROT_READ | PROT_WRITE,
                MAP_ANONYMOUS | MAP_PRIVATE,
                0,
                0,
            )
        };
        if ptr == MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let ptr = NonNull::new(ptr).expect("mmap() returned null ptr").cast();
        Ok(Self { ptr, size })
    }

    pub(crate) fn new_file(size: usize, offset: usize, fd: RawFd) -> io::Result<Self> {
        let ptr = unsafe {
            libc::mmap(
                ptr::null_mut(),
                size,
                PROT_READ | PROT_WRITE,
                MAP_SHARED,
                fd,
                offset as i64,
            )
        };
        if ptr == MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let ptr = NonNull::new(ptr).expect("mmap() returned null ptr").cast();
        Ok(Self { ptr, size })
    }

    pub(crate) fn new_file_at(
        addr: NonNull<u8>,
        size: usize,
        offset: usize,
        fd: RawFd,
    ) -> io::Result<Self> {
        let ptr = unsafe {
            libc::mmap(
                addr.as_ptr().cast(),
                size,
                PROT_READ | PROT_WRITE,
                MAP_FIXED | MAP_SHARED,
                fd,
                offset as i64,
            )
        };
        if ptr == MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        // Requested addr can't be null, since function takes `NonNull`.
        let ptr = NonNull::new(ptr).expect("mmap() returned null ptr").cast();
        Ok(Self { ptr, size })
    }

    pub(crate) fn ptr(&self) -> NonNull<u8> {
        self.ptr
    }

    pub(crate) fn len(&self) -> usize {
        self.size
    }
}

impl Drop for Mmap {
    fn drop(&mut self) {
        // SAFETY: we're passing valid ptr and size
        unsafe {
            libc::munmap(self.ptr.as_ptr().cast(), self.size);
        }
    }
}
