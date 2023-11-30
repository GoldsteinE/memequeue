use std::{io, mem, path::Path};

use memequeue::{MemeQueue, ShmemFutexControl};

use postcard_objects_bench::{
    args,
    model::{Batch, MarketInfo},
    MessageGenerator, MessageValidator,
};
use smallvec::SmallVec;

const QUEUE_SIZE: usize = 4096 * 1024;

fn main() -> io::Result<()> {
    let args = args::parse();
    match args.command {
        args::Command::Recv => recv(&args.file_name, args.count, args.batch_size),
        args::Command::Send => send(&args.file_name, args.count, args.batch_size),
    }
}

fn send(file_name: &Path, count: usize, batch_size: Option<usize>) -> io::Result<()> {
    let queue = MemeQueue::<_, ShmemFutexControl>::new(unsafe {
        memequeue::handshake::named_file(file_name, QUEUE_SIZE)?
    })?;
    let mut gen = MessageGenerator::new();
    let mut batch_buf = SmallVec::new();
    let clock = quanta::Clock::new();

    for _ in 0..count {
        match batch_size {
            Some(batch_size) => {
                batch_buf.clear();
                for _ in 0..batch_size {
                    batch_buf.push(gen.gen_message());
                }
                let message = Batch::new(mem::take(&mut batch_buf), clock.raw());
                queue.send(|writer| {
                    postcard::to_io(&message, writer).unwrap();
                    io::Result::Ok(())
                })?;
                batch_buf = message.inner;
            }
            None => {
                let message = gen.gen_message();
                queue.send(|writer| {
                    postcard::to_io(&message, writer).unwrap();
                    io::Result::Ok(())
                })?;
            }
        }
    }

    #[cfg(feature = "stats")]
    eprintln!("queue stats: {:?}", queue.stats());

    Ok(())
}

fn recv(file_name: &Path, count: usize, batch_size: Option<usize>) -> io::Result<()> {
    let queue = MemeQueue::<_, ShmemFutexControl>::new(unsafe {
        memequeue::handshake::named_file(file_name, QUEUE_SIZE)?
    })?;
    let mut validator = MessageValidator::new(count);

    for _ in 0..count {
        queue.recv(|buf| {
            if batch_size.is_none() {
                validator.check_message::<MarketInfo>(buf);
            } else {
                validator.check_message::<Batch<MarketInfo>>(buf);
            }
            io::Result::Ok(())
        })?;
    }

    validator.report();

    #[cfg(feature = "stats")]
    eprintln!("queue stats: {:?}", queue.stats());

    Ok(())
}
