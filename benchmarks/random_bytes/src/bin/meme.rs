use std::{
    io::{self, Write as _},
    path::Path,
};

use memequeue::{MemeQueue, ShmemFutexControl};

use random_bytes_bench::{args, MessageGenerator, MessageValidator};

const QUEUE_SIZE: usize = 4096 * 1024;

fn main() -> io::Result<()> {
    let args = args::parse();
    match args.command {
        args::Command::Recv { count } => recv(&args.file_name, count),
        args::Command::Send {
            count,
            min_size,
            max_size,
        } => send(&args.file_name, count, min_size, max_size),
    }
}

fn send(file_name: &Path, count: usize, min_size: usize, max_size: usize) -> io::Result<()> {
    let queue = MemeQueue::<_, ShmemFutexControl>::new(unsafe {
        memequeue::handshake::named_file(file_name, QUEUE_SIZE)?
    })?;
    let mut gen = MessageGenerator::new(min_size);
    let mut buf = vec![0; max_size];

    for _ in 0..count {
        let size = gen.gen_message(&mut buf);
        queue.send(|writer| writer.write_all(&buf[..size]))?;
    }

    Ok(())
}

fn recv(file_name: &Path, count: usize) -> io::Result<()> {
    let queue = MemeQueue::<_, ShmemFutexControl>::new(unsafe {
        memequeue::handshake::named_file(file_name, QUEUE_SIZE)?
    })?;
    let mut validator = MessageValidator::new(count);

    for _ in 0..count {
        queue.recv(|buf| validator.check_message(buf));
    }

    validator.report();

    Ok(())
}
