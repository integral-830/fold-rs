use std::io;
use std::sync::atomic::AtomicU64;
use std::{
    fs,
    path::{Path, PathBuf},
};

use bytes::Bytes;

use crate::error::Result;
use crate::sstable::reader::{SstableLookup, SstableReader};
use crate::sstable::writer::SstableWriter;
use crate::wal::format::{RecordType, WalRecord};
use crate::wal::reader::WalReader;
use crate::{memtable::Memtable, wal::writer::WalWriter};

const FLUSH_THRESHOLD_BYTES: usize = 4 * 1024 * 1024;

pub struct StorageEngine {
    dir: PathBuf,
    memtable: Memtable,
    nx_sstable_seq: AtomicU64,
    sstables: Vec<SstableReader>,
    wal: WalWriter,
    current_wal_path: PathBuf,
    next_wal_seq: AtomicU64,
}

impl StorageEngine {
    pub fn open(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        cleanup_orphan_sstables(dir.as_ref())?;
        let (sstables, nx_sstable_seq) = load_sstables(&dir)?;
        let (_latest_wal, next_wal_seq) = load_wal_generation(&dir)?;
        let wal_path = dir.join(format!("wal.{next_wal_seq:08}.log"));
        let wal = WalWriter::open(&wal_path)?;
        let memtable = Memtable::new();
        let mut engine = Self {
            dir,
            memtable,
            nx_sstable_seq: AtomicU64::new(nx_sstable_seq),
            sstables,
            wal,
            current_wal_path: wal_path,
            next_wal_seq: AtomicU64::new(next_wal_seq),
        };

        engine.replay()?;
        Ok(engine)
    }

    fn replay(&mut self) -> Result<()> {
        let wal_files = load_wals(&self.dir)?;
        for wal in wal_files {
            let reader = WalReader::open(&wal)?;
            for record in reader {
                match record.record_type {
                    RecordType::Put => self.memtable.put(record.key.into(), record.value.into()),
                    RecordType::Delete => self.memtable.delete(record.key.into()),
                }
            }
        }
        Ok(())
    }

    pub fn put(&mut self, key: &[u8], value: &[u8]) -> Result<()> {
        let record = WalRecord {
            record_type: RecordType::Put,
            key,
            value,
        };
        self.wal.append(&record)?;
        self.memtable
            .put(Bytes::copy_from_slice(key), Bytes::copy_from_slice(value));
        if self.memtable.size_bytes() >= FLUSH_THRESHOLD_BYTES {
            self.flush()?;
        }
        Ok(())
    }

    pub fn delete(&mut self, key: &[u8]) -> Result<()> {
        let record = WalRecord {
            record_type: RecordType::Delete,
            key,
            value: &[],
        };
        self.wal.append(&record)?;
        self.memtable.delete(Bytes::copy_from_slice(key));
        if self.memtable.size_bytes() >= FLUSH_THRESHOLD_BYTES {
            self.flush()?;
        }
        Ok(())
    }

    pub fn get(&self, key: &[u8]) -> Result<Option<Bytes>> {
        match self.memtable.get(key) {
            crate::memtable::LookupResult::Found(bytes) => return Ok(Some(bytes)),
            crate::memtable::LookupResult::Tombstone => return Ok(None),
            crate::memtable::LookupResult::NotInMemtable => {}
        }

        for sstable in self.sstables.iter().rev() {
            match sstable.get(key)? {
                Some(SstableLookup::Found(value)) => {
                    return Ok(Some(value));
                }
                Some(SstableLookup::Tombstone) => {
                    return Ok(None);
                }
                None => {}
            }
        }
        Ok(None)
    }

    fn next_sstable_seq(&self) -> u64 {
        self.nx_sstable_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    fn next_wal_seq(&self) -> u64 {
        self.next_wal_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    fn rotate_wal(&mut self) -> io::Result<()> {
        self.wal.sync_all()?;
        let old_path = self.current_wal_path.clone();
        let seq = self.next_wal_seq();
        let new_path = self.dir.join(format!("wal.{seq}:08"));
        let new_wal = WalWriter::open(&new_path)?;
        self.current_wal_path = new_path;
        self.wal = new_wal;
        fs::remove_file(old_path)?;
        Ok(())
    }
    fn flush(&mut self) -> io::Result<()> {
        if self.memtable.is_empty() {
            return Ok(());
        }
        let sstable_seq = self.next_sstable_seq();
        let mut writer = SstableWriter::new(&self.dir, sstable_seq, self.memtable.len())?;
        for (key, entry) in self.memtable.iter() {
            writer.add(key, entry)?;
        }
        let meta = writer.finish()?;
        let reader = SstableReader::open(&meta.path)?;
        self.rotate_wal()?;
        self.memtable = Memtable::new();
        self.sstables.push(reader);
        Ok(())
    }
}

fn load_sstables(dir: &Path) -> io::Result<(Vec<SstableReader>, u64)> {
    let mut files = Vec::<(u64, PathBuf)>::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".sst") {
            continue;
        }
        let Some(seq) = name
            .strip_suffix(".sst")
            .and_then(|s| s.parse::<u64>().ok())
        else {
            continue;
        };
        files.push((seq, path));
    }
    files.sort_by_key(|(seq, _)| *seq);
    let next_seq = files.last().map(|(seq, _)| seq + 1).unwrap_or(1);
    let mut sstables = Vec::with_capacity(files.len());
    for (_, path) in files.into_iter().rev() {
        sstables.push(SstableReader::open(path)?);
    }
    Ok((sstables, next_seq))
}

