use std::{
    fs,
    path::{Path, PathBuf},
};

use bytes::Bytes;

use crate::error::Result;
use crate::wal::format::{RecordType, WalRecord};
use crate::wal::reader::WalReader;
use crate::{memtable::Memtable, wal::writer::WalWriter};

const WAL_FILE_NAME: &str = "wal.log";

pub struct StorageEngine {
    wal: WalWriter,
    memtable: Memtable,
    dir: PathBuf,
}

impl StorageEngine {
    pub fn open(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        let wal_path = dir.join(WAL_FILE_NAME);
        let wal = WalWriter::open(&wal_path)?;
        let memtable = Memtable::new();
        let mut engine = Self { wal, memtable, dir };
        engine.replay()?;
        Ok(engine)
    }

    fn replay(&mut self) -> Result<()> {
        let wal_path = self.dir.join(WAL_FILE_NAME);
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

    pub fn get(&mut self, key: &[u8]) -> Result<Option<Bytes>> {
        match self.memtable.get(key) {
            crate::memtable::LookupResult::Found(bytes) => Ok(Some(bytes)),
            crate::memtable::LookupResult::Tombstone => Ok(None),
            crate::memtable::LookupResult::NotInMemtable => {
                //TODO: check self.sstables later
                Ok(None)
            }
        }
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

        let mut engine = StorageEngine::open(dir.path()).unwrap();

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

        let mut engine = StorageEngine::open(dir.path()).unwrap();

        assert_eq!(engine.get(b"a").unwrap(), None,);
    }
}
