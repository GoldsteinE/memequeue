use std::{fs::File, io, mem};

use memequeue::{MemeQueue, ShmemFutexControl};

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
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open("/dev/shm/test.queue")?;

    let consumer = unsafe { MemeQueue::<ShmemFutexControl>::from_file(file, 4096, true)? };

    loop {
        consumer.recv(check_buf);
        // std::thread::sleep(std::time::Duration::from_millis(50));
    }
}
