use std::{io, os::fd::RawFd};

mod named_file;
pub use named_file::{named_file, NamedFileHandshakeResult};

#[cfg(feature = "handshake_uds_memfd")]
mod uds_memfd;
#[cfg(feature = "handshake_uds_memfd")]
pub use uds_memfd::{uds_memfd, UdsMemfdHandshakeResult};

/// # Safety
/// 1. `shmem_fd` must point to a mmapable object of size `page_size + queue_size`.
/// 2. `is_owner` must be true for only one side at a time.
/// 3. Before `.mark_ready()` is called, nobody but owner can have an instance of
///    [`HandshakeResult`] for this queue.
pub unsafe trait HandshakeResult {
    fn shmem_fd(&self) -> RawFd;
    fn is_owner(&self) -> bool;
    fn queue_size(&self) -> usize;
    fn mark_ready(&mut self) -> io::Result<()>;
}

pub trait ExchangeFd {
    fn send_fd(&mut self, fd: RawFd) -> io::Result<()>;
    fn recv_fd(&mut self) -> io::Result<RawFd>;
}
