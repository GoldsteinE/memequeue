use std::time::Duration;

// First digits (after decimal) of pi in hex.
#[rustfmt::skip]
pub const RNG_SEED: [u8; 16] = [
    0x24, 0x3f, 0x6a, 0x88,
    0x85, 0xa3, 0x08, 0xd3,
    0x13, 0x19, 0x13, 0x19,
    0x8a, 0x2e, 0x03, 0x70,
];

pub struct ValidatorStats {
    latencies: Vec<u64>,
    got_bytes: usize,
    got_messages: usize,
    first_msg_at: Option<u64>,
    last_msg_at: Option<u64>,
    clock: quanta::Clock,
}

impl ValidatorStats {
    pub fn new(count: usize) -> Self {
        // preheat quanta
        quanta::Instant::now();

        Self {
            latencies: Vec::with_capacity(count),
            got_bytes: 0,
            got_messages: 0,
            first_msg_at: None,
            last_msg_at: None,
            clock: quanta::Clock::new(),
        }
    }

    pub fn time(&self) -> u64 {
        self.clock.raw()
    }

    pub fn record_message(&mut self, sent_at: u64, received_at: u64, size: usize) {
        self.latencies.push(received_at - sent_at);
        self.got_messages += 1;
        self.got_bytes += size;
        if self.first_msg_at.is_none() {
            self.first_msg_at = Some(received_at);
        }
        self.last_msg_at = Some(received_at);
    }

    pub fn avg_latency(&self) -> u64 {
        self.latencies.iter().sum::<u64>() / (self.latencies.len() as u64)
    }

    pub fn total_time(&self) -> u64 {
        self.last_msg_at.unwrap() - self.first_msg_at.unwrap()
    }

    pub fn report(&self) {
        let latency_cycles = self.avg_latency();
        let latency_ns = self.clock.delta_as_nanos(0, latency_cycles);
        eprintln!("average latency: {latency_ns}ns / {latency_cycles} raw");

        let total_time = Duration::from_nanos(self.total_time());
        let total_bytes = self.got_bytes;
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
