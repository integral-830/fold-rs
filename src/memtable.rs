use std::collections::BTreeMap;

use bytes::Bytes;

const ENTRY_OVERHEAD_BYTES: usize = 48;

#[derive(Clone, PartialEq, Debug)]
pub enum Entry {
    Value(Bytes),
    Tombstone,
}

#[derive(Clone, PartialEq, Debug)]
pub enum LookupResult {
    Found(Bytes),
    Tombstone,
    NotInMemtable,
}

#[derive(Debug)]
pub struct Memtable {
    data: BTreeMap<Bytes, Entry>,
    size_bytes: usize,
}

impl Memtable {
    pub fn new() -> Self {
        Self {
            data: BTreeMap::new(),
            size_bytes: 0,
        }
    }
    pub fn put(&mut self, key: Bytes, value: Bytes) {
        let entry = Entry::Value(value);
        let new_size = calc_entry_size(&key, &entry);

        match self.data.insert(key.clone(), entry) {
            Some(old_val) => {
                self.size_bytes -= calc_entry_size(&key, &old_val);
                self.size_bytes += new_size;
            }
            None => {
                self.size_bytes += new_size;
            }
        }
    }

    pub fn delete(&mut self, key: Bytes) {
        let entry = Entry::Tombstone;
        let new_size = calc_entry_size(&key, &entry);

        match self.data.insert(key.clone(), entry) {
            Some(old_val) => {
                self.size_bytes -= calc_entry_size(&key, &old_val);
                self.size_bytes += new_size;
            }
            None => {
                self.size_bytes += new_size;
            }
        }
    }

    pub fn get(&self, key: &[u8]) -> LookupResult {
        match self.data.get(key) {
            Some(Entry::Value(value)) => LookupResult::Found(value.clone()),
            Some(Entry::Tombstone) => LookupResult::Tombstone,
            None => LookupResult::NotInMemtable,
        }
    }
    pub fn iter(&self) -> impl Iterator<Item = (&Bytes, &Entry)> {
        self.data.iter()
    }
}

impl Default for Memtable {
    fn default() -> Self {
        Self::new()
    }
}

fn calc_entry_size(key: &Bytes, entry: &Entry) -> usize {
    let value_len = match entry {
        Entry::Value(bytes) => bytes.len(),
        Entry::Tombstone => 0,
    };
    key.len() + value_len + ENTRY_OVERHEAD_BYTES
}

#[cfg(test)]
mod tests {
    use crate::memtable::{calc_entry_size, LookupResult, Memtable, ENTRY_OVERHEAD_BYTES};

    #[test]
    fn iter_returns_sorted_keys() {
        use bytes::Bytes;

        let mut memtable = Memtable::new();

        memtable.put(Bytes::from("z"), Bytes::from("1"));

        memtable.put(Bytes::from("a"), Bytes::from("2"));

        memtable.put(Bytes::from("m"), Bytes::from("3"));

        let mut prev: Option<&Bytes> = None;

        for (key, _) in memtable.iter() {
            if let Some(prev_key) = prev {
                debug_assert!(prev_key < key, "keys are not strictly increasing");
            }

            prev = Some(key);
        }
    }

    #[test]
    fn tombstone_round_trip() {
        use bytes::Bytes;

        let mut memtable = Memtable::new();

        memtable.put(Bytes::from("a"), Bytes::from("1"));

        let initial_size = memtable.size_bytes;

        assert_eq!(memtable.get(b"a"), LookupResult::Found(Bytes::from("1"),));

        memtable.delete(Bytes::from("a"));

        let tombstone_size = memtable.size_bytes;

        assert_eq!(memtable.get(b"a"), LookupResult::Tombstone);

        assert_ne!(tombstone_size, initial_size);

        memtable.put(Bytes::from("a"), Bytes::from("2"));

        assert_eq!(memtable.get(b"a"), LookupResult::Found(Bytes::from("2"),));

        let expected = 1 + 1 + ENTRY_OVERHEAD_BYTES;

        assert_eq!(memtable.size_bytes, expected);
    }

    #[test]
    fn size_tracking_matches_ground_truth() {
        use bytes::Bytes;

        let mut memtable = Memtable::new();

        for i in 0..1000 {
            let key = Bytes::from(format!("key-{i}"));

            let value = Bytes::from(vec![b'x'; i % 100 + 1]);

            memtable.put(key, value);
        }

        let actual = memtable.size_bytes;

        let expected: usize = memtable
            .iter()
            .map(|(key, entry)| calc_entry_size(key, entry))
            .sum();

        assert_eq!(actual, expected);
    }
}
