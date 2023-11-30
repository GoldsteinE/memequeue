use std::{
    collections::VecDeque,
    fs::{self, File},
    io::{self, IoSlice, IoSliceMut},
    os::{
        fd::{AsRawFd as _, FromRawFd as _, RawFd},
        unix::net::{UnixListener, UnixStream},
    },
    path::Path,
};

use nix::{
    cmsg_space,
    sys::socket::{recvmsg, sendmsg, ControlMessage, ControlMessageOwned, MsgFlags},
};

use crate::{
    handshake::{ExchangeFd, HandshakeResult},
    mmap::get_page_size,
};

const NEGOTIATION_MESSAGE: &[u8] = b"memequeue uds memfd negotiation";
const PAYLOAD_BUF_SIZE: usize = 128;

pub struct UdsMemfdHandshakeResult {
    file: File,
    owner: bool,
    queue_size: usize,
    stream: UnixStream,
    exchange_fd_counter: usize,
    recv_fd_queue: VecDeque<RawFd>,
}

// TODO: safety
unsafe impl HandshakeResult for UdsMemfdHandshakeResult {
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
        if self.owner {
            send_fd(
                self.stream.as_raw_fd(),
                self.file.as_raw_fd(),
                NEGOTIATION_MESSAGE,
            )?;
        }

        Ok(())
    }
}

impl ExchangeFd for UdsMemfdHandshakeResult {
    fn send_fd(&mut self, fd: RawFd) -> io::Result<()> {
        self.exchange_fd_counter += 1;
        send_fd(
            self.stream.as_raw_fd(),
            fd,
            &self.exchange_fd_counter.to_le_bytes(),
        )
    }

    fn recv_fd(&mut self) -> io::Result<RawFd> {
        if let Some(fd) = self.recv_fd_queue.pop_front() {
            return Ok(fd);
        }

        self.exchange_fd_counter += 1;
        recv_fd_expecting(
            self.stream.as_raw_fd(),
            &self.exchange_fd_counter.to_le_bytes(),
        )
    }
}

// TODO: explain safety considerations
pub fn uds_memfd(
    uds_path: impl AsRef<Path>,
    mut queue_size: usize,
) -> io::Result<UdsMemfdHandshakeResult> {
    let page_size = get_page_size();
    queue_size = queue_size.next_multiple_of(page_size);

    let (stream, owner) = match UnixListener::bind(&uds_path) {
        Ok(listener) => {
            let (stream, _peer_addr) = listener.accept()?;
            (stream, true)
        }
        Err(err) if err.kind() == io::ErrorKind::AddrInUse => {
            let stream = UnixStream::connect(&uds_path)?;
            // We already connected, no need for file anymore.
            fs::remove_file(&uds_path)?;
            (stream, false)
        }
        Err(err) => return Err(err),
    };

    if owner {
        // SAFETY: `name` points to a valid NULL-terminated C string.
        let memfd = unsafe { libc::memfd_create(b"memequeue\0".as_ptr().cast(), 0) };
        if memfd < 0 {
            return Err(io::Error::last_os_error());
        }

        // SAFETY: memfd behaves like a regular file.
        let file = unsafe { File::from_raw_fd(memfd) };
        file.set_len((page_size + queue_size) as u64)?;

        Ok(UdsMemfdHandshakeResult {
            file,
            owner,
            queue_size,
            stream,
            exchange_fd_counter: 0,
            recv_fd_queue: VecDeque::new(),
        })
    } else {
        let mut payload_buf = [0; PAYLOAD_BUF_SIZE];
        let mut exchange_fd_counter = 0;
        let mut recv_fd_queue = VecDeque::new();
        let memfd = loop {
            let (raw_fd, payload) = recv_fd(stream.as_raw_fd(), &mut payload_buf)?;
            if payload == NEGOTIATION_MESSAGE {
                break raw_fd;
            } else if payload == usize::to_le_bytes(exchange_fd_counter + 1) {
                recv_fd_queue.push_back(raw_fd);
                exchange_fd_counter += 1;
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unexpected message payload: `{payload:?}`"),
                ));
            }
        };

        // SAFETY: memfd behaves like a regular file, and we believe that other part is honest.
        let file = unsafe { File::from_raw_fd(memfd) };

        // TODO: probably should be deduped with named_file
        queue_size = usize::try_from(file.metadata()?.len())
            .expect("queue file size must fit in usize")
            .checked_sub(page_size)
            .expect("queue file size must be greater than page size");

        if queue_size % page_size != 0 {
            panic!("queue size ({queue_size}) is not a multiple of page size ({page_size})");
        }

        Ok(UdsMemfdHandshakeResult {
            file,
            owner,
            queue_size,
            stream,
            exchange_fd_counter,
            recv_fd_queue,
        })
    }
}

fn send_fd(send_to: RawFd, to_send: RawFd, payload: &[u8]) -> io::Result<()> {
    sendmsg::<()>(
        send_to,
        &[IoSlice::new(payload)],
        &[ControlMessage::ScmRights(&[to_send])],
        MsgFlags::empty(),
        None,
    )?;
    Ok(())
}

fn recv_fd(recv_from: RawFd, buf: &mut [u8]) -> io::Result<(RawFd, &[u8])> {
    let mut fd_space = cmsg_space!(RawFd);
    let mut bufs = [IoSliceMut::new(buf)];
    let msg = recvmsg::<()>(recv_from, &mut bufs, Some(&mut fd_space), MsgFlags::empty())?;

    let fd = msg
        .cmsgs()
        .find_map(|cmsg| {
            if let ControlMessageOwned::ScmRights(fds) = cmsg {
                fds.first().copied()
            } else {
                None
            }
        })
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "didn't receive fd in the message",
            )
        })?;

    let payload = msg.iovs().next().unwrap();

    // Reborrow to bypass borrowchecker.
    let n = payload.len();
    Ok((fd, &buf[..n]))
}

fn recv_fd_expecting(recv_from: RawFd, expected_payload: &[u8]) -> io::Result<RawFd> {
    let mut buf = [0; PAYLOAD_BUF_SIZE];
    let (fd, payload) = recv_fd(recv_from, &mut buf)?;

    if payload != expected_payload {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            // TODO: better formatting
            format!("wrong message payload: expected `{expected_payload:?}`, got `{payload:?}`"),
        ));
    }

    Ok(fd)
}
