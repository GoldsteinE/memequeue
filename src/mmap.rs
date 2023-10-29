use std::{fs::File, io, os::fd::AsRawFd, ptr, sync::OnceLock};

pub(crate) fn get_page_size() -> usize {
    static PAGE_SIZE: OnceLock<usize> = OnceLock::new();

    *PAGE_SIZE.get_or_init(|| {
        usize::try_from(unsafe { libc::sysconf(libc::_SC_PAGE_SIZE) })
            .expect("page size must fit into usize")
    })
}

pub struct Mmap {
    ptr: *mut u8,
    size: usize,
}

// SAFETY: todo
unsafe impl Send for Mmap {}

impl Mmap {
    pub(crate) fn as_ptr(&self) -> *mut u8 {
        self.ptr
    }

    pub(crate) fn size(&self) -> usize {
        self.size
    }
}

impl Drop for Mmap {
    fn drop(&mut self) {
        // SAFETY: points to a valid mapping
        unsafe {
            // Can't do anything useful with the error.
            let _res = libc::munmap(self.ptr.cast(), self.size);
        }
    }
}

pub(crate) struct QueueMmaps {
    pub(crate) header: Mmap,
    pub(crate) left: Mmap,
    pub(crate) right: Mmap,
}

impl QueueMmaps {
    /// # Safety
    /// `fd` must point to a file which has enough space for `PAGE_SIZE + queue_size * 2` bytes.
    #[rustfmt::skip]
    pub(crate) unsafe fn from_fd<F: AsRawFd>(fd: &F, queue_size: usize) -> io::Result<Self> {
        use libc::{
            mmap, munmap, MAP_ANONYMOUS, MAP_FAILED, MAP_FIXED, MAP_SHARED, PROT_READ, PROT_WRITE,
        };

        let fd = fd.as_raw_fd();
        let page_size = get_page_size();
        let offset = i64::try_from(page_size).expect("page size must fit into i64");

        if queue_size % page_size != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "queue size must be a multiple of page size",
            ));
        }

        // SAFETY: a valid anonymous mapping.
        let big = unsafe {
            mmap(
                ptr::null_mut(), queue_size * 2,
                PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANONYMOUS,
                0, 0,
            )
        };
        if big == MAP_FAILED {
            return Err(io::Error::last_os_error());
        }

        // SAFETY: a fixed mapping in pre-reserved area.
        let left = unsafe {
            mmap(
                big, queue_size,
                PROT_READ | PROT_WRITE, MAP_SHARED | MAP_FIXED,
                fd, offset,
            )
        };
        if left == MAP_FAILED {
            let err = io::Error::last_os_error();
            // SAFETY: unmapping what we just mapped.
            unsafe { munmap(big, queue_size * 2) };
            return Err(err);
        }

        // SAFETY: another fixed mapping in pre-reserved area.
        let right = unsafe {
            mmap(
                big.add(queue_size), queue_size,
                PROT_READ | PROT_WRITE, MAP_SHARED | MAP_FIXED,
                fd, offset,
            )
        };
        if right == MAP_FAILED {
            let err = io::Error::last_os_error();
            // SAFETY: unmapping `left` and what remains of the `big`.
            unsafe {
                munmap(left, queue_size);
                munmap(big.add(queue_size), queue_size);
            }
            return Err(err);
        }

        // Re-derive pointers from `big` for provenance reasons.
        let left = Mmap { ptr: big.cast(), size: queue_size };
        // SAFETY: stays in bounds
        let right = Mmap { ptr: unsafe { big.add(queue_size) }.cast(), size: queue_size };

        // SAFETY: another anonymous mapping into our file.
        let header = unsafe {
            mmap(
                ptr::null_mut(), page_size,
                PROT_READ | PROT_WRITE, MAP_SHARED,
                fd, 0,
            )
        };
        if header == MAP_FAILED {
            // No need to manually unmap `left` + `right`, destructors will take care of this.
            return Err(io::Error::last_os_error());
        }

        Ok(Self {
            left,
            right,
            header: Mmap {
                ptr: header.cast(),
                size: page_size,
            },
        })
    }

    /// # Safety
    /// No one else can access the file while this function is executing.
    pub(crate) unsafe fn create_from_file(file: &File, queue_size: usize) -> io::Result<Self> {
        let page_size = get_page_size();
        file.set_len((page_size + queue_size * 2) as u64)?;
        // SAFETY: we just set the file size.
        let mmaps = unsafe { Self::from_fd(file, queue_size)? };
        // SAFETY: header is page-sized.
        unsafe {
            mmaps.header.ptr.write_bytes(0, page_size);
        }
        Ok(mmaps)
    }
}
