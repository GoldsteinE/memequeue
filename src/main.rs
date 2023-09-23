use std::io::Write;

use memequeue::MemeQueue;
use rand::Rng;

fn main() {
    let queue = MemeQueue::from_path("/tmp/memememe", 4096 * 10).unwrap();
    let mut rng = rand::thread_rng();
    let mut counter = 0;
    for _ in 0..10_000 {
        let size = rng.gen_range(0..8000);
        let mut bytes = vec![0; size];
        rng.fill(&mut *bytes);
        queue.write(|writer| {
            writer.write_all(&bytes).unwrap();
        });
        queue.read(|buf| {
            counter += 1;
            assert_eq!(buf, &bytes);
        });
    }
    println!("{counter} messages received!");
}
