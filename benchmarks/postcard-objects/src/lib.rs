use self::model::Message;
use benchmarks_common::ValidatorStats;
use rand::SeedableRng as _;
use rand_xorshift::XorShiftRng;
use serde::Deserialize;

pub mod args;
pub mod model;

pub struct MessageGenerator {
    rng: XorShiftRng,
    clock: quanta::Clock,
}

impl MessageGenerator {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        // Preheat quanta.
        quanta::Instant::now();
        Self {
            rng: XorShiftRng::from_seed(benchmarks_common::RNG_SEED),
            clock: quanta::Clock::new(),
        }
    }

    pub fn gen_message(&mut self) -> model::MarketInfo {
        model::MarketInfo::random(|| self.clock.raw(), &mut self.rng)
    }
}

pub struct MessageValidator {
    stats: ValidatorStats,
}

impl MessageValidator {
    pub fn new(count: usize) -> Self {
        Self {
            stats: ValidatorStats::new(count),
        }
    }

    pub fn check_message<'de, M: Message + Deserialize<'de>>(&mut self, buf: &'de [u8]) {
        let now = self.stats.time();
        let message: M = postcard::from_bytes(buf).unwrap();
        self.stats.record_message(message.time(), now, buf.len());
        message.check();
    }

    pub fn report(&mut self) {
        self.stats.report()
    }
}
