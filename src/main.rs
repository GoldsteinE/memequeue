use std::{
    fs::File,
    io::{self, Write},
    thread,
    time::Duration,
};

use memequeue::{MemeQueue, ShmemFutexControl};

fn main() -> io::Result<()> {
    let mut options = File::options();
    options.read(true).write(true).truncate(false).create(true);

    let consumer_file = options.open("/dev/shm/rust.queue")?;
    let producer_file = options.open("/dev/shm/rust.queue")?;

    let consumer = unsafe { MemeQueue::<ShmemFutexControl>::from_file(consumer_file, 4096, true)? };
    let producer =
        unsafe { MemeQueue::<ShmemFutexControl>::from_file(producer_file, 4096, false)? };

    let producer_thread = thread::spawn(move || {
        let mut idx = 0_usize;
        loop {
            producer
                .send(|writer| writer.write_all(format!("lol lmao #{idx}").as_bytes()))
                .unwrap();
            idx += 1;
        }
    });
    let consumer_thread = thread::spawn(move || {
        let mut idx = 0_usize;
        loop {
            consumer.recv(|buf| println!("got message #{idx}: {}", String::from_utf8_lossy(buf)));
            thread::sleep(Duration::from_millis(100));
            idx += 1;
        }
    });

    producer_thread.join().unwrap();
    consumer_thread.join().unwrap();

    Ok(())
}
