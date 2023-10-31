use std::{io, path::Path};

use futures::{sink::SinkExt as _, StreamExt as _};
use postcard_objects_bench::{args, model::MarketInfo, MessageGenerator, MessageValidator};
use tokio::net::{UnixListener, UnixStream};
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

#[tokio::main]
async fn main() -> io::Result<()> {
    let args = args::parse();
    if args.batch_size.is_some() {
        panic!("uds_tokio_framed doesn't support batched mode yet, sorry");
    }
    match args.command {
        args::Command::Recv => recv(&args.file_name, args.count).await,
        args::Command::Send => send(&args.file_name, args.count).await,
    }
}

async fn send(file_name: &Path, count: usize) -> io::Result<()> {
    let mut stream = FramedWrite::new(
        UnixStream::connect(file_name).await?,
        LengthDelimitedCodec::new(),
    );
    let mut gen = MessageGenerator::new();
    // `Encoder` interface doesn't actually allow to get rid of this buffer.
    let mut buf = Vec::with_capacity(1024);

    for _ in 0..count {
        let message = gen.gen_message();
        buf.clear();
        postcard::to_io(&message, &mut buf).unwrap();
        stream.feed(&*buf).await?;
    }

    stream.close().await?;

    Ok(())
}

async fn recv(file_name: &Path, count: usize) -> io::Result<()> {
    let listener = UnixListener::bind(file_name)?;
    let (stream, _addr) = listener.accept().await?;
    let mut stream = FramedRead::new(stream, LengthDelimitedCodec::new());
    let mut validator = MessageValidator::new(count);

    for _ in 0..count {
        let Some(buf) = stream.next().await else {
            continue;
        };
        validator.check_message::<MarketInfo>(&buf?);
    }

    validator.report();

    Ok(())
}
