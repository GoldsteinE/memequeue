use std::{fs::File, io, path::Path};

use memequeue::{MemeQueue, ShmemFutexControl};

use postcard_objects_bench::{args, MessageGenerator, MessageValidator};

const QUEUE_SIZE: usize = 4096 * 1024;

fn main() -> io::Result<()> {
    let args = args::parse();
    match args.command {
        args::Command::Recv => recv(&args.file_name, args.count),
        args::Command::Send => send(&args.file_name, args.count),
    }
}

fn send(file_name: &Path, count: usize) -> io::Result<()> {
    let queue = unsafe {
        MemeQueue::<ShmemFutexControl>::from_file(open_file(file_name)?, QUEUE_SIZE, false)?
    };
    let mut gen = MessageGenerator::new();

    for _ in 0..count {
        let message = gen.gen_message();
        queue.send(|writer| {
            postcard::to_io(&message, writer).unwrap();
            io::Result::Ok(())
        })?;
    }

    Ok(())
}

fn recv(file_name: &Path, count: usize) -> io::Result<()> {
    let queue = unsafe {
        MemeQueue::<ShmemFutexControl>::from_file(open_file(file_name)?, QUEUE_SIZE, true)?
    };
    let mut validator = MessageValidator::new(count);

    for _ in 0..count {
        queue.recv(|buf| {
            validator.check_message(buf);
        });
    }

    validator.report();

    Ok(())
}

fn open_file(file_name: &Path) -> io::Result<File> {
    File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(file_name)
}
