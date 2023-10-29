#![allow(unused_variables, unused_mut)]

use std::{io, path::Path};

use futures::{sink::SinkExt as _, StreamExt as _};
use random_bytes_bench::{args, MessageGenerator, MessageValidator};
use tokio::net::{UnixListener, UnixStream};
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

#[tokio::main]
async fn main() -> io::Result<()> {
    let args = args::parse();
    match args.command {
        args::Command::Recv { count } => recv(&args.file_name, count).await,
        args::Command::Send {
            count,
            min_size,
            max_size,
        } => send(&args.file_name, count, min_size, max_size).await,
    }
}

async fn send(file_name: &Path, count: usize, min_size: usize, max_size: usize) -> io::Result<()> {
    let mut stream = FramedWrite::new(
        UnixStream::connect(file_name).await?,
        LengthDelimitedCodec::new(),
    );
    let mut gen = MessageGenerator::new(min_size);
    let mut buf = vec![0; max_size];

    for _ in 0..count {
        let size = gen.gen_message(&mut buf);
        let message = &buf[..size];
        stream.feed(message).await?;
    }

    stream.close().await?;

    Ok(())
}

async fn recv(file_name: &Path, count: usize) -> io::Result<()> {
    let mut listener = UnixListener::bind(file_name)?;
    let (stream, _addr) = listener.accept().await?;
    let mut stream = FramedRead::new(stream, LengthDelimitedCodec::new());
    let mut validator = MessageValidator::new(count);

    for _ in 0..count {
        let Some(buf) = stream.next().await else {
            continue;
        };
        validator.check_message(&buf?);
    }

    validator.report();

    Ok(())
}