fn load_wals(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::<(u64, PathBuf)>::new();

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        if !name.starts_with("wal.") || !name.ends_with(".log") {
            continue;
        }

        let Some(seq) = name
            .strip_prefix("wal.")
            .and_then(|s| s.strip_suffix(".log"))
            .and_then(|s| s.parse::<u64>().ok())
        else {
            continue;
        };

        files.push((seq, path));
    }

    files.sort_by_key(|(seq, _)| *seq);

    Ok(files.into_iter().map(|(_, path)| path).collect())
}

fn load_wal_generation(dir: &Path) -> io::Result<(Option<PathBuf>, u64)> {
    let mut latest: Option<(u64, PathBuf)> = None;

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        let Some(seq) = name
            .strip_prefix("wal.")
            .and_then(|s| s.parse::<u64>().ok())
        else {
            continue;
        };

        match &latest {
            Some((max, _)) if *max >= seq => {}
            _ => latest = Some((seq, path)),
        }
    }

    match latest {
        Some((seq, path)) => Ok((Some(path), seq + 1)),
        None => Ok((None, 1)),
    }
}

fn cleanup_orphan_sstables(dir: &Path) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;

        let path = entry.path();

        if let Some(name) = path.file_name().and_then(|n| n.to_str())
            && name.ends_with(".sst.tmp")
        {
            fs::remove_file(path)?;
        }
    }

    Ok(())
}

#[cfg(test)]
impl StorageEngine {
    pub fn sstable_count(&self) -> usize {
        self.sstables.len()
    }
    fn flush_for_test(&mut self) -> std::io::Result<()> {
        self.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::StorageEngine;

    #[test]
    fn recovers_after_reopen() {
        let dir = tempfile::tempdir().unwrap();

        {
            let mut engine = StorageEngine::open(dir.path()).unwrap();

            for i in 0..100 {
                let key = format!("key-{i}");

                let value = format!("value-{i}");

                engine.put(key.as_bytes(), value.as_bytes()).unwrap();
            }
        }

        let engine = StorageEngine::open(dir.path()).unwrap();

        for i in 0..100 {
            let key = format!("key-{i}");

            let value = format!("value-{i}");

            let actual = engine.get(key.as_bytes()).unwrap();

            assert_eq!(actual, Some(bytes::Bytes::from(value,),),);
        }
    }

    #[test]
    fn recovers_deletes_after_reopen() {
        let dir = tempfile::tempdir().unwrap();

        {
            let mut engine = StorageEngine::open(dir.path()).unwrap();

            engine.put(b"a", b"1").unwrap();

            engine.delete(b"a").unwrap();
        }

        let engine = StorageEngine::open(dir.path()).unwrap();

        assert_eq!(engine.get(b"a").unwrap(), None,);
    }

    #[test]
    fn removes_orphan_sstables() {
        let dir = tempfile::tempdir().unwrap();

        let orphan = dir.path().join("000001.sst.tmp");

        std::fs::write(&orphan, b"garbage").unwrap();

        assert!(orphan.exists());

        StorageEngine::open(dir.path()).unwrap();

        assert!(!orphan.exists());
    }

    #[test]
    fn overwrite_across_flush_boundary_returns_newest_value() {
        let dir = tempfile::tempdir().unwrap();

        let mut engine = StorageEngine::open(dir.path()).unwrap();

        engine.put(b"k", b"v1").unwrap();

        engine.flush_for_test().unwrap();

        assert_eq!(engine.sstable_count(), 1);

        engine.put(b"k", b"v2").unwrap();

        assert_eq!(engine.get(b"k").unwrap(), Some(bytes::Bytes::from("v2")),);
    }

    #[test]
fn tombstone_shadows_value_across_flush_boundary() {
    let dir = tempfile::tempdir().unwrap();

    let mut engine = StorageEngine::open(dir.path()).unwrap();

    engine.put(b"k", b"v").unwrap();

    engine.flush_for_test().unwrap();

    assert_eq!(engine.sstable_count(), 1);

    engine.delete(b"k").unwrap();

    assert_eq!(
        engine.get(b"k").unwrap(),
        None,
    );
}

#[test]
fn tombstone_in_newer_sstable_shadows_older_value() {
    let dir = tempfile::tempdir().unwrap();

    let mut engine = StorageEngine::open(dir.path()).unwrap();

    engine.put(b"k", b"v").unwrap();

    engine.flush_for_test().unwrap();

    assert_eq!(engine.sstable_count(), 1);

    engine.delete(b"k").unwrap();

    engine.flush_for_test().unwrap();

    assert_eq!(engine.sstable_count(), 2);

    assert_eq!(engine.get(b"k").unwrap(), None);
}
}

#[test]
fn creates_multiple_sstables_after_many_flushes() {
    use bytes::Bytes;

    const TOTAL_DATA: usize = 50 * 1024 * 1024;
    const VALUE_SIZE: usize = 1024;

    let dir = tempfile::tempdir().unwrap();

    let mut engine = StorageEngine::open(dir.path()).unwrap();

    let mut written = 0usize;
    let mut key_index = 0usize;

    while written < TOTAL_DATA {
        let key = format!("key-{key_index}");
        let value = Bytes::from(vec![(key_index % 256) as u8; VALUE_SIZE]);

        engine.put(key.as_bytes(), value.as_ref()).unwrap();

        written += key.len() + value.len();

        key_index += 1;

        if key_index.is_multiple_of(5000) {
            println!("inserted {key_index}");
        }
    }

    let expected_flushes = TOTAL_DATA / FLUSH_THRESHOLD_BYTES;

    assert!(
        engine.sstable_count() >= expected_flushes.saturating_sub(1),
        "too few SSTables: expected about {}, got {}",
        expected_flushes,
        engine.sstable_count(),
    );

    assert!(
        engine.sstable_count() <= expected_flushes + 1,
        "too many SSTables: expected about {}, got {}",
        expected_flushes,
        engine.sstable_count(),
    );
}
