//! Deterministic pseudo-random numbers: xorshift64* seeded through splitmix64.
//!
//! A bit-exact port of `uRandom.pas` from the Pascal version, so a given seed
//! produces the same sequence of draws in both programs. Each experiment owns
//! its own `Rng`; there is no global state.

const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;

/// A fresh base seed from the operating system's random-number generator
/// (the browser's crypto API on the web), never 0 (0 means "pick one").
/// Falls back to the clock if the OS source is unavailable.
pub fn random_base_seed() -> u32 {
    let seed = getrandom::u32().unwrap_or_else(|_| clock_seed());
    // Keep it at most 9 digits so it is easy to read and type back in.
    (seed % 1_000_000_000).max(1)
}

/// Fallback seed source (std::time is unavailable in the browser, where the
/// OS source always exists).
fn clock_seed() -> u32 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos()) as u32
    }
    #[cfg(target_arch = "wasm32")]
    {
        1
    }
}

#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
}

impl Rng {
    /// Seed the generator (`rndSetSeed`): a splitmix64 finaliser spreads the seed
    /// over the full 64-bit state.
    pub fn new(seed: u64) -> Self {
        let mut z = seed.wrapping_add(GOLDEN);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        if z == 0 {
            z = GOLDEN;
        }
        Self { state: z }
    }

    /// The seed for one experiment: the base seed in the high 32 bits and the
    /// experiment index in the low 32 bits, as in the Pascal version.
    pub fn for_experiment(base_seed: u32, experiment_id: u32) -> Self {
        Self::new((u64::from(base_seed) << 32) | u64::from(experiment_id))
    }

    fn next_raw(&mut self) -> u64 {
        let mut x = self.state;
        if x == 0 {
            x = GOLDEN;
        }
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in [0, 1) (`rndFloat`).
    pub fn next_f64(&mut self) -> f64 {
        (self.next_raw() >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0)
    }

    /// Uniform in 0..range (`rndInt`); 0 if `range <= 0`.
    pub fn next_int(&mut self, range: i32) -> i32 {
        if range <= 0 {
            return 0;
        }
        (self.next_raw() % range as u64) as i32
    }

    /// Uniform in lo..hi (`rndRange`); `lo` if `hi <= lo`.
    pub fn range(&mut self, lo: i32, hi: i32) -> i32 {
        if hi <= lo {
            return lo;
        }
        lo + self.next_int(hi - lo)
    }

    /// Uniform index in 0..n, for slices.
    pub fn index(&mut self, n: usize) -> usize {
        self.range(0, n as i32) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_sequence() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..1000 {
            assert_eq!(a.next_raw(), b.next_raw());
        }
    }

    #[test]
    fn floats_in_unit_interval() {
        let mut r = Rng::new(7);
        for _ in 0..10_000 {
            let x = r.next_f64();
            assert!((0.0..1.0).contains(&x));
        }
    }

    #[test]
    fn random_base_seeds_differ() {
        let a = random_base_seed();
        let b = random_base_seed();
        assert!(a > 0 && b > 0 && a < 1_000_000_000);
        assert_ne!(a, b, "two OS-random seeds collided (1 in a billion)");
    }

    #[test]
    fn range_bounds() {
        let mut r = Rng::new(1);
        for _ in 0..10_000 {
            let v = r.range(3, 8);
            assert!((3..8).contains(&v));
        }
        assert_eq!(r.range(5, 5), 5);
        assert_eq!(r.next_int(0), 0);
    }
}
