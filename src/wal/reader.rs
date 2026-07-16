use std::io::{self, ErrorKind, Read};
use std::path::Path;
use std::{fs::File, io::BufReader, path::PathBuf};

use crate::wal::format::{RecordType, WalRecordOwned};
use crc32fast::hash;

use super::format::HEADER_SIZE;

const READER_CAPACITY: usize = 64 * 1024;

pub struct WalReader {
    reader: BufReader<File>,
    path: PathBuf,
}

impl WalReader {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = File::open(&path)?;
        let reader = BufReader::with_capacity(READER_CAPACITY, file);
        Ok(Self { reader, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn read_one(&mut self) -> Option<WalRecordOwned> {
        let mut header = [0u8; HEADER_SIZE];
        match self.reader.read_exact(&mut header) {
            Ok(()) => {}
            Err(err) if err.kind() == ErrorKind::UnexpectedEof => return None,
            Err(_) => return None,
        }

        let stored_crc = u32::from_le_bytes(header[0..4].try_into().unwrap());
        let key_len = u32::from_le_bytes(header[4..8].try_into().unwrap());
        let value_len = u32::from_le_bytes(header[8..12].try_into().unwrap());

        let record_type = match header[12] {
            0 => RecordType::Put,
            1 => RecordType::Delete,
            _ => return None,
        };
        let mut payload = vec![0u8; (key_len + value_len) as usize];

        match self.reader.read_exact(&mut payload) {
            Ok(()) => {}
            Err(err) if err.kind() == ErrorKind::UnexpectedEof => return None,
            Err(_) => return None,
        }

        let mut new_crc_slice = Vec::with_capacity(4 + 4 + 1 + payload.len());
        new_crc_slice.extend_from_slice(&key_len.to_le_bytes());
        new_crc_slice.extend_from_slice(&value_len.to_le_bytes());
        new_crc_slice.push(record_type as u8);
        new_crc_slice.extend_from_slice(&payload);

        let computed_crc = hash(&new_crc_slice);

        if computed_crc != stored_crc {
            return None;
        }
        Some(WalRecordOwned {
            record_type,
            key: payload[..key_len as usize].to_vec(),
            value: payload[key_len as usize..].to_vec(),
        })
    }
}

impl Iterator for WalReader {
    type Item = WalRecordOwned;

    fn next(&mut self) -> Option<Self::Item> {
        self.read_one()
    }
}

#[cfg(test)]
mod tests {
    use crate::wal::format::{HEADER_SIZE, RecordType, WalRecord};
    use crate::wal::reader::WalReader;
    use crate::wal::writer::WalWriter;

    #[test]
    fn recovers_until_truncated_record() {
        let dir = tempfile::tempdir().unwrap();

        let wal = dir.path().join("wal.log");

        let mut writer = WalWriter::open(&wal).unwrap();

        for i in 0..1000 {
            let key = format!("key-{i}");

            let value = vec![b'x'; 128];

            let record = WalRecord {
                record_type: RecordType::Put,
                key: key.as_bytes(),
                value: &value,
            };

            writer.append(&record).unwrap();
        }

        drop(writer);

        let truncated = dir.path().join("truncated.log");

        std::fs::copy(&wal, &truncated).unwrap();

        let size = std::fs::metadata(&truncated).unwrap().len();

        let new_size = (size as f64 * 0.6) as u64;

        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&truncated)
            .unwrap();

        file.set_len(new_size).unwrap();

        let reader = WalReader::open(&truncated).unwrap();

        let recovered: Vec<_> = reader.collect();

        assert!(
            recovered.len() > 500,
            "recovered {} records",
            recovered.len()
        );

        assert!(
            recovered.len() < 700,
            "recovered {} records",
            recovered.len()
        );
    }

    #[test]
    fn stops_on_crc_failure() {
        use std::{
            fs::OpenOptions,
            io::{Read, Seek, SeekFrom, Write},
        };

        let dir = tempfile::tempdir().unwrap();

        let wal = dir.path().join("wal.log");

        let mut writer = WalWriter::open(&wal).unwrap();

        for i in 0..100 {
            let key = format!("key-{i}");

            let value = vec![b'x'; 128];

            let record = WalRecord {
                record_type: RecordType::Put,
                key: key.as_bytes(),
                value: &value,
            };

            writer.append(&record).unwrap();
        }

        drop(writer);

        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&wal)
            .unwrap();

        let mut bytes = Vec::new();

        file.read_to_end(&mut bytes).unwrap();

        let midpoint = bytes.len() / 2;

        bytes[midpoint] ^= 0xFF;

        file.seek(SeekFrom::Start(0)).unwrap();

        file.write_all(&bytes).unwrap();

        file.sync_all().unwrap();

        drop(file);

        let reader = WalReader::open(&wal).unwrap();

        let recovered: Vec<_> = reader.collect();

        assert_eq!(recovered.len(), 50);
    }

    #[test]
    fn empty_wal_yields_no_records() {
        let dir = tempfile::tempdir().unwrap();

        let wal = dir.path().join("empty.log");

        std::fs::File::create(&wal).unwrap();

        let mut reader = WalReader::open(&wal).unwrap();

        assert!(reader.read_one().is_none());

        let reader = WalReader::open(&wal).unwrap();

        let recovered: Vec<_> = reader.collect();

        assert_eq!(recovered.len(), 0);
    }

    #[test]
    fn truncated_header_yields_no_records() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();

        let wal = dir.path().join("truncated.log");

        let mut file = std::fs::File::create(&wal).unwrap();

        file.write_all(&[1, 2, 3, 4, 5]).unwrap();

        drop(file);

        let mut reader = WalReader::open(&wal).unwrap();

        assert!(reader.read_one().is_none());

        let reader = WalReader::open(&wal).unwrap();

        let recovered: Vec<_> = reader.collect();

        assert_eq!(recovered.len(), 0);
    }

    #[test]
    fn exactly_header_size_but_no_payload() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();

        let wal = dir.path().join("header_only.log");

        let mut file = std::fs::File::create(&wal).unwrap();

        file.write_all(&[0u8; HEADER_SIZE]).unwrap();

        drop(file);

        let reader = WalReader::open(&wal).unwrap();

        let recovered: Vec<_> = reader.collect();

        assert_eq!(recovered.len(), 0);
    }
}
