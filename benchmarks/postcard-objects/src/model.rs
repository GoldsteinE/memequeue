use rand::Rng;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

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

pub trait Message {
    fn time(&self) -> u64;
    fn check(&self);
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
}

impl Message for MarketInfo {
    fn time(&self) -> u64 {
        self.time
    }

    fn check(&self) {
        assert_eq!(self.checksum, self.calculate_checksum());
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Batch<M> {
    // TODO: arbitrarily chosen, is there a way to do better?
    pub inner: SmallVec<[M; 16]>,
    pub time: u64,
    pub size: usize,
}

impl<M> Batch<M> {
    pub fn new(inner: SmallVec<[M; 16]>, time: u64) -> Self {
        let size = inner.len();
        Self { inner, time, size }
    }
}

impl<M: Message> Message for Batch<M> {
    fn time(&self) -> u64 {
        self.time
    }

    fn check(&self) {
        assert_eq!(self.inner.len(), self.size);
        for m in &self.inner {
            m.check();
        }
    }
}
