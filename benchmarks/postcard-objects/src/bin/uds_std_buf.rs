use std::{
    io,
    os::unix::net::{UnixListener, UnixStream},
    path::Path,
};

use benchmarks_common::framing::StdBufFraming;
use postcard_objects_bench::{args, model::MarketInfo, MessageGenerator, MessageValidator};

const BUF_SIZE: usize = 4096 * 1024;

fn main() -> io::Result<()> {
    let args = args::parse();
    match args.command {
        args::Command::Recv => recv(&args.file_name, args.count, args.batch_size),
        args::Command::Send => send(&args.file_name, args.count, args.batch_size),
    }
}

fn send(file_name: &Path, count: usize, batch_size: Option<usize>) -> io::Result<()> {
    assert!(
        batch_size.is_none(),
        "batch mode is not supported for uds_std_buf",
    );

    let mut stream = StdBufFraming::new(BUF_SIZE, UnixStream::connect(file_name)?);
    let mut gen = MessageGenerator::new();
    let mut buf = Vec::with_capacity(256);

    for _ in 0..count {
        buf.clear();
        let model = gen.gen_message();
        postcard::to_io(&model, &mut buf).unwrap();
        stream.write_message(&buf)?;
    }

    Ok(())
}

fn recv(file_name: &Path, count: usize, batch_size: Option<usize>) -> io::Result<()> {
    assert!(
        batch_size.is_none(),
        "batch mode is not supported for uds_std_buf",
    );

    let listener = UnixListener::bind(file_name)?;
    let mut stream = StdBufFraming::new(BUF_SIZE, listener.accept()?.0);
    let mut validator = MessageValidator::new(count);

    for _ in 0..count {
        stream.read_message(|buf| validator.check_message::<MarketInfo>(buf))?;
    }

    validator.report();

    Ok(())
}
