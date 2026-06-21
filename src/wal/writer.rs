use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::wal::format::{serialize, WalRecord};

pub struct WalWriter {
    file: File,
    path: PathBuf,
    buf: Vec<u8>,
}

impl WalWriter {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();

        let file = OpenOptions::new().append(true).create(true).open(&path)?;
        Ok(Self {
            file,
            path,
            buf: Vec::new(),
        })
    }

    fn write_record(&mut self, record: &WalRecord, sync: bool) -> io::Result<()> {
        self.buf.clear();

        serialize(record, &mut self.buf);

        self.file.write_all(&self.buf)?;

        if sync {
            self.file.sync_all()?;
        }

        Ok(())
    }

    pub fn append(&mut self, record: &WalRecord<'_>) -> io::Result<()> {
        self.write_record(record, true)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(any(test, feature = "bench-utils"))]
impl WalWriter {
    pub fn append_without_sync(&mut self, record: &WalRecord) -> io::Result<()> {
        self.write_record(record, false)
    }

    pub fn sync(&mut self) -> io::Result<()> {
        self.file.sync_all()
    }
}

#[cfg(test)]
mod tests {
    use crate::wal::format::{encoded_len, RecordType, WalRecord};
    use crate::wal::writer::WalWriter;

    #[test]
    fn wal_file_size_matches_encoded_sizes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wal.log");

        let mut writer = WalWriter::open(&path).unwrap();

        let mut expected_size = 0usize;

        for i in 0..100 {
            let key = format!("key-{i}");
            let value = vec![b'x'; 128];

            let record = WalRecord {
                record_type: RecordType::Put,
                key: key.as_bytes(),
                value: &value,
            };

            expected_size += encoded_len(record.key, record.value);

            writer.append(&record).unwrap();
        }

        drop(writer);

        let actual_size = std::fs::metadata(path).unwrap().len() as usize;

        assert_eq!(actual_size, expected_size);
    }
}
