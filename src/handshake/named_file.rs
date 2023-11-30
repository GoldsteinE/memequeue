use std::{
    fs::{self, File},
    io,
    os::fd::{AsRawFd as _, RawFd},
    path::Path,
};

use crate::{handshake::HandshakeResult, mmap::get_page_size};

pub struct NamedFileHandshakeResult {
    file: File,
    owner: bool,
    queue_size: usize,
}

// SAFETY: as long as nobody else touches the file (which is the safety contract of [`named_file()`], we
// ensure that it has proper size and we're holding a shared lock, i.e. the owner is already done
// setting everything up.
unsafe impl HandshakeResult for NamedFileHandshakeResult {
    fn shmem_fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }

    fn is_owner(&self) -> bool {
        self.owner
    }

    fn queue_size(&self) -> usize {
        self.queue_size
    }

    fn mark_ready(&mut self) -> io::Result<()> {
        // Relock to shared, queue is ready to use.
        if self.owner {
            // SAFETY: `flock` is safe and we're passing valid fd + operation.
            if unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_SH) } != 0 {
                return Err(io::Error::last_os_error());
            }
        }

        Ok(())
    }
}

impl Drop for NamedFileHandshakeResult {
    fn drop(&mut self) {
        // SAFETY: `flock` is safe and we're passing valid fd + operation.
        unsafe {
            // Result is intentionally ignored, because we can't meaningfully
            // handle it here anyway.
            let _res = libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

/// Try to create a queue in a named file. File should be on a `tmpfs` for this to be fast.
///
/// `queue_size` is only relevant when we end up creating the queue. If we end up connecting,
/// existing queue size is used. `queue_size` will be rounded up to the next multiple of page size.
///
/// # Panics
/// If we connect to the queue and it has invalid size.
///
/// # Safety
/// This is inherently unsafe because any external modifications to the file would lead to a data race.
pub unsafe fn named_file(
    path: impl AsRef<Path>,
    mut queue_size: usize,
) -> io::Result<NamedFileHandshakeResult> {
    let page_size = get_page_size();
    queue_size = queue_size.next_multiple_of(page_size);

    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;

    // SAFETY: `flock` is safe and we're passing valid fd + operation.
    let flock_result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if flock_result != 0 {
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::WouldBlock {
            return Err(err);
        }
    }

    let owner = flock_result == 0;
    if owner {
        file.set_len((page_size + queue_size) as u64)?;
    } else {
        // Wait for a queue to be ready.
        // SAFETY: `flock` is safe and we're passing valid fd + operation.
        let flock_result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH) };
        if flock_result != 0 {
            return Err(io::Error::last_os_error());
        }

        queue_size = usize::try_from(file.metadata()?.len())
            .expect("queue file size must fit in usize")
            .checked_sub(page_size)
            .expect("queue file size must be greater than page size");

        if queue_size % page_size != 0 {
            panic!("queue size ({queue_size}) is not a multiple of page size ({page_size})");
        }
    }

    Ok(NamedFileHandshakeResult {
        file,
        owner,
        queue_size,
    })
}
