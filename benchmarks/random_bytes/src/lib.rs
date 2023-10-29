pub mod args;

use std::time::Duration;

use crc::{Crc, CRC_64_XZ};
use nix::sys::time::TimeValLike as _;
use rand::{Rng as _, SeedableRng as _};
use rand_xorshift::XorShiftRng;

const CRC: Crc<u64> = Crc::<u64>::new(&CRC_64_XZ);

pub struct MessageGenerator {
    min_size: usize,
    rng: XorShiftRng,
}

impl MessageGenerator {
    pub fn new(min_size: usize) -> Self {
        assert!(min_size >= 16, "need some space for checksum and time");

        // First digits (after decimal) of pi in hex.
        #[rustfmt::skip]
        let rng = XorShiftRng::from_seed([
            0x24, 0x3f, 0x6a, 0x88,
            0x85, 0xa3, 0x08, 0xd3,
            0x13, 0x19, 0x13, 0x19,
            0x8a, 0x2e, 0x03, 0x70,
        ]);
        Self { rng, min_size }
    }

    pub fn gen_message(&mut self, buf: &mut [u8]) -> usize {
        let message_size = self.rng.gen_range(self.min_size..buf.len());
        let payload_size = message_size - 8;
        self.rng.fill(&mut buf[..payload_size - 8]);
        buf[payload_size - 8..payload_size].copy_from_slice(&now().to_le_bytes());
        let checksum = CRC.checksum(&buf[..payload_size]);
        buf[payload_size..message_size].copy_from_slice(&checksum.to_le_bytes());
        message_size
    }
}

#[non_exhaustive]
pub struct MessageValidator {
    latencies: Vec<u64>,
    got_bytes: usize,
    got_messages: usize,
    first_msg_at: Option<u64>,
    last_msg_at: Option<u64>,
}

impl MessageValidator {
    pub fn new(count: usize) -> Self {
        Self {
            latencies: Vec::with_capacity(count),
            got_bytes: 0,
            got_messages: 0,
            first_msg_at: None,
            last_msg_at: None,
        }
    }

    pub fn check_message(&mut self, buf: &[u8]) {
        let size = buf.len();

        self.got_bytes += size;
        self.got_messages += 1;
        let time = {
            let mut b = [0; 8];
            b.copy_from_slice(&buf[size - 16..size - 8]);
            u64::from_le_bytes(b)
        };
        self.latencies.push(now() - time);
        if self.first_msg_at.is_none() {
            self.first_msg_at = Some(now());
        }
        self.last_msg_at = Some(now());

        let payload = &buf[..size - 8];

        let checksum = {
            let mut b = [0; 8];
            b.copy_from_slice(&buf[size - 8..]);
            u64::from_le_bytes(b)
        };
        assert_eq!(CRC.checksum(payload), checksum);
    }

    pub fn avg_latency(&self) -> u64 {
        self.latencies.iter().sum::<u64>() / (self.latencies.len() as u64)
    }

    pub fn total_bytes(&self) -> usize {
        self.got_bytes
    }

    pub fn total_time(&self) -> u64 {
        self.last_msg_at.unwrap() - self.first_msg_at.unwrap()
    }

    pub fn report(&self) {
        eprintln!("average latency: {}ns", self.avg_latency());

        let total_time = Duration::from_nanos(self.total_time());
        let total_bytes = self.total_bytes();

        eprintln!(
            "got {} in {total_time:?}",
            humansize::format_size(total_bytes, humansize::BINARY)
        );
        eprintln!(
            "...that's {} per second",
            humansize::format_size(
                (total_bytes as f64 / total_time.as_secs_f64()) as u64,
                humansize::BINARY,
            )
        );

        eprintln!("got {} messages in {total_time:?}", self.got_messages);
        eprintln!(
            "...that's {:.2} per second",
            self.got_messages as f64 / total_time.as_secs_f64(),
        );
    }
}

fn now() -> u64 {
    nix::time::clock_gettime(nix::time::ClockId::CLOCK_MONOTONIC)
        .unwrap()
        .num_nanoseconds()
        .try_into()
        .expect("time should be positive")
}
