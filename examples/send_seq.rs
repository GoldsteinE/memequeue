#![allow(dead_code, unused_variables, unused_imports)]

use std::{
    fs::File,
    io::{self, Write},
    mem,
};

use memequeue::{MemeQueue, ShmemFutexControl};
use rand::Rng;

fn fill_buf(buf: &mut [u8]) {
    let n = buf.len();
    assert!(n >= mem::size_of::<usize>());
    buf[..mem::size_of::<usize>()].copy_from_slice(&n.to_ne_bytes());
    for (idx, byte) in buf[mem::size_of::<usize>()..].iter_mut().enumerate() {
        *byte = idx as u8;
    }
}

fn main() -> io::Result<()> {
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open("/dev/shm/test.queue")?;

    let producer = unsafe { MemeQueue::<ShmemFutexControl>::from_file(file, 4096, false)? };

    let mut buf = vec![0; 4096 * 3 / 4];
    let mut rng = rand::thread_rng();
    for _ in 0..10_000 {
        let n = rng.gen_range(8..buf.len());
        let buf = &mut buf[..n];
        fill_buf(buf);
        producer.send(|writer| writer.write_all(buf))?;
    }

    Ok(())
}
