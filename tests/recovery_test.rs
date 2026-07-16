use std::time::Duration;
use std::{fs, process, thread};

use bytes::Bytes;
use fold_rs::storage_engine::StorageEngine;

fn run_crash_and_recover() -> Result<(), String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;

    let mut child = process::Command::new(env!("CARGO_BIN_EXE_crash_writer"))
        .arg(dir.path())
        .spawn()
        .map_err(|e| e.to_string())?;

    thread::sleep(Duration::from_secs(2));

    let _ = child.kill();
    let _ = child.wait();

    let acked = fs::read_to_string(dir.path().join("acked.log")).map_err(|e| e.to_string())?;

    let engine = StorageEngine::open(dir.path()).map_err(|e| e.to_string())?;

    for line in acked.lines() {
        let (key, value) = line
            .split_once('\t')
            .ok_or_else(|| format!("invalid acked.log line: {line}"))?;

        let actual = engine.get(key.as_bytes()).map_err(|e| e.to_string())?;

        if actual != Some(Bytes::from(value.to_owned())) {
            return Err(format!("key={key} expected={value:?} got={actual:?}"));
        }
    }

    Ok(())
}

#[test]
fn crash_and_recover() {
    run_crash_and_recover().unwrap();
}

#[test]
fn kill9_recovery_20x() {
    for i in 0..20 {
        run_crash_and_recover().unwrap_or_else(|e| panic!("iteration {i} failed: {e}"));
    }
}

#[test]
fn ignores_half_written_final_record() {
    use bytes::Bytes;
    use fold_rs::{
        storage_engine::StorageEngine,
        wal::{
            format::{RecordType, WalRecord},
            writer::WalWriter,
        },
    };

    const N: usize = 100;
    let dir = tempfile::tempdir().unwrap();

    let wal_path = wal_path(dir.path(), 1);

    let mut writer = WalWriter::open(&wal_path).unwrap();

    for i in 0..N {
        let key = format!("key-{i}");

        let value = format!("value-{i}");

        let record = WalRecord {
            record_type: RecordType::Put,
            key: key.as_bytes(),
            value: value.as_bytes(),
        };

        writer.append(&record).unwrap();
    }

    drop(writer);

    let original_len = std::fs::metadata(&wal_path).unwrap().len();

    let truncated_len = original_len - 3;

    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&wal_path)
        .unwrap();

    file.set_len(truncated_len).unwrap();

    drop(file);

    let engine = StorageEngine::open(dir.path()).unwrap();

    for i in 0..(N - 1) {
        let key = format!("key-{i}");

        let expected = Bytes::from(format!("value-{i}"));

        assert_eq!(
            engine.get(key.as_bytes()).unwrap(),
            Some(expected),
            "failed on key {key}"
        );
    }

    let final_key = format!("key-{}", N - 1);

    assert_eq!(
        engine.get(final_key.as_bytes()).unwrap(),
        None,
        "last record should be discarded",
    );
}

#[test]
fn opens_empty_wal() {
    use fold_rs::storage_engine::StorageEngine;

    let dir = tempfile::tempdir().unwrap();

    let wal_path = dir.path().join("wal.log");

    std::fs::File::create(&wal_path).unwrap();

    let engine = StorageEngine::open(dir.path()).unwrap();

    assert_eq!(engine.get(b"missing").unwrap(), None);
}

fn wal_path(dir: &std::path::Path, seq: u64) -> std::path::PathBuf {
    dir.join(format!("wal.{seq:08}.log"))
}
