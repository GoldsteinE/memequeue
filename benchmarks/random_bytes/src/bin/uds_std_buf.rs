use std::{
    io,
    os::unix::net::{UnixListener, UnixStream},
    path::Path,
};

use benchmarks_common::framing::StdBufFraming;
use random_bytes_bench::{args, MessageGenerator, MessageValidator};

const BUF_SIZE: usize = 4096 * 1024;

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
    let mut stream = StdBufFraming::new(BUF_SIZE, UnixStream::connect(file_name)?);
    let mut gen = MessageGenerator::new(min_size);
    let mut buf = vec![0; max_size];

    for _ in 0..count {
        let size = gen.gen_message(&mut buf);
        stream.write_message(&buf[..size])?;
    }

    Ok(())
}

fn recv(file_name: &Path, count: usize) -> io::Result<()> {
    let listener = UnixListener::bind(file_name)?;
    let mut stream = StdBufFraming::new(BUF_SIZE, listener.accept()?.0);
    let mut validator = MessageValidator::new(count);

    for _ in 0..count {
        stream.read_message(|buf| validator.check_message(buf))?;
    }

    validator.report();

    Ok(())
}
