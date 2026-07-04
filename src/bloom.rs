use core::f64;
use std::f64::consts::LN_2;
use std::io::{self, Read, Write};

const SEED1: u64 = 0x517cc1b727220a95;
const SEED2: u64 = 0x9e3779b97f4a7c15;

const PRIME: u64 = 0x100000001b3;

pub struct BloomFilter {
    pub bits: Vec<u8>,
    pub num_bits: usize,
    pub k: u32,
}

impl BloomFilter {
    pub fn new(num_bits: usize, k: u32) -> Self {
        let num_byte = num_bits.div_ceil(8);
        Self {
            bits: vec![0; num_byte],
            num_bits,
            k,
        }
    }

    fn bit_ops(&self, bit: usize) -> (usize, u8) {
        debug_assert!(bit < self.num_bits);
        let byte_index = bit / 8;
        let bit_index = bit % 8;
        let bit_mask = 1u8 << bit_index;
        (byte_index, bit_mask)
    }

    fn set_bit(&mut self, bit: usize) {
        let (byte_index, bit_mask) = self.bit_ops(bit);
        self.bits[byte_index] |= bit_mask;
    }

    fn test_bit(&self, bit: usize) -> bool {
        let (byte_index, bit_mask) = self.bit_ops(bit);
        (self.bits[byte_index] & bit_mask) != 0
    }

    pub fn insert(&mut self, key: &[u8]) {
        let (h1, h2) = hash_pair(key);
        for i in 0..self.k {
            let pos = h1.wrapping_add((i as u64).wrapping_mul(h2)) % self.num_bits as u64;
            self.set_bit(pos as usize);
        }
    }
    pub fn may_contain(&self, key: &[u8]) -> bool {
        let (h1, h2) = hash_pair(key);
        for i in 0..self.k {
            let pos = h1.wrapping_add((i as u64).wrapping_mul(h2)) % self.num_bits as u64;
            if !self.test_bit(pos as usize) {
                return false;
            }
        }
        true
    }

    pub fn write_to<W: Write>(&self, writer: &mut W) -> io::Result<()> {
        writer.write_all(&(self.num_bits as u32).to_le_bytes())?;
        writer.write_all(&[self.k as u8])?;
        writer.write_all(&self.bits)?;
        Ok(())
    }

    pub fn serialized_size(&self) -> usize {
        std::mem::size_of::<u32>() + std::mem::size_of::<u8>() + self.bits.len()
    }

    pub fn read_from(mut bytes: &[u8]) -> io::Result<Self> {
        let mut u32_buf = [0u8; 4];

        bytes.read_exact(&mut u32_buf)?;
        let num_bits = u32::from_le_bytes(u32_buf) as usize;

        let mut k = [0u8; 1];
        bytes.read_exact(&mut k)?;

        let byte_count = num_bits.div_ceil(8);

        let mut bit_array = vec![0; byte_count];

        bytes.read_exact(&mut bit_array)?;

        Ok(Self {
            bits: bit_array,
            num_bits,
            k: k[0] as u32,
        })
    }
}

fn hash_with_seed(key: &[u8], seed: u64) -> u64 {
    let mut hash = seed;
    for &byte in key {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(PRIME);
        hash ^= hash >> 32;
    }
    hash
}

pub fn hash_pair(key: &[u8]) -> (u64, u64) {
    (hash_with_seed(key, SEED1), hash_with_seed(key, SEED2))
}

pub fn bloom_size(n: usize, target_fpr: f64) -> (usize, u32) {
    assert!(
        (target_fpr > 0.0 && target_fpr < 1.0),
        "Target FPR:{target_fpr} not in range 0.0..1.0"
    );
    if n == 0 {
        return (0, 1);
    }

    let m = (-(n as f64) * target_fpr.ln() / (LN_2 * LN_2)).ceil() as usize;
    let k = (((m as f64 / n as f64) * LN_2).round() as u32).clamp(1, 30);
    (m, k)
}

#[cfg(test)]
mod tests {

    use rand::RngExt;

    use super::{bloom_size, BloomFilter};

    #[test]
    fn bloom_size_for_1000_keys_1_percent() {
        let (m, k) = bloom_size(1000, 0.01);

        assert!((9580..=9590).contains(&m));
        assert_eq!(k, 7);

        let bytes = m.div_ceil(8);

        assert_eq!(bytes, 1199);

        assert!((1199..=1200).contains(&bytes));
    }

    #[test]
    fn bloom_size_zero_keys() {
        let (m, k) = bloom_size(0, 0.01);

        assert_eq!(m, 0);
        assert_eq!(k, 1);
    }

    #[test]
    #[should_panic]
    fn bloom_size_invalid_fpr_zero() {
        bloom_size(1000, 0.0);
    }

    #[test]
    #[should_panic]
    fn bloom_size_invalid_fpr_one() {
        bloom_size(1000, 1.0);
    }
    #[test]
    fn new_filter_is_empty() {
        let filter = BloomFilter::new(64, 7);

        for i in 0..64 {
            assert!(!filter.test_bit(i));
        }
    }

