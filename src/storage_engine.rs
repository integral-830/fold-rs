use std::io;
use std::sync::atomic::AtomicU64;
use std::{
    fs,
    path::{Path, PathBuf},
};

use bytes::Bytes;

use crate::error::Result;
use crate::sstable::reader::SstableReader;
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
    wal_seq: AtomicU64,
}

impl StorageEngine {
    pub fn open(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        cleanup_orphan_sstables(dir.as_ref())?;
        let (sstables, nx_sstable_seq) = load_sstables(&dir)?;
        let (latest_wal,next_wal_seq) = load_wal_generation(&dir)?;
        let wal_path = dir.join(format!("wal.{next_wal_seq:08}"));
        let wal = WalWriter::open(&wal_path)?;
        let memtable = Memtable::new();
        let mut engine = Self {
            dir,
            memtable,
            nx_sstable_seq: AtomicU64::new(nx_sstable_seq),
            sstables,
            wal,
            wal_seq: AtomicU64::new(next_wal_seq),
        };

        if let Some(path) = latest_wal {
        engine.replay(&path)?;
        }
        Ok(engine)
    }

    fn replay(&mut self, wal_path: &Path) -> Result<()> {
        let reader = WalReader::open(&wal_path)?;

        for record in reader {
            match record.record_type {
                RecordType::Put => self.memtable.put(record.key.into(), record.value.into()),
                RecordType::Delete => self.memtable.delete(record.key.into()),
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
        Ok(())
    }

    pub fn get(&self, key: &[u8]) -> Result<Option<Bytes>> {
        match self.memtable.get(key) {
            crate::memtable::LookupResult::Found(bytes) => Ok(Some(bytes)),
            crate::memtable::LookupResult::Tombstone => Ok(None),
            crate::memtable::LookupResult::NotInMemtable => {
                //TODO: check self.sstables later
                Ok(None)
            }
        }
    }

    fn next_sstable_seq(&self)->u64{
        self.nx_sstable_seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    fn next_wal_seq(&self)->u64{
        self.wal_seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
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

fn load_wal_generation(
    dir: &Path,
) -> io::Result<(Option<PathBuf>, u64)> {
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

        if let Some(name) = path.file_name().and_then(|n| n.to_str()) && name.ends_with(".sst.tmp") {
            fs::remove_file(path)?;
        }
    }

    Ok(())
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
}
