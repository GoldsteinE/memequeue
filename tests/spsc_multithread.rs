use std::{
    io::{self, Write as _},
    ops::Range,
    thread,
};

use memequeue::MemeQueue;
use rand::Rng as _;

#[test]
fn fast() -> io::Result<()> {
    let queue_file = tempfile::NamedTempFile::new()?;
    let messages = gen_messages(10_000, 0..1000);
    thread::scope(|scope| {
        let j1 = scope.spawn(|| {
            let queue = MemeQueue::from_path(queue_file.path(), 4096).unwrap();
            for (idx, message) in messages.iter().enumerate() {
                queue.write(|writer| {
                    eprintln!("writing {idx}...");
                    writer.write_all(message).unwrap();
                })
            }
        });
        let j2 = scope.spawn(|| {
            let queue = MemeQueue::from_path(queue_file.path(), 4096 * 2).unwrap();
            for (idx, message) in messages.iter().enumerate() {
                eprintln!("reading {idx}...");
                queue.read(|buf| assert_eq!(buf, message));
            }
        });
        if let Err(err) = j2.join() {
            std::panic::resume_unwind(err);
        }
        if let Err(err) = j1.join() {
            std::panic::resume_unwind(err);
        }
    });
    Ok(())
}

fn gen_messages(n: usize, sizes: Range<usize>) -> Vec<Vec<u8>> {
    let mut rng = rand::thread_rng();
    (0..n)
        .map(|_| {
            let size = rng.gen_range(sizes.clone());
            let data = vec![0; size];
            data
        })
        .collect()
}
