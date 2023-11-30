use std::{io, mem};

use memequeue::{EventFdControl, MemeQueue};

fn check_buf(buf: &[u8]) {
    assert!(buf.len() >= mem::size_of::<usize>());
    // SAFETY: we have at least that many bytes
    let size = unsafe { buf.as_ptr().cast::<usize>().read_unaligned() };
    assert_eq!(buf.len(), size);
    for (idx, byte) in buf[mem::size_of::<usize>()..].iter().enumerate() {
        assert_eq!(*byte, idx as u8, "at pos {idx}");
    }
    println!("got {size} bytes");
}

fn main() -> io::Result<()> {
    let consumer = MemeQueue::<_, EventFdControl>::new(memequeue::handshake::uds_memfd(
        "/tmp/memequeue-uds",
        4096,
    )?)?;
    eprintln!("negotiation complete, created recv queue");

    loop {
        consumer.recv(|buf| {
            check_buf(buf);
            io::Result::Ok(())
        })?;
        // std::thread::sleep(std::time::Duration::from_millis(50));
    }
}
