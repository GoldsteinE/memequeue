pub mod args;

use benchmarks_common::ValidatorStats;
use crc::{Crc, CRC_64_XZ};
use rand::{Rng as _, SeedableRng as _};
use rand_xorshift::XorShiftRng;

const CRC: Crc<u64> = Crc::<u64>::new(&CRC_64_XZ);

pub struct MessageGenerator {
    min_size: usize,
    rng: XorShiftRng,
    clock: quanta::Clock,
}

impl MessageGenerator {
    pub fn new(min_size: usize) -> Self {
        assert!(min_size >= 16, "need some space for checksum and time");

        let rng = XorShiftRng::from_seed(benchmarks_common::RNG_SEED);
        let clock = quanta::Clock::new();
        // Preheat quanta.
        quanta::Instant::now();

        Self {
            min_size,
            rng,
            clock,
        }
    }

    pub fn gen_message(&mut self, buf: &mut [u8]) -> usize {
        let message_size = self.rng.gen_range(self.min_size..buf.len());
        let payload_size = message_size - 8;
        self.rng.fill(&mut buf[..payload_size - 8]);
        let checksum = CRC.checksum(&buf[..payload_size]);
        buf[payload_size..message_size].copy_from_slice(&checksum.to_le_bytes());
        buf[payload_size - 8..payload_size].copy_from_slice(&self.clock.raw().to_le_bytes());
        message_size
    }
}

#[non_exhaustive]
pub struct MessageValidator {
    stats: ValidatorStats,
}

impl MessageValidator {
    pub fn new(count: usize) -> Self {
        // preheat quanta
        quanta::Instant::now();
        Self {
            stats: ValidatorStats::new(count),
        }
    }

    pub fn check_message(&mut self, buf: &[u8]) {
        let now = self.stats.time();
        let size = buf.len();
        let sent_at = {
            let mut b = [0; 8];
            b.copy_from_slice(&buf[size - 16..size - 8]);
            u64::from_le_bytes(b)
        };
        self.stats.record_message(sent_at, now, size);

        let payload = &buf[..size - 8];
        let checksum = {
            let mut b = [0; 8];
            b.copy_from_slice(&buf[size - 8..]);
            u64::from_le_bytes(b)
        };
        assert_eq!(CRC.checksum(payload), checksum);
    }

    pub fn report(&self) {
        self.stats.report()
    }
}
