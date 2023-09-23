use std::{io, os::fd::AsRawFd};

pub(crate) struct Flock {
    fd: i32,
}

impl Flock {
    pub(crate) fn lock(file: &impl AsRawFd) -> io::Result<Self> {
        let fd = file.as_raw_fd();
        // SAFETY: we're passing a valid operation
        if unsafe { libc::flock(fd, libc::LOCK_EX) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { fd })
    }
}

impl Drop for Flock {
    fn drop(&mut self) {
        // SAFETY: we're passing a valid operation
        unsafe { libc::flock(self.fd, libc::LOCK_UN); }
    }
}
