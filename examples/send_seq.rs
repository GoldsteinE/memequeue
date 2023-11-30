#![allow(dead_code, unused_variables, unused_imports)]

use std::{
    io::{self, Write},
    mem,
};

use memequeue::{EventFdControl, MemeQueue};
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
    let producer = MemeQueue::<_, EventFdControl>::new(memequeue::handshake::uds_memfd(
        "/tmp/memequeue-uds",
        4096,
    )?)?;
    eprintln!("negotiation complete, created send queue");

    let mut buf = vec![0; 4096 * 3 / 4];
    let mut rng = rand::thread_rng();
    for _ in 0..1_000_000 {
        let n = rng.gen_range(8..buf.len());
        let buf = &mut buf[..n];
        fill_buf(buf);
        producer.send(|writer| writer.write_all(buf))?;
    }

    Ok(())
}
