use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
};

use bytes::Bytes;
use fold_rs::sstable::writer::SSTABLE_MAGIC;
use tempfile::tempdir;

use fold_rs::{memtable::Memtable, sstable::writer::SstableWriter};

#[test]
fn sstable_size_matches_test() {
    const NUM_KEYS: usize = 1000;
    const TOLERANCE: u64 = 4 * 4096;

    let dir = tempdir().unwrap();

    let mut memtable = Memtable::new();

    for i in 0..NUM_KEYS {
        let key = Bytes::from(format!("{i:016}"));

        let value = Bytes::from(vec![b'x'; 100]);

        memtable.put(key, value);
    }

    let mut writer = SstableWriter::new(dir.path(), 1, NUM_KEYS).unwrap();

    for (key, entry) in memtable.iter() {
        writer.add(key, entry).unwrap();
    }

    let meta = writer.finish().unwrap();

    let metadata = std::fs::metadata(&meta.path).unwrap();

    let file_size = metadata.len();

    println!("file size = {file_size}");

    let data_size: u64 = 1000 * (4 + 4 + 1 + 16 + 100);

    let index_entries = data_size.div_ceil(4096);

    let index_size = index_entries * (4 + 16 + 8);

    let bloom_size = 1200;

    let expected = data_size + index_size + bloom_size + 24;

    assert!(
        file_size >= expected - TOLERANCE && file_size <= expected + TOLERANCE,
        "expected ≈ {expected} bytes, got {file_size}",
    );

    let mut file = File::open(&meta.path).unwrap();

    file.seek(SeekFrom::End(-8)).unwrap();

    let mut magic = [0u8; 8];

    file.read_exact(&mut magic).unwrap();

    let magic = u64::from_le_bytes(magic);

    assert_eq!(magic, SSTABLE_MAGIC);
}