    #[test]
    fn set_and_test_single_bit() {
        let mut filter = BloomFilter::new(64, 7);

        filter.set_bit(21);

        assert!(filter.test_bit(21));

        assert!(!filter.test_bit(20));
        assert!(!filter.test_bit(22));
    }

    #[test]
    fn set_multiple_bits() {
        let mut filter = BloomFilter::new(64, 7);

        filter.set_bit(0);
        filter.set_bit(7);
        filter.set_bit(8);
        filter.set_bit(63);

        assert!(filter.test_bit(0));
        assert!(filter.test_bit(7));
        assert!(filter.test_bit(8));
        assert!(filter.test_bit(63));
    }

    #[test]
    fn bits_do_not_interfere() {
        let mut filter = BloomFilter::new(16, 7);

        filter.set_bit(3);

        for i in 0..16 {
            if i == 3 {
                assert!(filter.test_bit(i));
            } else {
                assert!(!filter.test_bit(i));
            }
        }
    }

    #[test]
    fn inserted_key_is_found() {
        let mut bloom = BloomFilter::new(1024, 7);

        bloom.insert(b"apple");

        assert!(bloom.may_contain(b"apple"));
    }

    #[test]
    fn multiple_inserted_keys_are_found() {
        let mut bloom = BloomFilter::new(8192, 7);

        let keys = [
            b"apple".as_slice(),
            b"banana".as_slice(),
            b"cat".as_slice(),
            b"dog".as_slice(),
        ];

        for key in keys {
            bloom.insert(key);
        }

        for key in keys {
            assert!(bloom.may_contain(key));
        }
    }

    #[test]
    fn empty_filter_contains_nothing() {
        let bloom = BloomFilter::new(1024, 7);

        assert!(!bloom.may_contain(b"apple"));
        assert!(!bloom.may_contain(b"banana"));
    }

    #[test]
    fn no_false_negatives() {
        let mut bloom = BloomFilter::new(20_000, 7);

        for i in 0..1000 {
            let key = format!("key{i}");
            bloom.insert(key.as_bytes());
        }

        for i in 0..1000 {
            let key = format!("key{i}");
            assert!(bloom.may_contain(key.as_bytes()));
        }
    }

    #[test]
    fn zero_false_negatives() {
        use rand::distr::Alphanumeric;

        const NUM_KEYS: usize = 10_000;

        let (num_bits, k) = bloom_size(NUM_KEYS, 0.01);

        let mut filter = BloomFilter::new(num_bits, k);

        let mut rng = rand::rng();

        let mut inserted_keys = Vec::with_capacity(NUM_KEYS);

        for _ in 0..NUM_KEYS {
            let key: String = (&mut rng)
                .sample_iter(&Alphanumeric)
                .take(32)
                .map(char::from)
                .collect();

            filter.insert(key.as_bytes());

            inserted_keys.push(key);
        }

        for key in &inserted_keys {
            assert!(
                filter.may_contain(key.as_bytes()),
                "false negative for key: {key}",
            );
        }
    }

    #[test]
    fn measured_false_positive_rate() {
        use std::collections::HashSet;

        use rand::distr::Alphanumeric;

        const NUM_INSERTED: usize = 10_000;
        const NUM_QUERIES: usize = 10_000;
        const TARGET_FPR: f64 = 0.01;

        let (num_bits, k) = bloom_size(NUM_INSERTED, TARGET_FPR);

        let mut filter = BloomFilter::new(num_bits, k);

        let mut rng = rand::rng();

        let mut inserted = HashSet::with_capacity(NUM_INSERTED);

        while inserted.len() < NUM_INSERTED {
            let key: String = (&mut rng)
                .sample_iter(&Alphanumeric)
                .take(32)
                .map(char::from)
                .collect();

            if inserted.insert(key.clone()) {
                filter.insert(key.as_bytes());
            }
        }

        let mut false_positives = 0usize;
        let mut queries = 0usize;

        while queries < NUM_QUERIES {
            let key = format!(
                "missing_{}",
                (&mut rng)
                    .sample_iter(&Alphanumeric)
                    .take(32)
                    .map(char::from)
                    .collect::<String>()
            );

            if inserted.contains(&key) {
                continue;
            }

            queries += 1;

            if filter.may_contain(key.as_bytes()) {
                false_positives += 1;
            }
        }

        let actual_fpr = false_positives as f64 / NUM_QUERIES as f64;

        println!(
            "Bloom Filter\n\
         Target FPR : {:.4}\n\
         Actual FPR : {:.4}\n\
         False Positives : {}/{}",
            TARGET_FPR, actual_fpr, false_positives, NUM_QUERIES,
        );

        assert!(
            (actual_fpr - TARGET_FPR).abs() < 0.005,
            "target={:.4}, actual={:.4}",
            TARGET_FPR,
            actual_fpr
        );
    }
}
