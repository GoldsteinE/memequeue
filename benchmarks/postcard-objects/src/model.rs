use rand::Rng;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    pub fn random(rng: &mut impl Rng) -> Self {
        if rng.gen() {
            Side::Buy
        } else {
            Side::Sell
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarketInfo {
    pub side: Side,
    pub amount: f64,
    pub price: f64,
    pub level: usize,
    pub time: u64,
    pub checksum: u64,
}

impl MarketInfo {
    pub fn random(time: impl FnOnce() -> u64, rng: &mut impl Rng) -> Self {
        let mut this = Self {
            side: Side::random(rng),
            amount: rng.gen(),
            price: rng.gen(),
            level: rng.gen_range(0..1000),
            time: 0,
            checksum: 0,
        };
        this.set_checksum();
        this.time = time();
        this
    }

    // Simplest checksum possible.
    fn calculate_checksum(&self) -> u64 {
        (self.side as u64) ^ self.amount.to_bits() ^ self.price.to_bits() ^ (self.level as u64)
    }

    fn set_checksum(&mut self) {
        self.checksum = self.calculate_checksum();
    }

    pub fn check(&self) {
        assert_eq!(self.checksum, self.calculate_checksum());
    }
}
