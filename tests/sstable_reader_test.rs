use bytes::Bytes;
use fold_rs::memtable::Entry;
use fold_rs::sstable::reader::{SstableLookup, SstableReader};
use fold_rs::sstable::writer::SstableWriter;

#[test]
fn zero_false_negative_end_to_end() {
    let dir = tempfile::tempdir().unwrap();

    let mut writer = SstableWriter::new(dir.path(), 1, 10_000).unwrap();

    let mut expected = Vec::new();

    for i in 0..10_000 {
        let key = Bytes::from(format!("key-{i:05}"));

        let value = Bytes::from(format!("value-{i:05}"));

        writer.add(&key, &Entry::Value(value.clone())).unwrap();

        expected.push((key, value));
    }

    let meta = writer.finish().unwrap();

    let reader = SstableReader::open(meta.path).unwrap();

    for (key, value) in expected {
        match reader.get(&key).unwrap() {
            Some(SstableLookup::Found(v)) => {
                assert_eq!(v, value);
            }

            other => panic!("unexpected lookup result: {other:?}"),
        }
    }
}
