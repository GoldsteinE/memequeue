use std::{
    io::{self, Write as _},
    ops::Range,
    thread,
    time::Duration,
};

use memequeue::MemeQueue;
use rand::Rng as _;

fn test(random_delays: bool) -> io::Result<()> {
    let queue_file = tempfile::NamedTempFile::new()?;
    let messages = gen_messages(1000, 0..100);
    thread::scope(|scope| {
        let j1 = scope.spawn(|| {
            let mut rng = rand::thread_rng();
            let queue = MemeQueue::from_path(queue_file.path(), 4096).unwrap();
            for (idx, message) in messages.iter().enumerate() {
                if random_delays {
                    thread::sleep(Duration::from_millis(rng.gen_range(10..25)));
                }
                queue.send(|writer| {
                    eprintln!("writing {idx}...");
                    writer.write_all(message).unwrap();
                })
            }
        });
        let j2 = scope.spawn(|| {
            let mut rng = rand::thread_rng();
            if random_delays {
                thread::sleep(Duration::from_millis(rng.gen_range(1..5)));
            }
            let queue = MemeQueue::from_path(queue_file.path(), 4096).unwrap();
            for (idx, message) in messages.iter().enumerate() {
                if random_delays {
                    thread::sleep(Duration::from_millis(rng.gen_range(10..50)));
                }
                eprintln!("reading {idx}...");
                queue.recv(|buf| {
                    assert!(
                        buf == message,
                        "{} vs {} bytes of data",
                        buf.len(),
                        message.len(),
                    )
                });
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

#[test]
fn fast() -> io::Result<()> {
    test(false)
}

#[test]
fn jittery() -> io::Result<()> {
    test(true)
}

fn gen_messages(n: usize, sizes: Range<usize>) -> Vec<Vec<u8>> {
    let mut rng = rand::thread_rng();
    (0..n)
        .map(|_| {
            let size = rng.gen_range(sizes.clone());
            let mut data = vec![0; size];
            rng.fill(&mut *data);
            data
        })
        .collect()
}
